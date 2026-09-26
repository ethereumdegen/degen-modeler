//! Review tier chain against wiremock endpoints. Env vars are process
//! globals, so every test takes ENV_LOCK and starts from a clean slate.

use std::sync::{Mutex, MutexGuard};

use dgm_jev::{GateReport, ReviewInputs, ReviewOptions, Tier, review};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static ENV_LOCK: Mutex<()> = Mutex::new(());

const ENV_KEYS: &[&str] = &[
    "TYPESAFE_API_KEY",
    "DGM_JEV_URL",
    "DGM_JEV_MODEL",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_BASE_URL",
    "DGM_VISION_MODEL",
];

/// Lock the env and clear every var the review stack reads.
fn clean_env() -> MutexGuard<'static, ()> {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for key in ENV_KEYS {
        unsafe { std::env::remove_var(key) };
    }
    guard
}

fn set(key: &str, value: &str) {
    unsafe { std::env::set_var(key, value) };
}

fn sample_metrics() -> Value {
    json!({
        "seam_contrast_max": 0.1, "seam_contrast_mean": 0.05,
        "uv_occupancy": 0.68, "stretch_max": 1.5, "stretch_mean": 1.1,
        "palette_distance": 0.2, "density_spread": 2.5, "mask_drift": 0.05,
        "per_object": {}
    })
}

fn sample_digest() -> Value {
    json!({"revision": 7, "budget": {"class": "prop", "max_tris": 800, "used_tris": 400}})
}

fn inputs<'a>() -> ReviewInputs<'a> {
    ReviewInputs {
        gate: GateReport::from_findings(vec![]),
        digest: sample_digest(),
        metrics: sample_metrics(),
        goal: Some("a wooden crate"),
        sheet_png: None,
        uv_png: None,
    }
}

fn skip_reason<'r>(report: &'r dgm_jev::ReviewReport, tier: &str) -> &'r str {
    &report.skipped.iter().find(|s| s.tier == tier).expect("skip entry").reason
}

#[tokio::test]
async fn jev_answers_override_formula_heads() {
    let _env = clean_env();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(header("authorization", "Bearer test-key"))
        .and(body_string_contains("\"model\":\"jev\""))
        .and(body_string_contains("questions"))
        .and(body_string_contains("a wooden crate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"answers": {
            "silhouette": 0.9,
            "style": {"value": 0.85},
            "seams": 0.7,
            "waste": 0.6,
            "done": 0.5,
            "bogus_head": 0.1,
        }})))
        .expect(1)
        .mount(&server)
        .await;
    set("TYPESAFE_API_KEY", "test-key");
    set("DGM_JEV_URL", &server.uri());

    let report = review(inputs(), &ReviewOptions { tier: Tier::Jev }).await.unwrap();

    assert_eq!(report.tiers_ran, ["metrics", "jev"]);
    assert!((report.heads["silhouette"] - 0.9).abs() < 1e-6);
    assert!((report.heads["style"] - 0.85).abs() < 1e-6);
    assert!((report.heads["seams"] - 0.7).abs() < 1e-6);
    assert!((report.heads["waste"] - 0.6).abs() < 1e-6);
    assert!((report.heads["done"] - 0.5).abs() < 1e-6);
    assert!(!report.heads.contains_key("bogus_head"));
    assert!(report.latency_ms.contains_key("jev"));
    assert!(report.latency_ms.contains_key("metrics"));
}

#[tokio::test]
async fn openai_vision_merges_notes_and_parses_cost() {
    let _env = clean_env();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer vision-key"))
        .and(body_string_contains("data:image/png;base64,"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content":
                "Looks solid. {\"heads\":{\"style\":0.7,\"done\":0.65},\"notes\":[\"clean silhouette\",\"seam on lid\"]} bye"
            }}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 50},
        })))
        .expect(1)
        .mount(&server)
        .await;
    set("OPENAI_API_KEY", "vision-key");
    set("OPENAI_BASE_URL", &server.uri());

    let png = b"\x89PNG fake bytes";
    let report = review(
        ReviewInputs { sheet_png: Some(png), ..inputs() },
        &ReviewOptions { tier: Tier::Vision },
    )
    .await
    .unwrap();

    assert_eq!(report.tiers_ran, ["metrics", "vision"]);
    assert!(skip_reason(&report, "jev").contains("TYPESAFE_API_KEY"));
    assert_eq!(report.notes, ["clean silhouette", "seam on lid"]);
    assert!((report.heads["style"] - 0.7).abs() < 1e-6);
    assert!((report.heads["done"] - 0.65).abs() < 1e-6);
    // 100 * $0.15/M + 50 * $0.60/M
    assert!((report.cost_usd.unwrap() - 0.000045).abs() < 1e-9);
    assert!(report.latency_ms.contains_key("vision"));
}

