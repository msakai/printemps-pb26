//! Phase-1 SCIP solver, called in-process through the `russcip` crate.
//!
//! Mirrors the shape of [`crate::exact`] (a sibling Phase-1 stage that shells
//! out to the Exact binary): it consumes the OPB instance, solves under a
//! time budget, and emits a [`crate::handoff::SolverHandoff`] containing the
//! incumbent, the variables SCIP has proved fixed, and both primal / dual
//! bounds. Bounds are persisted to disk for future hand-off back to
//! PRINTEMPS even though PRINTEMPS does not yet read them.
//!
//! Today the PB-competition output (`s …`, `v …`) is emitted only after
//! `solve()` returns. SCIP's own message output is suppressed
//! (`hide_output()`); a separate driver-level log file captures progress
//! commentary written by this module.

use crate::handoff::{SolverHandoff, VarAssignment};
use crate::signals::InterruptFlag;
use russcip::prelude::*;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScipVerdict {
    OptimumFound,
    Unsatisfiable,
    /// A feasible solution was found but the run did not prove optimality.
    Satisfiable,
    Unknown,
}

pub struct ScipRun {
    pub verdict: ScipVerdict,
    pub handoff: SolverHandoff,
    pub elapsed_sec: f64,
    /// Whether SCIP was asked to stop due to an external interrupt flag.
    pub interrupted: bool,
    /// `s …` line that would have been emitted if SCIP were terminal, in PB
    /// competition syntax. Kept so the driver can decide whether to print
    /// it directly or comment it out as `c scip-final: …`.
    pub last_s_line: Option<String>,
    /// `v …` line corresponding to `handoff.incumbent`, ready for output.
    pub last_v_line: Option<String>,
    /// `o …` value (objective) corresponding to `handoff.primal_bound`.
    pub last_o_value: Option<String>,
}

pub struct ScipConfig<'a> {
    pub instance: &'a Path,
    /// Whether the instance carries an objective (`min:` line). For a pure
    /// satisfaction instance (PBS) SCIP solves a constant zero objective and
    /// reports `Status::Optimal` on the first feasible solution, so without
    /// this flag the verdict would be misreported as `OptimumFound`.
    pub has_objective: bool,
    pub timeout_sec: f64,
    pub seed: Option<i64>,
    pub threads: Option<i32>,
    pub extra_params: &'a [(String, String)],
    pub log_path: &'a Path,
    pub bounds_path: &'a Path,
    pub incumbent_sol_path: &'a Path,
    pub fixed_vars_path: &'a Path,
    pub interrupt_flag: &'a InterruptFlag,
}

