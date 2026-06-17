//! Independent, exact-integer feasibility check for an OPB or WBO assignment.
//!
//! SCIP (the Phase-1 solver behind `scip-printemps`) works in `f64`, which only
//! represents integers exactly up to 2^53. On instances whose coefficients are
//! larger, SCIP can return an incumbent that violates a constraint it believes
//! satisfied — e.g. `a - b >= 1` where `a, b ≈ 2^64` loses the `±1`. This module
//! re-reads the *original* instance text and re-evaluates every constraint with
//! `i128` arithmetic, so such false-feasible solutions can be detected and
//! discarded.
//!
//! WBO (Weighted Boolean Optimization, `.wbo`) instances are handled too. They
//! open with a `soft: T ;` header giving the top cost `T`; hard constraints
//! (no prefix) must be satisfied; soft constraints are prefixed with their cost
//! in square brackets (`[w] +1 x1 >= 1 ;`) and may be violated at cost `w`. The
//! objective is the sum of the weights of violated soft constraints, and a
//! solution is admissible only if that sum is strictly less than `T`. See
//! `misc/OPBcompetition.md` §4.2.
//!
//! The check is intentionally conservative: anything it cannot evaluate exactly
//! (a coefficient or running activity outside `i128`, an unparsable token, a
//! malformed soft-constraint weight, or a literal whose variable is absent from
//! the assignment) yields [`VerifyOutcome::Unverifiable`], which callers treat
//! as "do not trust the solution" rather than silently passing.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::ops::ControlFlow;
use std::path::Path;

/// Outcome of checking an assignment against every constraint in an OPB/WBO file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// Every constraint is satisfied (and, for WBO, the total violated-soft cost
    /// is below the top cost).
    Satisfied,
    /// The constraint ending on `line` (1-based) is violated. For WBO this also
    /// covers a total violated-soft cost that reached the top cost.
    Violated { line: usize, detail: String },
    /// The check could not be completed exactly; the solution must not be
    /// trusted on the strength of this check.
    Unverifiable { reason: String },
}

/// Outcome of exactly recomputing the objective value for an assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectiveOutcome {
    /// The objective evaluated exactly to this value: for OPB the `min:`/`max:`
    /// weighted sum of literals plus any constant offset; for WBO the sum of the
    /// weights of violated soft constraints.
    Value(i128),
    /// The file has no objective (a pure satisfaction / PBS instance).
    NoObjective,
    /// The objective could not be evaluated exactly; the value must not be
    /// trusted on the strength of this check.
    Unverifiable { reason: String },
}

