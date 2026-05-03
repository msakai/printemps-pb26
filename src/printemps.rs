use crate::signals::ChildSlot;
use nix::unistd::Pid;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintempsVerdict {
    Satisfiable,
    Unsatisfiable,
    Unknown,
    Unsupported,
}

impl PrintempsVerdict {
    fn from_status_line(line: &str) -> PrintempsVerdict {
        let payload = line.trim();
        if payload.starts_with("s SATISFIABLE") {
            PrintempsVerdict::Satisfiable
        } else if payload.starts_with("s UNSATISFIABLE") {
            PrintempsVerdict::Unsatisfiable
        } else if payload.starts_with("s UNSUPPORTED") {
            PrintempsVerdict::Unsupported
        } else {
            // `s UNKNOWN`, an unfamiliar `s ...`, or no `s` line at all all
            // collapse to Unknown.
            PrintempsVerdict::Unknown
        }
    }
}

pub struct PrintempsRun {
    pub verdict: PrintempsVerdict,
    /// Kept for future diagnostic / fallback decisions.
    #[allow(dead_code)]
    pub last_s_line: Option<String>,
    /// Kept for future diagnostic / fallback decisions.
    #[allow(dead_code)]
    pub last_v_line: Option<String>,
    pub exit_code: Option<i32>,
}

pub struct PrintempsConfig<'a> {
    pub solver_path: &'a Path,
    pub instance: &'a Path,
    pub time_max: Option<f64>,
    pub iteration_max: Option<i64>,
    pub seed: Option<i64>,
    pub threads: Option<i32>,
    pub extra_args: &'a [String],
    pub log_path: &'a Path,
    pub child_slot: &'a ChildSlot,
}

/// Run pb_competition_2025_solver. Its stdout already conforms to PB
/// competition output, so we pass it through verbatim while keeping the last
/// `s` and `v` lines so the driver can fall back to a previously saved Exact
/// incumbent if PRINTEMPS finishes with no feasible solution.
pub fn run(cfg: PrintempsConfig<'_>) -> std::io::Result<PrintempsRun> {
    let mut log_file = File::create(cfg.log_path)?;

    let mut command = Command::new(cfg.solver_path);
    if let Some(t) = cfg.time_max {
        command.arg("-t").arg(format!("{}", t));
    }
    if let Some(k) = cfg.iteration_max {
        command.arg("-k").arg(format!("{}", k));
    }
    if let Some(s) = cfg.seed {
        command.arg("-r").arg(format!("{}", s));
    }
    if let Some(j) = cfg.threads {
        command.arg("-j").arg(format!("{}", j));
    }
    command
        .args(cfg.extra_args)
        .arg(cfg.instance)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    unsafe {
        command.pre_exec(|| {
            nix::unistd::setpgid(nix::unistd::Pid::from_raw(0), nix::unistd::Pid::from_raw(0))
                .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
            Ok(())
        });
    }

    let mut child = command.spawn()?;
    let pid = Pid::from_raw(child.id() as i32);
    cfg.child_slot.set(Some(pid));

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let stderr_log_path = cfg.log_path.with_extension("stderr.log");
    let stderr_thread = std::thread::spawn(move || -> std::io::Result<()> {
        let mut sf = File::create(stderr_log_path)?;
        let mut reader = BufReader::new(stderr);
        let mut buf = String::new();
        loop {
            buf.clear();
            let n = reader.read_line(&mut buf)?;
            if n == 0 {
                break;
            }
            eprint!("{}", buf);
            sf.write_all(buf.as_bytes())?;
        }
        Ok(())
    });

    let mut last_s_line: Option<String> = None;
    let mut last_v_line: Option<String> = None;

    {
        let mut reader = BufReader::new(stdout);
        let mut buf = String::new();
        let stdout_lock = std::io::stdout();
        loop {
            buf.clear();
            let n = reader.read_line(&mut buf)?;
            if n == 0 {
                break;
            }
            let body = buf.strip_suffix('\n').unwrap_or(&buf);
            let body = body.strip_suffix('\r').unwrap_or(body);

            log_file.write_all(buf.as_bytes())?;

            if body.starts_with("s ") {
                last_s_line = Some(body.to_string());
            } else if body.starts_with("v ") || body == "v" {
                last_v_line = Some(body.to_string());
            }

            let mut handle = stdout_lock.lock();
            handle.write_all(buf.as_bytes())?;
            handle.flush()?;
        }
    }

    let status = child.wait()?;
    cfg.child_slot.clear();
    let _ = stderr_thread.join();

    let verdict = last_s_line
        .as_deref()
        .map_or(PrintempsVerdict::Unknown, PrintempsVerdict::from_status_line);

    Ok(PrintempsRun {
        verdict,
        last_s_line,
        last_v_line,
        exit_code: status.code(),
    })
}

/// Emit a fallback `s SATISFIABLE` / `v ...` pair using a previously captured
/// Exact incumbent. Used when PRINTEMPS produced no feasible solution but
/// Exact had one.
pub fn emit_fallback(s_line_hint: &str, v_line: &str) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    writeln!(h, "c {}", s_line_hint)?;
    writeln!(h, "s SATISFIABLE")?;
    writeln!(h, "{}", v_line)?;
    h.flush()
}
