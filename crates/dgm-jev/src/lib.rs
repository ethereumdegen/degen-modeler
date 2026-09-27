//! The review stack: deterministic gate composition plus the tiered
//! reviewer. Owned by the JevReview slice; see CONTRACT.md §JevReview.
//!
//! Tier chain: formula heads from metrics always run; Jev (TypeSafe) and
//! vision tiers each degrade into a [`SkippedTier`] entry instead of
//! erroring. The only hard error is genuinely malformed [`ReviewInputs`].
//! The local CLIP scorer is post-v0 and always reports `scorer: not built`.

pub mod critique;
mod heads;
mod jev;
pub mod scene_rules;
mod vision;

pub use critique::{CritiqueError, CritiqueInputs, CritiqueReport, critique};
pub use scene_rules::scene_findings;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;
use std::path::Path;
use std::time::Instant;

use dgm_atlas::Pack;
use dgm_mesh::{FaceId, Finding, Mesh};
use dgm_scene::{Doc, Object, TextureRef};
pub use dgm_scene::GateReport;
use dgm_uv::TexInfo;
pub use heads::formula_heads;
use serde::Serialize;
use serde_json::Value;

/// The five review heads, 0..1 with 1 = good.
pub const HEADS: [&str; 5] = ["done", "seams", "silhouette", "style", "waste"];

/// Compose every deterministic gate rule family into one report:
/// mesh (per object, attributed), budget, uv, scene, rig, anim.
pub fn gate(doc: &Doc, pack: &Pack) -> GateReport {
    let mut findings: Vec<Finding> = Vec::new();
    for (name, object) in &doc.objects {
        findings.extend(
            dgm_mesh::validate_mesh(&object.mesh)
                .into_iter()
                .map(|f| Finding { message: format!("object `{name}`: {}", f.message), ..f }),
        );
    }
    findings.extend(dgm_scene::budget_findings(doc, pack));

    // dgm-uv sits below dgm-scene, so adapt the Doc to its plain inputs:
    // per-object mesh + TexInfo, and the union of declared mirror faces.
    let mut meshes: BTreeMap<String, (&Mesh, Option<TexInfo>)> = BTreeMap::new();
    for (name, o) in &doc.objects {
        if let Some(tex) = tex_info(doc, o, pack, name, &mut findings) {
            meshes.insert(name.clone(), (&o.mesh, tex));
        }
    }
    let mut mirror_faces: BTreeMap<String, BTreeSet<FaceId>> = BTreeMap::new();
    for m in doc.mirror_sets.values() {
        mirror_faces.entry(m.object.clone()).or_default().extend(m.faces.iter().copied());
    }
    findings.extend(dgm_uv::uv_findings(
        &meshes,
        &mirror_faces,
        pack.manifest.texel_density,
        pack.manifest.uv_waste_max,
    ));
    findings.extend(scene_rules::scene_findings(doc, pack));
    findings.extend(dgm_scene::rig::rig_findings(doc));
    findings.extend(dgm_scene::anim::clip_findings(doc));
    GateReport::from_findings(findings)
}

/// Adapter table from the dgm-uv crate docs: `Trim { sheet }` -> max sheet
/// edge, trim; `File { path }` -> PNG edge, owned; `Color`/no material ->
/// `Some(None)` (texture-size rules skip). Outer `None` excludes the object
/// from uv rules entirely: a Trim material whose sheet is missing from the
/// pack would otherwise draw false `uv.overlap` Hards on intentionally
/// stacked islands — that inconsistency is surfaced as its own Hard
/// `uv.missing_sheet` finding instead. An unreadable `File` texture stays
/// included (overlap on an owned atlas is still wrong) but gets a Warn.
fn tex_info(
    doc: &Doc,
    object: &Object,
    pack: &Pack,
    name: &str,
    findings: &mut Vec<Finding>,
) -> Option<Option<TexInfo>> {
    let Some(mat_name) = object.material.as_ref() else { return Some(None) };
    let Some(material) = doc.materials.get(mat_name) else { return Some(None) };
    match &material.texture {
        TextureRef::Trim { sheet } => match pack.manifest.trims.get(sheet) {
            Some(t) => Some(Some(TexInfo { px: t.size[0].max(t.size[1]), trim: true })),
            None => {
                findings.push(Finding::hard(
                    "uv.missing_sheet",
                    format!(
                        "object `{name}`: material `{mat_name}` references unknown trim sheet `{sheet}`"
                    ),
                ));
                None
            }
        },
        TextureRef::File { path } => match png_edge(&file_path(pack, path)) {
            Some(px) => Some(Some(TexInfo { px, trim: false })),
            None => {
                findings.push(Finding::warn(
                    "uv.missing_texture",
                    format!(
                        "object `{name}`: texture file `{path}` unreadable; texel-size checks skipped"
                    ),
                ));
                Some(None)
            }
        },
        TextureRef::Color { .. } => Some(None),
    }
}

/// `File` texture paths are project-relative; the project's pack copy lives
/// at `<project>/pack`, so the pack's parent is the project root.
fn file_path(pack: &Pack, path: &str) -> std::path::PathBuf {
    match pack.root.parent() {
        Some(root) => root.join(path),
        None => std::path::PathBuf::from(path),
    }
}

