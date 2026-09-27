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
    /// 0..1 — how close the renders sit to the attached reference images;
    /// only present when references were attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_match: Option<f32>,
    #[serde(default)]
    pub strengths: Vec<String>,
    #[serde(default)]
    pub issues: Vec<Issue>,
    #[serde(default)]
    pub suggestions: Vec<Suggestion>,
    /// Orchestration notes (e.g. a reference file that could not be read);
    /// never model output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
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
    /// (label, png/jpg bytes) — the project's reference images (style
    /// targets). Attached after `images`; when non-empty the reply carries
    /// a `reference_match` head.
    pub references: Vec<(String, Vec<u8>)>,
    pub palette: &'a [String],
    /// (used_tris, max_tris) when a class budget applies.
    pub budget: Option<(u32, u32)>,
    /// Walkable space (building/environment): the prompt asks about
    /// continuity, scale, lighting and focal points instead of a prop's
    /// silhouette read.
    pub environment: bool,
}

const OP_VOCAB: &str = "extrude, inset, bevel, loop_cut, bridge, merge_verts, dissolve, mirror, \
decimate_to_budget, lattice, translate/rotate/scale (proportional), snap_to_grid, mark_seams, \
uv_unwrap, uv_project, uv_assign_trim, uv_assign_rect, uv_set_texel_density, uv_pack, \
uv_declare_mirror, material_new (emissive), rig_apply, rig_auto_weights, rig_paint_weights, \
clip_apply, clip_key, lod_generate, prim_tunnel, prim_cavern, subdivide, displace_noise, smooth, \
solidify, join, snap_to_surface, tag_object, bake_ao, bake_sun, bake_glow, paint_vertex, \
clear_vertex_colors, plus repainting the texture atlas in degen-paint";

fn prompt(inputs: &CritiqueInputs<'_>) -> String {
    let n_images = inputs.images.len();
    let image_list = inputs
        .images
        .iter()
        .enumerate()
        .map(|(i, (label, _))| format!("image {}: {label}", i + 1))
        .chain(inputs.references.iter().enumerate().map(|(i, (label, _))| {
            format!("image {}: reference image {} (style target) — {label}", n_images + i + 1, i + 1)
        }))
        .collect::<Vec<_>>()
        .join("\n");
    let budget = inputs
        .budget
        .map(|(used, max)| format!("{used} of {max} triangles used"))
        .unwrap_or_else(|| "no explicit budget".into());
    let subject = if inputs.environment {
        "Critique this environment (a walkable space: cave, room, ruin) like a level-art \
         review: specific, actionable, no flattery. Judge it as ONE continuous space, \
         not a pile of props. Answer explicitly on: continuity (do shell, floor, passages \
         and formations read as one connected surface, or do seams, gaps, floating and \
         intersecting pieces break it?), scale (does a player-height figure fit the \
         openings, ceiling and formations believably?), lighting (baked light and shadow \
         range, crevice darkness, where glow comes from; is anything flat/unlit?), and \
         focal points (where does the eye go, is there a hero element and a path through). \
         The interior views are the ones that matter; the exterior turntable only checks \
         the shell."
    } else {
        "Critique this asset like a portfolio review: specific, actionable, no flattery."
    };
    let reference_rules = if inputs.references.is_empty() {
        String::new()
    } else {
        format!(
            "The last {} image(s) are reference images (style targets), NOT renders of \
             this asset. Compare the renders against them — silhouette language, colour \
             and value structure, lighting mood, level of detail — and score \
             `reference_match` 0..1 for how close the asset sits to them.\n",
            inputs.references.len()
        )
    };
    let reference_field = if inputs.references.is_empty() { "" } else { "\n  \"reference_match\": 0.0," };
    format!(
        "You are a senior art director for a stylized low-poly indie game \
         (classic-MMO look: strong silhouettes, hand-painted textures, unlit, texel economy).\n\
         {subject}\n\
         \n\
         Goal: {goal}\n\
         Budget: {budget}\n\
         Style palette (hex): {palette}\n\
         Attached, in order:\n{image_list}\n\
         {reference_rules}\
         Scene digest JSON: {digest}\n\
         Measured metrics JSON: {metrics}\n\
         \n\
         The modeling tool's op vocabulary (use these names in `ops`): {vocab}\n\
         \n\
         Reply with strict JSON only:\n\
         {{\"verdict\": \"2-3 sentence overall judgment\",\n\
           \"style_fit\": 0.0,{reference_field}\n\
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
    let images: Vec<&[u8]> = inputs
        .images
        .iter()
        .chain(&inputs.references)
        .map(|(_, b)| b.as_slice())
        .collect();
    let (text, model, cost_usd) = vision::chat(&provider, &prompt(&inputs), &images, 2048)
        .await
        .map_err(CritiqueError::Call)?;
    let parsed = vision::extract_json_object(&text, &["verdict"])
        .ok_or_else(|| CritiqueError::Parse(text.clone()))?;

    let reference_match = (!inputs.references.is_empty()).then(|| {
        parsed
            .get("reference_match")
            .and_then(Value::as_f64)
            .map(|v| (v as f32).clamp(0.0, 1.0))
            .unwrap_or(0.0)
    });
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
    report.reference_match = reference_match;
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
