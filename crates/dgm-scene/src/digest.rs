//! The structural scene digest: what an agent reads before deciding the next
//! op. Perceptual metrics (`dgm-render`) are merged in by the API layer via
//! `extra`.

use std::collections::BTreeMap;

use dgm_atlas::{AssetClass, Pack};
use serde::Serialize;
use serde_json::Value;

use crate::doc::{Doc, Material};
use crate::gate::GateReport;

#[derive(Debug, Serialize)]
pub struct RigDigest {
    pub preset: String,
    pub bones: usize,
    pub weighted_verts: usize,
}

#[derive(Debug, Serialize)]
pub struct ObjectDigest {
    pub verts: usize,
    pub faces: usize,
    pub tris: u32,
    pub bounds: Option<[[f32; 3]; 2]>,
    pub material: Option<String>,
    pub seam_edges: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rig: Option<RigDigest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lod_of: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ClipDigest {
    pub object: String,
    pub duration: f32,
    pub looped: bool,
    pub channels: usize,
}

#[derive(Debug, Serialize)]
pub struct BudgetDigest {
    pub class: AssetClass,
    pub max_tris: u32,
    pub used_tris: u32,
}

#[derive(Debug, Serialize)]
pub struct Digest {
    pub revision: u64,
    pub goal: Option<String>,
    pub asset_class: AssetClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetDigest>,
    pub objects: BTreeMap<String, ObjectDigest>,
    pub materials: BTreeMap<String, Material>,
    pub selections: BTreeMap<String, usize>,
    pub mirror_sets: BTreeMap<String, usize>,
    pub clips: BTreeMap<String, ClipDigest>,
    pub gate_pass: bool,
    pub findings: Vec<dgm_mesh::Finding>,
    /// Metric extras merged by the API layer (uv occupancy, seam contrast…).
    #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

pub fn digest(doc: &Doc, pack: &Pack, gate: &GateReport) -> Digest {
    let used_tris: u32 = doc
        .objects
        .values()
        .filter(|o| o.lod_of.is_none())
        .map(|o| o.mesh.tri_count())
        .sum();
    Digest {
        revision: doc.revision,
        goal: doc.goal.clone(),
        asset_class: doc.asset_class,
        budget: pack.budget(doc.asset_class).map(|b| BudgetDigest {
            class: doc.asset_class,
            max_tris: b.max_tris,
            used_tris,
        }),
        objects: doc
            .objects
            .iter()
            .map(|(name, o)| {
                (
                    name.clone(),
                    ObjectDigest {
                        verts: o.mesh.verts.len(),
                        faces: o.mesh.faces.len(),
                        tris: o.mesh.tri_count(),
                        bounds: o.mesh.bounds().map(|(lo, hi)| [lo.to_array(), hi.to_array()]),
                        material: o.material.clone(),
                        seam_edges: o.mesh.seams.len(),
                        rig: o.rig.as_ref().map(|r| RigDigest {
                            preset: r.preset.clone(),
                            bones: r.bones.len(),
                            weighted_verts: r.weights.len(),
                        }),
                        lod_of: o.lod_of.clone(),
                    },
                )
            })
            .collect(),
        materials: doc.materials.clone(),
        selections: doc.selections.iter().map(|(k, s)| (k.clone(), s.elems.len())).collect(),
        mirror_sets: doc.mirror_sets.iter().map(|(k, m)| (k.clone(), m.faces.len())).collect(),
        clips: doc
            .clips
            .iter()
            .map(|(k, c)| {
                (
                    k.clone(),
                    ClipDigest {
                        object: c.object.clone(),
                        duration: c.duration,
                        looped: c.looped,
                        channels: c.channels.len(),
                    },
                )
            })
            .collect(),
        gate_pass: gate.pass,
        findings: gate.findings.clone(),
        extra: serde_json::Map::new(),
    }
}