/// PNG edge (max of width/height) straight from the IHDR chunk, keeping
/// dgm-jev free of an image dependency. Unreadable or non-PNG -> `None`.
fn png_edge(path: &Path) -> Option<u32> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 24];
    file.read_exact(&mut head).ok()?;
    if &head[0..8] != b"\x89PNG\r\n\x1a\n" || &head[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(head[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(head[20..24].try_into().unwrap());
    Some(w.max(h))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Metrics,
    Jev,
    Vision,
}

#[derive(Debug, Clone)]
pub struct ReviewOptions {
    pub tier: Tier,
}

impl Default for ReviewOptions {
    fn default() -> Self {
        Self { tier: Tier::Jev }
    }
}

/// dgm-jev does NOT depend on dgm-render: the caller renders and measures,
/// then hands everything in.
pub struct ReviewInputs<'a> {
    pub gate: GateReport,
    /// `dgm_scene::digest` as JSON.
    pub digest: Value,
    /// `dgm_render::metrics` as JSON.
    pub metrics: Value,
    pub goal: Option<&'a str>,
    /// Vision tier only.
    pub sheet_png: Option<&'a [u8]>,
    pub uv_png: Option<&'a [u8]>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkippedTier {
    pub tier: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct ReviewReport {
    pub gate: GateReport,
    /// silhouette, style, seams, waste, done — later tiers override.
    pub heads: BTreeMap<String, f32>,
    pub tiers_ran: Vec<String>,
    pub skipped: Vec<SkippedTier>,
    /// Vision prose, if it ran.
    pub notes: Vec<String>,
    pub metrics: Value,
    pub latency_ms: BTreeMap<String, u64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    #[error("malformed review inputs: {0}")]
    BadInputs(String),
}

pub async fn review(
    inputs: ReviewInputs<'_>,
    opts: &ReviewOptions,
) -> Result<ReviewReport, ReviewError> {
    if !inputs.metrics.is_object() {
        return Err(ReviewError::BadInputs("`metrics` must be a JSON object".into()));
    }
    if !inputs.digest.is_object() {
        return Err(ReviewError::BadInputs("`digest` must be a JSON object".into()));
    }

    let mut tiers_ran = Vec::new();
    let mut skipped = Vec::new();
    let mut notes = Vec::new();
    let mut latency_ms = BTreeMap::new();
    let mut cost_usd = None;

    // Tier 1: formula heads from metrics + gate. Always runs.
    let t0 = Instant::now();
    let mut heads = heads::formula_heads(&inputs.gate, &inputs.metrics, &inputs.digest);
    latency_ms.insert("metrics".to_string(), t0.elapsed().as_millis() as u64);
    tiers_ran.push("metrics".to_string());

    // Local CLIP scorer: post-v0.
    skipped.push(SkippedTier { tier: "scorer".into(), reason: "not built".into() });

    // Tier 2: Jev. Answers 0..1 override the formula heads.
    if opts.tier != Tier::Metrics {
        match std::env::var("TYPESAFE_API_KEY") {
            Err(_) => skipped
                .push(SkippedTier { tier: "jev".into(), reason: "TYPESAFE_API_KEY not set".into() }),
            Ok(key) => {
                let t = Instant::now();
                let outcome = jev::ask(&key, inputs.goal, &inputs.digest, &inputs.metrics).await;
                latency_ms.insert("jev".to_string(), t.elapsed().as_millis() as u64);
                match outcome {
                    Ok(answers) => {
                        merge_heads(&mut heads, answers);
                        tiers_ran.push("jev".to_string());
                    }
                    Err(reason) => skipped.push(SkippedTier { tier: "jev".into(), reason }),
                }
            }
        }
    }

    // Tier 3: vision. Only with an image; heads win over Jev, notes merge.
    if opts.tier == Tier::Vision {
        if inputs.sheet_png.is_none() && inputs.uv_png.is_none() {
            skipped
                .push(SkippedTier { tier: "vision".into(), reason: "no image provided".into() });
        } else {
            match vision::provider_from_env() {
                None => skipped.push(SkippedTier {
                    tier: "vision".into(),
                    reason: "OPENAI_API_KEY or ANTHROPIC_API_KEY not set".into(),
                }),
                Some(provider) => {
                    let t = Instant::now();
                    let outcome =
                        vision::ask(&provider, inputs.goal, inputs.sheet_png, inputs.uv_png).await;
                    latency_ms.insert("vision".to_string(), t.elapsed().as_millis() as u64);
                    match outcome {
                        Ok(v) => {
                            merge_heads(&mut heads, v.heads);
                            notes.extend(v.notes);
                            cost_usd = v.cost_usd;
                            tiers_ran.push("vision".to_string());
                        }
                        Err(reason) => {
                            skipped.push(SkippedTier { tier: "vision".into(), reason });
                        }
                    }
                }
            }
        }
    }

    Ok(ReviewReport {
        gate: inputs.gate,
        heads,
        tiers_ran,
        skipped,
        notes,
        metrics: inputs.metrics,
        latency_ms,
        cost_usd,
    })
}

/// Later tiers override earlier heads, but only the known five, clamped
/// into 0..1; unknown keys from upstream replies are dropped.
fn merge_heads(heads: &mut BTreeMap<String, f32>, answers: BTreeMap<String, f32>) {
    for (k, v) in answers {
        if HEADS.contains(&k.as_str()) {
            heads.insert(k, v.clamp(0.0, 1.0));
        }
    }
}
