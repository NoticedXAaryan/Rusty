use crate::benchmark::{self, Comparison, Statistics};
use crate::hir::Program as HirProgram;
use crate::project::{Optimization, Project};
use crate::{analyze, llvm, rewrite, rir};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub type Result<T> = std::result::Result<T, String>;

pub const SCHEMA: &str = "rustybench-v0";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Split {
    Train,
    Validation,
    Development,
    FinalHeldOut,
    CaseStudy,
}

impl Split {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "train" => Ok(Self::Train),
            "validation" => Ok(Self::Validation),
            "development" => Ok(Self::Development),
            "final-held-out" => Ok(Self::FinalHeldOut),
            "case-study" => Ok(Self::CaseStudy),
            _ => Err(format!("unknown RustyBench split `{value}`")),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Train => "train",
            Self::Validation => "validation",
            Self::Development => "development",
            Self::FinalHeldOut => "final-held-out",
            Self::CaseStudy => "case-study",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Category {
    Microbenchmark,
    Algorithm,
    SystemsKernel,
    Memory,
    Branch,
    Numerical,
    Application,
    Generated,
    Doom,
}

impl Category {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "microbenchmark" => Ok(Self::Microbenchmark),
            "algorithm" => Ok(Self::Algorithm),
            "systems-kernel" => Ok(Self::SystemsKernel),
            "memory" => Ok(Self::Memory),
            "branch" => Ok(Self::Branch),
            "numerical" => Ok(Self::Numerical),
            "application" => Ok(Self::Application),
            "generated" => Ok(Self::Generated),
            "doom" => Ok(Self::Doom),
            _ => Err(format!("unknown RustyBench category `{value}`")),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Microbenchmark => "microbenchmark",
            Self::Algorithm => "algorithm",
            Self::SystemsKernel => "systems-kernel",
            Self::Memory => "memory",
            Self::Branch => "branch",
            Self::Numerical => "numerical",
            Self::Application => "application",
            Self::Generated => "generated",
            Self::Doom => "doom",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Case {
    pub id: String,
    pub category: Category,
    pub split: Split,
    pub project: PathBuf,
    pub source_checksum: u64,
    pub expected_stdout_checksum: u64,
    pub args: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Suite {
    pub manifest: PathBuf,
    pub cases: Vec<Case>,
}

impl Suite {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).map_err(|error| {
            format!(
                "cannot read RustyBench manifest {}: {error}",
                path.display()
            )
        })?;
        let mut lines = text.lines();
        if lines.next().map(str::trim) != Some(SCHEMA) {
            return Err(format!("{} must begin with `{SCHEMA}`", path.display()));
        }
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut cases = Vec::new();
        for (index, raw) in lines.enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields = line.split('\t').collect::<Vec<_>>();
            if !(6..=7).contains(&fields.len()) {
                return Err(format!(
                    "{}:{}: expected 6 or 7 tab-separated fields",
                    path.display(),
                    index + 2
                ));
            }
            let id = fields[0].to_owned();
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(format!("{}:{}: invalid case id", path.display(), index + 2));
            }
            cases.push(Case {
                id,
                category: Category::parse(fields[1])?,
                split: Split::parse(fields[2])?,
                project: base.join(fields[3]),
                source_checksum: parse_checksum(fields[4])?,
                expected_stdout_checksum: parse_checksum(fields[5])?,
                args: fields
                    .get(6)
                    .map(|value| value.split_ascii_whitespace().map(str::to_owned).collect())
                    .unwrap_or_default(),
            });
        }
        if cases.is_empty() {
            return Err("RustyBench manifest contains no cases".to_owned());
        }
        validate_separation(&cases)?;
        Ok(Self {
            manifest: path.to_path_buf(),
            cases,
        })
    }

    pub fn validate_frozen_inputs(&self) -> Result<()> {
        for case in &self.cases {
            let source = case.project.join("src").join("main.ry");
            let actual = checksum_file(&source)?;
            if actual != case.source_checksum {
                return Err(format!(
                    "RustyBench source checksum mismatch for `{}`: expected {:016x}, found {actual:016x}",
                    case.id, case.source_checksum
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct CaseResult {
    pub id: String,
    pub category: Category,
    pub split: Split,
    pub source_checksum: u64,
    pub stdout_checksum: u64,
    pub samples_ns: Vec<u128>,
    pub statistics: Statistics,
}

#[derive(Clone, Debug)]
pub struct SuiteReport {
    pub manifest: PathBuf,
    pub captured_unix_ms: u128,
    pub warmup: usize,
    pub repeat: usize,
    pub cpu_frequency_khz_start: Option<u64>,
    pub cpu_frequency_khz_end: Option<u64>,
    pub results: Vec<CaseResult>,
}

impl SuiteReport {
    pub fn geometric_mean_median_ns(&self) -> Option<f64> {
        benchmark::geometric_mean(
            &self
                .results
                .iter()
                .map(|result| result.statistics.median_ns)
                .collect::<Vec<_>>(),
        )
    }

    pub fn category_counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for result in &self.results {
            *counts.entry(result.category.as_str()).or_insert(0) += 1;
        }
        counts
    }

    pub fn to_json(&self) -> String {
        let mut json = String::new();
        write!(
            json,
            "{{\n  \"schema\": \"rustybench-result-v0\",\n  \"captured_unix_ms\": {},\n  \"manifest\": \"{}\",\n  \"backend\": \"reference-clang-o3\",\n  \"host\": {{\"os\": \"{}\", \"arch\": \"{}\", \"cpu_frequency_khz_start\": {}, \"cpu_frequency_khz_end\": {}}},\n  \"warmup\": {},\n  \"repeat\": {},\n  \"outlier_policy\": \"retain-all\",\n  \"results\": [\n",
            self.captured_unix_ms,
            json_escape(&self.manifest.to_string_lossy()),
            json_escape(std::env::consts::OS),
            json_escape(std::env::consts::ARCH),
            optional_number(self.cpu_frequency_khz_start),
            optional_number(self.cpu_frequency_khz_end),
            self.warmup,
            self.repeat
        )
        .expect("writing to a string cannot fail");
        for (index, result) in self.results.iter().enumerate() {
            if index != 0 {
                json.push_str(",\n");
            }
            write_case_json(&mut json, result);
        }
        json.push_str("\n  ]\n}\n");
        json
    }
}

pub fn run_suite(
    suite: &Suite,
    warmup: usize,
    repeat: usize,
    split: Option<Split>,
) -> Result<SuiteReport> {
    if repeat == 0 {
        return Err("RustyBench repeat count must be greater than zero".to_owned());
    }
    suite.validate_frozen_inputs()?;
    let cpu_frequency_khz_start = cpu_frequency_khz();
    let selected = suite
        .cases
        .iter()
        .filter(|case| split.is_none_or(|wanted| case.split == wanted))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err("no RustyBench cases match the requested split".to_owned());
    }
    let mut results = Vec::with_capacity(selected.len());
    for case in selected {
        let project = Project::discover(Some(&case.project))?;
        let artifacts = project.build(Optimization::O3)?;
        let execute = || {
            let output = Command::new(&artifacts.executable)
                .args(&case.args)
                .output()
                .map_err(|error| {
                    format!("RustyBench case `{}` failed to start: {error}", case.id)
                })?;
            if !output.status.success() {
                return Err(format!(
                    "RustyBench case `{}` exited with {}",
                    case.id, output.status
                ));
            }
            let checksum = stdout_checksum(&output.stdout);
            if checksum != case.expected_stdout_checksum {
                return Err(format!(
                    "RustyBench oracle mismatch for `{}`: expected {:016x}, found {checksum:016x}",
                    case.id, case.expected_stdout_checksum
                ));
            }
            Ok(())
        };
        execute()?;
        for _ in 0..warmup {
            execute()?;
        }
        let mut samples_ns = Vec::with_capacity(repeat);
        for _ in 0..repeat {
            let start = Instant::now();
            execute()?;
            samples_ns.push(start.elapsed().as_nanos());
        }
        let statistics = Statistics::from_samples(&samples_ns)
            .expect("a positive repeat count produces samples");
        results.push(CaseResult {
            id: case.id.clone(),
            category: case.category,
            split: case.split,
            source_checksum: case.source_checksum,
            stdout_checksum: case.expected_stdout_checksum,
            samples_ns,
            statistics,
        });
    }
    Ok(SuiteReport {
        manifest: suite.manifest.clone(),
        captured_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
            .as_millis(),
        warmup,
        repeat,
        cpu_frequency_khz_start,
        cpu_frequency_khz_end: cpu_frequency_khz(),
        results,
    })
}

#[derive(Clone, Debug)]
pub struct CompareColumn {
    pub label: &'static str,
    pub samples_ns: Vec<u128>,
    pub statistics: Statistics,
}

#[derive(Clone, Debug)]
pub struct CompareCase {
    pub id: String,
    pub category: Category,
    pub split: Split,
    pub pipeline: String,
    pub fallback: Option<String>,
    pub reference_o2: CompareColumn,
    pub reference_o3: CompareColumn,
    pub reference_native: CompareColumn,
    pub search: CompareColumn,
    pub search_native: CompareColumn,
}

impl CompareCase {
    pub fn speedup_vs_o3(&self, column: &CompareColumn) -> Option<f64> {
        if column.statistics.median_ns == 0.0 {
            return None;
        }
        Some(self.reference_o3.statistics.median_ns / column.statistics.median_ns)
    }

    pub fn search_comparison(&self) -> Option<Comparison> {
        Comparison::from_samples(&self.reference_o3.samples_ns, &self.search.samples_ns, 0.10)
    }

    pub fn native_comparison(&self) -> Option<Comparison> {
        Comparison::from_samples(
            &self.reference_native.samples_ns,
            &self.search_native.samples_ns,
            0.10,
        )
    }
}

#[derive(Clone, Debug)]
pub struct CompareReport {
    pub manifest: PathBuf,
    pub captured_unix_ms: u128,
    pub warmup: usize,
    pub repeat: usize,
    pub cpu_frequency_khz_start: Option<u64>,
    pub cpu_frequency_khz_end: Option<u64>,
    /// Fresh timing. This is the run to quote.
    pub results: Vec<CompareCase>,
    /// First timing, kept so a lucky layout is visible and not quoted.
    pub discovery: Vec<CompareCase>,
}

impl CompareReport {
    pub fn geometric_mean_speedup(
        &self,
        select: impl Fn(&CompareCase) -> Option<f64>,
    ) -> Option<f64> {
        let values = self.results.iter().filter_map(select).collect::<Vec<_>>();
        benchmark::geometric_mean(&values)
    }

    pub fn to_json(&self) -> String {
        let mut json = String::new();
        write!(
            json,
            "{{\n  \"schema\": \"rustybench-compare-v1\",\n  \"captured_unix_ms\": {},\n  \"manifest\": \"{}\",\n  \"host\": {{\"os\": \"{}\", \"arch\": \"{}\", \"cpu_frequency_khz_start\": {}, \"cpu_frequency_khz_end\": {}}},\n  \"warmup\": {},\n  \"repeat\": {},\n  \"outlier_policy\": \"retain-all\",\n  \"speedup_definition\": \"baseline_median / candidate_median\",\n  \"reported_phase\": \"confirmation\",\n  \"layout\": \"randomized-link-order-and-environment-size\",\n  \"gate_reported_against\": 0.10,\n  \"results\": [\n",
            self.captured_unix_ms,
            json_escape(&self.manifest.to_string_lossy()),
            json_escape(std::env::consts::OS),
            json_escape(std::env::consts::ARCH),
            optional_number(self.cpu_frequency_khz_start),
            optional_number(self.cpu_frequency_khz_end),
            self.warmup,
            self.repeat
        )
        .expect("writing to a string cannot fail");
        for (index, result) in self.results.iter().enumerate() {
            if index != 0 {
                json.push_str(",\n");
            }
            write_compare_json(&mut json, result);
        }
        json.push_str("\n  ],\n  \"discovery\": [\n");
        for (index, result) in self.discovery.iter().enumerate() {
            if index != 0 {
                json.push_str(",\n");
            }
            write_compare_json(&mut json, result);
        }
        json.push_str("\n  ]\n}\n");
        json
    }
}

pub fn run_comparison(
    suite: &Suite,
    warmup: usize,
    repeat: usize,
    split: Option<Split>,
) -> Result<CompareReport> {
    if repeat == 0 {
        return Err("RustyBench repeat count must be greater than zero".to_owned());
    }
    suite.validate_frozen_inputs()?;
    let cpu_frequency_khz_start = cpu_frequency_khz();
    let selected = suite
        .cases
        .iter()
        .filter(|case| split.is_none_or(|wanted| case.split == wanted))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err("no RustyBench cases match the requested split".to_owned());
    }
    let mut prepared = Vec::with_capacity(selected.len());
    for case in selected {
        prepared.push(prepare_case(case)?);
    }
    let mut discovery = Vec::with_capacity(prepared.len());
    let mut results = Vec::with_capacity(prepared.len());
    for case in &prepared {
        discovery.push(time_prepared(case, 0, warmup, repeat)?);
        results.push(time_prepared(case, 1, warmup, repeat)?);
    }
    Ok(CompareReport {
        manifest: suite.manifest.clone(),
        captured_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
            .as_millis(),
        warmup,
        repeat,
        cpu_frequency_khz_start,
        cpu_frequency_khz_end: cpu_frequency_khz(),
        results,
        discovery,
    })
}

struct PreparedCase {
    case: Case,
    pipeline: String,
    fallback: Option<String>,
    o2: Vec<PathBuf>,
    o3: Vec<PathBuf>,
    native: Vec<PathBuf>,
    search: Vec<PathBuf>,
    search_native: Vec<PathBuf>,
}

fn prepare_case(case: &Case) -> Result<PreparedCase> {
    let project = Project::discover(Some(&case.project))?;
    let source = fs::read_to_string(&project.source)
        .map_err(|error| format!("cannot read {}: {error}", project.source.display()))?;
    let hir = analyze(&project.source.to_string_lossy(), &source).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let outcome = rewrite::search(&hir, &rewrite::SearchOptions::default());
    let native_ok = rewrite::native_equivalent(&hir, &outcome.program)?;
    let (program, pipeline, fallback) = if native_ok {
        (outcome.program, outcome.pipeline.to_owned(), None)
    } else {
        (
            hir.clone(),
            "identity".to_owned(),
            Some(format!(
                "native differential mismatch for pipeline `{}`",
                outcome.pipeline
            )),
        )
    };
    let build_root = project.root.join("build").join("compare");
    Ok(PreparedCase {
        case: case.clone(),
        pipeline,
        fallback,
        o2: layout_binaries(&hir, &build_root.join("o2"), &project.name, "-O2", false)?,
        o3: layout_binaries(&hir, &build_root.join("o3"), &project.name, "-O3", false)?,
        native: layout_binaries(
            &hir,
            &build_root.join("o3-native"),
            &project.name,
            "-O3",
            true,
        )?,
        search: layout_binaries(
            &program,
            &build_root.join("search"),
            &project.name,
            "-O3",
            false,
        )?,
        search_native: layout_binaries(
            &program,
            &build_root.join("search-native"),
            &project.name,
            "-O3",
            true,
        )?,
    })
}

fn layout_binaries(
    program: &HirProgram,
    directory: &Path,
    name: &str,
    optimization: &str,
    march_native: bool,
) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let llvm_path = directory.join(format!("{name}.ll"));
    fs::write(&llvm_path, llvm::emit(&rir::Program::from_hir(program)))
        .map_err(|error| format!("cannot write {}: {error}", llvm_path.display()))?;
    let mut binaries = Vec::with_capacity(4);
    for variant in 0..4 {
        let pad = directory.join(format!("pad-{variant}.ll"));
        let bytes = 32 * (variant + 1);
        fs::write(
            &pad,
            format!(
                "source_filename = \"pad\"\n@rusty.pad.{variant} = global [{bytes} x i8] zeroinitializer\n@llvm.used = appending global [1 x ptr] [ptr @rusty.pad.{variant}]\n"
            ),
        )
        .map_err(|error| format!("cannot write {}: {error}", pad.display()))?;
        let executable = directory.join(format!("{name}-layout-{variant}"));
        link_layout(
            &llvm_path,
            &pad,
            &executable,
            optimization,
            march_native,
            variant % 2 == 0,
        )?;
        binaries.push(executable);
    }
    Ok(binaries)
}

fn link_layout(
    program: &Path,
    pad: &Path,
    executable: &Path,
    optimization: &str,
    march_native: bool,
    program_first: bool,
) -> Result<()> {
    let clang = std::env::var_os("CLANG").unwrap_or_else(|| "clang".into());
    let mut command = Command::new(&clang);
    command.arg(optimization);
    if march_native {
        command.arg("-march=native");
    }
    if program_first {
        command.arg(program).arg(pad);
    } else {
        command.arg(pad).arg(program);
    }
    let output = command
        .arg("-o")
        .arg(executable)
        .output()
        .map_err(|error| format!("cannot invoke clang: {error}"))?;
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

fn time_prepared(
    case: &PreparedCase,
    phase: u32,
    warmup: usize,
    repeat: usize,
) -> Result<CompareCase> {
    Ok(CompareCase {
        id: case.case.id.clone(),
        category: case.case.category,
        split: case.case.split,
        pipeline: case.pipeline.clone(),
        fallback: case.fallback.clone(),
        reference_o2: time_layouts(&case.o2, &case.case, warmup, repeat, phase, "clang-o2")?,
        reference_o3: time_layouts(&case.o3, &case.case, warmup, repeat, phase, "clang-o3")?,
        reference_native: time_layouts(
            &case.native,
            &case.case,
            warmup,
            repeat,
            phase,
            "clang-o3-native",
        )?,
        search: time_layouts(
            &case.search,
            &case.case,
            warmup,
            repeat,
            phase,
            "rir-eqsat-o3",
        )?,
        search_native: time_layouts(
            &case.search_native,
            &case.case,
            warmup,
            repeat,
            phase,
            "rir-eqsat-o3-native",
        )?,
    })
}

fn time_layouts(
    executables: &[PathBuf],
    case: &Case,
    warmup: usize,
    repeat: usize,
    phase: u32,
    label: &'static str,
) -> Result<CompareColumn> {
    let invoke = |sample: usize| {
        let variant = (sample + phase as usize) % executables.len();
        let pad = ((sample + 1)
            .saturating_mul(phase as usize + 3)
            .saturating_mul(131))
            % 4096;
        let output = Command::new(&executables[variant])
            .args(&case.args)
            .env("RUSTY_LAYOUT_PAD", "p".repeat(pad))
            .output()
            .map_err(|error| {
                format!(
                    "RustyBench case `{}` ({label}) failed to start: {error}",
                    case.id
                )
            })?;
        if !output.status.success() {
            return Err(format!(
                "RustyBench case `{}` ({label}) exited with {}",
                case.id, output.status
            ));
        }
        let checksum = stdout_checksum(&output.stdout);
        if checksum != case.expected_stdout_checksum {
            return Err(format!(
                "RustyBench oracle mismatch for `{}` ({label}): expected {:016x}, found {checksum:016x}",
                case.id, case.expected_stdout_checksum
            ));
        }
        Ok(())
    };
    invoke(0)?;
    for sample in 0..warmup {
        invoke(sample)?;
    }
    let mut samples_ns = Vec::with_capacity(repeat);
    for sample in 0..repeat {
        let start = Instant::now();
        invoke(warmup + sample)?;
        samples_ns.push(start.elapsed().as_nanos());
    }
    let statistics = Statistics::from_samples(&samples_ns).expect("repeat count is positive");
    Ok(CompareColumn {
        label,
        samples_ns,
        statistics,
    })
}

fn write_compare_json(json: &mut String, result: &CompareCase) {
    let comparison = result.search_comparison();
    let native = result.native_comparison();
    let native_speedup = (result.search_native.statistics.median_ns > 0.0).then_some(
        result.reference_native.statistics.median_ns / result.search_native.statistics.median_ns,
    );
    write!(
        json,
        "    {{\"id\": \"{}\", \"category\": \"{}\", \"split\": \"{}\", \"pipeline\": \"{}\", \"fallback\": {}, \"o2_median_ns\": {:.3}, \"o3_median_ns\": {:.3}, \"o3_native_median_ns\": {:.3}, \"search_median_ns\": {:.3}, \"search_native_median_ns\": {:.3}, \"speedup_o2_vs_o3\": {}, \"speedup_search_vs_o3\": {}, \"speedup_search_vs_native\": {}, \"search_speedup_ci95\": [{}, {}], \"search_native_speedup_ci95\": [{}, {}], \"search_clears_10_percent\": {}, \"search_native_clears_10_percent\": {}}}",
        json_escape(&result.id),
        result.category.as_str(),
        result.split.as_str(),
        json_escape(&result.pipeline),
        result
            .fallback
            .as_deref()
            .map(|reason| format!("\"{}\"", json_escape(reason)))
            .unwrap_or_else(|| "null".to_owned()),
        result.reference_o2.statistics.median_ns,
        result.reference_o3.statistics.median_ns,
        result.reference_native.statistics.median_ns,
        result.search.statistics.median_ns,
        result.search_native.statistics.median_ns,
        json_optional(result.speedup_vs_o3(&result.reference_o2)),
        json_optional(result.speedup_vs_o3(&result.search)),
        json_optional(native_speedup),
        comparison
            .as_ref()
            .map(|comparison| format!("{:.6}", comparison.speedup_ci95_low))
            .unwrap_or_else(|| "null".to_owned()),
        comparison
            .as_ref()
            .map(|comparison| format!("{:.6}", comparison.speedup_ci95_high))
            .unwrap_or_else(|| "null".to_owned()),
        native
            .as_ref()
            .map(|comparison| format!("{:.6}", comparison.speedup_ci95_low))
            .unwrap_or_else(|| "null".to_owned()),
        native
            .as_ref()
            .map(|comparison| format!("{:.6}", comparison.speedup_ci95_high))
            .unwrap_or_else(|| "null".to_owned()),
        comparison
            .as_ref()
            .is_some_and(|comparison| comparison.speedup_ci95_low >= 1.10),
        native
            .as_ref()
            .is_some_and(|comparison| comparison.speedup_ci95_low >= 1.10)
    )
    .unwrap();
}

fn json_optional(value: Option<f64>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| format!("{value:.6}"))
}

fn optional_number(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |number| number.to_string())
}

