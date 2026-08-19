use crate::benchmark::{Report, Statistics};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

pub type Result<T> = std::result::Result<T, String>;

const MAX_CANDIDATE_BYTES: u64 = 4 * 1024 * 1024;
const ALLOWED_EXTERNALS: &[&str] = &["printf", "llvm.trap"];
const FORBIDDEN_IR: &[&str] = &[
    " asm ",
    "callbr ",
    "blockaddress(",
    "inttoptr ",
    "ptrtoint ",
    "module asm",
    "@llvm.readcyclecounter",
    "@llvm.returnaddress",
    "@llvm.frameaddress",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticReport {
    pub bytes: u64,
    pub declarations: Vec<String>,
}

pub fn inspect_llvm(path: &Path) -> Result<StaticReport> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect candidate {}: {error}", path.display()))?;
    if metadata.len() > MAX_CANDIDATE_BYTES {
        return Err(format!(
            "candidate exceeds {} byte policy limit",
            MAX_CANDIDATE_BYTES
        ));
    }
    let text = fs::read_to_string(path)
        .map_err(|error| format!("candidate is not UTF-8 text: {error}"))?;
    if !text
        .lines()
        .any(|line| line.contains("@main(") && line.trim_start().starts_with("define"))
    {
        return Err("candidate does not define `main`".to_owned());
    }
    for forbidden in FORBIDDEN_IR {
        if text.contains(forbidden) {
            return Err(format!(
                "candidate uses forbidden LLVM construct `{forbidden}`"
            ));
        }
    }
    let mut declarations = Vec::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("declare "))
    {
        let name = declaration_name(line)
            .ok_or_else(|| format!("cannot parse external declaration `{line}`"))?;
        if !ALLOWED_EXTERNALS.contains(&name.as_str()) {
            return Err(format!("candidate declares forbidden external `{name}`"));
        }
        declarations.push(name);
    }
    Ok(StaticReport {
        bytes: metadata.len(),
        declarations,
    })
}

fn declaration_name(line: &str) -> Option<String> {
    let after_at = line.split_once('@')?.1;
    let name = after_at.split_once('(')?.0.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

#[derive(Clone, Debug)]
pub struct Sandbox {
    command: PathBuf,
}

impl Sandbox {
    pub fn from_env() -> Result<Self> {
        let command = std::env::var_os("RUSTY_SANDBOX")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "RUSTY_SANDBOX is required before executing untrusted candidates".to_owned()
            })?;
        Ok(Self {
            command: PathBuf::from(command),
        })
    }

    pub fn command(&self) -> &Path {
        &self.command
    }

    pub fn execute(&self, executable: &Path, args: &[String]) -> Result<Output> {
        let mut command = Command::new(&self.command);
        command.arg(executable);
        if !args.is_empty() {
            command.arg("--").args(args);
        }
        command.output().map_err(|error| {
            format!(
                "sandbox `{}` could not execute {}: {error}",
                self.command.display(),
                executable.display()
            )
        })
    }

    pub fn benchmark(
        &self,
        executable: &Path,
        args: &[String],
        warmup: usize,
        repeat: usize,
    ) -> Result<Report> {
        if repeat == 0 {
            return Err("benchmark repeat count must be positive".to_owned());
        }
        for _ in 0..warmup {
            require_success(self.execute(executable, args)?)?;
        }
        let mut samples_ns = Vec::with_capacity(repeat);
        for _ in 0..repeat {
            let started = Instant::now();
            require_success(self.execute(executable, args)?)?;
            samples_ns.push(started.elapsed().as_nanos());
        }
        let statistics =
            Statistics::from_samples(&samples_ns).expect("positive repeat count produces samples");
        Ok(Report {
            warmup_runs: warmup,
            samples_ns,
            statistics,
        })
    }
}

fn require_success(output: Output) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "sandboxed program failed with {}: {}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

pub fn equivalent(reference: &Output, candidate: &Output) -> bool {
    reference.status.code() == candidate.status.code()
        && reference.stdout == candidate.stdout
        && reference.stderr == candidate.stderr
}

pub fn sandbox_environment() -> Option<OsString> {
    std::env::var_os("RUSTY_SANDBOX")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rusty-verifier-{}-{}.ll",
            std::process::id(),
            contents.len()
        ));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn accepts_the_minimal_policy_surface() {
        let path = candidate("declare i32 @printf(ptr, ...)\ndefine i32 @main() { ret i32 0 }\n");
        let report = inspect_llvm(&path).unwrap();
        assert_eq!(report.declarations, ["printf"]);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_unknown_externals_and_raw_pointer_casts() {
        let external = candidate("declare i32 @system(ptr)\ndefine i32 @main() { ret i32 0 }\n");
        assert!(inspect_llvm(&external).unwrap_err().contains("system"));
        fs::remove_file(external).unwrap();

        let cast = candidate("define i32 @main() { %p = inttoptr i64 1 to ptr\n ret i32 0 }\n");
        assert!(inspect_llvm(&cast).unwrap_err().contains("inttoptr"));
        fs::remove_file(cast).unwrap();
    }
}
