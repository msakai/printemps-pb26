//! Independent, exact-integer feasibility check for an OPB assignment.
//!
//! SCIP (the Phase-1 solver behind `scip-printemps`) works in `f64`, which only
//! represents integers exactly up to 2^53. On instances whose coefficients are
//! larger, SCIP can return an incumbent that violates a constraint it believes
//! satisfied — e.g. `a - b >= 1` where `a, b ≈ 2^64` loses the `±1`. This module
//! re-reads the *original* OPB text and re-evaluates every constraint with
//! `i128` arithmetic, so such false-feasible solutions can be detected and
//! discarded.
//!
//! The check is intentionally conservative: anything it cannot evaluate exactly
//! (a coefficient or running activity outside `i128`, an unparsable token, or a
//! literal whose variable is absent from the assignment) yields
//! [`VerifyOutcome::Unverifiable`], which callers treat as "do not trust the
//! solution" rather than silently passing.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

/// Outcome of checking an assignment against every constraint in an OPB file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// Every constraint is satisfied.
    Satisfied,
    /// The constraint ending on `line` (1-based) is violated.
    Violated { line: usize, detail: String },
    /// The check could not be completed exactly; the solution must not be
    /// trusted on the strength of this check.
    Unverifiable { reason: String },
}

/// Outcome of exactly recomputing the objective value for an assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectiveOutcome {
    /// The objective evaluated exactly to this value (the `min:`/`max:`
    /// weighted sum of literals plus any constant offset).
    Value(i128),
    /// The OPB file has no objective line (a pure satisfaction instance).
    NoObjective,
    /// The objective could not be evaluated exactly; the value must not be
    /// trusted on the strength of this check.
    Unverifiable { reason: String },
}

/// Check `assignment` (variable index → boolean value) against every constraint
/// in the OPB file at `instance`. The objective line (`min:` / `max:`) is
/// ignored, since it does not constrain feasibility.
pub fn verify_assignment(
    instance: &Path,
    assignment: &HashMap<u32, bool>,
) -> io::Result<VerifyOutcome> {
    let f = File::open(instance)?;
    let r = BufReader::new(f);

    // OPB statements terminate with `;`. They are conventionally one per line,
    // but the grammar permits a statement to span lines or several statements to
    // share a line, so accumulate tokens until a `;` is seen.
    let mut pending: Vec<String> = Vec::new();
    let mut stmt_line = 0usize;

    for (idx, line) in r.lines().enumerate() {
        let line = line?;
        let line_no = idx + 1;
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('*') {
            continue;
        }
        for tok in line.split_whitespace() {
            if pending.is_empty() {
                stmt_line = line_no;
            }
            // `;` may stand alone or be attached to the previous token (`1;`).
            if let Some(stripped) = tok.strip_suffix(';') {
                if !stripped.is_empty() {
                    pending.push(stripped.to_string());
                }
                match check_statement(&pending, assignment, stmt_line) {
                    VerifyOutcome::Satisfied => {}
                    other => return Ok(other),
                }
                pending.clear();
            } else {
                pending.push(tok.to_string());
            }
        }
    }

    Ok(VerifyOutcome::Satisfied)
}

