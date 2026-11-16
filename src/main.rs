use rusty_lang::analyze;
use rusty_lang::neural::Options as NeuralOptions;
use rusty_lang::optimization;
use rusty_lang::project::{self, Optimization, Project};
use rusty_lang::research::{self, Registry};
use rusty_lang::rewrite::{self, SearchOutcome};
use rusty_lang::rustybench::{self, Split, Suite};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<u8, String> {
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        return Err("missing command".to_owned());
    };
    if matches!(command, "help" | "--help" | "-h") {
        print_usage();
        return Ok(0);
    }
    if matches!(command, "version" | "--version" | "-V") {
        println!("rusty {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    match command {
        "new" => command_new(&args[1..]),
        "research" => command_research(&args[1..]),
        "build" | "run" | "check" | "test" | "bench" | "optimize" | "clean" => {
            command_project(command, &args[1..])
        }
        _ => Err(format!("unknown command `{command}`; run `rusty help`")),
    }
}

fn command_new(args: &[String]) -> Result<u8, String> {
    if args.len() != 1 {
        return Err("usage: rusty new <name>".to_owned());
    }
    let root = project::create(&args[0])?;
    println!("Created {}", root.display());
    Ok(0)
}

fn command_research(args: &[String]) -> Result<u8, String> {
    let Some(action) = args.first().map(String::as_str) else {
        return Err("usage: rusty research <list|run|compare|report> [experiment]".to_owned());
    };
    let workspace = std::env::current_dir()
        .map_err(|error| format!("cannot determine workspace directory: {error}"))?;
    let registry_path = std::env::var_os("RUSTY_RESEARCH_REGISTRY")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("research").join("registry"));
    let registry = Registry::load(&registry_path)?;
    match action {
        "list" => {
            if args.len() != 1 {
                return Err("usage: rusty research list".to_owned());
            }
            for experiment in &registry.experiments {
                println!(
                    "{} status={} runner={} title={}",
                    experiment.id,
                    experiment.status,
                    experiment.runner.as_deref().unwrap_or("unassigned"),
                    experiment.title
                );
            }
        }
        "run" => {
            let id = args
                .get(1)
                .ok_or_else(|| "usage: rusty research run <experiment> [--repeat N]".to_owned())?;
            let mut repeat = 1;
            if args.len() > 2 {
                if args.get(2).map(String::as_str) != Some("--repeat") || args.len() != 4 {
                    return Err("usage: rusty research run <experiment> [--repeat N]".to_owned());
                }
                repeat = args[3]
                    .parse()
                    .map_err(|_| "--repeat requires a positive integer")?;
            }
            let output = research::run_experiment(&workspace, registry.find(id)?, repeat)?;
            println!("record={}", output.display());
        }
        "compare" => {
            if args.len() != 2 {
                return Err("usage: rusty research compare <experiment>".to_owned());
            }
            registry.find(&args[1])?;
            let summary = research::compare(&workspace, &args[1])?;
            println!(
                "experiment={} records={} successful_trials={} failed_trials={}",
                args[1], summary.records, summary.successful_trials, summary.failed_trials
            );
        }
        "report" => {
            if args.len() != 2 {
                return Err("usage: rusty research report <experiment>".to_owned());
            }
            let output = research::write_report(&workspace, registry.find(&args[1])?)?;
            println!("report={}", output.display());
        }
        _ => return Err(format!("unknown research command `{action}`")),
    }
    Ok(0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Backend {
    Reference,
    Search,
    External,
}

struct ProjectArgs {
    path: Option<PathBuf>,
    optimization: Option<Optimization>,
    backend: Option<Backend>,
    compare: bool,
    neural_requested: bool,
    budget: usize,
    target_cpu: String,
    minimum_speedup: f64,
    repeat: usize,
    warmup: usize,
    suite: Option<PathBuf>,
    output: Option<PathBuf>,
    split: Option<Split>,
    program_args: Vec<String>,
}

fn parse_project_args(args: &[String]) -> Result<ProjectArgs, String> {
    let mut parsed = ProjectArgs {
        path: None,
        optimization: None,
        backend: None,
        compare: false,
        neural_requested: false,
        budget: 8,
        target_cpu: "native".to_owned(),
        minimum_speedup: 0.02,
        repeat: 30,
        warmup: 3,
        suite: None,
        output: None,
        split: None,
        program_args: Vec::new(),
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--" => {
                parsed.program_args.extend_from_slice(&args[index + 1..]);
                break;
            }
            "-O0" => parsed.optimization = Some(Optimization::O0),
            "-O2" => parsed.optimization = Some(Optimization::O2),
            "-O3" => parsed.optimization = Some(Optimization::O3),
            "-Oai" => {
                parsed.optimization = Some(Optimization::O3);
                parsed.neural_requested = true;
                parsed.backend = Some(Backend::External);
            }
            "--compare" => parsed.compare = true,
            "--backend" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    "--backend requires reference, search, or external".to_owned()
                })?;
                parsed.backend = Some(match value.as_str() {
                    "reference" => Backend::Reference,
                    "search" => Backend::Search,
                    "external" => Backend::External,
                    _ => {
                        return Err("--backend requires reference, search, or external".to_owned());
                    }
                });
            }
            "--performance-gate" => {
                parsed.neural_requested = true;
            }
            "--target-cpu" | "--minimum-speedup" | "--budget" | "--repeat" | "--warmup"
            | "--suite" | "--output" | "--split" => {
                let flag = args[index].clone();
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("{flag} requires a value"))?;
                match flag.as_str() {
                    "--repeat" => {
                        parsed.repeat = value
                            .parse()
                            .map_err(|_| "--repeat requires a positive integer")?;
                        if parsed.repeat == 0 {
                            return Err("--repeat requires a positive integer".to_owned());
                        }
                    }
                    "--warmup" => {
                        parsed.warmup = value
                            .parse()
                            .map_err(|_| "--warmup requires a non-negative integer")?
                    }
                    "--budget" => {
                        parsed.budget = value
                            .parse()
                            .map_err(|_| "--budget requires a positive integer")?;
                        if parsed.budget == 0 {
                            return Err("--budget requires a positive integer".to_owned());
                        }
                        parsed.neural_requested = true;
                    }
                    "--target-cpu" => {
                        if value.is_empty()
                            || !value.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                            })
                        {
                            return Err("--target-cpu contains invalid characters".to_owned());
                        }
                        parsed.target_cpu.clone_from(value);
                        parsed.neural_requested = true;
                    }
                    "--minimum-speedup" => {
                        parsed.minimum_speedup = parse_speedup(value)?;
                        parsed.neural_requested = true;
                    }
                    "--suite" => parsed.suite = Some(PathBuf::from(value)),
                    "--output" => parsed.output = Some(PathBuf::from(value)),
                    "--split" => parsed.split = Some(Split::parse(value)?),
                    _ => unreachable!(),
                }
            }
            value if value.starts_with('-') => return Err(format!("unknown option `{value}`")),
            value => {
                if parsed.path.is_some() {
                    return Err(format!(
                        "unexpected argument `{value}`; use `--` before program arguments"
                    ));
                }
                parsed.path = Some(PathBuf::from(value));
            }
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_speedup(value: &str) -> Result<f64, String> {
    let (number, divisor) = value
        .strip_suffix('%')
        .map_or((value, 1.0), |number| (number, 100.0));
    let parsed = number
        .parse::<f64>()
        .map_err(|_| "--minimum-speedup requires a non-negative number or percentage")?
        / divisor;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err("--minimum-speedup requires a non-negative finite value".to_owned());
    }
    Ok(parsed)
}

