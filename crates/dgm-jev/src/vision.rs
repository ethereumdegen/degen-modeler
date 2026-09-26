//! Tier 3: one round against a vision model — OpenAI chat completions
//! (image_url data URLs) or the Anthropic messages API (base64 image
//! blocks) — with strict-JSON `{heads, notes}` extraction from wherever it
//! lands in the reply prose. `Err(String)` is a skip reason, never a
//! review failure. Keys come from env only and go into headers only.

use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(30);
const OPENAI_DEFAULT_BASE: &str = "https://api.openai.com";
const OPENAI_DEFAULT_MODEL: &str = "gpt-4o-mini";
const ANTHROPIC_DEFAULT_BASE: &str = "https://api.anthropic.com";
const ANTHROPIC_DEFAULT_MODEL: &str = "claude-sonnet-4-5";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Cost estimate: USD per million tokens for the *default* models
/// (gpt-4o-mini, claude-sonnet). An order-of-magnitude signal, not billing
/// truth — overriding `DGM_VISION_MODEL` does not reprice.
const OPENAI_USD_PER_MTOK_IN: f64 = 0.15;
const OPENAI_USD_PER_MTOK_OUT: f64 = 0.60;
const ANTHROPIC_USD_PER_MTOK_IN: f64 = 3.0;
const ANTHROPIC_USD_PER_MTOK_OUT: f64 = 15.0;

pub(crate) enum Provider {
    OpenAi { base: String, key: String },
    Anthropic { base: String, key: String },
}

/// OpenAI wins when both keys are set. Bases come from
/// `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL` (defaults above).
pub(crate) fn provider_from_env() -> Option<Provider> {
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        let base = std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| OPENAI_DEFAULT_BASE.into());
        return Some(Provider::OpenAi { base, key });
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        let base =
            std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| ANTHROPIC_DEFAULT_BASE.into());
        return Some(Provider::Anthropic { base, key });
    }
    None
}

pub(crate) struct VisionOutcome {
    pub heads: BTreeMap<String, f32>,
    pub notes: Vec<String>,
    pub cost_usd: Option<f64>,
}

fn prompt(goal: Option<&str>) -> String {
    format!(
        "You are reviewing renders of a low-poly game asset.\n\
         Goal: {}\n\
         Score each head 0-1 (1 = good): silhouette (shape readability), \
         style (palette and texel fit), seams (UV seam visibility, 1 = invisible), \
         waste (texture/budget efficiency), done (shippable for the goal).\n\
         Reply with strict JSON only: \
         {{\"heads\":{{\"silhouette\":0.0,\"style\":0.0,\"seams\":0.0,\"waste\":0.0,\"done\":0.0}},\
         \"notes\":[\"short observations\"]}}",
        goal.unwrap_or("(none stated)")
    )
}