/// Exactly recompute the objective value of `assignment` from the OPB file at
/// `instance`. SCIP reports the objective of its incumbent as an `f64`, which
/// only represents integers exactly up to 2^53; on large-coefficient instances
/// the reported value can disagree with the true integer objective. This
/// re-reads the original OPB text and re-evaluates the `min:`/`max:` line with
/// `i128` arithmetic, so such mismatches can be detected.
///
/// The weighted sum is returned as written (the objective's own sense); callers
/// compare it against SCIP's reported value, whose sign convention matches the
/// OPB. Like [`verify_assignment`], the check is conservative: anything that
/// cannot be evaluated exactly yields [`ObjectiveOutcome::Unverifiable`].
pub fn evaluate_objective(
    instance: &Path,
    assignment: &HashMap<u32, bool>,
) -> io::Result<ObjectiveOutcome> {
    let f = File::open(instance)?;
    let r = BufReader::new(f);

    // Same `;`-terminated tokenization as `verify_assignment`; accumulate
    // tokens until a `;` and evaluate the first objective statement found.
    let mut pending: Vec<String> = Vec::new();
    let mut stmt_line = 0usize;

    for (idx, line) in r.lines().enumerate() {
        let line = line?;
        let line_no = idx + 1;
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('*') {
            continue;
        }
        for tok in line.split_whitespace() {
            if pending.is_empty() {
                stmt_line = line_no;
            }
            if let Some(stripped) = tok.strip_suffix(';') {
                if !stripped.is_empty() {
                    pending.push(stripped.to_string());
                }
                if matches!(
                    pending.first().map(String::as_str),
                    Some("min:" | "max:" | "min" | "max")
                ) {
                    return Ok(eval_objective(&pending, assignment, stmt_line));
                }
                pending.clear();
            } else {
                pending.push(tok.to_string());
            }
        }
    }

    Ok(ObjectiveOutcome::NoObjective)
}

/// Evaluate the terms of an objective statement (`min:`/`max:` keyword already
/// confirmed as `tokens[0]`). Mirrors the term-folding of [`eval_constraint`],
/// but there is no operator or right-hand side, and a literal-less coefficient
/// is a legal constant offset rather than a malformed term.
fn eval_objective(
    tokens: &[String],
    assignment: &HashMap<u32, bool>,
    line: usize,
) -> ObjectiveOutcome {
    let mut total: i128 = 0;
    let mut term_coeff: Option<i128> = None;
    let mut term_val: i128 = 1;
    let mut term_has_lit = false;

    macro_rules! flush_term {
        () => {
            if let Some(c) = term_coeff.take() {
                // A coefficient with no literal is a constant offset.
                let factor = if term_has_lit { term_val } else { 1 };
                match c
                    .checked_mul(factor)
                    .and_then(|contrib| total.checked_add(contrib))
                {
                    Some(v) => total = v,
                    None => {
                        return ObjectiveOutcome::Unverifiable {
                            reason: format!("line {line}: objective exceeds i128 range"),
                        }
                    }
                }
            }
        };
    }

    for t in tokens.iter().skip(1) {
        let t = t.as_str();
        if is_literal_start(t) {
            if term_coeff.is_none() {
                return ObjectiveOutcome::Unverifiable {
                    reason: format!("line {line}: literal '{t}' without a coefficient"),
                };
            }
            match literal_value(t, assignment) {
                LitVal::Val(v) => {
                    term_val *= v as i128;
                    term_has_lit = true;
                }
                LitVal::MissingVar => {
                    return ObjectiveOutcome::Unverifiable {
                        reason: format!("line {line}: variable of literal '{t}' not in assignment"),
                    }
                }
                LitVal::Bad => {
                    return ObjectiveOutcome::Unverifiable {
                        reason: format!("line {line}: malformed literal '{t}'"),
                    }
                }
            }
        } else {
            flush_term!();
            match parse_int(t) {
                Some(v) => {
                    term_coeff = Some(v);
                    term_val = 1;
                    term_has_lit = false;
                }
                None => {
                    return ObjectiveOutcome::Unverifiable {
                        reason: format!("line {line}: unparsable token '{t}'"),
                    }
                }
            }
        }
    }
    flush_term!();

    ObjectiveOutcome::Value(total)
}

/// Evaluate one complete `;`-terminated statement. Objective statements are
/// skipped (reported as `Satisfied`); constraints are checked exactly.
fn check_statement(
    tokens: &[String],
    assignment: &HashMap<u32, bool>,
    line: usize,
) -> VerifyOutcome {
    let Some(first) = tokens.first() else {
        return VerifyOutcome::Satisfied;
    };
    match first.as_str() {
        "min:" | "max:" | "min" | "max" => VerifyOutcome::Satisfied,
        _ => eval_constraint(tokens, assignment, line),
    }
}

