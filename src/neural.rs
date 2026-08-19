use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug)]
pub struct Options {
    pub budget: NonZeroUsize,
    pub target_cpu: String,
    pub minimum_speedup: f64,
    pub warmup: usize,
    pub repeat: NonZeroUsize,
    pub generation_timeout: Duration,
}

impl Options {
    pub fn new(
        budget: usize,
        target_cpu: String,
        minimum_speedup: f64,
        warmup: usize,
        repeat: usize,
    ) -> Result<Self> {
        let budget = NonZeroUsize::new(budget).ok_or("candidate budget must be positive")?;
        let repeat = NonZeroUsize::new(repeat).ok_or("benchmark repeat count must be positive")?;
        if target_cpu.is_empty()
            || !target_cpu
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err("target CPU contains invalid characters".to_owned());
        }
        if !minimum_speedup.is_finite() || minimum_speedup < 0.0 {
            return Err("minimum speedup must be finite and non-negative".to_owned());
        }
        Ok(Self {
            budget,
            target_cpu,
            minimum_speedup,
            warmup,
            repeat,
            generation_timeout: Duration::from_secs(15 * 60),
        })
    }
}

#[derive(Clone, Debug)]
pub struct CommandGenerator {
    command: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Generation {
    pub candidate: usize,
    pub duration: Duration,
    pub llvm_path: PathBuf,
}

impl CommandGenerator {
    pub fn from_env() -> Result<Self> {
        let command = std::env::var_os("RUSTY_NEURAL_GENERATOR")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "RUSTY_NEURAL_GENERATOR is not configured".to_owned())?;
        Ok(Self {
            command: PathBuf::from(command),
        })
    }

    pub fn command(&self) -> &Path {
        &self.command
    }

    pub fn generate(
        &self,
        rir: &Path,
        directory: &Path,
        candidate: usize,
        target_cpu: &str,
        timeout: Duration,
    ) -> Result<Generation> {
        fs::create_dir_all(directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        let llvm_path = directory.join(format!("candidate-{candidate}.ll"));
        if llvm_path.exists() {
            fs::remove_file(&llvm_path)
                .map_err(|error| format!("cannot remove stale candidate: {error}"))?;
        }
        let started = Instant::now();
        let mut child = Command::new(&self.command)
            .arg("--input")
            .arg(rir)
            .arg("--output")
            .arg(&llvm_path)
            .arg("--target-cpu")
            .arg(target_cpu)
            .arg("--seed")
            .arg(candidate.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!(
                    "cannot start neural generator `{}`: {error}",
                    self.command.display()
                )
            })?;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() < timeout => {
                    thread::sleep(Duration::from_millis(50));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let output = child
                        .wait_with_output()
                        .map_err(|error| error.to_string())?;
                    return Err(format!(
                        "neural generation timed out after {:?}: {}{}",
                        timeout,
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }
                Err(error) => return Err(format!("cannot poll neural generator: {error}")),
            }
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("cannot collect neural generator output: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "neural generator failed with {}: {}{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        if !llvm_path.is_file() {
            return Err("neural generator succeeded without producing its output file".to_owned());
        }
        Ok(Generation {
            candidate,
            duration: started.elapsed(),
            llvm_path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_optimization_options() {
        assert!(Options::new(0, "native".to_owned(), 0.0, 0, 1).is_err());
        assert!(Options::new(1, "bad cpu".to_owned(), 0.0, 0, 1).is_err());
        assert!(Options::new(1, "native".to_owned(), f64::NAN, 0, 1).is_err());
        let options = Options::new(8, "zen4".to_owned(), 0.02, 3, 30).unwrap();
        assert_eq!(options.budget.get(), 8);
        assert_eq!(options.repeat.get(), 30);
    }
}
