//! Solver-to-solver hand-off payload.
//!
//! All information that one phase wants to communicate to a later phase is
//! collected into [`SolverHandoff`]. Today the only consumer is the PRINTEMPS
//! phase (which reads an initial-solution file), but more channels can be
//! added without changing call sites: the same struct can be produced by
//! PRINTEMPS for a subsequent SCIP / Exact phase, and the bounds it carries
//! can later be forwarded to PRINTEMPS once that solver grows a flag for
//! them.

use std::fs::File;
use std::io::Write;
use std::path::Path;

/// Variable assignment in a Boolean problem.
#[derive(Debug, Clone)]
pub struct VarAssignment {
    pub name: String,
    pub value: u8, // 0 or 1
}

/// Information handed from one solver phase to another.
///
/// Every field is optional / may be empty so a phase can fill in only what it
/// produces. Phases that consume a handoff should be tolerant of missing
/// pieces.
#[derive(Debug, Clone, Default)]
pub struct SolverHandoff {
    /// Best feasible assignment found so far (may be partial in the future).
    pub incumbent: Option<Vec<VarAssignment>>,
    /// Variables whose value the producing solver has proven to be fixed
    /// (e.g. by presolve / propagation / branch-and-bound at the root node).
    /// Disjoint from `incumbent` is *not* required; both views may overlap.
    pub fixed_vars: Vec<VarAssignment>,
    /// Best primal objective bound known (None if no solution found).
    pub primal_bound: Option<f64>,
    /// Best dual / lower objective bound known.
    pub dual_bound: Option<f64>,
}

impl SolverHandoff {
    pub fn new() -> Self {
        Self::default()
    }

    /// Write the incumbent (merged with fixed-variable assignments, fixed
    /// values winning on conflict) to a file in the format accepted by
    /// `pb_competition_2025_solver -i`, i.e. one `xN VALUE` per line.
    ///
    /// Returns `Ok(false)` when there is nothing to write (no incumbent and
    /// no fixed variables).
    pub fn write_printemps_initial_solution(&self, path: &Path) -> std::io::Result<bool> {
        if self.incumbent.is_none() && self.fixed_vars.is_empty() {
            return Ok(false);
        }
        let mut f = File::create(path)?;
        let mut seen = std::collections::HashSet::new();
        // Fixed variables first so duplicates from the incumbent are skipped.
        for a in &self.fixed_vars {
            if a.name.is_empty() {
                continue;
            }
            if seen.insert(a.name.clone()) {
                writeln!(f, "{} {}", a.name, a.value)?;
            }
        }
        if let Some(inc) = &self.incumbent {
            for a in inc {
                if a.name.is_empty() {
                    continue;
                }
                if seen.insert(a.name.clone()) {
                    writeln!(f, "{} {}", a.name, a.value)?;
                }
            }
        }
        Ok(true)
    }

    /// Persist the fixed-variable list to a standalone file. Reserved for a
    /// future PRINTEMPS flag that would consume it; today only used for
    /// auditing.
    pub fn write_fixed_vars(&self, path: &Path) -> std::io::Result<()> {
        let mut f = File::create(path)?;
        for a in &self.fixed_vars {
            if !a.name.is_empty() {
                writeln!(f, "{} {}", a.name, a.value)?;
            }
        }
        Ok(())
    }

    /// Persist primal / dual bounds and a free-form status string as JSON,
    /// matching the shape of `exact_bounds.json` but with an extra
    /// `dual_bound` field.
    pub fn write_bounds_json(
        &self,
        path: &Path,
        status: &str,
        elapsed_sec: f64,
        exit_code: Option<i32>,
    ) -> std::io::Result<()> {
        let primal = match self.primal_bound {
            Some(v) if v.is_finite() => format!("{}", v),
            _ => "null".to_string(),
        };
        let dual = match self.dual_bound {
            Some(v) if v.is_finite() => format!("{}", v),
            _ => "null".to_string(),
        };
        let exit_repr = match exit_code {
            Some(c) => c.to_string(),
            None => "null".to_string(),
        };
        let body = format!(
            "{{\n  \"status\": \"{}\",\n  \"primal_bound\": {},\n  \"dual_bound\": {},\n  \"elapsed_sec\": {:.6},\n  \"exit_code\": {}\n}}\n",
            escape_json(status), primal, dual, elapsed_sec, exit_repr
        );
        let mut f = File::create(path)?;
        f.write_all(body.as_bytes())
    }

    /// Render the incumbent as a PB competition `v` line (positive token =1,
    /// negated token =0). Returns `None` if there is no incumbent.
    pub fn to_pb_v_line(&self) -> Option<String> {
        let inc = self.incumbent.as_ref()?;
        let mut s = String::from("v");
        for a in inc {
            s.push(' ');
            if a.value == 0 {
                s.push('-');
            }
            s.push_str(&a.name);
        }
        Some(s)
    }
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
