//! Tier 2: one question round against Jev (TypeSafe `systemone`).
//! `Err(String)` is always a human-readable skip reason — the caller
//! degrades, never fails.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};

const DEFAULT_URL: &str = "https://api.typesafe.ai";
const DEFAULT_MODEL: &str = "jev";
const TIMEOUT: Duration = Duration::from_secs(5);

/// One POST `{DGM_JEV_URL|https://api.typesafe.ai}/v1/systemone` with bearer
/// `TYPESAFE_API_KEY` (header only), model `DGM_JEV_MODEL|"jev"`, and body
/// `{model, state, questions}`. `state` is text: goal + digest + metrics.
pub(crate) async fn ask(
    api_key: &str,
    goal: Option<&str>,
    digest: &Value,
    metrics: &Value,
) -> Result<BTreeMap<String, f32>, String> {
    let base = std::env::var("DGM_JEV_URL").unwrap_or_else(|_| DEFAULT_URL.into());
    let model = std::env::var("DGM_JEV_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());

    let mut state = String::new();
    if let Some(goal) = goal {
        state.push_str("goal: ");
        state.push_str(goal);
        state.push('\n');
    }
    state.push_str(&format!("scene digest: {digest}\nrender metrics: {metrics}\n"));

    let body = json!({
        "model": model,
        "state": state,
        "questions": {
            "silhouette": "Rate 0-1: does the model read cleanly in silhouette at game distance?",
            "style": "Rate 0-1: does it match the pack palette and texel-density style?",
            "seams": "Rate 0-1: are UV seams invisible (1.0) or glaring (0.0)?",
            "waste": "Rate 0-1: are texture space and the triangle budget used efficiently?",
            "done": "Rate 0-1: how close is the asset to shippable for the stated goal?",
        },
    });

    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| format!("jev client: {e}"))?;
    let resp = client
        .post(format!("{}/v1/systemone", base.trim_end_matches('/')))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "jev: timed out after 5s".to_string()
            } else {
                format!("jev request: {e}")
            }
        })?;
    if !resp.status().is_success() {
        return Err(format!("jev http {}", resp.status().as_u16()));
    }
    let reply: Value = resp.json().await.map_err(|e| format!("jev reply: {e}"))?;
    parse_answers(&reply).ok_or_else(|| "jev reply: no usable `answers` object".to_string())
}

/// Accepts `{answers: {head: 0.7}}` and `{answers: {head: {value: 0.7}}}`.
fn parse_answers(reply: &Value) -> Option<BTreeMap<String, f32>> {
    let answers = reply.get("answers")?.as_object()?;
    let mut out = BTreeMap::new();
    for (head, raw) in answers {
        let n = raw.as_f64().or_else(|| raw.get("value").and_then(Value::as_f64));
        if let Some(n) = n {
            out.insert(head.clone(), n as f32);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn answers_accept_numbers_and_value_objects() {
        let reply = json!({"answers": {
            "seams": 0.7,
            "style": {"value": 0.85},
            "junk": "not a number",
        }});
        let parsed = parse_answers(&reply).unwrap();
        assert_eq!(parsed.len(), 2);
        assert!((parsed["seams"] - 0.7).abs() < 1e-6);
        assert!((parsed["style"] - 0.85).abs() < 1e-6);
    }

    #[test]
    fn missing_answers_is_none() {
        assert!(parse_answers(&json!({"nope": 1})).is_none());
        assert!(parse_answers(&json!({"answers": {"a": "x"}})).is_none());
    }
}
