//! Orchestration shared by the HTTP handlers and the CLI: digest+metrics
//! merge, render artifacts, the review pipeline, and gated export. Artifacts
//! are always named `r<rev>-<kind>.png` / `r<rev>-review.json`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use dgm_scene::GateReport;
use dgm_scene::digest::Digest;
use dgm_scene::project::Project;
use serde_json::{Value, json};

pub const DEFAULT_VIEW_PX: u32 = 512;
pub const REVIEW_VIEW_PX: u32 = 256;
pub const FILMSTRIP_FRAMES: u32 = 8;

#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unknown object `{0}`")]
    UnknownObject(String),
    #[error("export gate failed")]
    GateFailed(GateReport),
    #[error("render: {0}")]
    Render(String),
    #[error("gltf: {0}")]
    Gltf(String),
    #[error("review upstream: {0}")]
    Review(String),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderKind {
    Sheet,
    Wireframe,
    Uv,
    Heatmap,
    Filmstrip,
}

impl RenderKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sheet" => Self::Sheet,
            "wireframe" => Self::Wireframe,
            "uv" => Self::Uv,
            "heatmap" => Self::Heatmap,
            "filmstrip" => Self::Filmstrip,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sheet => "sheet",
            Self::Wireframe => "wireframe",
            Self::Uv => "uv",
            Self::Heatmap => "heatmap",
            Self::Filmstrip => "filmstrip",
        }
    }
}

/// Scene digest with `dgm_render::metrics` merged into `extra.metrics`; when
/// metrics cannot be computed the digest still ships, with `extra.warn`.
pub fn scene_digest(project: &Project) -> Digest {
    let gate = dgm_jev::gate(&project.doc, &project.pack);
    let mut digest = dgm_scene::digest::digest(&project.doc, &project.pack, &gate);
    match dgm_render::metrics(&project.doc, &project.pack) {
        Ok(m) => {
            let value = serde_json::to_value(&m).unwrap_or(Value::Null);
            digest.extra.insert("metrics".into(), value);
        }
        Err(e) => {
            digest.extra.insert("warn".into(), json!(format!("metrics unavailable: {e}")));
        }
    }
    digest
}

/// Adapter for `dgm_atlas::usage::region_usage`, which sits below the scene
/// crate: for every object whose material is a pack trim, one representative
/// UV point per face (the face's UV centroid).
pub fn region_usage(doc: &dgm_scene::Doc, pack: &dgm_atlas::Pack) -> BTreeMap<String, Vec<String>> {
    let mut uv_points: BTreeMap<String, (String, Vec<[f32; 2]>)> = BTreeMap::new();
    for (name, object) in &doc.objects {
        let Some(sheet) = object
            .material
            .as_ref()
            .and_then(|m| doc.materials.get(m))
            .and_then(|m| match &m.texture {
                dgm_scene::TextureRef::Trim { sheet } => Some(sheet.clone()),
                _ => None,
            })
        else {
            continue;
        };
        let points: Vec<[f32; 2]> = object
            .mesh
            .faces
            .values()
            .filter(|f| !f.corners.is_empty())
            .map(|f| {
                let sum: glam::Vec2 = f.corners.iter().map(|c| c.uv).sum();
                (sum / f.corners.len() as f32).to_array()
            })
            .collect();
        uv_points.insert(name.clone(), (sheet, points));
    }
    dgm_atlas::usage::region_usage(&uv_points, pack)
}

pub fn png_bytes(img: &image::RgbaImage) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).expect("png encode into memory");
    buf.into_inner()
}

fn write_artifact(project: &Project, name: &str, bytes: &[u8]) -> Result<(), StageError> {
    let dir = project.artifacts_dir();
    std::fs::create_dir_all(&dir).map_err(|e| StageError::Io(dir.clone(), e))?;
    let path = dir.join(name);
    std::fs::write(&path, bytes).map_err(|e| StageError::Io(path, e))
}

pub struct RenderOut {
    pub png: Vec<u8>,
    /// Artifact file name (`r<rev>-<kind>.png`), also written to disk.
    pub artifact: String,
}