/// Check `assignment` (variable index → boolean value) against every constraint
/// in the OPB/WBO file at `instance`.
///
/// For OPB, the objective line (`min:` / `max:`) is ignored, since it does not
/// constrain feasibility, and every constraint must be satisfied. For WBO, hard
/// constraints must be satisfied, soft constraints (`[w] …`) may be violated,
/// and the solution is feasible only if the total weight of violated soft
/// constraints is strictly less than the top cost `T` from the `soft: T ;`
/// header (a cost `>= T` is inadmissible and reported as [`VerifyOutcome::Violated`]).
pub fn verify_assignment(
    instance: &Path,
    assignment: &HashMap<u32, bool>,
) -> io::Result<VerifyOutcome> {
    // WBO state, accumulated across the single forward pass. `soft:` always
    // precedes any soft constraint, so by the time a `[w] …` line is reached
    // `seen_soft_header` is already set.
    let mut seen_soft_header = false;
    let mut top: Option<i128> = None;
    let mut soft_header_line = 0usize;
    let mut soft_cost: i128 = 0;

    let broke = for_each_statement(instance, |tokens, line| {
        match classify_wbo_statement(tokens, line, seen_soft_header) {
            Err(reason) => ControlFlow::Break(VerifyOutcome::Unverifiable { reason }),
            Ok(WboStmt::SoftHeader(t)) => {
                seen_soft_header = true;
                top = t;
                soft_header_line = line;
                ControlFlow::Continue(())
            }
            // The objective line does not constrain feasibility.
            Ok(WboStmt::Objective) => ControlFlow::Continue(()),
            Ok(WboStmt::Hard) => match eval_constraint(tokens, assignment, line) {
                VerifyOutcome::Satisfied => ControlFlow::Continue(()),
                other => ControlFlow::Break(other),
            },
            // A soft constraint may be violated; charge its weight instead of
            // failing. Only an exact-arithmetic problem in its body, or a
            // cost overflow, makes the result untrustworthy.
            Ok(WboStmt::Soft { weight, body }) => match eval_constraint(&body, assignment, line) {
                VerifyOutcome::Satisfied => ControlFlow::Continue(()),
                VerifyOutcome::Violated { .. } => match soft_cost.checked_add(weight) {
                    Some(v) => {
                        soft_cost = v;
                        ControlFlow::Continue(())
                    }
                    None => ControlFlow::Break(unverifiable(
                        "accumulated soft cost exceeds i128 range".to_string(),
                        line,
                    )),
                },
                u @ VerifyOutcome::Unverifiable { .. } => ControlFlow::Break(u),
            },
        }
    })?;

    if let Some(outcome) = broke {
        return Ok(outcome);
    }

    // WBO admissibility: the total violated-soft cost must be strictly less than
    // the top cost `T` (a cost `>= T` is inadmissible — e.g. example3.wbo, whose
    // minimum cost equals `T`). `top == None` is `soft: ;` (T = ∞), which never
    // fails this check.
    if seen_soft_header {
        if let Some(t) = top {
            if soft_cost >= t {
                return Ok(VerifyOutcome::Violated {
                    line: soft_header_line,
                    detail: format!("total soft cost {soft_cost} >= top cost {t} (inadmissible)"),
                });
            }
        }
    }

    Ok(VerifyOutcome::Satisfied)
}