fn command_project(command: &str, args: &[String]) -> Result<u8, String> {
    let args = parse_project_args(args)?;
    if command == "bench"
        && let Some(manifest) = args.suite.as_deref()
    {
        return command_benchmark_suite(manifest, &args);
    }
    let project = Project::discover(args.path.as_deref())?;
    match command {
        "check" => {
            project.check()?;
            println!("Checked {}", project.name);
        }
        "build" => match selected_backend(&args) {
            Backend::Search => {
                let (artifacts, outcome, fallback) =
                    build_with_search(&project, clang_level(&args))?;
                print_artifacts(&project, &artifacts);
                print_search_outcome(&outcome, fallback.as_deref());
            }
            Backend::External => {
                let outcome = run_optimization(&project, &args)?;
                print_artifacts(&project, &outcome.artifacts);
                print_optimization_outcome(&outcome, &args);
            }
            Backend::Reference => {
                fallback_notice(false);
                let artifacts = project.build(clang_level(&args))?;
                print_artifacts(&project, &artifacts);
            }
        },
        "run" => {
            let status = match selected_backend(&args) {
                Backend::Search => {
                    let (artifacts, outcome, fallback) =
                        build_with_search(&project, clang_level(&args))?;
                    print_search_outcome(&outcome, fallback.as_deref());
                    Command::new(&artifacts.executable)
                        .args(&args.program_args)
                        .status()
                        .map_err(|error| format!("cannot run searched program: {error}"))?
                }
                Backend::External => {
                    let outcome = run_optimization(&project, &args)?;
                    print_optimization_outcome(&outcome, &args);
                    Command::new(&outcome.artifacts.executable)
                        .args(&args.program_args)
                        .status()
                        .map_err(|error| format!("cannot run optimized program: {error}"))?
                }
                Backend::Reference => {
                    fallback_notice(false);
                    project.run(clang_level(&args), &args.program_args)?
                }
            };
            if !status.success() {
                return Ok(status.code().unwrap_or(1).clamp(1, 255) as u8);
            }
        }
        "test" => {
            fallback_notice(args.neural_requested);
            let passed = project.test()?;
            println!("Test result: ok. {passed} project tests passed; source check passed");
        }
        "bench" => {
            eprintln!(
                "note: reference-only whole-process timing; use --suite for frozen private RustyBench inputs"
            );
            fallback_notice(args.neural_requested);
            let report = project.bench(
                args.optimization.unwrap_or(Optimization::O3),
                args.repeat,
                args.warmup,
                &args.program_args,
            )?;
            let statistics = report.statistics;
            println!(
                "reference warmup={} samples={} min_ns={} max_ns={}",
                report.warmup_runs,
                report.samples_ns.len(),
                statistics.minimum_ns,
                statistics.maximum_ns
            );
            println!(
                "median_ns={:.0} median_ci95_ns=[{:.0},{:.0}] mean_ns={:.0} variance_ns2={:.0} stddev_ns={:.0}",
                statistics.median_ns,
                statistics.median_ci95_low_ns,
                statistics.median_ci95_high_ns,
                statistics.mean_ns,
                statistics.variance_ns2,
                statistics.standard_deviation_ns
            );
            println!(
                "mean_ci95_ns=[{:.0},{:.0}] tukey_outliers={} outlier_policy=retain-all",
                statistics.mean_ci95_low_ns,
                statistics.mean_ci95_high_ns,
                statistics.tukey_outliers
            );
        }
        "optimize" => {
            let outcome = run_optimization(&project, &args)?;
            print_artifacts(&project, &outcome.artifacts);
            print_optimization_outcome(&outcome, &args);
        }
        "clean" => {
            if project.clean()? {
                println!("Removed {}", project.root.join("build").display());
            } else {
                println!("Already clean: {}", project.name);
            }
        }
        _ => unreachable!(),
    }
    Ok(0)
}