fn cpu_frequency_khz() -> Option<u64> {
    fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq")
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn validate_separation(cases: &[Case]) -> Result<()> {
    let mut ids = BTreeSet::new();
    let mut projects = BTreeMap::<PathBuf, Split>::new();
    for case in cases {
        if !ids.insert(&case.id) {
            return Err(format!("duplicate RustyBench case id `{}`", case.id));
        }
        let key = case
            .project
            .canonicalize()
            .unwrap_or_else(|_| case.project.clone());
        if let Some(previous) = projects.insert(key, case.split)
            && previous != case.split
        {
            return Err(format!(
                "RustyBench project `{}` appears in both `{}` and `{}` splits",
                case.project.display(),
                previous.as_str(),
                case.split.as_str()
            ));
        }
    }
    Ok(())
}

fn parse_checksum(value: &str) -> Result<u64> {
    u64::from_str_radix(value.trim_start_matches("0x"), 16)
        .map_err(|_| format!("invalid FNV-1a checksum `{value}`"))
}

pub fn checksum_file(path: &Path) -> Result<u64> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot checksum {}: {error}", path.display()))?;
    Ok(checksum(&bytes))
}

pub fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn stdout_checksum(bytes: &[u8]) -> u64 {
    if !bytes.windows(2).any(|pair| pair == b"\r\n") {
        return checksum(bytes);
    }
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"\r\n") {
            normalized.push(b'\n');
            index += 2;
        } else {
            normalized.push(bytes[index]);
            index += 1;
        }
    }
    checksum(&normalized)
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