/// Exactly recompute the objective value of `assignment` from the OPB/WBO file
/// at `instance`. SCIP reports the objective of its incumbent as an `f64`, which
/// only represents integers exactly up to 2^53; on large-coefficient instances
/// the reported value can disagree with the true integer objective. This
/// re-reads the original instance text and re-evaluates the objective with
/// `i128` arithmetic, so such mismatches can be detected.
///
/// For OPB, the objective is the `min:`/`max:` weighted sum, returned as written
/// (the objective's own sense). For WBO there is no `min:` line: the objective
/// is the sum of the weights of violated soft constraints (a minimization),
/// which is exactly what SCIP's `.wbo` reader optimizes, so the two are directly
/// comparable. A file with neither is [`ObjectiveOutcome::NoObjective`]. Like
/// [`verify_assignment`], the check is conservative: anything that cannot be
/// evaluated exactly yields [`ObjectiveOutcome::Unverifiable`].
pub fn evaluate_objective(
    instance: &Path,
    assignment: &HashMap<u32, bool>,
) -> io::Result<ObjectiveOutcome> {
    let mut seen_soft_header = false;
    let mut soft_cost: i128 = 0;
    // Set when the pass produces a definitive answer early (an OPB `min:`/`max:`
    // line, or an unverifiable soft constraint).
    let mut result: Option<ObjectiveOutcome> = None;

    for_each_statement(instance, |tokens, line| {
        match classify_wbo_statement(tokens, line, seen_soft_header) {
            Err(reason) => {
                result = Some(ObjectiveOutcome::Unverifiable { reason });
                ControlFlow::Break(())
            }
            Ok(WboStmt::SoftHeader(_)) => {
                seen_soft_header = true;
                ControlFlow::Continue(())
            }
            // OPB objective: evaluate the line and stop (WBO has no `min:`/`max:`).
            Ok(WboStmt::Objective) => {
                result = Some(eval_objective(tokens, assignment, line));
                ControlFlow::Break(())
            }
            // Hard constraints do not contribute to the WBO objective.
            Ok(WboStmt::Hard) => ControlFlow::Continue(()),
            Ok(WboStmt::Soft { weight, body }) => match eval_constraint(&body, assignment, line) {
                VerifyOutcome::Satisfied => ControlFlow::Continue(()),
                VerifyOutcome::Violated { .. } => match soft_cost.checked_add(weight) {
                    Some(v) => {
                        soft_cost = v;
                        ControlFlow::Continue(())
                    }
                    None => {
                        result = Some(ObjectiveOutcome::Unverifiable {
                            reason: format!("line {line}: objective exceeds i128 range"),
                        });
                        ControlFlow::Break(())
                    }
                },
                VerifyOutcome::Unverifiable { reason } => {
                    result = Some(ObjectiveOutcome::Unverifiable { reason });
                    ControlFlow::Break(())
                }
            },
        }
    })?;

    if let Some(outcome) = result {
        return Ok(outcome);
    }
    if seen_soft_header {
        return Ok(ObjectiveOutcome::Value(soft_cost));
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

/// Drive the `;`-terminated OPB/WBO tokenizer, invoking `f` once per complete
/// statement with its tokens and the 1-based line on which the statement began.
///
/// Statements conventionally occupy one line, but the grammar permits a
/// statement to span lines or several to share a line, so tokens are
/// accumulated until a `;` (which may stand alone or be attached to the
/// previous token, e.g. `1;`). `f` returns a [`ControlFlow`]; the first `Break`
/// stops the scan and its value is returned as `Ok(Some(_))`. If every statement
/// yields `Continue`, returns `Ok(None)`. Comment (`*`) and blank lines are
/// skipped.
fn for_each_statement<T>(
    instance: &Path,
    mut f: impl FnMut(&[String], usize) -> ControlFlow<T>,
) -> io::Result<Option<T>> {
    let file = File::open(instance)?;
    let r = BufReader::new(file);
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
                if let ControlFlow::Break(v) = f(&pending, stmt_line) {
                    return Ok(Some(v));
                }
                pending.clear();
            } else {
                pending.push(tok.to_string());
            }
        }
    }
    Ok(None)
}

/// A structurally-classified OPB/WBO statement.
///
/// Constraint *satisfaction* is deliberately not evaluated here — each caller
/// evaluates only what it needs (feasibility vs. objective cost) via
/// [`eval_constraint`] — so this stays a pure parse and the two callers cannot
/// drift on the WBO syntax.
enum WboStmt {
    /// A `soft:` header with its top cost `T` (`None` = `soft: ;`, i.e. T = ∞).
    SoftHeader(Option<i128>),
    /// An objective line (`min:`/`max:`).
    Objective,
    /// A soft constraint: its weight and the constraint-body tokens (the `[w]`
    /// prefix removed).
    Soft { weight: i128, body: Vec<String> },
    /// A hard constraint; the caller evaluates its own `tokens`.
    Hard,
}

/// Structurally classify one `;`-terminated statement. `seen_soft_header` is the
/// running "a `soft:` line has already been seen" flag, used to reject a soft
/// constraint that appears before the header. The `Err` carries a ready-to-use
/// `Unverifiable` reason string (with the line prefix) for any malformed soft
/// header or weight; constraint bodies themselves are validated by the caller.
fn classify_wbo_statement(
    tokens: &[String],
    line: usize,
    seen_soft_header: bool,
) -> Result<WboStmt, String> {
    let Some(first) = tokens.first() else {
        return Ok(WboStmt::Hard);
    };
    if first.starts_with("soft:") || first == "soft" {
        return match parse_soft_header(tokens) {
            Some(top) => Ok(WboStmt::SoftHeader(top)),
            None => Err(format!("line {line}: malformed soft: header")),
        };
    }
    match first.as_str() {
        "min:" | "max:" | "min" | "max" => Ok(WboStmt::Objective),
        _ if first.starts_with('[') => {
            if !seen_soft_header {
                return Err(format!("line {line}: soft constraint before soft: header"));
            }
            match parse_soft_prefix(tokens) {
                Some((weight, body)) => Ok(WboStmt::Soft { weight, body }),
                None => Err(format!("line {line}: malformed soft-constraint weight")),
            }
        }
        _ => Ok(WboStmt::Hard),
    }
}