#[tokio::test]
async fn anthropic_vision_parses_content_blocks() {
    let _env = clean_env();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "anthro-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(body_string_contains("\"type\":\"base64\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "text", "text": "{\"heads\":{\"seams\":0.55},\"notes\":[\"visible seam\"]}"}],
            "usage": {"input_tokens": 1000, "output_tokens": 100},
        })))
        .expect(1)
        .mount(&server)
        .await;
    set("ANTHROPIC_API_KEY", "anthro-key");
    set("ANTHROPIC_BASE_URL", &server.uri());

    let png = b"\x89PNG fake bytes";
    let report = review(
        ReviewInputs { uv_png: Some(png), ..inputs() },
        &ReviewOptions { tier: Tier::Vision },
    )
    .await
    .unwrap();

    assert!(report.tiers_ran.contains(&"vision".to_string()));
    assert!((report.heads["seams"] - 0.55).abs() < 1e-6);
    assert_eq!(report.notes, ["visible seam"]);
    // 1000 * $3/M + 100 * $15/M
    assert!((report.cost_usd.unwrap() - 0.0045).abs() < 1e-9);
}

#[tokio::test]
async fn no_keys_degrades_to_skipped_entries_with_formula_heads() {
    let _env = clean_env();
    let png = b"\x89PNG fake bytes";
    let report = review(
        ReviewInputs { sheet_png: Some(png), ..inputs() },
        &ReviewOptions { tier: Tier::Vision },
    )
    .await
    .unwrap();

    assert_eq!(report.tiers_ran, ["metrics"]);
    assert!(skip_reason(&report, "jev").contains("TYPESAFE_API_KEY"));
    assert!(skip_reason(&report, "vision").contains("API_KEY"));
    assert_eq!(skip_reason(&report, "scorer"), "not built");
    // Formula heads survive untouched: seams = 1 - 0.1 * 2.
    assert!((report.heads["seams"] - 0.8).abs() < 1e-4);
    assert_eq!(report.cost_usd, None);
    assert!(report.notes.is_empty());
}

#[tokio::test]
async fn vision_without_image_and_metrics_tier_skip_cleanly() {
    let _env = clean_env();
    set("OPENAI_API_KEY", "unused");

    let report = review(inputs(), &ReviewOptions { tier: Tier::Vision }).await.unwrap();
    assert_eq!(skip_reason(&report, "vision"), "no image provided");

    let report = review(inputs(), &ReviewOptions { tier: Tier::Metrics }).await.unwrap();
    assert_eq!(report.tiers_ran, ["metrics"]);
    assert!(!report.skipped.iter().any(|s| s.tier == "jev" || s.tier == "vision"));
}

#[tokio::test]
async fn jev_bad_reply_and_http_error_degrade() {
    let _env = clean_env();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"nope": true})))
        .expect(1)
        .mount(&server)
        .await;
    set("TYPESAFE_API_KEY", "k");
    set("DGM_JEV_URL", &server.uri());

    let report = review(inputs(), &ReviewOptions { tier: Tier::Jev }).await.unwrap();
    assert_eq!(report.tiers_ran, ["metrics"]);
    assert!(skip_reason(&report, "jev").contains("answers"));
    assert!((report.heads["seams"] - 0.8).abs() < 1e-4); // formula head kept
    server.reset().await;

    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let report = review(inputs(), &ReviewOptions { tier: Tier::Jev }).await.unwrap();
    assert!(skip_reason(&report, "jev").contains("500"));
}

#[tokio::test]
async fn malformed_inputs_are_the_only_error() {
    let _env = clean_env();
    let bad = ReviewInputs { metrics: json!("not an object"), ..inputs() };
    let err = review(bad, &ReviewOptions { tier: Tier::Metrics }).await.unwrap_err();
    assert!(err.to_string().contains("metrics"));

    let bad = ReviewInputs { digest: json!([1, 2]), ..inputs() };
    let err = review(bad, &ReviewOptions { tier: Tier::Metrics }).await.unwrap_err();
    assert!(err.to_string().contains("digest"));
}
