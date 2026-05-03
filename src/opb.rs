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