/// Parse a `soft:` header (`tokens[0]` already confirmed to be `soft:`/`soft`).
/// Returns `Some(Some(T))` for `soft: T ;`, `Some(None)` for `soft: ;` (T = ∞),
/// and `None` if malformed. Tolerant of a top cost attached to the keyword
/// (`soft:6`) and of a detached `:` token (`soft : 6`).
fn parse_soft_header(tokens: &[String]) -> Option<Option<i128>> {
    let first = tokens.first()?.as_str();
    let after_kw = if let Some(s) = first.strip_prefix("soft:") {
        s
    } else if first == "soft" {
        ""
    } else {
        return None;
    };
    let mut nums: Vec<&str> = Vec::new();
    if !after_kw.is_empty() {
        nums.push(after_kw);
    }
    for t in &tokens[1..] {
        let s = t.as_str();
        let s = s.strip_prefix(':').unwrap_or(s);
        if !s.is_empty() {
            nums.push(s);
        }
    }
    match nums.as_slice() {
        [] => Some(None),
        [n] => parse_unsigned_i128(n).map(Some),
        _ => None,
    }
}

/// Parse a `[w]` soft-constraint weight prefix, tolerating whitespace-split
/// tokens (`[5]`, `[5` + `]`, `[` + `5]`, `[` + `5` + `]`, `[ 5 ]`) and body
/// text attached to the closing bracket (`[5]+1`). Returns the weight and the
/// remaining constraint-body tokens. `None` on any malformed prefix (missing
/// bracket, empty/non-digit weight, or a weight outside `i128`), which callers
/// map to `Unverifiable`.
fn parse_soft_prefix(tokens: &[String]) -> Option<(i128, Vec<String>)> {
    if !tokens.first()?.starts_with('[') {
        return None;
    }
    let mut joined = String::new();
    let mut body: Vec<String> = Vec::new();
    let mut closed = false;
    for t in tokens {
        if closed {
            body.push(t.clone());
            continue;
        }
        if let Some(pos) = t.find(']') {
            joined.push_str(&t[..pos]);
            let trailing = &t[pos + 1..];
            if !trailing.is_empty() {
                body.push(trailing.to_string());
            }
            closed = true;
        } else {
            joined.push_str(t);
        }
    }
    if !closed {
        // No closing `]` before the end of the statement.
        return None;
    }
    let digits = joined.strip_prefix('[')?;
    let weight = parse_unsigned_i128(digits)?;
    Some((weight, body))
}