fn run_optimization(
    project: &Project,
    args: &ProjectArgs,
) -> Result<optimization::Outcome, String> {
    let options = NeuralOptions::new(
        args.budget,
        args.target_cpu.clone(),
        args.minimum_speedup,
        args.warmup,
        args.repeat,
    )?;
    optimization::optimize(project, &options, &args.program_args)
}

fn print_optimization_outcome(outcome: &optimization::Outcome, args: &ProjectArgs) {
    for candidate in &outcome.candidates {
        if let Some(comparison) = &candidate.comparison {
            println!(
                "candidate={} classification={} generation_ms={} speedup={:.6} ci95=[{:.6},{:.6}] accepted={} reason={}",
                candidate.candidate,
                candidate.classification.as_str(),
                candidate.generation_ms,
                comparison.speedup,
                comparison.speedup_ci95_low,
                comparison.speedup_ci95_high,
                comparison.accepted,
                candidate.reason
            );
        } else {
            println!(
                "candidate={} classification={} generation_ms={} reason={}",
                candidate.candidate,
                candidate.classification.as_str(),
                candidate.generation_ms,
                candidate.reason
            );
        }
    }
    if let Some(candidate) = outcome.selected_candidate {
        println!(
            "selection=NEURAL candidate={} budget={} target_cpu={} minimum_speedup={:.6}",
            candidate, args.budget, args.target_cpu, args.minimum_speedup
        );
    } else {
        println!(
            "selection=REFERENCE_FALLBACK reason={} budget={} target_cpu={} minimum_speedup={:.6}",
            outcome
                .fallback_reason
                .as_deref()
                .unwrap_or("NO_ACCEPTED_CANDIDATE"),
            args.budget,
            args.target_cpu,
            args.minimum_speedup
        );
    }
}