fn eval_constraint(
    tokens: &[String],
    assignment: &HashMap<u32, bool>,
    line: usize,
) -> VerifyOutcome {
    let mut activity: i128 = 0;
    // Current term: a coefficient and the running product of its literal values
    // (each 0/1, so the product stays 0/1 and supports non-linear product terms).
    let mut term_coeff: Option<i128> = None;
    let mut term_val: i128 = 1;
    let mut term_has_lit = false;
    let mut op: Option<String> = None;
    let mut rhs: Option<i128> = None;

    // Fold the in-progress term into `activity`. Returns an error string on
    // overflow or a malformed (literal-less) term.
    macro_rules! flush_term {
        () => {
            if let Some(c) = term_coeff.take() {
                if !term_has_lit {
                    return unverifiable(format!("coefficient {c} without a literal"), line);
                }
                match c
                    .checked_mul(term_val)
                    .and_then(|contrib| activity.checked_add(contrib))
                {
                    Some(v) => activity = v,
                    None => return unverifiable("activity exceeds i128 range".to_string(), line),
                }
            }
        };
    }

    for t in tokens {
        let t = t.as_str();
        if op.is_none() && is_operator(t) {
            flush_term!();
            op = Some(t.to_string());
            continue;
        }
        if op.is_some() {
            match parse_int(t) {
                Some(v) => rhs = Some(v),
                None => return unverifiable(format!("unparsable right-hand side '{t}'"), line),
            }
            continue;
        }
        if is_literal_start(t) {
            if term_coeff.is_none() {
                return unverifiable(format!("literal '{t}' without a coefficient"), line);
            }
            match literal_value(t, assignment) {
                LitVal::Val(v) => {
                    term_val *= v as i128;
                    term_has_lit = true;
                }
                LitVal::MissingVar => {
                    return unverifiable(
                        format!("variable of literal '{t}' not in assignment"),
                        line,
                    )
                }
                LitVal::Bad => return unverifiable(format!("malformed literal '{t}'"), line),
            }
        } else {
            // A coefficient starts a new term; close out the previous one first.
            flush_term!();
            match parse_int(t) {
                Some(v) => {
                    term_coeff = Some(v);
                    term_val = 1;
                    term_has_lit = false;
                }
                None => return unverifiable(format!("unparsable token '{t}'"), line),
            }
        }
    }

    let Some(op) = op else {
        return unverifiable("constraint has no relational operator".to_string(), line);
    };
    let Some(rhs) = rhs else {
        return unverifiable("constraint has no right-hand side".to_string(), line);
    };

    let satisfied = match op.as_str() {
        ">=" => activity >= rhs,
        "<=" => activity <= rhs,
        "=" | "==" => activity == rhs,
        ">" => activity > rhs,
        "<" => activity < rhs,
        other => return unverifiable(format!("unknown operator '{other}'"), line),
    };

    if satisfied {
        VerifyOutcome::Satisfied
    } else {
        VerifyOutcome::Violated {
            line,
            detail: format!("activity {activity} {op} {rhs} is false"),
        }
    }
}

fn unverifiable(reason: String, line: usize) -> VerifyOutcome {
    VerifyOutcome::Unverifiable {
        reason: format!("line {line}: {reason}"),
    }
}

fn is_operator(t: &str) -> bool {
    matches!(t, ">=" | "<=" | "=" | "==" | ">" | "<")
}

fn is_literal_start(t: &str) -> bool {
    t.starts_with('x') || t.starts_with('~')
}

fn parse_int(t: &str) -> Option<i128> {
    t.parse::<i128>().ok()
}

enum LitVal {
    Val(u8),
    MissingVar,
    Bad,
}

