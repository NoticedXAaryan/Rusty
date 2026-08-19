use crate::rustybench::{self, Suite};
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug)]
pub struct Experiment {
    pub id: String,
    pub title: String,
    pub hypothesis: String,
    pub status: String,
    pub runner: Option<String>,
    pub suite: Option<PathBuf>,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Registry {
    pub experiments: Vec<Experiment>,
}

impl Registry {
    pub fn load(path: &Path) -> Result<Self> {
        let entries = fs::read_dir(path).map_err(|error| {
            format!("cannot read research registry {}: {error}", path.display())
        })?;
        let mut files = entries
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|file| {
                file.extension()
                    .is_some_and(|extension| extension == "yaml")
            })
            .collect::<Vec<_>>();
        files.sort();
        let mut experiments = Vec::new();
        for file in files {
            experiments.push(parse_experiment(&file)?);
        }
        if experiments.is_empty() {
            return Err(format!("no experiment definitions in {}", path.display()));
        }
        Ok(Self { experiments })
    }

    pub fn find(&self, id: &str) -> Result<&Experiment> {
        self.experiments
            .iter()
            .find(|experiment| experiment.id == id)
            .ok_or_else(|| format!("unknown experiment `{id}`"))
    }
}

fn parse_experiment(path: &Path) -> Result<Experiment> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let field = |name: &str| {
        text.lines().find_map(|line| {
            if line.starts_with(char::is_whitespace) || line.trim_start().starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once(':')?;
            (key.trim() == name).then(|| value.trim().trim_matches('"').to_owned())
        })
    };
    let required = |name: &str| {
        field(name)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{}: missing non-empty root field `{name}`", path.display()))
    };
    let id = required("id")?;
    if path.file_stem().and_then(|value| value.to_str()) != Some(&id) {
        return Err(format!(
            "{}: filename must match experiment id",
            path.display()
        ));
    }
    Ok(Experiment {
        id,
        title: required("title")?,
        hypothesis: required("hypothesis")?,
        status: required("status")?,
        runner: field("runner").filter(|value| !value.is_empty()),
        suite: field("suite")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        path: path.to_path_buf(),
    })
}

#[derive(Clone, Debug)]
struct Trial {
    success: bool,
    duration_ms: u128,
    detail: String,
}

pub fn run_experiment(workspace: &Path, experiment: &Experiment, repeat: usize) -> Result<PathBuf> {
    if repeat == 0 {
        return Err("research repeat count must be positive".to_owned());
    }
    if experiment.status.starts_with("blocked") {
        return Err(format!(
            "experiment `{}` is blocked with status `{}`",
            experiment.id, experiment.status
        ));
    }
    let runner = experiment
        .runner
        .as_deref()
        .ok_or_else(|| format!("experiment `{}` has no runner", experiment.id))?;
    let mut trials = Vec::with_capacity(repeat);
    for _ in 0..repeat {
        trials.push(run_trial(workspace, experiment, runner)?);
    }
    write_raw_record(workspace, experiment, runner, &trials)
}

fn run_trial(workspace: &Path, experiment: &Experiment, runner: &str) -> Result<Trial> {
    let start = Instant::now();
    match runner {
        "conformance" => {
            let output = Command::new("cargo")
                .args(["test", "--workspace", "--all-targets"])
                .current_dir(workspace)
                .output()
                .map_err(|error| format!("cannot run conformance experiment: {error}"))?;
            let mut detail = String::from_utf8_lossy(&output.stdout).into_owned();
            detail.push_str(&String::from_utf8_lossy(&output.stderr));
            Ok(Trial {
                success: output.status.success(),
                duration_ms: start.elapsed().as_millis(),
                detail,
            })
        }
        "rustybench" => {
            let suite_path = experiment
                .suite
                .as_ref()
                .ok_or_else(|| format!("experiment `{}` requires `suite`", experiment.id))?;
            let suite = Suite::load(&workspace.join(suite_path))?;
            let report = rustybench::run_suite(&suite, 3, 30, None)?;
            Ok(Trial {
                success: true,
                duration_ms: start.elapsed().as_millis(),
                detail: report.to_json(),
            })
        }
        value => Err(format!(
            "experiment `{}` uses unsupported runner `{value}`",
            experiment.id
        )),
    }
}

