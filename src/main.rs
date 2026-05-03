mod exact;
mod opb;
mod printemps;
mod signals;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

const DEFAULT_EXACT_TIME_SEC: f64 = 300.0;

struct Args {
    instance: PathBuf,
    exact_time: f64,
    overall_time: Option<f64>,
    exact_path: PathBuf,
    printemps_path: PathBuf,
    save_dir: PathBuf,
    seed: Option<i64>,
    threads: Option<i32>,
    extra_exact: Vec<String>,
    extra_printemps: Vec<String>,
    verbose: bool,
}

fn print_usage() {
    eprintln!("\npb26-hybrid: PRINTEMPS + Exact hybrid driver for PB Competition 2026\n");
    eprintln!(
        "Usage: pb26-hybrid [OPTIONS] <instance.opb>\n\n\
         Options:\n  \
           --exact-time SEC        Time budget for the Exact phase (default: {default_exact}s).\n  \
           -t, --time-max SEC      Overall time budget; PRINTEMPS uses what's left.\n  \
           --exact-path PATH       Path to the Exact binary (default: ./bin/Exact).\n  \
           --printemps-path PATH   Path to pb_competition_2025_solver\n                          (default: ./bin/pb_competition_2025_solver).\n  \
           --save-dir DIR          Directory for state files (default: ./.pb26-state).\n  \
           -r, --seed N            Random seed forwarded to both solvers.\n  \
           -j, --threads N         Number of threads forwarded to both solvers.\n  \
           --exact-arg ARG         Extra argument to forward to Exact (repeatable).\n  \
           --printemps-arg ARG     Extra argument to forward to PRINTEMPS (repeatable).\n  \
           --verbose               Enable driver-level logs on stderr.\n  \
           -h, --help              Show this help and exit.\n",
        default_exact = DEFAULT_EXACT_TIME_SEC
    );
}

fn parse_args() -> Result<Args, String> {
    let argv: Vec<String> = env::args().collect();
    let mut instance: Option<PathBuf> = None;
    let mut exact_time: f64 = DEFAULT_EXACT_TIME_SEC;
    let mut overall_time: Option<f64> = None;
    let mut exact_path = default_exact_path();
    let mut printemps_path = default_printemps_path();
    let mut save_dir = exact::default_save_dir();
    let mut seed: Option<i64> = None;
    let mut threads: Option<i32> = None;
    let mut extra_exact: Vec<String> = Vec::new();
    let mut extra_printemps: Vec<String> = Vec::new();
    let mut verbose = false;

    let mut i = 1;
    while i < argv.len() {
        let a = argv[i].as_str();
        match a {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "--exact-time" => {
                exact_time = next_arg(&argv, &mut i, "--exact-time")?
                    .parse()
                    .map_err(|e| format!("invalid --exact-time: {e}"))?;
            }
            "-t" | "--time-max" => {
                let v: f64 = next_arg(&argv, &mut i, "--time-max")?
                    .parse()
                    .map_err(|e| format!("invalid --time-max: {e}"))?;
                overall_time = Some(v);
            }
            "--exact-path" => {
                exact_path = PathBuf::from(next_arg(&argv, &mut i, "--exact-path")?);
            }
            "--printemps-path" => {
                printemps_path = PathBuf::from(next_arg(&argv, &mut i, "--printemps-path")?);
            }
            "--save-dir" => {
                save_dir = PathBuf::from(next_arg(&argv, &mut i, "--save-dir")?);
            }
            "-r" | "--seed" => {
                let v: i64 = next_arg(&argv, &mut i, "--seed")?
                    .parse()
                    .map_err(|e| format!("invalid --seed: {e}"))?;
                seed = Some(v);
            }
            "-j" | "--threads" => {
                let v: i32 = next_arg(&argv, &mut i, "--threads")?
                    .parse()
                    .map_err(|e| format!("invalid --threads: {e}"))?;
                threads = Some(v);
            }
            "--exact-arg" => {
                extra_exact.push(next_arg(&argv, &mut i, "--exact-arg")?);
            }
            "--printemps-arg" => {
                extra_printemps.push(next_arg(&argv, &mut i, "--printemps-arg")?);
            }
            "--verbose" => {
                verbose = true;
                i += 1;
            }
            x if x.starts_with('-') => {
                return Err(format!("unknown option: {x}"));
            }
            _ => {
                if instance.is_some() {
                    return Err(format!("unexpected positional argument: {a}"));
                }
                instance = Some(PathBuf::from(a));
                i += 1;
            }
        }
    }

    let instance = instance.ok_or_else(|| "missing <instance.opb>".to_string())?;

    if exact_time < 0.0 {
        return Err("--exact-time must be non-negative".into());
    }
    if let Some(t) = overall_time {
        if t < 0.0 {
            return Err("--time-max must be non-negative".into());
        }
    }

    Ok(Args {
        instance,
        exact_time,
        overall_time,
        exact_path,
        printemps_path,
        save_dir,
        seed,
        threads,
        extra_exact,
        extra_printemps,
        verbose,
    })
}

