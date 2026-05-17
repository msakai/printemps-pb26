use pb_hybrid::{opb, printemps, scip, signals};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

const DEFAULT_SCIP_TIME_SEC: f64 = 300.0;

struct Args {
    instance: PathBuf,
    scip_time: f64,
    overall_time: Option<f64>,
    printemps_path: PathBuf,
    save_dir: PathBuf,
    seed: Option<i64>,
    threads: Option<i32>,
    scip_params: Vec<(String, String)>,
    extra_printemps: Vec<String>,
    verbose: bool,
}

fn print_usage() {
    eprintln!("\nscip-printemps: PRINTEMPS + SCIP hybrid driver for PB Competitions\n");
    eprintln!(
        "Usage: scip-printemps [OPTIONS] <instance.opb>\n\n\
         Options:\n  \
           --scip-time SEC         Time budget for the SCIP phase (default: {default_scip}s).\n  \
           -t, --time-max SEC      Overall time budget; PRINTEMPS uses what's left.\n  \
           --printemps-path PATH   Path to pb_competition_2025_solver\n                          (default: ./bin/pb_competition_2025_solver).\n  \
           --save-dir DIR          Directory for state files (default: ./.pb-scip-state).\n  \
           -r, --seed N            Random seed forwarded to both solvers.\n  \
           -j, --threads N         Number of threads forwarded to PRINTEMPS (SCIP ignores).\n  \
           --scip-arg NAME=VALUE   Extra SCIP parameter (repeatable).\n  \
           --printemps-arg ARG     Extra argument to forward to PRINTEMPS (repeatable).\n  \
           --verbose               Enable driver-level logs on stderr.\n  \
           -h, --help              Show this help and exit.\n",
        default_scip = DEFAULT_SCIP_TIME_SEC
    );
}

