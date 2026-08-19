use crate::benchmark::{Classification, Comparison};
use crate::neural::{CommandGenerator, Options};
use crate::project::{self, Artifacts, Optimization, Project};
use crate::verifier::{self, Sandbox};
use std::fs;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug)]
pub struct CandidateRecord {
    pub candidate: usize,
    pub classification: Classification,
    pub reason: String,
    pub generation_ms: u128,
    pub comparison: Option<Comparison>,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub artifacts: Artifacts,
    pub selected_candidate: Option<usize>,
    pub fallback_reason: Option<String>,
    pub candidates: Vec<CandidateRecord>,
}

pub fn optimize(project: &Project, options: &Options, args: &[String]) -> Result<Outcome> {
    let artifacts = project.build(Optimization::O3)?;
    let generator = match CommandGenerator::from_env() {
        Ok(generator) => generator,
        Err(reason) => return Ok(fallback(artifacts, reason)),
    };
    let sandbox = match Sandbox::from_env() {
        Ok(sandbox) => sandbox,
        Err(reason) => return Ok(fallback(artifacts, reason)),
    };
    let reference_output = match sandbox.execute(&artifacts.executable, args) {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            return Ok(fallback(
                artifacts,
                format!("trusted reference failed in sandbox with {}", output.status),
            ));
        }
        Err(reason) => return Ok(fallback(artifacts, reason)),
    };
    let reference_report = match sandbox.benchmark(
        &artifacts.executable,
        args,
        options.warmup,
        options.repeat.get(),
    ) {
        Ok(report) => report,
        Err(reason) => return Ok(fallback(artifacts, reason)),
    };
    let candidate_directory = project.root.join("build").join("neural");
    let mut records = Vec::with_capacity(options.budget.get());
    let mut best: Option<(usize, PathBuf, Comparison)> = None;

    for candidate in 1..=options.budget.get() {
        let generation = match generator.generate(
            &artifacts.rir,
            &candidate_directory,
            candidate,
            &options.target_cpu,
            options.generation_timeout,
        ) {
            Ok(generation) => generation,
            Err(reason) => {
                records.push(record(
                    candidate,
                    Classification::NeuralFailed,
                    reason,
                    0,
                    None,
                ));
                continue;
            }
        };
        if let Err(reason) = verifier::inspect_llvm(&generation.llvm_path) {
            records.push(record(
                candidate,
                Classification::VerifierRejected,
                reason,
                generation.duration.as_millis(),
                None,
            ));
            continue;
        }
        let executable = candidate_executable(&generation.llvm_path);
        if let Err(reason) = project::compile_llvm(
            &generation.llvm_path,
            &executable,
            Optimization::O3,
            Some(&options.target_cpu),
        ) {
            records.push(record(
                candidate,
                Classification::NeuralFailed,
                reason,
                generation.duration.as_millis(),
                None,
            ));
            continue;
        }
        let candidate_output = match sandbox.execute(&executable, args) {
            Ok(output) => output,
            Err(reason) => {
                records.push(record(
                    candidate,
                    Classification::NeuralFailed,
                    reason,
                    generation.duration.as_millis(),
                    None,
                ));
                continue;
            }
        };
        if !verifier::equivalent(&reference_output, &candidate_output) {
            records.push(record(
                candidate,
                Classification::NeuralIncorrect,
                "differential output or exit status mismatch".to_owned(),
                generation.duration.as_millis(),
                None,
            ));
            continue;
        }
        let candidate_report =
            match sandbox.benchmark(&executable, args, options.warmup, options.repeat.get()) {
                Ok(report) => report,
                Err(reason) => {
                    records.push(record(
                        candidate,
                        Classification::NeuralFailed,
                        reason,
                        generation.duration.as_millis(),
                        None,
                    ));
                    continue;
                }
            };

        let comparison = Comparison::from_samples(
            &reference_report.samples_ns,
            &candidate_report.samples_ns,
            options.minimum_speedup,
        )
        .expect("non-empty benchmark reports and validated speedup");
        let classification = comparison.classification;
        let reason = if comparison.accepted {
            "correct, verified, stable, and above the performance threshold".to_owned()
        } else {
            "performance gate rejected the candidate".to_owned()
        };
        if comparison.accepted
            && best
                .as_ref()
                .is_none_or(|(_, _, current)| comparison.speedup > current.speedup)
        {
            best = Some((candidate, executable, comparison.clone()));
        }
        records.push(record(
            candidate,
            classification,
            reason,
            generation.duration.as_millis(),
            Some(comparison),
        ));
    }

    let Some((selected_candidate, executable, _)) = best else {
        return Ok(Outcome {
            artifacts,
            selected_candidate: None,
            fallback_reason: Some(
                "no candidate passed verification and the performance gate".to_owned(),
            ),
            candidates: records,
        });
    };
    install_candidate(&artifacts.executable, &executable)?;
    Ok(Outcome {
        artifacts,
        selected_candidate: Some(selected_candidate),
        fallback_reason: None,
        candidates: records,
    })
}

fn fallback(artifacts: Artifacts, reason: String) -> Outcome {
    Outcome {
        artifacts,
        selected_candidate: None,
        fallback_reason: Some(reason),
        candidates: Vec::new(),
    }
}

fn record(
    candidate: usize,
    classification: Classification,
    reason: String,
    generation_ms: u128,
    comparison: Option<Comparison>,
) -> CandidateRecord {
    CandidateRecord {
        candidate,
        classification,
        reason,
        generation_ms,
        comparison,
    }
}

fn candidate_executable(llvm: &Path) -> PathBuf {
    if cfg!(windows) {
        llvm.with_extension("exe")
    } else {
        llvm.with_extension("")
    }
}

fn install_candidate(destination: &Path, candidate: &Path) -> Result<()> {
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("final executable has no valid filename")?;
    let reference = destination.with_file_name(format!("reference-{file_name}"));
    let staged = destination.with_file_name(format!("candidate-{file_name}.tmp"));
    fs::copy(destination, &reference).map_err(|error| {
        format!(
            "cannot preserve trusted reference {}: {error}",
            reference.display()
        )
    })?;
    fs::copy(candidate, &staged)
        .map_err(|error| format!("cannot stage selected candidate: {error}"))?;
    if let Err(error) = fs::remove_file(destination) {
        let _ = fs::remove_file(&staged);
        return Err(format!("cannot replace final executable: {error}"));
    }
    if let Err(error) = fs::rename(&staged, destination) {
        let restore = fs::copy(&reference, destination);
        return match restore {
            Ok(_) => Err(format!(
                "candidate installation failed and reference was restored: {error}"
            )),
            Err(restore_error) => Err(format!(
                "candidate installation failed ({error}) and reference restoration failed ({restore_error})"
            )),
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_executable_uses_the_host_suffix() {
        let path = candidate_executable(Path::new("candidate.ll"));
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            cfg!(windows).then_some("exe")
        );
    }
}