fn write_case_json(json: &mut String, result: &CaseResult) {
    write!(
        json,
        "    {{\"id\": \"{}\", \"category\": \"{}\", \"split\": \"{}\", \"source_fnv1a64\": \"{:016x}\", \"stdout_fnv1a64\": \"{:016x}\", \"median_ns\": {:.0}, \"ci95_median_ns\": [{:.3}, {:.3}], \"mean_ns\": {:.3}, \"variance_ns2\": {:.3}, \"ci95_mean_normal_approx_ns\": [{:.3}, {:.3}], \"tukey_outliers\": {}, \"samples_ns\": [",
        json_escape(&result.id),
        result.category.as_str(),
        result.split.as_str(),
        result.source_checksum,
        result.stdout_checksum,
        result.statistics.median_ns,
        result.statistics.median_ci95_low_ns,
        result.statistics.median_ci95_high_ns,
        result.statistics.mean_ns,
        result.statistics.variance_ns2,
        result.statistics.mean_ci95_low_ns,
        result.statistics.mean_ci95_high_ns,
        result.statistics.tukey_outliers,
    )
    .unwrap();
    for (index, sample) in result.samples_ns.iter().enumerate() {
        if index != 0 {
            json.push_str(", ");
        }
        write!(json, "{sample}").unwrap();
    }
    json.push_str("]}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_development_suite_is_frozen() {
        let bytes = include_bytes!("../examples/kernels/suite.tsv");
        assert_eq!(checksum(bytes), 0x990e_917b_9a65_fba2);
    }

    #[test]
    fn checksum_is_stable() {
        assert_eq!(checksum(b"hello"), 0xa430_d846_80aa_bd0b);
        assert_eq!(stdout_checksum(b"hello\r\n"), checksum(b"hello\n"));
    }

    #[test]
    fn rejects_cross_split_project_reuse() {
        let base = Case {
            id: "one".to_owned(),
            category: Category::Algorithm,
            split: Split::Train,
            project: PathBuf::from("same"),
            source_checksum: 1,
            expected_stdout_checksum: 2,
            args: Vec::new(),
        };
        let mut other = base.clone();
        other.id = "two".to_owned();
        other.split = Split::FinalHeldOut;
        assert!(validate_separation(&[base, other]).is_err());
    }

    #[test]
    fn escapes_json_control_characters() {
        assert_eq!(json_escape("a\n\"b"), "a\\n\\\"b");
    }
}