fn next_arg(argv: &[String], i: &mut usize, name: &str) -> Result<String, String> {
    if *i + 1 >= argv.len() {
        return Err(format!("{name} requires an argument"));
    }
    let v = argv[*i + 1].clone();
    *i += 2;
    Ok(v)
}

fn default_exact_path() -> PathBuf {
    if let Ok(p) = env::var("PB26_EXACT") {
        return PathBuf::from(p);
    }
    PathBuf::from("./bin/Exact")
}

fn default_printemps_path() -> PathBuf {
    if let Ok(p) = env::var("PB26_PRINTEMPS") {
        return PathBuf::from(p);
    }
    PathBuf::from("./bin/pb_competition_2025_solver")
}

fn driver_log(verbose: bool, msg: &str) {
    if verbose {
        eprintln!("[pb26-hybrid] {msg}");
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let started = Instant::now();

    if !args.instance.exists() {
        return Err(format!("instance file not found: {}", args.instance.display()));
    }
    if !args.exact_path.exists() {
        return Err(format!("Exact binary not found: {}", args.exact_path.display()));
    }
    if !args.printemps_path.exists() {
        return Err(format!(
            "PRINTEMPS binary not found: {}",
            args.printemps_path.display()
        ));
    }

    fs::create_dir_all(&args.save_dir)
        .map_err(|e| format!("cannot create save-dir {}: {e}", args.save_dir.display()))?;

    let opb_info = opb::scan(&args.instance).map_err(|e| format!("cannot read OPB: {e}"))?;
    driver_log(
        args.verbose,
        &format!(
            "instance={} has_objective={}",
            args.instance.display(),
            opb_info.has_objective
        ),
    );

    let (child_slot, interrupt_flag) = signals::install_forwarder();

    // -----------------------------------------------------------------
    // Phase 1: Exact
    // -----------------------------------------------------------------
    let exact_budget = match args.overall_time {
        Some(t) => args.exact_time.min(t),
        None => args.exact_time,
    };

    let log_path = args.save_dir.join("exact_log.txt");
    let bounds_path = args.save_dir.join("exact_bounds.json");
    let incumbent_pb_path = args.save_dir.join("exact_incumbent_pb.txt");
    let incumbent_sol_path = args.save_dir.join("exact_incumbent.sol");

    println!("c pb26-hybrid: phase 1 (Exact, budget={:.3}s)", exact_budget);
    let exact_run = exact::run(exact::ExactConfig {
        exact_path: &args.exact_path,
        instance: &args.instance,
        timeout_sec: exact_budget,
        extra_args: &args.extra_exact,
        log_path: &log_path,
        bounds_path: &bounds_path,
        incumbent_pb_path: &incumbent_pb_path,
        incumbent_sol_path: &incumbent_sol_path,
        child_slot: &child_slot,
    })
    .map_err(|e| format!("Exact phase failed: {e}"))?;
    driver_log(
        args.verbose,
        &format!(
            "Exact verdict={:?} elapsed={:.3}s exit={:?}",
            exact_run.verdict, exact_run.elapsed_sec, exact_run.exit_code
        ),
    );

    let is_final = match exact_run.verdict {
        exact::Verdict::OptimumFound => true,
        exact::Verdict::Unsatisfiable => true,
        exact::Verdict::Satisfiable => !opb_info.has_objective,
        exact::Verdict::Unknown | exact::Verdict::NoStatus => false,
    };

    if is_final {
        exact::flush_buffered_lines(&exact_run, false)
            .map_err(|e| format!("failed to flush Exact final lines: {e}"))?;
        return Ok(());
    }

    // If the driver itself was interrupted (SIGINT/SIGTERM/SIGXCPU), we honour
    // the request and skip the PRINTEMPS phase. Whatever incumbent Exact has
    // is the best answer we can give.
    if interrupt_flag.is_set() {
        driver_log(args.verbose, "interrupt received during phase 1; skipping PRINTEMPS");
        emit_best_after_interrupt(&exact_run)?;
        return Ok(());
    }

    exact::flush_buffered_lines(&exact_run, true)
        .map_err(|e| format!("failed to comment out Exact lines: {e}"))?;

    // -----------------------------------------------------------------
    // Phase 2: PRINTEMPS
    // -----------------------------------------------------------------
    let elapsed_total = started.elapsed().as_secs_f64();
    let printemps_time = match args.overall_time {
        Some(t) => {
            let remaining = t - elapsed_total;
            if remaining <= 0.0 {
                println!("c pb26-hybrid: overall time budget already exhausted before PRINTEMPS");
                return Ok(());
            }
            Some(remaining)
        }
        None => None,
    };

    let p_log_path = args.save_dir.join("printemps_log.txt");

    match printemps_time {
        Some(t) => println!("c pb26-hybrid: phase 2 (PRINTEMPS, budget={:.3}s)", t),
        None => println!("c pb26-hybrid: phase 2 (PRINTEMPS, no time limit)"),
    }
    let p_run = printemps::run(printemps::PrintempsConfig {
        solver_path: &args.printemps_path,
        instance: &args.instance,
        time_max: printemps_time,
        iteration_max: Some(-1),
        seed: args.seed,
        threads: args.threads,
        extra_args: &args.extra_printemps,
        log_path: &p_log_path,
        child_slot: &child_slot,
    })
    .map_err(|e| format!("PRINTEMPS phase failed: {e}"))?;
    driver_log(
        args.verbose,
        &format!(
            "PRINTEMPS verdict={:?} exit={:?}",
            p_run.verdict, p_run.exit_code
        ),
    );

    // Fallback: PRINTEMPS produced no feasible solution but Exact captured one.
    let needs_fallback = matches!(
        p_run.verdict,
        printemps::PrintempsVerdict::Unknown
            | printemps::PrintempsVerdict::Unsupported
            | printemps::PrintempsVerdict::NoStatus
    ) && exact_run.last_v_line.is_some();

    if needs_fallback {
        let v = exact_run.last_v_line.as_deref().unwrap();
        printemps::emit_fallback(
            "pb26-hybrid: PRINTEMPS did not improve; falling back to Exact incumbent",
            v,
        )
        .map_err(|e| format!("failed to emit fallback: {e}"))?;
    }

    Ok(())
}

fn emit_best_after_interrupt(exact_run: &exact::ExactRun) -> Result<(), String> {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    match exact_run.verdict {
        exact::Verdict::OptimumFound | exact::Verdict::Unsatisfiable => {
            // Exact already produced a final answer; just emit it.
            if let Some(ref s) = exact_run.last_s_line {
                writeln!(h, "{}", s).map_err(|e| e.to_string())?;
            }
            if let Some(ref v) = exact_run.last_v_line {
                writeln!(h, "{}", v).map_err(|e| e.to_string())?;
            }
        }
        exact::Verdict::Satisfiable => {
            // Best feasible-but-not-proven-optimal incumbent.
            writeln!(h, "s SATISFIABLE").map_err(|e| e.to_string())?;
            if let Some(ref v) = exact_run.last_v_line {
                writeln!(h, "{}", v).map_err(|e| e.to_string())?;
            }
        }
        exact::Verdict::Unknown | exact::Verdict::NoStatus => {
            if let Some(ref v) = exact_run.last_v_line {
                writeln!(h, "s SATISFIABLE").map_err(|e| e.to_string())?;
                writeln!(h, "{}", v).map_err(|e| e.to_string())?;
            } else {
                writeln!(h, "s UNKNOWN").map_err(|e| e.to_string())?;
            }
        }
    }
    h.flush().map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::from(0),
        Err(msg) => {
            eprintln!("pb26-hybrid: error: {msg}");
            ExitCode::from(1)
        }
    }
}