/// Solve `cfg.instance` with SCIP, write artifacts, and return a [`ScipRun`].
///
/// The function blocks for at most `cfg.timeout_sec` seconds (enforced by
/// SCIP's `limits/time` parameter). If the caller's `interrupt_flag` is
/// already raised when this function is entered, the SCIP solve is skipped
/// entirely so the driver can fall through to the best-effort emission path.
pub fn run(cfg: ScipConfig<'_>) -> Result<ScipRun, String> {
    let started = Instant::now();
    let mut log_file =
        File::create(cfg.log_path).map_err(|e| format!("cannot create scip log: {e}"))?;

    let instance_str = cfg
        .instance
        .to_str()
        .ok_or_else(|| "instance path is not valid UTF-8".to_string())?;

    log_line(&mut log_file, &format!("instance={}", instance_str));
    log_line(
        &mut log_file,
        &format!("timeout_sec={:.3}", cfg.timeout_sec),
    );

    if cfg.interrupt_flag.is_set() {
        log_line(&mut log_file, "interrupt flag already set; skipping solve");
        return Ok(ScipRun {
            verdict: ScipVerdict::Unknown,
            handoff: SolverHandoff::new(),
            elapsed_sec: started.elapsed().as_secs_f64(),
            interrupted: true,
            last_s_line: None,
            last_v_line: None,
            last_o_value: None,
        });
    }

    // Build the model. `read_prob` chooses a reader from the file extension,
    // so a `.opb` / `.pbo` / `.wbo` file dispatches to SCIP's PB reader.
    let mut model_builder = Model::new()
        .hide_output()
        .set_real_param("limits/time", cfg.timeout_sec)
        .map_err(|e| format!("set limits/time failed: {e:?}"))?;

    if let Some(seed) = cfg.seed {
        // SCIP only accepts unsigned shifts in i32 range; clamp gracefully.
        let v = seed.clamp(0, i32::MAX as i64) as i32;
        model_builder = model_builder
            .set_int_param("randomization/randomseedshift", v)
            .map_err(|e| format!("set randomization/randomseedshift failed: {e:?}"))?;
    }
    if let Some(threads) = cfg.threads {
        // SCIP itself is single-threaded for the core branch-and-bound. The
        // `-j` flag is accepted for parity with the other phase but only
        // logged: forwarding it as a SCIP parameter (e.g.
        // `parallel/maxnthreads`) only works in ParaSCIP / concurrent-solver
        // configurations that may not be enabled in this build, and the
        // typestate API consumes the model on failure. Users who need to
        // tune SCIP-side parallelism can pass `--scip-arg name=value`.
        log_line(
            &mut log_file,
            &format!(
                "requested threads={} (forwarded only via --scip-arg)",
                threads
            ),
        );
    }
    for (k, v) in cfg.extra_params {
        match parse_param_value(v) {
            ParamValue::Bool(b) => {
                model_builder = match model_builder.set_bool_param(k, b) {
                    Ok(m) => m,
                    Err(e) => return Err(format!("set_bool_param({k}={v}) failed: {e:?}")),
                };
            }
            ParamValue::Int(i) => {
                model_builder = match model_builder.set_int_param(k, i) {
                    Ok(m) => m,
                    Err(e) => return Err(format!("set_int_param({k}={v}) failed: {e:?}")),
                };
            }
            ParamValue::Real(r) => {
                model_builder = match model_builder.set_real_param(k, r) {
                    Ok(m) => m,
                    Err(e) => return Err(format!("set_real_param({k}={v}) failed: {e:?}")),
                };
            }
            ParamValue::Str => {
                model_builder = match model_builder.set_str_param(k, v) {
                    Ok(m) => m,
                    Err(e) => return Err(format!("set_str_param({k}={v}) failed: {e:?}")),
                };
            }
        }
    }

    let problem = model_builder
        .include_default_plugins()
        .read_prob(instance_str)
        .map_err(|e| format!("SCIP could not read {instance_str}: {e:?}"))?;

    log_line(
        &mut log_file,
        &format!(
            "loaded problem: {} variables, {} constraints",
            problem.n_vars(),
            problem.n_conss()
        ),
    );

    // Snapshot original variables before solving. SCIP's `vars()` returned
    // post-solve walks active (transformed) variables, which is empty for
    // instances entirely resolved during presolve. The Rc<Variable> handles
    // captured here stay valid through `solve()` because they share the
    // SCIP pointer; their `lb()` / `ub()` after solve report the tightest
    // proved bounds on the original variable.
    let original_vars: Vec<_> = problem.vars();

    // NOTE: russcip's `solve()` consumes the model, so we cannot install a
    // background thread that holds the SCIP pointer to call
    // `SCIPinterruptSolve`. Mid-solve external interruption would require a
    // custom event handler; left as a future improvement.
    let solved = problem.solve();
    let elapsed_sec = started.elapsed().as_secs_f64();
    let status = solved.status();
    log_line(
        &mut log_file,
        &format!("status={:?} elapsed={:.3}s", status, elapsed_sec),
    );

    let primal_bound = {
        let v = solved.obj_val();
        if v.is_finite() {
            Some(v)
        } else {
            None
        }
    };
    let dual_bound = {
        let v = solved.best_bound();
        if v.is_finite() {
            Some(v)
        } else {
            None
        }
    };

    let mut handoff = SolverHandoff::new();
    handoff.primal_bound = primal_bound;
    handoff.dual_bound = dual_bound;

    let best_sol = solved.best_sol();
    let mut incumbent: Vec<VarAssignment> = Vec::new();
    let mut fixed_vars: Vec<VarAssignment> = Vec::new();

    for var in &original_vars {
        let name = var.name();
        if !looks_like_pb_var(&name) {
            continue;
        }
        // After `solve()`, local bounds at the focus (root) node equal the
        // tightest bounds SCIP could prove globally for this variable, and a
        // binary variable with lb == ub is fixed by SCIP. Note that these
        // global bounds are tightened by both primal reductions (preserving
        // every feasible solution) and dual reductions (which use the
        // objective cutoff and only preserve at least one optimal solution).
        // So fixings may cut off feasible-but-suboptimal solutions whose
        // objective is better than the current incumbent — at least one true
        // optimum is still preserved, so handing these to PRINTEMPS as hard
        // fixings is safe for optimization but is not equivalent to "fixed
        // across all feasible solutions".
        let lb = var.lb();
        let ub = var.ub();
        if (ub - lb).abs() < 0.5 {
            let v = if lb >= 0.5 { 1 } else { 0 };
            fixed_vars.push(VarAssignment {
                name: name.clone(),
                value: v,
            });
        }
        if let Some(ref sol) = best_sol {
            let raw = sol.val(var.clone());
            let v = if raw >= 0.5 { 1 } else { 0 };
            incumbent.push(VarAssignment { name, value: v });
        }
    }

    if best_sol.is_some() {
        handoff.incumbent = Some(incumbent);
    }
    handoff.fixed_vars = fixed_vars;

    let verdict = classify_verdict(status, cfg.has_objective, handoff.incumbent.is_some());

    // Persistence.
    let _ = handoff.write_printemps_initial_solution(cfg.incumbent_sol_path);
    let _ = handoff.write_fixed_vars(cfg.fixed_vars_path);
    let _ = handoff.write_bounds_json(cfg.bounds_path, verdict_label(verdict), elapsed_sec, None);

    let last_v_line = handoff.to_pb_v_line();
    // Only report an objective value for optimization instances; a PBS
    // instance has no objective (its primal bound is the constant 0).
    let last_o_value = if cfg.has_objective {
        primal_bound.map(|v| format!("{}", v))
    } else {
        None
    };
    let last_s_line = match verdict {
        ScipVerdict::OptimumFound => Some("s OPTIMUM FOUND".to_string()),
        ScipVerdict::Unsatisfiable => Some("s UNSATISFIABLE".to_string()),
        ScipVerdict::Satisfiable => Some("s SATISFIABLE".to_string()),
        ScipVerdict::Unknown => None,
    };

    log_line(
        &mut log_file,
        &format!(
            "primal={:?} dual={:?} n_fixed={} has_incumbent={}",
            primal_bound,
            dual_bound,
            handoff.fixed_vars.len(),
            handoff.incumbent.is_some()
        ),
    );

    Ok(ScipRun {
        verdict,
        handoff,
        elapsed_sec,
        interrupted: cfg.interrupt_flag.is_set(),
        last_s_line,
        last_v_line,
        last_o_value,
    })
}