fn command_benchmark_suite(manifest: &std::path::Path, args: &ProjectArgs) -> Result<u8, String> {
    if args.path.is_some() {
        return Err("a project path cannot be combined with --suite".to_owned());
    }
    if args.compare {
        return command_compare(manifest, args);
    }
    let suite = Suite::load(manifest)?;
    let report = rustybench::run_suite(&suite, args.warmup, args.repeat, args.split)?;
    println!(
        "RustyBench cases={} warmup={} repeat={} backend=reference-clang-o3",
        report.results.len(),
        report.warmup,
        report.repeat
    );
    for result in &report.results {
        println!(
            "{} category={} split={} median_ns={:.0} outliers={}",
            result.id,
            result.category.as_str(),
            result.split.as_str(),
            result.statistics.median_ns,
            result.statistics.tukey_outliers
        );
    }
    println!(
        "geometric_mean_median_ns={:.0} outlier_policy=retain-all",
        report.geometric_mean_median_ns().unwrap_or_default()
    );
    if let Some(path) = &args.output {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        fs::write(path, report.to_json())
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        println!("record={}", path.display());
    } else {
        eprintln!("note: no result record written; pass --output <private-path>.bench.json");
    }
    Ok(0)
}

fn selected_backend(args: &ProjectArgs) -> Backend {
    if let Some(backend) = args.backend {
        return backend;
    }
    if args.neural_requested || std::env::var_os("RUSTY_NEURAL_GENERATOR").is_some() {
        Backend::External
    } else {
        Backend::Reference
    }
}

fn clang_level(args: &ProjectArgs) -> Optimization {
    args.optimization.unwrap_or(Optimization::O0)
}

fn build_with_search(
    project: &Project,
    optimization: Optimization,
) -> Result<(project::Artifacts, SearchOutcome, Option<String>), String> {
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
    let (program, fallback) = if outcome.pipeline == "identity" {
        (hir.clone(), None)
    } else if rewrite::native_equivalent(&hir, &outcome.program)? {
        (outcome.program.clone(), None)
    } else {
        (
            hir.clone(),
            Some(format!(
                "native differential mismatch for pipeline `{}`",
                outcome.pipeline
            )),
        )
    };
    let artifacts = project::emit_executable(
        &program,
        &project.root.join("build"),
        &project.name,
        optimization,
    )?;
    Ok((artifacts, outcome, fallback))
}

fn print_search_outcome(outcome: &SearchOutcome, fallback: Option<&str>) {
    println!(
        "backend: rir-search pipeline={} predicted_cost={:.1} identity_cost={:.1} estimated_iterations={}",
        outcome.pipeline,
        outcome.predicted_cost,
        outcome.identity_cost,
        outcome.estimated_iterations
    );
    for candidate in &outcome.candidates {
        println!(
            "candidate={} status={} predicted_cost={:.1} iterations={} detail={}",
            candidate.pipeline,
            candidate.status,
            candidate.predicted_cost,
            candidate.estimated_iterations,
            candidate.detail
        );
    }
    if let Some(reason) = fallback {
        println!("selection=REFERENCE_FALLBACK reason={reason}");
    }
}

