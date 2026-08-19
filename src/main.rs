use rusty_lang::project::{self, Optimization, Project};
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

struct ProjectArgs {
    path: Option<PathBuf>,
    optimization: Optimization,
    neural_requested: bool,
    repeat: usize,
    program_args: Vec<String>,
}

fn parse_project_args(args: &[String]) -> Result<ProjectArgs, String> {
    let mut parsed = ProjectArgs {
        path: None,
        optimization: Optimization::O0,
        neural_requested: false,
        repeat: 10,
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
            "--performance-gate" => parsed.neural_requested = true,
            "--target-cpu" | "--minimum-speedup" | "--budget" | "--repeat" => {
                let flag = args[index].clone();
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("{flag} requires a value"))?;
                match flag.as_str() {
                    "--repeat" => {
                        parsed.repeat = value
                            .parse()
                            .map_err(|_| "--repeat requires a positive integer")?
                    }
                    "--budget" => {
                        let _: usize = value.parse().map_err(|_| "--budget requires an integer")?;
                        parsed.neural_requested = true;
                    }
                    "--target-cpu" | "--minimum-speedup" => parsed.neural_requested = true,
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

fn command_project(command: &str, args: &[String]) -> Result<u8, String> {
    let args = parse_project_args(args)?;
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
                "note: reference-only wall-clock sample; this is not a publishable statistical comparison"
            );
            fallback_notice(args.neural_requested);
            let (median, minimum, repeat) = project.bench(args.repeat)?;
            println!("reference samples={repeat} median_ns={median} min_ns={minimum}");
        }
        "optimize" => {
            eprintln!("neural backend unavailable: no candidates were generated or measured");
            eprintln!("fallback: building the trusted reference backend with clang -O3");
            let artifacts = project.build(Optimization::O3)?;
            print_artifacts(&project, &artifacts);
            println!("selection=REFERENCE_FALLBACK reason=NEURAL_UNAVAILABLE");
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
    println!("  bench [project] [--repeat N]");
    println!("  optimize [project]         Honest reference fallback");
    println!("  clean [project]            Remove project build artifacts");
}