/// Emit `s …` and `v …` lines either as the final answer or commented out.
/// Mirrors [`crate::exact::flush_buffered_lines`].
pub fn flush_buffered_lines(run: &ScipRun, as_comments: bool) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    if let Some(ref o) = run.last_o_value {
        // Always emit the objective value as a comment when commented out;
        // otherwise emit it as a final `o …` for downstream consumers.
        if as_comments {
            writeln!(h, "c scip-objective: o {}", o)?;
        } else {
            writeln!(h, "o {}", o)?;
        }
    }
    if let Some(ref s) = run.last_s_line {
        if as_comments {
            writeln!(h, "c scip-final: {}", s)?;
        } else {
            writeln!(h, "{}", s)?;
        }
    }
    if let Some(ref v) = run.last_v_line {
        if as_comments {
            writeln!(h, "c scip-incumbent: {}", v)?;
        } else {
            writeln!(h, "{}", v)?;
        }
    }
    h.flush()
}

fn looks_like_pb_var(name: &str) -> bool {
    // PB variables are conventionally named `xN`. SCIP may introduce auxiliary
    // variables (e.g. for non-linear / soft constraint relaxation) that do not
    // belong on a `v …` line; filter them out.
    let mut chars = name.chars();
    match chars.next() {
        Some('x') => chars.all(|c| c.is_ascii_digit()) && name.len() >= 2,
        _ => false,
    }
}