fn literal_value(t: &str, assignment: &HashMap<u32, bool>) -> LitVal {
    let (negated, rest) = match t.strip_prefix('~') {
        Some(r) => (true, r),
        None => (false, t),
    };
    let Some(digits) = rest.strip_prefix('x') else {
        return LitVal::Bad;
    };
    let Ok(idx) = digits.parse::<u32>() else {
        return LitVal::Bad;
    };
    let Some(&value) = assignment.get(&idx) else {
        return LitVal::MissingVar;
    };
    let bit = u8::from(value);
    LitVal::Val(if negated { 1 - bit } else { bit })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_tmp(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    fn assign(pairs: &[(u32, bool)]) -> HashMap<u32, bool> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn satisfied_ge() {
        let f = make_tmp("* header\n+1 x1 +1 x2 >= 1 ;\n");
        let a = assign(&[(1, true), (2, false)]);
        assert_eq!(
            verify_assignment(f.path(), &a).unwrap(),
            VerifyOutcome::Satisfied
        );
    }

    #[test]
    fn violated_ge() {
        let f = make_tmp("+1 x1 +1 x2 >= 1 ;\n");
        let a = assign(&[(1, false), (2, false)]);
        assert!(matches!(
            verify_assignment(f.path(), &a).unwrap(),
            VerifyOutcome::Violated { .. }
        ));
    }

    #[test]
    fn equality_pass_and_fail() {
        let f = make_tmp("+1 x1 +1 x2 = 1 ;\n");
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, true), (2, false)])).unwrap(),
            VerifyOutcome::Satisfied
        );
        assert!(matches!(
            verify_assignment(f.path(), &assign(&[(1, true), (2, true)])).unwrap(),
            VerifyOutcome::Violated { .. }
        ));
    }

    #[test]
    fn negated_literal() {
        // ~x1 contributes (1 - x1).
        let f = make_tmp("+1 ~x1 >= 1 ;\n");
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, false)])).unwrap(),
            VerifyOutcome::Satisfied
        );
        assert!(matches!(
            verify_assignment(f.path(), &assign(&[(1, true)])).unwrap(),
            VerifyOutcome::Violated { .. }
        ));
    }

    #[test]
    fn product_term() {
        // +1 x1 x2 >= 1 holds only when both are true.
        let f = make_tmp("+1 x1 x2 >= 1 ;\n");
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, true), (2, true)])).unwrap(),
            VerifyOutcome::Satisfied
        );
        assert!(matches!(
            verify_assignment(f.path(), &assign(&[(1, true), (2, false)])).unwrap(),
            VerifyOutcome::Violated { .. }
        ));
    }

    #[test]
    fn objective_line_ignored() {
        let f = make_tmp("min: +5 x1 +3 x2 ;\n+1 x1 >= 1 ;\n");
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, true), (2, true)])).unwrap(),
            VerifyOutcome::Satisfied
        );
    }

    #[test]
    fn large_i128_coefficient() {
        // Beyond i64 but within i128: +9223372036854775808 (2^63) x1 >= 2^63.
        let f = make_tmp("+9223372036854775808 x1 >= 9223372036854775808 ;\n");
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, true)])).unwrap(),
            VerifyOutcome::Satisfied
        );
    }

    #[test]
    fn coefficient_out_of_i128_is_unverifiable() {
        // 10^40 does not fit in i128.
        let huge = "1".to_string() + &"0".repeat(40);
        let f = make_tmp(&format!("+{huge} x1 >= 1 ;\n"));
        assert!(matches!(
            verify_assignment(f.path(), &assign(&[(1, true)])).unwrap(),
            VerifyOutcome::Unverifiable { .. }
        ));
    }

    #[test]
    fn missing_variable_is_unverifiable() {
        let f = make_tmp("+1 x1 +1 x2 >= 1 ;\n");
        assert!(matches!(
            verify_assignment(f.path(), &assign(&[(1, true)])).unwrap(),
            VerifyOutcome::Unverifiable { .. }
        ));
    }

    #[test]
    fn arraycomm_style_difference_violated() {
        // a - b >= 1 with a == b (all "diff bits" zero) must be flagged. The
        // f64-lossy SCIP run produced exactly this: paired weights cancel.
        let f = make_tmp("+1 x1 -1 x2 +2 x3 -2 x4 >= 1 ;\n");
        let a = assign(&[(1, true), (2, true), (3, true), (4, true)]);
        assert!(matches!(
            verify_assignment(f.path(), &a).unwrap(),
            VerifyOutcome::Violated { .. }
        ));
    }

    #[test]
    fn statement_spanning_two_lines() {
        let f = make_tmp("+1 x1 +1 x2\n>= 1 ;\n");
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, true), (2, false)])).unwrap(),
            VerifyOutcome::Satisfied
        );
    }

    #[test]
    fn attached_semicolon() {
        let f = make_tmp("+1 x1 >= 1;\n");
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, true)])).unwrap(),
            VerifyOutcome::Satisfied
        );
    }

    #[test]
    fn objective_basic_sum() {
        let f = make_tmp("min: +5 x1 +3 x2 ;\n+1 x1 >= 1 ;\n");
        // 5*1 + 3*0 = 5
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, true), (2, false)])).unwrap(),
            ObjectiveOutcome::Value(5)
        );
        // 5*1 + 3*1 = 8
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, true), (2, true)])).unwrap(),
            ObjectiveOutcome::Value(8)
        );
    }

    #[test]
    fn objective_negative_coeff_and_constant_offset() {
        // A literal-less coefficient is a constant offset: 7 + (-2)*x1.
        let f = make_tmp("min: 7 -2 x1 ;\n+1 x1 >= 0 ;\n");
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, false)])).unwrap(),
            ObjectiveOutcome::Value(7)
        );
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, true)])).unwrap(),
            ObjectiveOutcome::Value(5)
        );
    }

    #[test]
    fn objective_product_term() {
        // +4 x1 x2 contributes 4 only when both literals are true.
        let f = make_tmp("min: +4 x1 x2 +1 x3 ;\n+1 x3 >= 0 ;\n");
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, true), (2, true), (3, false)])).unwrap(),
            ObjectiveOutcome::Value(4)
        );
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, true), (2, false), (3, true)])).unwrap(),
            ObjectiveOutcome::Value(1)
        );
    }

    #[test]
    fn objective_negated_literal() {
        // ~x1 contributes (1 - x1).
        let f = make_tmp("min: +3 ~x1 ;\n+1 x1 >= 0 ;\n");
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, false)])).unwrap(),
            ObjectiveOutcome::Value(3)
        );
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, true)])).unwrap(),
            ObjectiveOutcome::Value(0)
        );
    }

    #[test]
    fn objective_absent_is_no_objective() {
        let f = make_tmp("+1 x1 +1 x2 >= 1 ;\n");
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, true), (2, false)])).unwrap(),
            ObjectiveOutcome::NoObjective
        );
    }

    #[test]
    fn objective_large_i128_coefficient() {
        // 2^63, beyond i64 but within i128.
        let f = make_tmp("min: +9223372036854775808 x1 ;\n+1 x1 >= 0 ;\n");
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, true)])).unwrap(),
            ObjectiveOutcome::Value(9223372036854775808)
        );
    }

    #[test]
    fn objective_coefficient_out_of_i128_is_unverifiable() {
        let huge = "1".to_string() + &"0".repeat(40);
        let f = make_tmp(&format!("min: +{huge} x1 ;\n+1 x1 >= 0 ;\n"));
        assert!(matches!(
            evaluate_objective(f.path(), &assign(&[(1, true)])).unwrap(),
            ObjectiveOutcome::Unverifiable { .. }
        ));
    }

    #[test]
    fn objective_missing_variable_is_unverifiable() {
        let f = make_tmp("min: +1 x1 +1 x2 ;\n+1 x1 >= 0 ;\n");
        assert!(matches!(
            evaluate_objective(f.path(), &assign(&[(1, true)])).unwrap(),
            ObjectiveOutcome::Unverifiable { .. }
        ));
    }
}
