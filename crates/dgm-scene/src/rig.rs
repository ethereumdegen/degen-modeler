use std::collections::BTreeMap;

use dgm_mesh::{Finding, VertId};
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::doc::Doc;

pub const MAX_BONES: usize = 40;
pub const MAX_INFLUENCES: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bone {
    pub name: String,
    pub parent: Option<u16>,
    pub head: Vec3,
    pub tail: Vec3,
}

/// A fitted skeleton plus skin weights. Weights: per-vertex (bone index,
/// weight) pairs, at most [`MAX_INFLUENCES`], summing to ~1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rig {
    pub preset: String,
    pub bones: Vec<Bone>,
    pub weights: BTreeMap<VertId, Vec<(u16, f32)>>,
}

impl Rig {
    pub fn bone_index(&self, name: &str) -> Option<u16> {
        self.bones.iter().position(|b| b.name == name).map(|i| i as u16)
    }
}

/// Rig gate rules: bone/influence caps, weight normalization, coverage.
pub fn rig_findings(doc: &Doc) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (name, object) in &doc.objects {
        let Some(rig) = &object.rig else { continue };

        if rig.bones.len() > MAX_BONES {
            findings.push(Finding::hard(
                "rig.too_many_bones",
                format!("object `{name}`: {} bones exceeds the cap of {MAX_BONES}", rig.bones.len()),
            ));
        }
        for (i, bone) in rig.bones.iter().enumerate() {
            if let Some(p) = bone.parent
                && p as usize >= rig.bones.len()
            {
                findings.push(Finding::hard(
                    "rig.bad_parent",
                    format!("object `{name}`: bone `{}` ({i}) parents missing index {p}", bone.name),
                ));
            }
        }

        let mut unweighted = Vec::new();
        for &v in object.mesh.verts.keys() {
            match rig.weights.get(&v) {
                None => unweighted.push(v.to_string()),
                Some(w) => {
                    if w.is_empty() {
                        unweighted.push(v.to_string());
                        continue;
                    }
                    if w.len() > MAX_INFLUENCES {
                        findings.push(
                            Finding::hard(
                                "rig.too_many_influences",
                                format!("object `{name}`: vertex {v} has {} influences (max {MAX_INFLUENCES})", w.len()),
                            )
                            .with_elems(vec![v.to_string()]),
                        );
                    }
                    let sum: f32 = w.iter().map(|(_, x)| x).sum();
                    if (sum - 1.0).abs() > 1e-3 {
                        findings.push(
                            Finding::hard(
                                "rig.unnormalized_weights",
                                format!("object `{name}`: vertex {v} weights sum to {sum:.4}"),
                            )
                            .with_elems(vec![v.to_string()])
                            .with_value(sum as f64),
                        );
                    }
                    if let Some((bad, _)) = w.iter().find(|(b, _)| *b as usize >= rig.bones.len()) {
                        findings.push(
                            Finding::hard(
                                "rig.bad_bone_index",
                                format!("object `{name}`: vertex {v} weights missing bone index {bad}"),
                            )
                            .with_elems(vec![v.to_string()]),
                        );
                    }
                }
            }
        }
        if !unweighted.is_empty() {
            findings.push(
                Finding::hard(
                    "rig.unweighted_verts",
                    format!("object `{name}`: {} vertices carry no weights", unweighted.len()),
                )
                .with_elems(unweighted),
            );
        }
    }
    findings
}