/// Map SCIP's terminal [`Status`] to a [`ScipVerdict`].
///
/// The subtlety is `Status::Optimal`: on a pure satisfaction instance
/// (`has_objective == false`) SCIP solves a constant zero objective, so
/// "optimal" only certifies that a feasible solution exists. Such an instance
/// must report `Satisfiable`, not `OptimumFound`.
fn classify_verdict(status: Status, has_objective: bool, has_incumbent: bool) -> ScipVerdict {
    match status {
        Status::Optimal if has_objective => ScipVerdict::OptimumFound,
        Status::Optimal => ScipVerdict::Satisfiable,
        Status::Infeasible => ScipVerdict::Unsatisfiable,
        _ if has_incumbent => ScipVerdict::Satisfiable,
        _ => ScipVerdict::Unknown,
    }
}

fn verdict_label(v: ScipVerdict) -> &'static str {
    match v {
        ScipVerdict::OptimumFound => "OPTIMUM_FOUND",
        ScipVerdict::Unsatisfiable => "UNSATISFIABLE",
        ScipVerdict::Satisfiable => "SATISFIABLE",
        ScipVerdict::Unknown => "UNKNOWN",
    }
}

fn log_line(f: &mut File, msg: &str) {
    let _ = writeln!(f, "{msg}");
}

enum ParamValue {
    Bool(bool),
    Int(i32),
    Real(f64),
    Str,
}

fn parse_param_value(s: &str) -> ParamValue {
    match s {
        "true" | "TRUE" | "True" => return ParamValue::Bool(true),
        "false" | "FALSE" | "False" => return ParamValue::Bool(false),
        _ => {}
    }
    if let Ok(i) = s.parse::<i32>() {
        return ParamValue::Int(i);
    }
    if let Ok(r) = s.parse::<f64>() {
        return ParamValue::Real(r);
    }
    ParamValue::Str
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimal_with_objective_is_optimum_found() {
        assert_eq!(
            classify_verdict(Status::Optimal, true, true),
            ScipVerdict::OptimumFound
        );
    }

    #[test]
    fn optimal_without_objective_is_satisfiable() {
        // A PBS (satisfaction) instance must not be reported as OPTIMUM FOUND
        // just because SCIP solved its constant zero objective to optimality.
        assert_eq!(
            classify_verdict(Status::Optimal, false, true),
            ScipVerdict::Satisfiable
        );
    }

    #[test]
    fn infeasible_is_unsatisfiable_regardless_of_objective() {
        assert_eq!(
            classify_verdict(Status::Infeasible, true, false),
            ScipVerdict::Unsatisfiable
        );
        assert_eq!(
            classify_verdict(Status::Infeasible, false, false),
            ScipVerdict::Unsatisfiable
        );
    }

    #[test]
    fn nonterminal_with_incumbent_is_satisfiable() {
        // e.g. a time limit hit after a feasible solution was found.
        assert_eq!(
            classify_verdict(Status::TimeLimit, true, true),
            ScipVerdict::Satisfiable
        );
        assert_eq!(
            classify_verdict(Status::TimeLimit, false, true),
            ScipVerdict::Satisfiable
        );
    }

    #[test]
    fn nonterminal_without_incumbent_is_unknown() {
        assert_eq!(
            classify_verdict(Status::TimeLimit, true, false),
            ScipVerdict::Unknown
        );
        assert_eq!(
            classify_verdict(Status::TimeLimit, false, false),
            ScipVerdict::Unknown
        );
    }
}
