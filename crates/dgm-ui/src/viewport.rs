//! Doc mesh -> flat vertex buffers, plus CPU ray picking.
//!
//! Bevy-free (arrays out) so the geometry path is testable headless. Faces
//! are emitted unindexed (three corners per fan triangle); normals come from
//! `corner_normals(pack.hard_edge_angle_deg)`, UVs from the corners.

use std::hash::{Hash, Hasher};

use dgm_mesh::{FaceId, Mesh, VertId};
use dgm_scene::Doc;
use glam::Vec3;

/// Flat, unindexed vertex streams for one object, with the source vertex id
/// per emitted vertex so the CPU skinner can rewrite positions in place.
#[derive(Debug, Clone, Default)]
pub struct BuiltObject {
    pub verts: Vec<VertId>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Linear RGBA per emitted vertex: baked vertex colors (white when the
    /// mesh has none), multiplied into the unlit base color by Bevy.
    pub colors: Vec<[f32; 4]>,
}

/// Triangulate `mesh` into flat streams with hard-edge-aware corner normals.
pub fn build_object(mesh: &Mesh, hard_angle_deg: f32) -> BuiltObject {
    let normals = mesh.corner_normals(hard_angle_deg);
    let mut out = BuiltObject::default();
    for tri in mesh.triangulate() {
        let Ok(face) = mesh.face(tri.face) else { continue };
        for ci in tri.corner_idx {
            let corner = face.corners[ci as usize];
            let Ok(pos) = mesh.pos(corner.vert) else { continue };
            let n = normals.get(&(tri.face, ci)).copied().unwrap_or(Vec3::Y);
            out.verts.push(corner.vert);
            out.positions.push(pos.to_array());
            out.normals.push(n.to_array());
            out.uvs.push(corner.uv.to_array());
            let [r, g, b] = mesh.colors.get(&corner.vert).copied().unwrap_or([1.0; 3]);
            out.colors.push([r, g, b, 1.0]);
        }
    }
    out
}

/// Change signature for an object's viewport state: mesh topology/geometry,
/// material assignment, and the material definition itself.
pub fn object_signature(doc: &Doc, name: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let Some(object) = doc.objects.get(name) else { return 0 };
    serde_json::to_string(&object.mesh).unwrap_or_default().hash(&mut h);
    object.material.hash(&mut h);
    if let Some(mat) = object.material.as_ref().and_then(|m| doc.materials.get(m)) {
        serde_json::to_string(mat).unwrap_or_default().hash(&mut h);
    }
    h.finish()
}

/// Union AABB of every object in the doc.
pub fn doc_bounds(doc: &Doc) -> Option<(Vec3, Vec3)> {
    let mut acc: Option<(Vec3, Vec3)> = None;
    for object in doc.objects.values() {
        if let Some((lo, hi)) = object.mesh.bounds() {
            acc = Some(match acc {
                None => (lo, hi),
                Some((alo, ahi)) => (alo.min(lo), ahi.max(hi)),
            });
        }
    }
    acc
}

/// A CPU picking hit: nearest face along the ray plus its closest corner
/// vertex, in display id terms (`f12`, `v3`).
#[derive(Debug, Clone, PartialEq)]
pub struct PickHit {
    pub object: String,
    pub face: FaceId,
    pub vert: VertId,
    pub distance: f32,
}

/// Möller–Trumbore against every (fan) triangle of every doc object.
pub fn pick(doc: &Doc, origin: Vec3, dir: Vec3) -> Option<PickHit> {
    let mut best: Option<(f32, String, FaceId, Vec3)> = None;
    for (name, object) in &doc.objects {
        let mesh = &object.mesh;
        for tri in mesh.triangulate() {
            let Ok(face) = mesh.face(tri.face) else { continue };
            let p = |ci: u16| mesh.pos(face.corners[ci as usize].vert).ok();
            let (Some(a), Some(b), Some(c)) =
                (p(tri.corner_idx[0]), p(tri.corner_idx[1]), p(tri.corner_idx[2]))
            else {
                continue;
            };
            if let Some(t) = ray_triangle(origin, dir, a, b, c)
                && best.as_ref().is_none_or(|(bt, ..)| t < *bt)
            {
                best = Some((t, name.clone(), tri.face, origin + dir * t));
            }
        }
    }
    let (distance, object, face, point) = best?;
    let vert = doc
        .objects
        .get(&object)?
        .mesh
        .face(face)
        .ok()?
        .corners
        .iter()
        .map(|c| c.vert)
        .min_by(|&a, &b| {
            let da = dist_sq(doc, &object, a, point);
            let db = dist_sq(doc, &object, b, point);
            da.total_cmp(&db)
        })?;
    Some(PickHit { object, face, vert, distance })
}

fn dist_sq(doc: &Doc, object: &str, v: VertId, point: Vec3) -> f32 {
    doc.objects
        .get(object)
        .and_then(|o| o.mesh.pos(v).ok())
        .map(|p| p.distance_squared(point))
        .unwrap_or(f32::INFINITY)
}

/// Ray/triangle intersection distance (both-sided), `None` on miss.
fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    const EPS: f32 = 1e-7;
    let ab = b - a;
    let ac = c - a;
    let pvec = dir.cross(ac);
    let det = ab.dot(pvec);
    if det.abs() < EPS {
        return None;
    }
    let inv_det = 1.0 / det;
    let tvec = origin - a;
    let u = tvec.dot(pvec) * inv_det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qvec = tvec.cross(ab);
    let v = dir.dot(qvec) * inv_det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = ac.dot(qvec) * inv_det;
    (t > EPS).then_some(t)
}