pub(crate) async fn ask(
    provider: &Provider,
    goal: Option<&str>,
    sheet_png: Option<&[u8]>,
    uv_png: Option<&[u8]>,
) -> Result<VisionOutcome, String> {
    let images: Vec<&[u8]> = sheet_png.into_iter().chain(uv_png).collect();
    let model_env = std::env::var("DGM_VISION_MODEL").ok();
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| format!("vision client: {e}"))?;

    let (text, cost_usd) = match provider {
        Provider::OpenAi { base, key } => {
            let model = model_env.unwrap_or_else(|| OPENAI_DEFAULT_MODEL.into());
            let mut content = vec![json!({"type": "text", "text": prompt(goal)})];
            for img in &images {
                content.push(json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:image/png;base64,{}", B64.encode(img))},
                }));
            }
            let body = json!({"model": model, "messages": [{"role": "user", "content": content}]});
            let reply = post_json(
                client.post(format!("{}/v1/chat/completions", base.trim_end_matches('/')))
                    .bearer_auth(key),
                &body,
            )
            .await?;
            let text = reply
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .ok_or_else(|| "vision reply: no message content".to_string())?
                .to_string();
            let cost = cost_from_usage(
                reply.get("usage"),
                "prompt_tokens",
                "completion_tokens",
                OPENAI_USD_PER_MTOK_IN,
                OPENAI_USD_PER_MTOK_OUT,
            );
            (text, cost)
        }
        Provider::Anthropic { base, key } => {
            let model = model_env.unwrap_or_else(|| ANTHROPIC_DEFAULT_MODEL.into());
            let mut content = Vec::new();
            for img in &images {
                content.push(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": B64.encode(img),
                    },
                }));
            }
            content.push(json!({"type": "text", "text": prompt(goal)}));
            let body = json!({
                "model": model,
                "max_tokens": 1024,
                "messages": [{"role": "user", "content": content}],
            });
            let reply = post_json(
                client.post(format!("{}/v1/messages", base.trim_end_matches('/')))
                    .header("x-api-key", key)
                    .header("anthropic-version", ANTHROPIC_VERSION),
                &body,
            )
            .await?;
            let text = reply
                .get("content")
                .and_then(Value::as_array)
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|t| !t.is_empty())
                .ok_or_else(|| "vision reply: no content text".to_string())?;
            let cost = cost_from_usage(
                reply.get("usage"),
                "input_tokens",
                "output_tokens",
                ANTHROPIC_USD_PER_MTOK_IN,
                ANTHROPIC_USD_PER_MTOK_OUT,
            );
            (text, cost)
        }
    };

    let parsed = extract_review_json(&text)
        .ok_or_else(|| "vision reply: no {heads, notes} JSON found".to_string())?;
    let mut heads = BTreeMap::new();
    if let Some(hs) = parsed.get("heads").and_then(Value::as_object) {
        for (head, raw) in hs {
            let n = raw.as_f64().or_else(|| raw.get("value").and_then(Value::as_f64));
            if let Some(n) = n {
                heads.insert(head.clone(), n as f32);
            }
        }
    }
    let notes = parsed
        .get("notes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    Ok(VisionOutcome { heads, notes, cost_usd })
}

async fn post_json(req: reqwest::RequestBuilder, body: &Value) -> Result<Value, String> {
    let resp = req.json(body).send().await.map_err(|e| {
        if e.is_timeout() {
            "vision: timed out".to_string()
        } else {
            format!("vision request: {e}")
        }
    })?;
    if !resp.status().is_success() {
        return Err(format!("vision http {}", resp.status().as_u16()));
    }
    resp.json().await.map_err(|e| format!("vision reply: {e}"))
}

fn cost_from_usage(
    usage: Option<&Value>,
    in_key: &str,
    out_key: &str,
    usd_in: f64,
    usd_out: f64,
) -> Option<f64> {
    let usage = usage?;
    let input = usage.get(in_key).and_then(Value::as_f64)?;
    let output = usage.get(out_key).and_then(Value::as_f64).unwrap_or(0.0);
    Some((input * usd_in + output * usd_out) / 1_000_000.0)
}

/// Find the first balanced `{...}` in `text` that parses as a JSON object
/// with a `heads` or `notes` key. Brace matching skips string contents;
/// `{` / `}` are ASCII so byte slicing stays on char boundaries.
pub(crate) fn extract_review_json(text: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    for start in 0..bytes.len() {
        if bytes[start] != b'{' {
            continue;
        }
        let (mut depth, mut in_str, mut esc) = (0usize, false, false);
        for (i, &b) in bytes.iter().enumerate().skip(start) {
            if in_str {
                if esc {
                    esc = false;
                } else if b == b'\\' {
                    esc = true;
                } else if b == b'"' {
                    in_str = false;
                }
                continue;
            }
            match b {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        if let Ok(v) = serde_json::from_str::<Value>(&text[start..=i])
                            && v.is_object()
                            && (v.get("heads").is_some() || v.get("notes").is_some())
                        {
                            return Some(v);
                        }
                        break; // balanced but not ours; try the next '{'
                    }
                }
                _ => {}
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_prose() {
        let text = "Sure! Here is my review:\n```json\n\
                    {\"heads\":{\"style\":0.7},\"notes\":[\"boxy {but} nice\"]}\n```\nDone.";
        let v = extract_review_json(text).unwrap();
        assert_eq!(v["heads"]["style"], 0.7);
        assert_eq!(v["notes"][0], "boxy {but} nice");
    }

    #[test]
    fn skips_unrelated_objects_and_handles_none() {
        let text = "config {\"a\":1} then {\"notes\":[]} trailing";
        assert_eq!(extract_review_json(text).unwrap(), serde_json::json!({"notes": []}));
        assert!(extract_review_json("no json here {broken").is_none());
    }
}
