use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub struct OpbInfo {
    pub has_objective: bool,
}

pub fn scan<P: AsRef<Path>>(path: P) -> io::Result<OpbInfo> {
    let f = File::open(path)?;
    let r = BufReader::new(f);
    for line in r.lines() {
        let line = line?;
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('*') {
            continue;
        }
        if trimmed.starts_with("min:") || trimmed.starts_with("min ") {
            return Ok(OpbInfo {
                has_objective: true,
            });
        }
    }
    Ok(OpbInfo {
        has_objective: false,
    })
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
}
