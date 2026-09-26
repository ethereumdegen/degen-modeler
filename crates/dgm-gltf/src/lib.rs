//! Deterministic glTF 2.0 (.glb) export and import summary.
//!
//! [`export_glb`] writes a hand-rolled GLB: JSON keys inserted in one fixed
//! order (`serde_json` keeps insertion order), a single BIN buffer appended
//! in one fixed phase order, every buffer view 4-byte aligned, and nothing
//! derived from time, hashing or platform. Exporting the same [`dgm_scene::Doc`]
//! twice yields byte-identical output — the replay law depends on it.

mod export;
mod import;

use std::path::PathBuf;

pub use export::{ExportOptions, export_glb};
pub use import::import_summary;

#[derive(Debug, thiserror::Error)]
pub enum GltfError {
    #[error("object `{object}` references unknown material `{material}`")]
    UnknownMaterial { object: String, material: String },
    #[error("material `{material}` references unknown trim sheet `{sheet}`")]
    UnknownSheet { material: String, sheet: String },
    #[error("texture `{path}`: {source}")]
    Texture {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("object `{object}` bone `{bone}`: parent index {parent} out of range")]
    BadBoneParent { object: String, bone: String, parent: u16 },
    #[error("clip `{clip}`: no bone `{bone}` on object `{object}`")]
    ClipBone { clip: String, object: String, bone: String },
    #[error("clip `{clip}` bone `{bone}`: {kind} has {got} keys for {want} times")]
    CurveLength { clip: String, bone: String, kind: &'static str, got: usize, want: usize },
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("glTF parse: {0}")]
    Parse(#[from] gltf::Error),
}
