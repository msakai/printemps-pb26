use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub struct OpbInfo {
    pub has_objective: bool,
    /// Whether this is a WBO (Weighted Boolean Optimization) instance, detected
    /// by a `soft:` header line. WBO instances have no explicit `min:` objective
    /// (so `has_objective` stays `false`); their objective is the sum of the
    /// weights of violated soft constraints. Callers that need "is this an
    /// optimization instance?" should test `has_objective || is_wbo`.
    pub is_wbo: bool,
    /// Value of the `intsize=` field in the PB-competition header comment
    /// (`* #variable= .. #constraint= .. intsize= N`), i.e. the bit length of
    /// the largest coefficient. `None` when no such header is present.
    pub intsize: Option<u64>,
}

pub fn scan<P: AsRef<Path>>(path: P) -> io::Result<OpbInfo> {
    let f = File::open(path)?;
    let r = BufReader::new(f);
    let mut has_objective = false;
    let mut is_wbo = false;
    let mut intsize = None;
    for line in r.lines() {
        let line = line?;
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('*') {
            // The PB header is a comment line; parse `intsize=` from it but keep
            // scanning for the objective in the (non-comment) body.
            if intsize.is_none() {
                intsize = parse_intsize(trimmed);
            }
            continue;
        }
        // A WBO instance's first non-comment line is the `soft:` header; it has
        // no `min:` objective (the objective is the violated-soft-cost sum,
        // recomputed in `verify::evaluate_objective`).
        if trimmed.starts_with("soft:") || trimmed.starts_with("soft ") {
            is_wbo = true;
            break;
        }
        if trimmed.starts_with("min:") || trimmed.starts_with("min ") {
            has_objective = true;
            break;
        }
    }
    Ok(OpbInfo {
        has_objective,
        is_wbo,
        intsize,
    })
}

/// Extract the integer after `intsize=` in a header comment, tolerating an
/// optional space (`intsize= 65` and `intsize=65`). Returns `None` if the
/// marker is absent or not followed by digits.
fn parse_intsize(line: &str) -> Option<u64> {
    let after = line.split_once("intsize=")?.1;
    let digits: String = after
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
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

    #[test]
    fn scan_detects_min_colon() {
        let f = make_tmp("min: +1 x1;\n+1 x1 >= 1;\n");
        assert!(scan(f.path()).unwrap().has_objective);
    }

    #[test]
    fn scan_detects_min_space() {
        let f = make_tmp("min +1 x1;\n+1 x1 >= 1;\n");
        assert!(scan(f.path()).unwrap().has_objective);
    }

    #[test]
    fn scan_no_objective() {
        let f = make_tmp("+1 x1 >= 1;\n+1 x2 >= 0;\n");
        assert!(!scan(f.path()).unwrap().has_objective);
    }

    #[test]
    fn scan_empty_file() {
        let f = make_tmp("");
        assert!(!scan(f.path()).unwrap().has_objective);
    }

    #[test]
    fn scan_only_comments() {
        let f = make_tmp("* this is a comment\n* another comment\n");
        assert!(!scan(f.path()).unwrap().has_objective);
    }

    #[test]
    fn scan_blank_lines_then_objective() {
        let f = make_tmp("\n\n\nmin: +1 x1;\n");
        assert!(scan(f.path()).unwrap().has_objective);
    }

    #[test]
    fn scan_comment_hides_objective() {
        let f = make_tmp("* min: +1 x1;\n+1 x1 >= 1;\n");
        assert!(!scan(f.path()).unwrap().has_objective);
    }

    #[test]
    fn scan_whitespace_prefix_on_min() {
        let f = make_tmp("   min: +1 x1;\n");
        assert!(scan(f.path()).unwrap().has_objective);
    }

    #[test]
    fn scan_nonexistent_file_returns_error() {
        assert!(scan("/nonexistent/path/does/not/exist.opb").is_err());
    }

    #[test]
    fn scan_objective_after_constraints() {
        let f = make_tmp("+1 x1 >= 1;\n+1 x2 >= 0;\nmin: +1 x1 +2 x2;\n");
        assert!(scan(f.path()).unwrap().has_objective);
    }

    #[test]
    fn scan_intsize_with_space() {
        let f =
            make_tmp("* #variable= 6208 #constraint= 3201 #equal= 3200 intsize= 65\n+1 x1 >= 1;\n");
        let info = scan(f.path()).unwrap();
        assert_eq!(info.intsize, Some(65));
        assert!(!info.has_objective);
    }

    #[test]
    fn scan_intsize_no_space() {
        let f = make_tmp("* #variable= 10 #constraint= 2 intsize=22\nmin: +1 x1;\n");
        let info = scan(f.path()).unwrap();
        assert_eq!(info.intsize, Some(22));
        assert!(info.has_objective);
    }

    #[test]
    fn scan_intsize_absent() {
        let f = make_tmp("* just a comment\n+1 x1 >= 1;\n");
        assert_eq!(scan(f.path()).unwrap().intsize, None);
    }

    #[test]
    fn scan_intsize_followed_by_extra_fields() {
        // Non-linear header: intsize is not the last field on the line.
        let f = make_tmp(
            "* #variable= 50 #constraint= 0 #equal= 0 intsize= 22 #product= 14362\n+1 x1 >= 1;\n",
        );
        assert_eq!(scan(f.path()).unwrap().intsize, Some(22));
    }

    #[test]
    fn scan_detects_wbo_soft_header() {
        let f = make_tmp("* #variable= 1\nsoft: 6 ;\n[2] +1 x1 >= 1 ;\n");
        let info = scan(f.path()).unwrap();
        assert!(info.is_wbo);
        // WBO has no `min:` objective; its objective is the soft-cost sum.
        assert!(!info.has_objective);
    }

    #[test]
    fn scan_detects_wbo_infinite_top() {
        // `soft: ;` (top cost omitted) is still a WBO instance.
        let f = make_tmp("soft: ;\n[1] +1 x1 >= 1 ;\n");
        let info = scan(f.path()).unwrap();
        assert!(info.is_wbo);
        assert!(!info.has_objective);
    }

    #[test]
    fn scan_opb_is_not_wbo() {
        let f = make_tmp("+1 x1 >= 1;\n+1 x2 >= 0;\n");
        let info = scan(f.path()).unwrap();
        assert!(!info.is_wbo);
        assert!(!info.has_objective);
    }

    #[test]
    fn scan_min_is_not_wbo() {
        let f = make_tmp("min: +1 x1;\n+1 x1 >= 1;\n");
        let info = scan(f.path()).unwrap();
        assert!(!info.is_wbo);
        assert!(info.has_objective);
    }

    #[test]
    fn scan_wbo_intsize_from_header() {
        // The PB header (carrying intsize=, derived from sumcost= for WBO)
        // precedes the `soft:` line, so intsize still parses.
        let f = make_tmp(
            "* #variable= 15 #constraint= 21 #equal= 0 intsize= 20 #soft= 5 mincost= 1 maxcost= 1 sumcost= 5\nsoft: 6 ;\n[1] +1 x1 >= 1 ;\n",
        );
        let info = scan(f.path()).unwrap();
        assert!(info.is_wbo);
        assert_eq!(info.intsize, Some(20));
    }
}
