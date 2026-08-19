use rusty_lang::project::{self, Optimization, Project};
use rusty_lang::research::{self, Registry};
use rusty_lang::rustybench::{self, Split, Suite};
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

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

struct ProjectArgs {
    path: Option<PathBuf>,
    optimization: Optimization,
    neural_requested: bool,
    performance_gate: bool,
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
        optimization: Optimization::O0,
        neural_requested: false,
        performance_gate: false,
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
            "-O0" => parsed.optimization = Optimization::O0,
            "-O2" => parsed.optimization = Optimization::O2,
            "-O3" => parsed.optimization = Optimization::O3,
            "-Oai" => {
                parsed.optimization = Optimization::O3;
                parsed.neural_requested = true;
            }
            "--performance-gate" => {
                parsed.neural_requested = true;
                parsed.performance_gate = true;
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
        "build" => {
            fallback_notice(args.neural_requested);
            let artifacts = project.build(args.optimization)?;
            print_artifacts(&project, &artifacts);
        }
        "run" => {
            fallback_notice(args.neural_requested);
            let status = project.run(args.optimization, &args.program_args)?;
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
            let report = project.bench(args.repeat, args.warmup, &args.program_args)?;
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
            eprintln!("neural backend unavailable: no candidates were generated or measured");
            eprintln!("fallback: building the trusted reference backend with clang -O3");
            let artifacts = project.build(Optimization::O3)?;
            print_artifacts(&project, &artifacts);
            println!(
                "selection=REFERENCE_FALLBACK reason=NEURAL_UNAVAILABLE budget={} target_cpu={} performance_gate={} minimum_speedup={:.6}",
                args.budget, args.target_cpu, args.performance_gate, args.minimum_speedup
            );
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

fn command_benchmark_suite(manifest: &std::path::Path, args: &ProjectArgs) -> Result<u8, String> {
    if args.path.is_some() {
        return Err("a project path cannot be combined with --suite".to_owned());
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

fn fallback_notice(neural_requested: bool) {
    if neural_requested {
        eprintln!(
            "neural backend unavailable: using trusted reference fallback (no neural performance claim)"
        );
    } else {
        eprintln!("backend: trusted reference (neural backend unavailable in this milestone)");
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
    println!("  build [project] [-O0|-O2|-O3|-Oai]");
    println!("  run [project] [-- args...] Build and execute");
    println!("  test [project]             Check and run tests/*.ry");
    println!("  bench [project] [--warmup N] [--repeat N] [-- args...]");
    println!("  bench --suite FILE [--split NAME] [--output FILE]");
    println!("  optimize [project] [--budget N] [--target-cpu CPU]");
    println!("                     [--performance-gate] [--minimum-speedup 2%]");
    println!("  research list|run|compare|report");
    println!("  clean [project]            Remove project build artifacts");
}