fn parse_args() -> Result<Args, String> {
    let argv: Vec<String> = env::args().collect();
    let mut instance: Option<PathBuf> = None;
    let mut scip_time: f64 = DEFAULT_SCIP_TIME_SEC;
    let mut overall_time: Option<f64> = None;
    let mut printemps_path = default_printemps_path();
    let mut save_dir = default_save_dir();
    let mut seed: Option<i64> = None;
    let mut threads: Option<i32> = None;
    let mut scip_params: Vec<(String, String)> = Vec::new();
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
            "--scip-time" => {
                scip_time = next_arg(&argv, &mut i, "--scip-time")?
                    .parse()
                    .map_err(|e| format!("invalid --scip-time: {e}"))?;
            }
            "-t" | "--time-max" => {
                let v: f64 = next_arg(&argv, &mut i, "--time-max")?
                    .parse()
                    .map_err(|e| format!("invalid --time-max: {e}"))?;
                overall_time = Some(v);
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
            "--scip-arg" => {
                let s = next_arg(&argv, &mut i, "--scip-arg")?;
                let (k, v) = s
                    .split_once('=')
                    .ok_or_else(|| format!("--scip-arg expects NAME=VALUE, got {s}"))?;
                scip_params.push((k.to_string(), v.to_string()));
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

    if scip_time < 0.0 {
        return Err("--scip-time must be non-negative".into());
    }
    if let Some(t) = overall_time {
        if t < 0.0 {
            return Err("--time-max must be non-negative".into());
        }
    }

    Ok(Args {
        instance,
        scip_time,
        overall_time,
        printemps_path,
        save_dir,
        seed,
        threads,
        scip_params,
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

fn default_printemps_path() -> PathBuf {
    if let Ok(p) = env::var("PB_PRINTEMPS") {
        return PathBuf::from(p);
    }
    PathBuf::from("./bin/pb_competition_2025_solver")
}

fn default_save_dir() -> PathBuf {
    PathBuf::from(".pb-scip-state")
}

fn driver_log(verbose: bool, msg: &str) {
    if verbose {
        eprintln!("[scip-printemps] {msg}");
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let started = Instant::now();

    if !args.instance.exists() {
        return Err(format!(
            "instance file not found: {}",
            args.instance.display()
        ));
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
    // Phase 1: SCIP
    // -----------------------------------------------------------------
    let scip_budget = match args.overall_time {
        Some(t) => args.scip_time.min(t),
        None => args.scip_time,
    };

    let scip_log_path = args.save_dir.join("scip_log.txt");
    let scip_bounds_path = args.save_dir.join("scip_bounds.json");
    let scip_incumbent_sol_path = args.save_dir.join("scip_incumbent.sol");
    let scip_fixed_vars_path = args.save_dir.join("scip_fixed_vars.txt");

    println!(
        "c scip-printemps: phase 1 (SCIP, budget={:.3}s)",
        scip_budget
    );
    let scip_run = scip::run(scip::ScipConfig {
        instance: &args.instance,
        timeout_sec: scip_budget,
        seed: args.seed,
        threads: args.threads,
        extra_params: &args.scip_params,
        log_path: &scip_log_path,
        bounds_path: &scip_bounds_path,
        incumbent_sol_path: &scip_incumbent_sol_path,
        fixed_vars_path: &scip_fixed_vars_path,
        interrupt_flag: &interrupt_flag,
    })
    .map_err(|e| format!("SCIP phase failed: {e}"))?;
    driver_log(
        args.verbose,
        &format!(
            "SCIP verdict={:?} elapsed={:.3}s primal={:?} dual={:?} n_fixed={}",
            scip_run.verdict,
            scip_run.elapsed_sec,
            scip_run.handoff.primal_bound,
            scip_run.handoff.dual_bound,
            scip_run.handoff.fixed_vars.len(),
        ),
    );

    let is_final = match scip_run.verdict {
        scip::ScipVerdict::OptimumFound => true,
        scip::ScipVerdict::Unsatisfiable => true,
        scip::ScipVerdict::Satisfiable => !opb_info.has_objective,
        scip::ScipVerdict::Unknown => false,
    };

    if is_final {
        scip::flush_buffered_lines(&scip_run, false)
            .map_err(|e| format!("failed to flush SCIP final lines: {e}"))?;
        return Ok(());
    }

    if interrupt_flag.is_set() {
        driver_log(
            args.verbose,
            "interrupt received during phase 1; skipping PRINTEMPS",
        );
        emit_best_after_interrupt(&scip_run)?;
        return Ok(());
    }

    scip::flush_buffered_lines(&scip_run, true)
        .map_err(|e| format!("failed to comment out SCIP lines: {e}"))?;

    // -----------------------------------------------------------------
    // Phase 2: PRINTEMPS
    // -----------------------------------------------------------------
    let elapsed_total = started.elapsed().as_secs_f64();
    let printemps_time = match args.overall_time {
        Some(t) => {
            let remaining = t - elapsed_total;
            if remaining <= 0.0 {
                println!(
                    "c scip-printemps: overall time budget already exhausted before PRINTEMPS"
                );
                return Ok(());
            }
            Some(remaining)
        }
        None => None,
    };

    let p_log_path = args.save_dir.join("printemps_log.txt");

    match printemps_time {
        Some(t) => println!("c scip-printemps: phase 2 (PRINTEMPS, budget={:.3}s)", t),
        None => println!("c scip-printemps: phase 2 (PRINTEMPS, no time limit)"),
    }

    let initial_sol = if scip_incumbent_sol_path.exists() {
        Some(scip_incumbent_sol_path.as_path())
    } else {
        None
    };

    let fixed_vars = if scip_fixed_vars_path.exists() {
        Some(scip_fixed_vars_path.as_path())
    } else {
        None
    };

    let p_run = printemps::run(printemps::PrintempsConfig {
        solver_path: &args.printemps_path,
        instance: &args.instance,
        time_max: printemps_time,
        iteration_max: Some(-1),
        seed: args.seed,
        threads: args.threads,
        initial_solution: initial_sol,
        fixed_variable: fixed_vars,
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

    // Fallback: PRINTEMPS produced no feasible solution but SCIP captured one.
    let needs_fallback = matches!(
        p_run.verdict,
        printemps::PrintempsVerdict::Unknown | printemps::PrintempsVerdict::Unsupported
    ) && scip_run.last_v_line.is_some();

    if needs_fallback {
        let v = scip_run.last_v_line.as_deref().unwrap();
        printemps::emit_fallback(
            "scip-printemps: PRINTEMPS did not improve; falling back to SCIP incumbent",
            v,
        )
        .map_err(|e| format!("failed to emit fallback: {e}"))?;
    }

    Ok(())
}

fn emit_best_after_interrupt(scip_run: &scip::ScipRun) -> Result<(), String> {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    match scip_run.verdict {
        scip::ScipVerdict::OptimumFound | scip::ScipVerdict::Unsatisfiable => {
            if let Some(ref s) = scip_run.last_s_line {
                writeln!(h, "{}", s).map_err(|e| e.to_string())?;
            }
            if let Some(ref v) = scip_run.last_v_line {
                writeln!(h, "{}", v).map_err(|e| e.to_string())?;
            }
        }
        scip::ScipVerdict::Satisfiable => {
            writeln!(h, "s SATISFIABLE").map_err(|e| e.to_string())?;
            if let Some(ref v) = scip_run.last_v_line {
                writeln!(h, "{}", v).map_err(|e| e.to_string())?;
            }
        }
        scip::ScipVerdict::Unknown => {
            if let Some(ref v) = scip_run.last_v_line {
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
            eprintln!("scip-printemps: error: {msg}");
            ExitCode::from(1)
        }
    }
}