fn write_raw_record(
    workspace: &Path,
    experiment: &Experiment,
    runner: &str,
    trials: &[Trial],
) -> Result<PathBuf> {
    let captured_ms = unix_ms()?;
    let output = workspace
        .join("results")
        .join("raw")
        .join(format!("{}-{captured_ms}.json", experiment.id));
    fs::create_dir_all(output.parent().expect("result has a parent"))
        .map_err(|error| format!("cannot create raw results directory: {error}"))?;
    let record = result_json(workspace, experiment, runner, captured_ms, trials);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .map_err(|error| {
            format!(
                "cannot create append-only result {}: {error}",
                output.display()
            )
        })?;
    file.write_all(record.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    Ok(output)
}

fn result_json(
    workspace: &Path,
    experiment: &Experiment,
    runner: &str,
    captured_ms: u128,
    trials: &[Trial],
) -> String {
    let commit = command_output(workspace, "git", &["rev-parse", "HEAD"]);
    let worktree = command_output(workspace, "git", &["status", "--short"]);
    let rustc = command_output(workspace, "rustc", &["--version"]);
    let clang = command_output(workspace, "clang", &["--version"]);
    let definition_checksum = rustybench::checksum_file(&experiment.path).ok();
    let mut json = String::new();
    write!(
        json,
        "{{\n  \"schema\": \"rusty-research-result-v0\",\n  \"captured_unix_ms\": {captured_ms},\n  \"experiment\": \"{}\",\n  \"title\": \"{}\",\n  \"hypothesis\": \"{}\",\n  \"runner\": \"{}\",\n  \"definition_fnv1a64\": {},\n  \"rusty_commit\": \"{}\",\n  \"worktree_status\": \"{}\",\n  \"host\": {{\"os\": \"{}\", \"arch\": \"{}\", \"logical_cpus\": {}}},\n  \"toolchain\": {{\"rustc\": \"{}\", \"clang\": \"{}\"}},\n  \"trials\": [\n",
        json_escape(&experiment.id),
        json_escape(&experiment.title),
        json_escape(&experiment.hypothesis),
        json_escape(runner),
        definition_checksum.map_or_else(|| "null".to_owned(), |value| format!("\"{value:016x}\"")),
        json_escape(commit.trim()),
        json_escape(&worktree),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(1, usize::from),
        json_escape(rustc.trim()),
        json_escape(clang.trim()),
    )
    .unwrap();
    for (index, trial) in trials.iter().enumerate() {
        if index != 0 {
            json.push_str(",\n");
        }
        write!(
            json,
            "    {{\"success\": {}, \"duration_ms\": {}, \"detail\": \"{}\"}}",
            trial.success,
            trial.duration_ms,
            json_escape(&trial.detail)
        )
        .unwrap();
    }
    json.push_str("\n  ]\n}\n");
    json
}

fn command_output(workspace: &Path, program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .current_dir(workspace)
        .output()
        .map(|output| {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            text.trim_end().to_owned()
        })
        .unwrap_or_else(|error| format!("unavailable: {error}"))
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub records: usize,
    pub successful_trials: usize,
    pub failed_trials: usize,
}

pub fn compare(workspace: &Path, id: &str) -> Result<Summary> {
    let raw = workspace.join("results").join("raw");
    let entries = fs::read_dir(&raw)
        .map_err(|error| format!("cannot read raw results {}: {error}", raw.display()))?;
    let mut summary = Summary::default();
    for path in entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&format!("{id}-")) && name.ends_with(".json"))
        })
    {
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if !text.contains("\"schema\": \"rusty-research-result-v0\"") {
            continue;
        }
        summary.records += 1;
        summary.successful_trials += text.matches("\"success\": true").count();
        summary.failed_trials += text.matches("\"success\": false").count();
    }
    if summary.records == 0 {
        return Err(format!("no research result records found for `{id}`"));
    }
    Ok(summary)
}

pub fn write_report(workspace: &Path, experiment: &Experiment) -> Result<PathBuf> {
    let summary = compare(workspace, &experiment.id)?;
    let output = workspace
        .join("results")
        .join("processed")
        .join(format!("{}-report.md", experiment.id));
    fs::create_dir_all(output.parent().expect("report has a parent"))
        .map_err(|error| format!("cannot create report directory: {error}"))?;
    let text = format!(
        "# {} — {}\n\nStatus: `{}`\n\nHypothesis: {}\n\nRaw records: {}\n\nSuccessful trials: {}\n\nFailed trials: {}\n\nThis operational report does not establish a scientific conclusion. Statistical analysis, failure analysis, and an independent reproduction are required before drawing one.\n",
        experiment.id,
        experiment.title,
        experiment.status,
        experiment.hypothesis,
        summary.records,
        summary.successful_trials,
        summary.failed_trials,
    );
    fs::write(&output, text)
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    Ok(output)
}

fn unix_ms() -> Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            value if value.is_control() => {
                write!(escaped, "\\u{:04x}", value as u32).unwrap();
            }
            value => escaped.push(value),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_experiment_definition() {
        let root = std::env::temp_dir().join(format!(
            "rusty-research-test-{}-{}",
            std::process::id(),
            unix_ms().unwrap()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("EXP-999.yaml");
        fs::write(
            &path,
            "id: EXP-999\ntitle: Test\nhypothesis: It works.\nstatus: defined\nrunner: conformance\n",
        )
        .unwrap();
        let experiment = parse_experiment(&path).unwrap();
        assert_eq!(experiment.id, "EXP-999");
        assert_eq!(experiment.runner.as_deref(), Some("conformance"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn escapes_multiline_provenance_as_json() {
        assert_eq!(json_escape("one\ntwo"), "one\\ntwo");
    }
}
