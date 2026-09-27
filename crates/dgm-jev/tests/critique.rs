//! Critique tool against a wiremock OpenAI endpoint: references ride along
//! as extra images, the environment prompt asks the level-art questions,
//! and `reference_match` is parsed only when references were attached.
//! Env vars are process globals, so every test takes ENV_LOCK.

use std::sync::{Mutex, MutexGuard};

use base64::Engine as _;
use dgm_jev::{CritiqueInputs, critique};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn clean_env() -> MutexGuard<'static, ()> {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for key in ["OPENAI_API_KEY", "OPENAI_BASE_URL", "ANTHROPIC_API_KEY", "DGM_VISION_MODEL"] {
        unsafe { std::env::remove_var(key) };
    }
    guard
}

const SHEET: &[u8] = b"\x89PNG sheet bytes";
const REF_JPG: &[u8] = b"\xff\xd8\xff\xe0 jpeg reference";
const REF_PNG: &[u8] = b"\x89PNG reference two";

async fn mock_openai(reply: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content": reply}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5},
        })))
        .expect(1)
        .mount(&server)
        .await;
    unsafe {
        std::env::set_var("OPENAI_API_KEY", "critique-key");
        std::env::set_var("OPENAI_BASE_URL", server.uri());
    }
    server
}

/// The single user message's content parts: (prompt text, image data URLs).
async fn sent_content(server: &MockServer) -> (String, Vec<String>) {
    let reqs = server.received_requests().await.expect("recording on");
    assert_eq!(reqs.len(), 1);
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    let parts = body["messages"][0]["content"].as_array().unwrap();
    let text = parts
        .iter()
        .filter(|p| p["type"] == "text")
        .map(|p| p["text"].as_str().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let urls = parts
        .iter()
        .filter(|p| p["type"] == "image_url")
        .map(|p| p["image_url"]["url"].as_str().unwrap().to_string())
        .collect();
    (text, urls)
}

fn inputs<'a>(digest: &'a Value, references: Vec<(String, Vec<u8>)>, environment: bool) -> CritiqueInputs<'a> {
    CritiqueInputs {
        goal: Some("a stylized cave"),
        digest,
        metrics: digest,
        images: vec![("turntable contact sheet (8 views)".into(), SHEET.to_vec())],
        references,
        palette: &[],
        budget: Some((900, 12000)),
        environment,
    }
}

#[tokio::test]
async fn references_attach_as_extra_images_and_env_prompt_asks_continuity() {
    let _env = clean_env();
    let server = mock_openai(
        "Sure. {\"verdict\":\"reads as one space\",\"style_fit\":0.6,\"reference_match\":0.45,\
         \"strengths\":[],\"issues\":[],\"suggestions\":[]}",
    )
    .await;
    let digest = json!({"revision": 3});
    let refs = vec![
        ("refs/cave.jpg".to_string(), REF_JPG.to_vec()),
        ("refs/moss.png".to_string(), REF_PNG.to_vec()),
    ];
    let report = critique(inputs(&digest, refs, true)).await.unwrap();

    let (text, urls) = sent_content(&server).await;
    // Sheet first, then both references — in order, with sniffed MIME types.
    let b64 = base64::engine::general_purpose::STANDARD;
    assert_eq!(
        urls,
        [
            format!("data:image/png;base64,{}", b64.encode(SHEET)),
            format!("data:image/jpeg;base64,{}", b64.encode(REF_JPG)),
            format!("data:image/png;base64,{}", b64.encode(REF_PNG)),
        ]
    );
    assert!(text.contains("image 1: turntable contact sheet (8 views)"));
    assert!(text.contains("image 2: reference image 1 (style target) — refs/cave.jpg"));
    assert!(text.contains("image 3: reference image 2 (style target) — refs/moss.png"));
    assert!(text.contains("The last 2 image(s) are reference images"));
    assert!(text.contains("\"reference_match\": 0.0"));
    for topic in ["continuity", "scale", "lighting", "focal points"] {
        assert!(text.contains(topic), "environment prompt lacks `{topic}`");
    }
    assert!(text.contains("prim_cavern") && text.contains("bake_ao"), "vocab lacks the new ops");

    assert_eq!(report.verdict, "reads as one space");
    assert!((report.style_fit - 0.6).abs() < 1e-6);
    assert!((report.reference_match.unwrap() - 0.45).abs() < 1e-6);
    let json = serde_json::to_value(&report).unwrap();
    // f32 serializes as 0.44999998…; compare numerically, not as JSON.
    assert!((json["reference_match"].as_f64().unwrap() - 0.45).abs() < 1e-6);
    assert!(json.get("notes").is_none(), "empty notes are not serialized");
}

#[tokio::test]
async fn prop_without_references_has_no_reference_head() {
    let _env = clean_env();
    let server = mock_openai("{\"verdict\":\"fine crate\",\"style_fit\":0.8,\"reference_match\":0.9}").await;
    let digest = json!({"revision": 1});
    let report = critique(inputs(&digest, vec![], false)).await.unwrap();

    let (text, urls) = sent_content(&server).await;
    assert_eq!(urls.len(), 1);
    assert!(text.contains("portfolio review"));
    assert!(!text.contains("reference image"));
    assert!(!text.contains("continuity"));
    assert!(!text.contains("reference_match"));
    // A stray reference_match in the reply is ignored without references.
    assert_eq!(report.reference_match, None);
    let json = serde_json::to_value(&report).unwrap();
    assert!(json.get("reference_match").is_none());
}

#[tokio::test]
async fn missing_reference_score_defaults_to_zero_when_references_were_sent() {
    let _env = clean_env();
    let _server = mock_openai("{\"verdict\":\"no score given\",\"style_fit\":1.4}").await;
    let digest = json!({"revision": 1});
    let refs = vec![("refs/a.png".to_string(), REF_PNG.to_vec())];
    let report = critique(inputs(&digest, refs, true)).await.unwrap();
    assert_eq!(report.reference_match, Some(0.0));
    assert_eq!(report.style_fit, 1.0, "style_fit is clamped");
}
