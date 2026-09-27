//! `critique` — the art-direction judgment tool.
//!
//! Unlike the review tiers (advisory, degrade silently), a critique is an
//! explicit paid judgment: it REQUIRES a configured vision provider
//! (`OPENAI_API_KEY` preferred, `ANTHROPIC_API_KEY` accepted) and fails
//! loudly without one. Renders + digest + metrics go in; a structured
//! verdict comes back whose suggestions speak the op vocabulary, so an
//! agent can act on them directly.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::vision::{self, Provider};

#[derive(Debug, thiserror::Error)]
pub enum CritiqueError {
    #[error(
        "no vision provider configured: set OPENAI_API_KEY (preferred) or ANTHROPIC_API_KEY"
    )]
    NoProvider,
    #[error("critique needs at least one render image")]
    NoImages,
    #[error("vision call: {0}")]
    Call(String),
    #[error("critique reply had no parseable verdict JSON; raw reply:\n{0}")]
    Parse(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub what: String,
    #[serde(default, rename = "where")]
    pub where_: String,
    #[serde(default)]
    pub severity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub action: String,
    /// Op names from the dgm vocabulary that would implement the fix.
    #[serde(default)]
    pub ops: Vec<String>,
    #[serde(default)]
    pub impact: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CritiqueReport {
    pub verdict: String,
    /// 0..1 — how well this reads as the stated goal in the stated style.
    pub style_fit: f32,
    #[serde(default)]
    pub strengths: Vec<String>,
    #[serde(default)]
    pub issues: Vec<Issue>,
    #[serde(default)]
    pub suggestions: Vec<Suggestion>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub latency_ms: u64,
}

pub struct CritiqueInputs<'a> {
    pub goal: Option<&'a str>,
    pub digest: &'a Value,
    pub metrics: &'a Value,
    /// (label, png bytes) — contact sheet, wireframe, the texture atlas…
    pub images: Vec<(String, Vec<u8>)>,
    pub palette: &'a [String],
    /// (used_tris, max_tris) when a class budget applies.
    pub budget: Option<(u32, u32)>,
}

const OP_VOCAB: &str = "extrude, inset, bevel, loop_cut, bridge, merge_verts, dissolve, mirror, \
decimate_to_budget, lattice, translate/rotate/scale (proportional), snap_to_grid, mark_seams, \
uv_unwrap, uv_project, uv_assign_trim, uv_assign_rect, uv_set_texel_density, uv_pack, \
uv_declare_mirror, material_new, rig_apply, rig_auto_weights, rig_paint_weights, clip_apply, \
clip_key, lod_generate, plus repainting the texture atlas in degen-paint";

fn prompt(inputs: &CritiqueInputs<'_>) -> String {
    let image_list = inputs
        .images
        .iter()
        .enumerate()
        .map(|(i, (label, _))| format!("image {}: {label}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    let budget = inputs
        .budget
        .map(|(used, max)| format!("{used} of {max} triangles used"))
        .unwrap_or_else(|| "no explicit budget".into());
    format!(
        "You are a senior art director for a stylized low-poly indie game \
         (classic-MMO look: strong silhouettes, hand-painted textures, unlit, texel economy).\n\
         Critique this asset like a portfolio review: specific, actionable, no flattery.\n\
         \n\
         Goal: {goal}\n\
         Budget: {budget}\n\
         Style palette (hex): {palette}\n\
         Attached, in order:\n{image_list}\n\
         Scene digest JSON: {digest}\n\
         Measured metrics JSON: {metrics}\n\
         \n\
         The modeling tool's op vocabulary (use these names in `ops`): {vocab}\n\
         \n\
         Reply with strict JSON only:\n\
         {{\"verdict\": \"2-3 sentence overall judgment\",\n\
           \"style_fit\": 0.0,\n\
           \"strengths\": [\"...\"],\n\
           \"issues\": [{{\"what\": \"...\", \"where\": \"object/face/texture region\", \
         \"severity\": \"low|medium|high\"}}],\n\
           \"suggestions\": [{{\"action\": \"concrete change\", \"ops\": [\"op_name\"], \
         \"impact\": \"why it helps\"}}]}}\n\
         Order suggestions by impact; 3-6 of them; each must be executable with the vocabulary.",
        goal = inputs.goal.unwrap_or("(none stated)"),
        palette = inputs.palette.join(", "),
        digest = inputs.digest,
        metrics = inputs.metrics,
        vocab = OP_VOCAB,
    )
}

pub async fn critique(inputs: CritiqueInputs<'_>) -> Result<CritiqueReport, CritiqueError> {
    let provider = vision::provider_from_env().ok_or(CritiqueError::NoProvider)?;
    if inputs.images.is_empty() {
        return Err(CritiqueError::NoImages);
    }
    let started = std::time::Instant::now();
    let images: Vec<&[u8]> = inputs.images.iter().map(|(_, b)| b.as_slice()).collect();
    let (text, model, cost_usd) = vision::chat(&provider, &prompt(&inputs), &images, 2048)
        .await
        .map_err(CritiqueError::Call)?;
    let parsed = vision::extract_json_object(&text, &["verdict"])
        .ok_or_else(|| CritiqueError::Parse(text.clone()))?;

    let mut report: CritiqueReport = serde_json::from_value(serde_json::json!({
        "verdict": parsed.get("verdict").and_then(Value::as_str).unwrap_or("(no verdict)"),
        "style_fit": parsed.get("style_fit").and_then(Value::as_f64).unwrap_or(0.0),
        "strengths": parsed.get("strengths").cloned().unwrap_or(Value::Array(vec![])),
        "issues": parsed.get("issues").cloned().unwrap_or(Value::Array(vec![])),
        "suggestions": parsed.get("suggestions").cloned().unwrap_or(Value::Array(vec![])),
        "model": model,
        "latency_ms": 0,
    }))
    .map_err(|e| CritiqueError::Parse(format!("{e}; in reply:\n{text}")))?;
    report.style_fit = report.style_fit.clamp(0.0, 1.0);
    report.cost_usd = cost_usd;
    report.latency_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

/// The provider the tool would use, for `doctor`-style reporting.
pub fn provider_name() -> Option<&'static str> {
    match vision::provider_from_env()? {
        Provider::OpenAi { .. } => Some("openai"),
        Provider::Anthropic { .. } => Some("anthropic"),
    }
}
