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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn var(name: &str, value: u8) -> VarAssignment {
        VarAssignment {
            name: name.to_string(),
            value,
        }
    }

    // --- to_pb_v_line ---

    #[test]
    fn to_pb_v_line_none() {
        assert_eq!(SolverHandoff::new().to_pb_v_line(), None);
    }

    #[test]
    fn to_pb_v_line_empty_incumbent() {
        let h = SolverHandoff {
            incumbent: Some(vec![]),
            ..Default::default()
        };
        assert_eq!(h.to_pb_v_line(), Some("v".to_string()));
    }

    #[test]
    fn to_pb_v_line_all_ones() {
        let h = SolverHandoff {
            incumbent: Some(vec![var("x1", 1), var("x2", 1), var("x3", 1)]),
            ..Default::default()
        };
        assert_eq!(h.to_pb_v_line(), Some("v x1 x2 x3".to_string()));
    }

    #[test]
    fn to_pb_v_line_mixed() {
        let h = SolverHandoff {
            incumbent: Some(vec![var("x1", 1), var("x2", 0), var("x3", 1)]),
            ..Default::default()
        };
        assert_eq!(h.to_pb_v_line(), Some("v x1 -x2 x3".to_string()));
    }

    // --- write_printemps_initial_solution ---

    #[test]
    fn write_printemps_empty_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.sol");
        assert!(!SolverHandoff::new().write_printemps_initial_solution(&p).unwrap());
    }

    #[test]
    fn write_printemps_incumbent_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.sol");
        let h = SolverHandoff {
            incumbent: Some(vec![var("x1", 1), var("x2", 0), var("x3", 1)]),
            ..Default::default()
        };
        assert!(h.write_printemps_initial_solution(&p).unwrap());
        assert_eq!(fs::read_to_string(&p).unwrap(), "x1 1\nx2 0\nx3 1\n");
    }

    #[test]
    fn write_printemps_fixed_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.sol");
        let h = SolverHandoff {
            fixed_vars: vec![var("x5", 0), var("x7", 1)],
            ..Default::default()
        };
        assert!(h.write_printemps_initial_solution(&p).unwrap());
        assert_eq!(fs::read_to_string(&p).unwrap(), "x5 0\nx7 1\n");
    }

    #[test]
    fn write_printemps_fixed_wins_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.sol");
        let h = SolverHandoff {
            fixed_vars: vec![var("x1", 0)],
            incumbent: Some(vec![var("x1", 1), var("x2", 1)]),
            ..Default::default()
        };
        h.write_printemps_initial_solution(&p).unwrap();
        let content = fs::read_to_string(&p).unwrap();
        // x1 must appear exactly once with value 0 (fixed wins).
        assert_eq!(content, "x1 0\nx2 1\n");
    }

    #[test]
    fn write_printemps_skips_empty_names() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.sol");
        let h = SolverHandoff {
            fixed_vars: vec![var("", 1), var("x1", 1)],
            incumbent: Some(vec![var("", 0), var("x2", 0)]),
            ..Default::default()
        };
        h.write_printemps_initial_solution(&p).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "x1 1\nx2 0\n");
    }

    // --- write_fixed_vars ---

    #[test]
    fn write_fixed_vars_normal() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("fixed.txt");
        let h = SolverHandoff {
            fixed_vars: vec![var("x3", 1), var("x9", 0)],
            ..Default::default()
        };
        h.write_fixed_vars(&p).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "x3 1\nx9 0\n");
    }

    #[test]
    fn write_fixed_vars_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("fixed.txt");
        SolverHandoff::new().write_fixed_vars(&p).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "");
    }

    #[test]
    fn write_fixed_vars_skips_empty_names() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("fixed.txt");
        let h = SolverHandoff {
            fixed_vars: vec![var("", 1), var("x4", 0)],
            ..Default::default()
        };
        h.write_fixed_vars(&p).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "x4 0\n");
    }

    // --- write_bounds_json ---

    #[test]
    fn write_bounds_json_with_values() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bounds.json");
        let h = SolverHandoff {
            primal_bound: Some(42.0),
            dual_bound: Some(-1.5),
            ..Default::default()
        };
        h.write_bounds_json(&p, "OPTIMUM_FOUND", 1.25, Some(0)).unwrap();
        let s = fs::read_to_string(&p).unwrap();
        assert!(s.contains("\"status\": \"OPTIMUM_FOUND\""));
        assert!(s.contains("\"primal_bound\": 42"));
        assert!(s.contains("\"dual_bound\": -1.5"));
        assert!(s.contains("\"elapsed_sec\": 1.250000"));
        assert!(s.contains("\"exit_code\": 0"));
    }

    #[test]
    fn write_bounds_json_none_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bounds.json");
        SolverHandoff::new()
            .write_bounds_json(&p, "UNKNOWN", 0.0, None)
            .unwrap();
        let s = fs::read_to_string(&p).unwrap();
        assert!(s.contains("\"primal_bound\": null"));
        assert!(s.contains("\"dual_bound\": null"));
        assert!(s.contains("\"exit_code\": null"));
    }

    #[test]
    fn write_bounds_json_inf_primal() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bounds.json");
        let h = SolverHandoff {
            primal_bound: Some(f64::INFINITY),
            dual_bound: Some(f64::NEG_INFINITY),
            ..Default::default()
        };
        h.write_bounds_json(&p, "UNKNOWN", 0.0, None).unwrap();
        let s = fs::read_to_string(&p).unwrap();
        assert!(s.contains("\"primal_bound\": null"));
        assert!(s.contains("\"dual_bound\": null"));
    }

    #[test]
    fn write_bounds_json_no_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bounds.json");
        SolverHandoff::new()
            .write_bounds_json(&p, "UNKNOWN", 0.5, None)
            .unwrap();
        assert!(fs::read_to_string(&p).unwrap().contains("\"exit_code\": null"));
    }

    #[test]
    fn write_bounds_json_json_escaping() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bounds.json");
        SolverHandoff::new()
            .write_bounds_json(&p, "say \"hello\" \\world", 0.0, None)
            .unwrap();
        let s = fs::read_to_string(&p).unwrap();
        assert!(s.contains(r#"say \"hello\" \\world"#));
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