pub fn render_project(
    project: &Project,
    kind: RenderKind,
    object: Option<&str>,
    px: u32,
) -> Result<RenderOut, StageError> {
    let doc = &project.doc;
    let pack = &project.pack;
    let img = match kind {
        RenderKind::Sheet => dgm_render::contact_sheet(doc, pack, px),
        RenderKind::Wireframe => dgm_render::wireframe_sheet(doc, pack, px),
        RenderKind::Heatmap => dgm_render::heatmap(doc, pack, px),
        RenderKind::Filmstrip => dgm_render::filmstrip(doc, pack, FILMSTRIP_FRAMES, px),
        RenderKind::Uv => {
            let object = object
                .ok_or_else(|| StageError::BadRequest("render kind `uv` needs ?object=".into()))?;
            if !doc.objects.contains_key(object) {
                return Err(StageError::UnknownObject(object.into()));
            }
            dgm_render::uv_layout(doc, pack, object)
        }
    }
    .map_err(|e| StageError::Render(e.to_string()))?;
    let png = png_bytes(&img);
    let artifact = format!("r{}-{}.png", doc.revision, kind.as_str());
    write_artifact(project, &artifact, &png)?;
    Ok(RenderOut { png, artifact })
}

/// The review pipeline: gate + digest + metrics + best-effort renders in,
/// `dgm_jev::review` out; the sheet/uv PNGs and the report land in
/// `artifacts/` under the current revision.
pub async fn review_project(
    project: &Project,
    tier: dgm_jev::Tier,
) -> Result<dgm_jev::ReviewReport, StageError> {
    let doc = &project.doc;
    let pack = &project.pack;
    let rev = doc.revision;

    let gate = dgm_jev::gate(doc, pack);
    let digest_json = serde_json::to_value(dgm_scene::digest::digest(doc, pack, &gate))
        .expect("digest serializes");
    let metrics_json = match dgm_render::metrics(doc, pack) {
        Ok(m) => serde_json::to_value(&m).expect("metrics serialize"),
        Err(e) => json!({ "warn": format!("metrics unavailable: {e}") }),
    };

    // Renders are best-effort: a scene the rasterizer refuses (e.g. empty)
    // still reviews on gate + digest alone.
    let sheet_png =
        dgm_render::contact_sheet(doc, pack, REVIEW_VIEW_PX).ok().map(|i| png_bytes(&i));
    let uv_png = doc
        .objects
        .keys()
        .next()
        .and_then(|name| dgm_render::uv_layout(doc, pack, name).ok())
        .map(|i| png_bytes(&i));
    if let Some(bytes) = &sheet_png {
        write_artifact(project, &format!("r{rev}-sheet.png"), bytes)?;
    }
    if let Some(bytes) = &uv_png {
        write_artifact(project, &format!("r{rev}-uv.png"), bytes)?;
    }

    let inputs = dgm_jev::ReviewInputs {
        gate,
        digest: digest_json,
        metrics: metrics_json,
        goal: doc.goal.as_deref(),
        sheet_png: sheet_png.as_deref(),
        uv_png: uv_png.as_deref(),
    };
    let report = dgm_jev::review(inputs, &dgm_jev::ReviewOptions { tier })
        .await
        .map_err(|e| StageError::Review(e.to_string()))?;
    write_artifact(
        project,
        &format!("r{rev}-review.json"),
        &serde_json::to_vec_pretty(&report).expect("report serializes"),
    )?;
    Ok(report)
}

pub struct ExportOut {
    pub path: PathBuf,
    pub bytes: usize,
    pub gate: GateReport,
}

/// Gate, then export. Hard findings block with the full report; the report
/// also rides along inside the GLB extras on success.
pub fn export_project(project: &Project, out: Option<&str>) -> Result<ExportOut, StageError> {
    let gate = dgm_jev::gate(&project.doc, &project.pack);
    if !gate.pass {
        return Err(StageError::GateFailed(gate));
    }
    let opts = dgm_gltf::ExportOptions {
        base_dir: project.root.clone(),
        embed_report: Some(serde_json::to_value(&gate).expect("gate serializes")),
    };
    let glb = dgm_gltf::export_glb(&project.doc, &project.pack, &opts)
        .map_err(|e| StageError::Gltf(e.to_string()))?;
    let path = match out {
        Some(o) => {
            let p = PathBuf::from(o);
            if p.is_absolute() { p } else { project.root.join(p) }
        }
        None => project.exports_dir().join("model.glb"),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| StageError::Io(parent.to_path_buf(), e))?;
    }
    std::fs::write(&path, &glb).map_err(|e| StageError::Io(path.clone(), e))?;
    Ok(ExportOut { path, bytes: glb.len(), gate })
}
