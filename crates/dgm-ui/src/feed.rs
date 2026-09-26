//! Ledger ticker + review artifact parsing for the panels.
//!
//! Tolerant by design: the ledger is written by another process while we
//! read, so a torn or malformed line is skipped, never fatal.

use serde_json::Value;

/// One op ticker row: `r12  ui  extrude`.
#[derive(Debug, Clone, PartialEq)]
pub struct TickerLine {
    pub rev: u64,
    pub actor: String,
    pub op: String,
}

/// Parse the last `keep` well-formed ledger lines, oldest first.
pub fn parse_ticker_tail(text: &str, keep: usize) -> Vec<TickerLine> {
    let mut lines: Vec<TickerLine> = text
        .lines()
        .filter_map(|raw| {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            let v: Value = serde_json::from_str(raw).ok()?;
            Some(TickerLine {
                rev: v.get("rev")?.as_u64()?,
                actor: v.get("actor")?.as_str()?.to_string(),
                op: v.get("op")?.get("op")?.as_str()?.to_string(),
            })
        })
        .collect();
    if lines.len() > keep {
        lines.drain(..lines.len() - keep);
    }
    lines
}

/// One gate finding row for display.
#[derive(Debug, Clone, PartialEq)]
pub struct FindingView {
    pub severity: String,
    pub rule: String,
    pub message: String,
}

/// The digestible part of an `artifacts/r*-review.json`.
#[derive(Debug, Clone, Default)]
pub struct ReviewView {
    pub file: String,
    pub heads: Vec<(String, f32)>,
    pub findings: Vec<FindingView>,
    pub notes: Vec<String>,
}

/// `r<rev>-review.json` -> `rev`.
pub fn review_rev(name: &str) -> Option<u64> {
    name.strip_prefix('r')?.strip_suffix("-review.json")?.parse().ok()
}

/// Pick the newest review artifact by revision number.
pub fn newest_review_name(names: impl IntoIterator<Item = String>) -> Option<String> {
    names
        .into_iter()
        .filter_map(|n| review_rev(&n).map(|rev| (rev, n)))
        .max_by_key(|(rev, _)| *rev)
        .map(|(_, n)| n)
}

/// Extract heads, gate findings and notes from a review report JSON.
pub fn parse_review(file: &str, v: &Value) -> ReviewView {
    let mut heads: Vec<(String, f32)> = v
        .get("heads")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32)))
                .collect()
        })
        .unwrap_or_default();
    heads.sort_by(|a, b| a.0.cmp(&b.0));
    let findings = v
        .get("gate")
        .and_then(|g| g.get("findings"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|f| FindingView {
                    severity: f
                        .get("severity")
                        .and_then(Value::as_str)
                        .unwrap_or("warn")
                        .to_string(),
                    rule: f.get("rule").and_then(Value::as_str).unwrap_or("?").to_string(),
                    message: f.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let notes = v
        .get("notes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    ReviewView { file: file.to_string(), heads, findings, notes }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn ticker_tail_keeps_last_n_in_order() {
        let text = (1..=5)
            .map(|i| {
                format!(
                    r#"{{"rev":{i},"parent":{},"actor":"cli","time":"t","op":{{"op":"extrude","sel":{{}},"offset":0.1}}}}"#,
                    i - 1
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let tail = parse_ticker_tail(&text, 3);
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].rev, 3);
        assert_eq!(tail[2].rev, 5);
        assert_eq!(tail[0].op, "extrude");
        assert_eq!(tail[0].actor, "cli");
    }

    #[test]
    fn ticker_skips_torn_and_blank_lines() {
        let text = "\n{\"rev\":1,\"actor\":\"ui\",\"op\":{\"op\":\"prim_box\"}}\n{\"rev\":2,\"acto";
        let tail = parse_ticker_tail(text, 10);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].op, "prim_box");
        assert_eq!(tail[0].rev, 1);
    }

    #[test]
    fn newest_review_picks_numeric_max_not_lexicographic() {
        let names = ["r2-review.json", "r10-review.json", "r9-review.json", "junk.txt", "r3-sheet.png"]
            .into_iter()
            .map(String::from);
        assert_eq!(newest_review_name(names).unwrap(), "r10-review.json");
        assert_eq!(newest_review_name(Vec::<String>::new()), None);
    }

    #[test]
    fn review_parse_pulls_heads_findings_notes() {
        let v = json!({
            "gate": { "pass": false, "findings": [
                { "rule": "budget.tris", "severity": "hard", "message": "over budget" },
                { "rule": "uv.stretch", "severity": "warn", "message": "stretchy" },
            ]},
            "heads": { "silhouette": 0.8, "done": 0.25 },
            "notes": ["needs a handle"],
        });
        let view = parse_review("r4-review.json", &v);
        assert_eq!(view.heads, vec![("done".to_string(), 0.25), ("silhouette".to_string(), 0.8)]);
        assert_eq!(view.findings.len(), 2);
        assert_eq!(view.findings[0].severity, "hard");
        assert_eq!(view.findings[0].rule, "budget.tris");
        assert_eq!(view.notes, vec!["needs a handle"]);
        assert_eq!(view.file, "r4-review.json");
    }
}
