use serde::Serialize;

use crate::mesh::Mesh;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Advisory; shows up in digests and reviews.
    Warn,
    /// Blocks export; budget rules also fail the op itself.
    Hard,
}

/// One gate finding. `rule` is a stable dotted id ("mesh.non_manifold_edge");
/// `elems` are display ids ("v3", "f12", "e4:9") for attribution.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub rule: String,
    pub severity: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub elems: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

impl Finding {
    pub fn hard(rule: &str, message: impl Into<String>) -> Self {
        Self { rule: rule.into(), severity: Severity::Hard, message: message.into(), elems: Vec::new(), value: None }
    }

    pub fn warn(rule: &str, message: impl Into<String>) -> Self {
        Self { rule: rule.into(), severity: Severity::Warn, message: message.into(), elems: Vec::new(), value: None }
    }

    pub fn with_elems(mut self, elems: Vec<String>) -> Self {
        self.elems = elems;
        self
    }

    pub fn with_value(mut self, value: f64) -> Self {
        self.value = Some(value);
        self
    }
}

const DEGENERATE_AREA: f32 = 1e-9;

/// Mesh-level gate rules: non-manifold edges, degenerate faces, unused verts.
pub fn validate_mesh(mesh: &Mesh) -> Vec<Finding> {
    let mut findings = Vec::new();

    for (edge, faces) in mesh.edge_faces() {
        if faces.len() > 2 {
            findings.push(
                Finding::hard(
                    "mesh.non_manifold_edge",
                    format!("edge {edge} is shared by {} faces", faces.len()),
                )
                .with_elems(
                    std::iter::once(edge.to_string())
                        .chain(faces.iter().map(|f| f.to_string()))
                        .collect(),
                ),
            );
        }
    }

    for &fid in mesh.faces.keys() {
        let area = mesh.face_area(fid).unwrap_or(0.0);
        if area < DEGENERATE_AREA {
            findings.push(
                Finding::hard("mesh.degenerate_face", format!("face {fid} has ~zero area"))
                    .with_elems(vec![fid.to_string()])
                    .with_value(area as f64),
            );
        }
    }

    let used: std::collections::BTreeSet<_> = mesh.faces.values().flat_map(|f| f.verts()).collect();
    let unused: Vec<String> = mesh
        .verts
        .keys()
        .filter(|v| !used.contains(v))
        .map(|v| v.to_string())
        .collect();
    if !unused.is_empty() {
        findings.push(
            Finding::warn("mesh.unused_vert", format!("{} vertices belong to no face", unused.len()))
                .with_elems(unused),
        );
    }

    findings
}