/// Parse a non-negative integer (surrounding whitespace allowed), rejecting
/// signs and non-digit characters. `None` on empty, malformed, or out-of-`i128`.
fn parse_unsigned_i128(s: &str) -> Option<i128> {
    let s = s.trim();
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i128>().ok()
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

    // ----------------------------------------------------------------------
    // WBO feasibility (`verify_assignment`)
    // ----------------------------------------------------------------------

    /// example1.wbo: a violated soft constraint is allowed as long as the total
    /// soft cost stays below the top cost.
    #[test]
    fn wbo_violated_soft_under_top_is_satisfied() {
        let f = make_tmp("* #variable= 1\nsoft: 6 ;\n[2] +1 x1 >= 1 ;\n[3] -1 x1 >= 0 ;\n");
        // x1=0: [2] violated (cost 2), [3] satisfied. 2 < 6 -> Satisfied.
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, false)])).unwrap(),
            VerifyOutcome::Satisfied
        );
        // x1=1: [2] satisfied, [3] violated (cost 3). 3 < 6 -> Satisfied.
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, true)])).unwrap(),
            VerifyOutcome::Satisfied
        );
    }

    #[test]
    fn wbo_top_cost_exceeded_is_violated() {
        // Both soft constraints violated -> cost 3+4=7 >= top 6 -> inadmissible.
        let f = make_tmp("soft: 6 ;\n[3] +1 x1 >= 1 ;\n[4] +1 x2 >= 1 ;\n");
        assert!(matches!(
            verify_assignment(f.path(), &assign(&[(1, false), (2, false)])).unwrap(),
            VerifyOutcome::Violated { .. }
        ));
    }

    #[test]
    fn wbo_top_cost_equal_is_violated() {
        // Boundary: cost exactly equals the top cost is inadmissible (`>=`).
        let f = make_tmp("soft: 6 ;\n[2] +1 x1 >= 1 ;\n[4] +1 x2 >= 1 ;\n");
        // Both violated -> 2+4 == 6.
        assert!(matches!(
            verify_assignment(f.path(), &assign(&[(1, false), (2, false)])).unwrap(),
            VerifyOutcome::Violated { .. }
        ));
    }

    #[test]
    fn wbo_infinite_top_never_fails_on_cost() {
        // `soft: ;` (T = infinity): any finite soft cost is admissible.
        let f = make_tmp("soft: ;\n[2] +1 x1 >= 1 ;\n[3] +1 x2 >= 1 ;\n");
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, false), (2, false)])).unwrap(),
            VerifyOutcome::Satisfied
        );
    }

    #[test]
    fn wbo_hard_constraint_violation_is_violated() {
        // A hard constraint (no `[w]` prefix) must hold regardless of soft cost.
        let f = make_tmp("soft: 100 ;\n[2] +1 x1 >= 1 ;\n+1 x2 >= 1 ;\n");
        assert!(matches!(
            verify_assignment(f.path(), &assign(&[(1, true), (2, false)])).unwrap(),
            VerifyOutcome::Violated { .. }
        ));
    }

    #[test]
    fn wbo_negated_literal_in_soft() {
        // [3] +1 ~x1 >= 1 : satisfied when x1=0, violated (cost 3) when x1=1.
        let f = make_tmp("soft: 10 ;\n[3] +1 ~x1 >= 1 ;\n");
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, false)])).unwrap(),
            VerifyOutcome::Satisfied
        );
        // x1=1 -> cost 3 < 10 -> still feasible (soft may be violated).
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, true)])).unwrap(),
            VerifyOutcome::Satisfied
        );
    }

    #[test]
    fn wbo_product_term_in_soft() {
        // [2] +1 x1 x2 >= 1 violated unless both are true.
        let f = make_tmp("soft: 1 ;\n[2] +1 x1 x2 >= 1 ;\n");
        // Both true -> satisfied, cost 0 < 1.
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, true), (2, true)])).unwrap(),
            VerifyOutcome::Satisfied
        );
        // Product 0 -> violated, cost 2 >= top 1 -> inadmissible.
        assert!(matches!(
            verify_assignment(f.path(), &assign(&[(1, true), (2, false)])).unwrap(),
            VerifyOutcome::Violated { .. }
        ));
    }

    #[test]
    fn wbo_split_weight_token() {
        // `[ 2 ]` splits into three tokens; must still parse.
        let f = make_tmp("soft: 10 ;\n[ 2 ] +1 x1 >= 1 ;\n");
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, true)])).unwrap(),
            VerifyOutcome::Satisfied
        );
        // x1=0 -> violated, cost 2 < 10 -> feasible.
        assert_eq!(
            verify_assignment(f.path(), &assign(&[(1, false)])).unwrap(),
            VerifyOutcome::Satisfied
        );
    }

    #[test]
    fn wbo_weight_overflow_is_unverifiable() {
        let huge = "1".to_string() + &"0".repeat(40);
        let f = make_tmp(&format!("soft: 10 ;\n[{huge}] +1 x1 >= 1 ;\n"));
        assert!(matches!(
            verify_assignment(f.path(), &assign(&[(1, false)])).unwrap(),
            VerifyOutcome::Unverifiable { .. }
        ));
    }

    #[test]
    fn wbo_missing_var_in_soft_is_unverifiable() {
        let f = make_tmp("soft: 10 ;\n[2] +1 x1 +1 x2 >= 1 ;\n");
        assert!(matches!(
            verify_assignment(f.path(), &assign(&[(1, true)])).unwrap(),
            VerifyOutcome::Unverifiable { .. }
        ));
    }

    #[test]
    fn wbo_soft_before_header_is_unverifiable() {
        // A `[w]` constraint with no preceding `soft:` header is malformed.
        let f = make_tmp("[2] +1 x1 >= 1 ;\n");
        assert!(matches!(
            verify_assignment(f.path(), &assign(&[(1, true)])).unwrap(),
            VerifyOutcome::Unverifiable { .. }
        ));
    }

    // ----------------------------------------------------------------------
    // WBO objective (`evaluate_objective`)
    // ----------------------------------------------------------------------

    #[test]
    fn wbo_objective_is_violated_soft_sum() {
        let f = make_tmp("soft: 6 ;\n[2] +1 x1 >= 1 ;\n[3] -1 x1 >= 0 ;\n");
        // x1=0: only [2] violated -> cost 2 (the spec's optimal cost).
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, false)])).unwrap(),
            ObjectiveOutcome::Value(2)
        );
        // x1=1: only [3] violated -> cost 3.
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, true)])).unwrap(),
            ObjectiveOutcome::Value(3)
        );
    }

    #[test]
    fn wbo_objective_zero_when_all_soft_satisfied() {
        let f = make_tmp("soft: 10 ;\n[2] +1 x1 >= 1 ;\n[3] +1 x2 >= 1 ;\n");
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, true), (2, true)])).unwrap(),
            ObjectiveOutcome::Value(0)
        );
    }

    #[test]
    fn wbo_objective_infinite_top_still_sums() {
        // The top cost does not affect the objective value, only admissibility.
        let f = make_tmp("soft: ;\n[2] +1 x1 >= 1 ;\n[3] +1 x2 >= 1 ;\n");
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, false), (2, false)])).unwrap(),
            ObjectiveOutcome::Value(5)
        );
    }

    #[test]
    fn wbo_objective_ignores_hard_constraints() {
        // example2.wbo: hard `-1 x1 -1 x2 >= -1`; objective counts only soft.
        let f = make_tmp("soft: 6 ;\n[2] +1 x1 >= 1 ;\n[3] +1 x2 >= 1 ;\n-1 x1 -1 x2 >= -1 ;\n");
        // x1=0, x2=1: [2] violated (2), [3] satisfied, hard satisfied -> cost 2.
        assert_eq!(
            evaluate_objective(f.path(), &assign(&[(1, false), (2, true)])).unwrap(),
            ObjectiveOutcome::Value(2)
        );
    }

    #[test]
    fn wbo_objective_missing_var_is_unverifiable() {
        let f = make_tmp("soft: 10 ;\n[2] +1 x1 +1 x2 >= 1 ;\n");
        assert!(matches!(
            evaluate_objective(f.path(), &assign(&[(1, true)])).unwrap(),
            ObjectiveOutcome::Unverifiable { .. }
        ));
    }

    #[test]
    fn wbo_objective_weight_overflow_is_unverifiable() {
        let huge = "1".to_string() + &"0".repeat(40);
        let f = make_tmp(&format!("soft: 10 ;\n[{huge}] +1 x1 >= 1 ;\n"));
        assert!(matches!(
            evaluate_objective(f.path(), &assign(&[(1, false)])).unwrap(),
            ObjectiveOutcome::Unverifiable { .. }
        ));
    }
}
