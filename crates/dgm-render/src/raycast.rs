//! Möller–Trumbore raycast against one object's triangulated faces.

use dgm_mesh::FaceId;
use dgm_scene::Doc;
use glam::Vec3;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct RayHit {
    pub face: FaceId,
    /// World distance along the normalized direction.
    pub distance: f32,
    pub point: Vec3,
}

/// Nearest hit over the object's fan-triangulated faces; both windings hit
/// (double-sided). `dir` need not be normalized. Unknown objects and
/// zero-length directions miss. Ties keep the lowest face id (face order
/// is deterministic).
pub fn raycast(doc: &Doc, object: &str, origin: Vec3, dir: Vec3) -> Option<RayHit> {
    let obj = doc.objects.get(object)?;
    let dir = dir.try_normalize()?;
    let mesh = &obj.mesh;
    let mut best: Option<RayHit> = None;
    for (&fid, face) in &mesh.faces {
        for i in 1..face.corners.len() - 1 {
            let v0 = mesh.verts[&face.corners[0].vert];
            let v1 = mesh.verts[&face.corners[i].vert];
            let v2 = mesh.verts[&face.corners[i + 1].vert];
            if let Some(t) = ray_tri(origin, dir, v0, v1, v2)
                && best.is_none_or(|b| t < b.distance)
            {
                best = Some(RayHit { face: fid, distance: t, point: origin + dir * t });
            }
        }
    }
    best
}

fn ray_tri(orig: Vec3, dir: Vec3, v0: Vec3, v1: Vec3, v2: Vec3) -> Option<f32> {
    const EPS: f32 = 1e-7;
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < EPS {
        return None;
    }
    let inv = 1.0 / det;
    let s = orig - v0;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-5).then_some(t)
}
