use crate::{analyze, benchmark, llvm, rir};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::Instant;

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug)]
pub enum Optimization {
    O0,
    O2,
    O3,
}

impl Optimization {
    fn clang_flag(self) -> &'static str {
        match self {
            Self::O0 => "-O0",
            Self::O2 => "-O2",
            Self::O3 => "-O3",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub source: PathBuf,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Artifacts {
    pub rir: PathBuf,
    pub llvm: PathBuf,
    pub executable: PathBuf,
}

impl Project {
    pub fn discover(path: Option<&Path>) -> Result<Self> {
        let candidate = path.unwrap_or_else(|| Path::new("."));
        if candidate.is_file() {
            let name = candidate
                .file_stem()
                .and_then(OsStr::to_str)
                .ok_or("source file has no valid name")?
                .to_owned();
            let root = candidate
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf();
            return Ok(Self {
                root,
                source: candidate.to_path_buf(),
                name,
            });
        }
        let root = candidate.to_path_buf();
        let source = root.join("src").join("main.ry");
        if !source.is_file() {
            return Err(format!("no Rusty source at {}", source.display()));
        }
        let name = manifest_name(&root)
            .or_else(|| root.file_name().and_then(OsStr::to_str).map(str::to_owned))
            .unwrap_or_else(|| "rusty-program".to_owned());
        Ok(Self { root, source, name })
    }

    pub fn check(&self) -> Result<()> {
        let source = fs::read_to_string(&self.source)
            .map_err(|error| format!("cannot read {}: {error}", self.source.display()))?;
        analyze(&display_path(&self.source), &source)
            .map(|_| ())
            .map_err(render_diagnostics)
    }

    pub fn build(&self, optimization: Optimization) -> Result<Artifacts> {
        let source = fs::read_to_string(&self.source)
            .map_err(|error| format!("cannot read {}: {error}", self.source.display()))?;
        let hir = analyze(&display_path(&self.source), &source).map_err(render_diagnostics)?;
        self.build_hir(&hir, &self.root.join("build"), &self.name, optimization)
    }

    fn build_hir(
        &self,
        hir: &crate::hir::Program,
        directory: &Path,
        name: &str,
        optimization: Optimization,
    ) -> Result<Artifacts> {
        let rir_program = rir::Program::from_hir(hir);
        let rir_text = rir_program.serialize();
        let llvm_text = llvm::emit(&rir_program);
        fs::create_dir_all(directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        let rir_path = directory.join(format!("{name}.rir"));
        let llvm_path = directory.join(format!("{name}.ll"));
        let executable = directory.join(executable_name(name));
        write_if_changed(&rir_path, &rir_text)?;
        write_if_changed(&llvm_path, &llvm_text)?;
        invoke_clang(&llvm_path, &executable, optimization)?;
        Ok(Artifacts {
            rir: rir_path,
            llvm: llvm_path,
            executable,
        })
    }

    pub fn run(&self, optimization: Optimization, args: &[String]) -> Result<ExitStatus> {
        let artifacts = self.build(optimization)?;
        Command::new(&artifacts.executable)
            .args(args)
            .status()
            .map_err(|error| format!("cannot run {}: {error}", artifacts.executable.display()))
    }

    pub fn test(&self) -> Result<usize> {
        self.check()?;
        let tests_dir = self.root.join("tests");
        if !tests_dir.is_dir() {
            return Ok(0);
        }
        let mut files = fs::read_dir(&tests_dir)
            .map_err(|error| format!("cannot read {}: {error}", tests_dir.display()))?
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension() == Some(OsStr::new("ry")))
            .collect::<Vec<_>>();
        files.sort();
        let mut passed = 0;
        for file in files {
            let source = fs::read_to_string(&file)
                .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
            let hir = analyze(&display_path(&file), &source).map_err(render_diagnostics)?;
            let name = file
                .file_stem()
                .and_then(OsStr::to_str)
                .ok_or("invalid test name")?;
            let artifacts = self.build_hir(
                &hir,
                &self.root.join("build").join("tests"),
                name,
                Optimization::O0,
            )?;
            let status = Command::new(&artifacts.executable)
                .status()
                .map_err(|error| format!("cannot run test {name}: {error}"))?;
            if !status.success() {
                return Err(format!("test `{name}` failed with {status}"));
            }
            passed += 1;
        }
        Ok(passed)
    }

    pub fn bench(
        &self,
        repeat: usize,
        warmup: usize,
        args: &[String],
    ) -> Result<benchmark::Report> {
        if repeat == 0 {
            return Err("benchmark repeat count must be greater than zero".to_owned());
        }
        let artifacts = self.build(Optimization::O3)?;
        let execute = || {
            let output = Command::new(&artifacts.executable)
                .args(args)
                .output()
                .map_err(|error| format!("benchmark failed: {error}"))?;
            if output.status.success() {
                Ok(output)
            } else {
                Err(format!("benchmark program failed with {}", output.status))
            }
        };
        for _ in 0..warmup {
            execute()?;
        }
        let mut samples_ns = Vec::with_capacity(repeat);
        for _ in 0..repeat {
            let start = Instant::now();
            execute()?;
            samples_ns.push(start.elapsed().as_nanos());
        }
        let statistics = benchmark::Statistics::from_samples(&samples_ns)
            .expect("a positive repeat count produces samples");
        Ok(benchmark::Report {
            warmup_runs: warmup,
            samples_ns,
            statistics,
        })
    }

    pub fn clean(&self) -> Result<bool> {
        let build = self.root.join("build");
        if !build.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(&build)
            .map_err(|error| format!("cannot remove {}: {error}", build.display()))?;
        Ok(true)
    }
}

pub fn create(name: &str) -> Result<PathBuf> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("project name must contain only ASCII letters, digits, `-`, or `_`".to_owned());
    }
    let root = PathBuf::from(name);
    if root.exists() {
        return Err(format!("{} already exists", root.display()));
    }
    fs::create_dir_all(root.join("src"))
        .map_err(|error| format!("cannot create project: {error}"))?;
    fs::write(root.join("Rusty.toml"), format!("name = \"{name}\"\n"))
        .map_err(|error| format!("cannot write manifest: {error}"))?;
    fs::write(
        root.join("src").join("main.ry"),
        "fn main() {\n    print(\"Hello from Rusty\")\n}\n",
    )
    .map_err(|error| format!("cannot write source: {error}"))?;
    Ok(root)
}