#[cfg(test)]
mod tests {
    use dgm_atlas::AssetClass;
    use dgm_mesh::primitives;
    use dgm_scene::doc::Object;

    use super::*;

    fn box_doc() -> Doc {
        let mut doc = Doc::new(AssetClass::Prop);
        let mesh = primitives::prim_box(Vec3::ONE).expect("unit box");
        doc.objects.insert("cube".into(), Object::new(mesh));
        doc
    }

    #[test]
    fn build_emits_three_verts_per_tri_with_unit_normals() {
        let doc = box_doc();
        let built = build_object(&doc.objects["cube"].mesh, 40.0);
        let tris = doc.objects["cube"].mesh.tri_count() as usize;
        assert_eq!(tris, 12);
        assert_eq!(built.positions.len(), tris * 3);
        assert_eq!(built.normals.len(), tris * 3);
        assert_eq!(built.uvs.len(), tris * 3);
        assert_eq!(built.verts.len(), tris * 3);
        for n in &built.normals {
            let len = Vec3::from_array(*n).length();
            assert!((len - 1.0).abs() < 1e-4, "normal length {len}");
        }
    }

    #[test]
    fn box_corner_normals_stay_hard_under_forty_degrees() {
        // All box dihedral angles are 90deg > 40deg threshold: every corner
        // normal must equal its face normal (no smoothing across edges).
        let doc = box_doc();
        let mesh = &doc.objects["cube"].mesh;
        let built = build_object(mesh, 40.0);
        let mut i = 0;
        for tri in mesh.triangulate() {
            let fnorm = mesh.face_normal(tri.face).unwrap();
            for _ in 0..3 {
                let n = Vec3::from_array(built.normals[i]);
                assert!(n.abs_diff_eq(fnorm, 1e-4), "smoothed across a hard edge");
                i += 1;
            }
        }
    }

    #[test]
    fn pick_hits_front_face_of_box() {
        let doc = box_doc();
        let hit = pick(&doc, Vec3::new(0.1, 0.1, 5.0), Vec3::new(0.0, 0.0, -1.0)).expect("hit");
        assert_eq!(hit.object, "cube");
        assert!((hit.distance - 4.5).abs() < 1e-4, "distance {}", hit.distance);
        // The hit face really contains the picked vertex.
        let face = doc.objects["cube"].mesh.face(hit.face).unwrap();
        assert!(face.corners.iter().any(|c| c.vert == hit.vert));
    }

    #[test]
    fn pick_misses_beside_the_box() {
        let doc = box_doc();
        assert_eq!(pick(&doc, Vec3::new(5.0, 5.0, 5.0), Vec3::new(0.0, 0.0, -1.0)), None);
    }

    #[test]
    fn signature_tracks_mesh_and_material_changes() {
        let mut doc = box_doc();
        let before = object_signature(&doc, "cube");
        assert_eq!(before, object_signature(&doc, "cube"));
        doc.objects.get_mut("cube").unwrap().material = Some("wood".into());
        let after = object_signature(&doc, "cube");
        assert_ne!(before, after);
        assert_eq!(object_signature(&doc, "missing"), 0);
    }
}
