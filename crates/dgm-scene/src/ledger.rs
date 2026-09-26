//! The append-only op ledger: one JSON line per applied op.
//!
//! Laws: revisions strictly increase and are never reused; append never
//! rewrites an earlier line; every line's parent is the previous revision.
//! `time` and `actor` are provenance only — replay ignores them, so a
//! replayed doc (and its export) is byte-identical.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::op::Op;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerLine {
    pub rev: u64,
    pub parent: u64,
    pub actor: String,
    pub time: String,
    pub op: Op,
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("ledger {0}: {1}")]
    Io(std::path::PathBuf, std::io::Error),
    #[error("ledger {0} line {1}: {2}")]
    Parse(std::path::PathBuf, usize, serde_json::Error),
    #[error("ledger {0} line {1}: rev {2} does not follow parent {3}")]
    Order(std::path::PathBuf, usize, u64, u64),
}

pub fn append(path: &Path, line: &LedgerLine) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    let mut text = serde_json::to_string(line).expect("ledger lines always serialize");
    text.push('\n');
    file.write_all(text.as_bytes())
}

pub fn read(path: &Path) -> Result<Vec<LedgerLine>, LedgerError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(LedgerError::Io(path.to_path_buf(), e)),
    };
    let mut lines = Vec::new();
    let mut last_rev = 0u64;
    for (i, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let line: LedgerLine = serde_json::from_str(raw)
            .map_err(|e| LedgerError::Parse(path.to_path_buf(), i + 1, e))?;
        if line.parent != last_rev || line.rev != last_rev + 1 {
            return Err(LedgerError::Order(path.to_path_buf(), i + 1, line.rev, line.parent));
        }
        last_rev = line.rev;
        lines.push(line);
    }
    Ok(lines)
}