fn command_compare(manifest: &std::path::Path, args: &ProjectArgs) -> Result<u8, String> {
    let suite = Suite::load(manifest)?;
    let report = rustybench::run_comparison(&suite, args.warmup, args.repeat, args.split)?;
    println!(
        "RustyBench compare cases={} warmup={} repeat={} speedup=o3_median/candidate_median",
        report.results.len(),
        report.warmup,
        report.repeat
    );
    for case in &report.results {
        let search_speedup = case.speedup_vs_o3(&case.search);
        let o2_speedup = case.speedup_vs_o3(&case.reference_o2);
        let comparison = case.search_comparison();
        let (low, high, clears) = comparison
            .as_ref()
            .map(|value| {
                (
                    format!("{:.4}", value.speedup_ci95_low),
                    format!("{:.4}", value.speedup_ci95_high),
                    value.speedup_ci95_low >= 1.10,
                )
            })
            .unwrap_or_else(|| ("n/a".to_owned(), "n/a".to_owned(), false));
        println!(
            "{} pipeline={} fallback={} o2_ns={:.0} o3_ns={:.0} search_ns={:.0} speedup_o2={} speedup_search={} ci95=[{}, {}] clears_10pct={}",
            case.id,
            case.pipeline,
            case.fallback.as_deref().unwrap_or("no"),
            case.reference_o2.statistics.median_ns,
            case.reference_o3.statistics.median_ns,
            case.search.statistics.median_ns,
            o2_speedup
                .map(|value| format!("{value:.4}"))
                .unwrap_or_else(|| "n/a".to_owned()),
            search_speedup
                .map(|value| format!("{value:.4}"))
                .unwrap_or_else(|| "n/a".to_owned()),
            low,
            high,
            clears
        );
    }
    println!(
        "geometric_mean_speedup_vs_o3 search={} o2={}",
        report
            .geometric_mean_speedup(|case| case.speedup_vs_o3(&case.search))
            .map(|value| format!("{value:.4}"))
            .unwrap_or_else(|| "n/a".to_owned()),
        report
            .geometric_mean_speedup(|case| case.speedup_vs_o3(&case.reference_o2))
            .map(|value| format!("{value:.4}"))
            .unwrap_or_else(|| "n/a".to_owned())
    );
    if let Some(path) = &args.output {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        fs::write(path, report.to_json())
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        println!("record={}", path.display());
    }
    Ok(0)
}

fn fallback_notice(neural_requested: bool) {
    if neural_requested {
        eprintln!(
            "neural optimization is not applied to this command; using trusted reference output"
        );
    } else {
        eprintln!("backend: trusted reference");
    }
}

fn print_artifacts(project: &Project, artifacts: &project::Artifacts) {
    println!("Built {}", project.name);
    println!("  RIR: {}", artifacts.rir.display());
    println!("  LLVM: {}", artifacts.llvm.display());
    println!("  Binary: {}", artifacts.executable.display());
}

fn print_usage() {
    println!("Rusty trusted reference compiler");
    println!("usage: rusty <command> [project] [options]");
    println!("commands:");
    println!("  new <name>                 Create a project");
    println!("  check [project]            Parse, resolve, and type-check");
    println!("  build [project] [-O0|-O2|-O3|-Oai] [--backend reference|search|external]");
    println!("  run [project] [-- args...] Build and execute");
    println!("  test [project]             Check and run tests/*.ry");
    println!("  bench [project] [-O0|-O2|-O3] [--warmup N] [--repeat N] [-- args...]");
    println!("  bench --suite FILE [--split NAME] [--output FILE] [--compare]");
    println!("  optimize [project] [--budget N] [--target-cpu CPU]");
    println!("                     [--performance-gate] [--minimum-speedup 2%]");
    println!("  research list|run|compare|report");
    println!("  clean [project]            Remove project build artifacts");
}
