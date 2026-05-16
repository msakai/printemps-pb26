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
        } else {
            // Anything else (`s UNKNOWN`, an unfamiliar `s ...`, or no `s`
            // line at all) collapses to Unknown.
            Verdict::Unknown
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
    /// When `Some`, parse ` c fixed <signed-int>` lines from Exact's output
    /// and write them as a PRINTEMPS fixed-variable file at this path.
    pub fixed_literals_path: Option<&'a Path>,
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
        .arg("--verbosity=1");
    if cfg.fixed_literals_path.is_some() {
        command.arg("--log-fixed-lits");
    }
    command
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
    let mut fixed_literals: Vec<i64> = Vec::new();
    let collect_fixed = cfg.fixed_literals_path.is_some();

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

            if collect_fixed {
                if let Some(lit) = parse_fixed_literal_line(body) {
                    fixed_literals.push(lit);
                }
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
        .map_or(Verdict::Unknown, Verdict::from_status_line);
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

    if let Some(p) = cfg.fixed_literals_path {
        if !fixed_literals.is_empty() {
            write_fixed_literals_file(p, &fixed_literals)?;
        }
    }

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
    let payload = v_line
        .strip_prefix("v ")
        .unwrap_or(v_line.trim_start_matches('v'));
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
    PathBuf::from(".pb-state")
}

/// Parse a ` c fixed <signed-int>` log line emitted by the `msakai/exact`
/// fork. Returns `Some(literal)` on a match, where the sign follows the
/// DIMACS convention (positive = literal asserted true, negative = false).
fn parse_fixed_literal_line(body: &str) -> Option<i64> {
    let mut it = body.split_whitespace();
    match it.next()? {
        "c" => {}
        _ => return None,
    }
    if it.next()? != "fixed" {
        return None;
    }
    let tok = it.next()?;
    if it.next().is_some() {
        return None;
    }
    tok.parse::<i64>().ok().filter(|&v| v != 0)
}

/// Write `xN VALUE` lines (one per fixed literal) to `path`, in the format
/// accepted by the PRINTEMPS standalone solver's `-f` option. Duplicate
/// variable indices are kept as the first occurrence; conflicting fixings
/// are skipped.
fn write_fixed_literals_file(path: &Path, literals: &[i64]) -> std::io::Result<()> {
    use std::collections::HashMap;
    let mut seen: HashMap<i64, i32> = HashMap::new();
    let mut order: Vec<i64> = Vec::new();
    for &lit in literals {
        let var = lit.abs();
        let val: i32 = if lit > 0 { 1 } else { 0 };
        if let Some(&prev) = seen.get(&var) {
            if prev != val {
                eprintln!(
                    "exact-printemps: warning: conflicting fixings for x{} ({} vs {}); keeping first",
                    var, prev, val
                );
            }
            continue;
        }
        seen.insert(var, val);
        order.push(var);
    }
    let mut f = File::create(path)?;
    for var in order {
        writeln!(f, "x{} {}", var, seen[&var])?;
    }
    Ok(())
}
