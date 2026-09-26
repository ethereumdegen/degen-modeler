//! Crate-internal UV geometry helpers.

use std::collections::BTreeSet;

use dgm_mesh::{Face, FaceId, Mesh, MeshDelta};
use glam::Vec2;

use crate::UvError;

/// Shoelace area of the face's UV polygon.
pub(crate) fn face_uv_area(face: &Face) -> f32 {
    let n = face.corners.len();
    let mut s = 0.0f32;
    for i in 0..n {
        let a = face.corners[i].uv;
        let b = face.corners[(i + 1) % n].uv;
        s += a.x * b.y - b.x * a.y;
    }
    (s * 0.5).abs()
}

/// UV bounding box over the island's corners. Islands are never empty.
pub(crate) fn island_uv_bbox(mesh: &Mesh, island: &BTreeSet<FaceId>) -> (Vec2, Vec2) {
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for &f in island {
        if let Some(face) = mesh.faces.get(&f) {
            for c in &face.corners {
                lo = lo.min(c.uv);
                hi = hi.max(c.uv);
            }
        }
    }
    (lo, hi)
}

/// Selection preflight: non-empty and every face exists.
pub(crate) fn check_faces(mesh: &Mesh, faces: &BTreeSet<FaceId>) -> Result<(), UvError> {
    if faces.is_empty() {
        return Err(UvError::EmptySelection);
    }
    for &f in faces {
        if !mesh.faces.contains_key(&f) {
            return Err(UvError::UnknownFace(f));
        }
    }
    Ok(())
}

/// A delta that touched only UVs, on the given faces.
pub(crate) fn uv_delta(faces: impl IntoIterator<Item = FaceId>) -> MeshDelta {
    MeshDelta { uv_faces: faces.into_iter().collect(), ..MeshDelta::default() }
}
