use crate::signals::ChildSlot;
use nix::unistd::Pid;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    OptimumFound,
    Unsatisfiable,
    Satisfiable,
    Unknown,
    NoStatus,
}

impl Verdict {
    fn from_status_line(line: &str) -> Verdict {
        let payload = line.trim();
        if payload.starts_with("s OPTIMUM FOUND") {
            Verdict::OptimumFound
        } else if payload.starts_with("s UNSATISFIABLE") {
            Verdict::Unsatisfiable
        } else if payload.starts_with("s SATISFIABLE") {
            Verdict::Satisfiable
        } else if payload.starts_with("s UNKNOWN") {
            Verdict::Unknown
        } else if payload.starts_with("s ") {
            Verdict::Unknown
        } else {
            Verdict::NoStatus
        }
    }
}

pub struct ExactRun {
    pub verdict: Verdict,
    pub last_s_line: Option<String>,
    pub last_v_line: Option<String>,
    /// Reserved for future hand-off to PRINTEMPS.
    #[allow(dead_code)]
    pub last_o_value: Option<String>,
    pub elapsed_sec: f64,
    pub exit_code: Option<i32>,
}

pub struct ExactConfig<'a> {
    pub exact_path: &'a Path,
    pub instance: &'a Path,
    pub timeout_sec: f64,
    pub extra_args: &'a [String],
    pub log_path: &'a Path,
    pub bounds_path: &'a Path,
    pub incumbent_pb_path: &'a Path,
    pub incumbent_sol_path: &'a Path,
    pub child_slot: &'a ChildSlot,
}

/// Run Exact, tee its stdout to ours (with `s` and `v` lines buffered to the
/// caller for post-hoc decision making), capture stderr to the log file, and
/// persist intermediate artifacts under the save directory.
pub fn run(cfg: ExactConfig<'_>) -> std::io::Result<ExactRun> {
    let started = Instant::now();
    let mut log_file = File::create(cfg.log_path)?;

    let mut command = Command::new(cfg.exact_path);
    command
        .arg(format!("--timeout={}", cfg.timeout_sec))
        .arg("--print-sol")
        .arg("--print-uniform=0")
        .arg("--verbosity=1")
        .args(cfg.extra_args)
        .arg(cfg.instance)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Place the child in its own process group so the terminal-driven Ctrl-C
    // does not get double-delivered. Our signal forwarder explicitly relays
    // signals to it.
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
            // Forward stderr to our stderr verbatim and also persist it.
            eprint!("{}", buf);
            sf.write_all(buf.as_bytes())?;
        }
        Ok(())
    });

    let mut last_s_line: Option<String> = None;
    let mut last_v_line: Option<String> = None;
    let mut last_o_value: Option<String> = None;

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
            // Strip just trailing newline for inspection.
            let body = buf.strip_suffix('\n').unwrap_or(&buf);
            let body = body.strip_suffix('\r').unwrap_or(body);

            log_file.write_all(buf.as_bytes())?;

            if body.starts_with("s ") {
                last_s_line = Some(body.to_string());
                continue;
            }
            if body.starts_with("v ") || body == "v" {
                last_v_line = Some(body.to_string());
                continue;
            }
            if let Some(rest) = body.strip_prefix("o ") {
                last_o_value = Some(rest.trim().to_string());
            }

            let mut handle = stdout_lock.lock();
            handle.write_all(buf.as_bytes())?;
            handle.flush()?;
        }
    }

    let status = child.wait()?;
    cfg.child_slot.clear();
    let _ = stderr_thread.join();

    let verdict = match last_s_line.as_deref() {
        Some(line) => Verdict::from_status_line(line),
        None => Verdict::NoStatus,
    };
    let exit_code = status.code();
    let elapsed_sec = started.elapsed().as_secs_f64();

    // Persist artifacts.
    if let Some(ref v) = last_v_line {
        let mut f = File::create(cfg.incumbent_pb_path)?;
        writeln!(f, "{}", v)?;
        write_printemps_initial_solution(cfg.incumbent_sol_path, v)?;
    }

    write_bounds_json(
        cfg.bounds_path,
        verdict,
        last_o_value.as_deref(),
        elapsed_sec,
        exit_code,
    )?;

    Ok(ExactRun {
        verdict,
        last_s_line,
        last_v_line,
        last_o_value,
        elapsed_sec,
        exit_code,
    })
}

/// Emit a buffered `s` / `v` pair as either pass-through or commented-out form.
pub fn flush_buffered_lines(run: &ExactRun, as_comments: bool) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    if let Some(ref s) = run.last_s_line {
        if as_comments {
            writeln!(h, "c exact-final: {}", s)?;
        } else {
            writeln!(h, "{}", s)?;
        }
    }
    if let Some(ref v) = run.last_v_line {
        if as_comments {
            writeln!(h, "c exact-incumbent: {}", v)?;
        } else {
            writeln!(h, "{}", v)?;
        }
    }
    h.flush()
}

fn write_bounds_json(
    path: &Path,
    verdict: Verdict,
    primal: Option<&str>,
    elapsed_sec: f64,
    exit_code: Option<i32>,
) -> std::io::Result<()> {
    let status = match verdict {
        Verdict::OptimumFound => "OPTIMUM_FOUND",
        Verdict::Unsatisfiable => "UNSATISFIABLE",
        Verdict::Satisfiable => "SATISFIABLE",
        Verdict::Unknown => "UNKNOWN",
        Verdict::NoStatus => "NO_STATUS",
    };
    let primal_repr = match primal {
        Some(s) => format!("\"{}\"", escape_json(s)),
        None => "null".to_string(),
    };
    let exit_repr = match exit_code {
        Some(c) => c.to_string(),
        None => "null".to_string(),
    };
    let body = format!(
        "{{\n  \"status\": \"{}\",\n  \"primal_bound\": {},\n  \"elapsed_sec\": {:.6},\n  \"exit_code\": {}\n}}\n",
        status, primal_repr, elapsed_sec, exit_repr
    );
    let mut f = File::create(path)?;
    f.write_all(body.as_bytes())
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Convert a PB-format `v` line (e.g. "v x1 -x2 x3") into a sequence of
/// "xN VALUE" lines as accepted by the PRINTEMPS standalone solver's `-i`
/// option, written to `path`.
fn write_printemps_initial_solution(path: &Path, v_line: &str) -> std::io::Result<()> {
    let mut f = File::create(path)?;
    let payload = v_line.strip_prefix("v ").unwrap_or(v_line.trim_start_matches('v'));
    for tok in payload.split_whitespace() {
        let (name, val) = if let Some(rest) = tok.strip_prefix('-') {
            (rest, 0)
        } else {
            (tok, 1)
        };
        if name.is_empty() {
            continue;
        }
        writeln!(f, "{} {}", name, val)?;
    }
    Ok(())
}

pub fn default_save_dir() -> PathBuf {
    PathBuf::from(".pb26-state")
}