fn manifest_name(root: &Path) -> Option<String> {
    let content = fs::read_to_string(root.join("Rusty.toml")).ok()?;
    content.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim() != "name" {
            return None;
        }
        let value = value.trim();
        value
            .strip_prefix('"')?
            .strip_suffix('"')
            .map(str::to_owned)
    })
}

fn write_if_changed(path: &Path, content: &str) -> Result<()> {
    if fs::read_to_string(path).ok().as_deref() == Some(content) {
        return Ok(());
    }
    fs::write(path, content).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn invoke_clang(llvm: &Path, executable: &Path, optimization: Optimization) -> Result<()> {
    let clang = std::env::var_os("CLANG").unwrap_or_else(|| "clang".into());
    let output = Command::new(&clang)
        .arg(optimization.clang_flag())
        .arg(llvm)
        .arg("-o")
        .arg(executable)
        .output()
        .map_err(|error| {
            format!(
                "unable to invoke `{}`: {error}; install clang or set CLANG",
                PathBuf::from(&clang).display()
            )
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "clang failed with {}:\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn render_diagnostics(diagnostics: Vec<crate::diagnostic::Diagnostic>) -> String {
    diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest_name() {
        let unique = format!("rusty-project-test-{}", std::process::id());
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Rusty.toml"), "name = \"sample\"\n").unwrap();
        assert_eq!(manifest_name(&root).as_deref(), Some("sample"));
        fs::remove_dir_all(root).unwrap();
    }
}
