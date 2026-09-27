//! bake_ao / bake_sun / bake_glow / paint_vertex: baked per-vertex lighting.
//! Owned by the Bake slice; see CONTRACT-19.md.
//!
//! Every op writes linear RGB into `Mesh.colors` (absent = white) and the
//! results compose multiplicatively (AO, sun) or additively (glow, which
//! adds light back into darkened crevices), so the playbook order is
//! `bake_ao -> bake_sun -> bake_glow -> paint_vertex`. Stored colors are
//! clamped to 0..1 so `COLOR_0` export never saturates silently.

use std::collections::{BTreeMap, BTreeSet};

use dgm_mesh::{FaceId, Mesh, MeshDelta, VertId};
use glam::Vec3;

use crate::doc::Doc;
use crate::error::OpError;
use crate::op::Diff;
use crate::select::{self, Elems, Selection};

/// AO rays stop caring past this fraction of the occluder scene diagonal:
/// hits closer count with a linear falloff, farther ones not at all. Keeps
/// a closed cave from baking uniformly black while crevices go dark.
const AO_REACH: f32 = 0.25;
/// Ray origins step off the surface by this fraction of the scene diagonal
/// (also the minimum hit distance) so a face never shadows itself.
const AO_EPS: f32 = 1e-4;
/// Ray origins also slide this many `AO_EPS` from the vertex toward the
/// face centroid, so walls meeting at the vertex are hit at `t > AO_EPS`.
const AO_INSET: f32 = 4.0;
/// Triangles per BVH leaf.
const BVH_LEAF: usize = 8;

fn check_finite(name: &str, v: f32, min: f32) -> Result<(), OpError> {
    if !v.is_finite() || v < min {
        return Err(OpError::BadParams(format!("{name} {v} must be finite and >= {min}")));
    }
    Ok(())
}

fn color_of(mesh: &Mesh, v: VertId) -> Vec3 {
    mesh.colors.get(&v).map_or(Vec3::ONE, |&c| Vec3::from(c))
}

fn store(mesh: &mut Mesh, v: VertId, c: Vec3) {
    mesh.colors.insert(v, c.clamp(Vec3::ZERO, Vec3::ONE).to_array());
}

/// Area-weighted vertex normals (Newell face vectors are 2x area long, so
/// summing them weights by area for free). Verts on no face get `None`.
fn vertex_normals(mesh: &Mesh) -> BTreeMap<VertId, Vec3> {
    let mut acc: BTreeMap<VertId, Vec3> = BTreeMap::new();
    for face in mesh.faces.values() {
        let k = face.corners.len();
        let mut n = Vec3::ZERO;
        for i in 0..k {
            let a = mesh.verts[&face.corners[i].vert];
            let b = mesh.verts[&face.corners[(i + 1) % k].vert];
            n += Vec3::new(
                (a.y - b.y) * (a.z + b.z),
                (a.z - b.z) * (a.x + b.x),
                (a.x - b.x) * (a.y + b.y),
            );
        }
        for c in &face.corners {
            *acc.entry(c.vert).or_insert(Vec3::ZERO) += n;
        }
    }
    acc.into_iter().filter_map(|(v, n)| Some((v, n.try_normalize()?))).collect()
}

/// Diff naming the object and the touched verts (capped like every op).
fn color_diff(object: &str, what: &str, verts: Vec<VertId>) -> Diff {
    let n = verts.len();
    let mut diff = Diff::from_delta(object, &MeshDelta { moved_verts: verts, ..MeshDelta::default() });
    diff.summary = format!("{object}: {what} on {n} verts");
    diff
}

// ---- ray casting against the whole scene ----

struct SceneTri {
    obj: u32,
    face: FaceId,
    a: Vec3,
    e1: Vec3,
    e2: Vec3,
    /// Geometric (unnormalized) normal; sign gives front/back facing.
    n: Vec3,
}

enum Node {
    Leaf { lo: Vec3, hi: Vec3, start: u32, end: u32 },
    Inner { lo: Vec3, hi: Vec3, right: u32 },
}

/// Midpoint-split BVH over every non-LOD triangle in the doc. Tri order
/// (object name, face id, fan index) and the median sort are deterministic.
struct SceneBvh {
    tris: Vec<SceneTri>,
    nodes: Vec<Node>,
    diag: f32,
}

impl SceneBvh {
    fn build(doc: &Doc) -> Self {
        let mut tris = Vec::new();
        let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
        for (oi, obj) in doc.objects.values().enumerate() {
            if obj.lod_of.is_some() {
                continue;
            }
            let mesh = &obj.mesh;
            for (&fid, face) in &mesh.faces {
                for i in 1..face.corners.len() - 1 {
                    let a = mesh.verts[&face.corners[0].vert];
                    let b = mesh.verts[&face.corners[i].vert];
                    let c = mesh.verts[&face.corners[i + 1].vert];
                    let (e1, e2) = (b - a, c - a);
                    let n = e1.cross(e2);
                    if n.length_squared() <= 0.0 {
                        continue;
                    }
                    lo = lo.min(a.min(b).min(c));
                    hi = hi.max(a.max(b).max(c));
                    tris.push(SceneTri { obj: oi as u32, face: fid, a, e1, e2, n });
                }
            }
        }
        let diag = if tris.is_empty() { 1.0 } else { (hi - lo).length().max(1e-6) };
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = Vec::with_capacity(tris.len() / BVH_LEAF * 2 + 1);
        if !tris.is_empty() {
            Self::split(&tris, &mut order, 0, tris.len(), &mut nodes);
        }
        // Reorder tris so leaves index contiguous ranges.
        let tris: Vec<SceneTri> = {
            let mut slots: Vec<Option<SceneTri>> = tris.into_iter().map(Some).collect();
            order.iter().map(|&i| slots[i as usize].take().expect("each tri placed once")).collect()
        };
        Self { tris, nodes, diag }
    }

    fn bounds(tris: &[SceneTri], order: &[u32]) -> (Vec3, Vec3) {
        let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
        for &i in order {
            let t = &tris[i as usize];
            let (b, c) = (t.a + t.e1, t.a + t.e2);
            lo = lo.min(t.a.min(b).min(c));
            hi = hi.max(t.a.max(b).max(c));
        }
        (lo, hi)
    }

    fn split(tris: &[SceneTri], order: &mut [u32], start: usize, end: usize, nodes: &mut Vec<Node>) {
        let (lo, hi) = Self::bounds(tris, &order[start..end]);
        let count = end - start;
        if count <= BVH_LEAF {
            nodes.push(Node::Leaf { lo, hi, start: start as u32, end: end as u32 });
            return;
        }
        let ext = hi - lo;
        let axis = if ext.x >= ext.y && ext.x >= ext.z {
            0
        } else if ext.y >= ext.z {
            1
        } else {
            2
        };
        let centroid = |i: u32| {
            let t = &tris[i as usize];
            (t.a * 3.0 + t.e1 + t.e2)[axis]
        };
        // Stable sort on the centroid; ties keep tri order -> deterministic.
        order[start..end].sort_by(|&x, &y| centroid(x).total_cmp(&centroid(y)));
        let mid = start + count / 2;
        let me = nodes.len();
        nodes.push(Node::Inner { lo, hi, right: 0 });
        Self::split(tris, order, start, mid, nodes);
        let right = nodes.len() as u32;
        Self::split(tris, order, mid, end, nodes);
        nodes[me] = Node::Inner { lo, hi, right };
    }

    /// Nearest front-facing hit along `dir` within `(tmin, tmax)`, skipping
    /// triangles of `skip_obj` whose face is in `skip_faces` (the faces
    /// around the shooting vertex). Returns the hit distance.
    fn nearest(
        &self,
        origin: Vec3,
        dir: Vec3,
        tmin: f32,
        mut tmax: f32,
        skip_obj: u32,
        skip_faces: &BTreeSet<FaceId>,
    ) -> Option<f32> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = Vec3::new(1.0 / dir.x, 1.0 / dir.y, 1.0 / dir.z);
        let mut best = None;
        let mut stack: Vec<u32> = vec![0];
        while let Some(ni) = stack.pop() {
            let (lo, hi) = match &self.nodes[ni as usize] {
                Node::Leaf { lo, hi, .. } | Node::Inner { lo, hi, .. } => (*lo, *hi),
            };
            if !slab(origin, inv, lo, hi, tmin, tmax) {
                continue;
            }
            match &self.nodes[ni as usize] {
                Node::Inner { right, .. } => {
                    stack.push(*right);
                    stack.push(ni + 1);
                }
                Node::Leaf { start, end, .. } => {
                    for t in &self.tris[*start as usize..*end as usize] {
                        if t.obj == skip_obj && skip_faces.contains(&t.face) {
                            continue;
                        }
                        // Back faces: the ray is inside the solid or leaving
                        // a shell from behind; never an occluder.
                        if dir.dot(t.n) >= 0.0 {
                            continue;
                        }
                        if let Some(hit) = moller_trumbore(t, origin, dir, tmin, tmax) {
                            tmax = hit;
                            best = Some(hit);
                        }
                    }
                }
            }
        }
        best
    }
}

fn slab(o: Vec3, inv: Vec3, lo: Vec3, hi: Vec3, tmin: f32, tmax: f32) -> bool {
    let t0 = (lo - o) * inv;
    let t1 = (hi - o) * inv;
    let near = t0.min(t1);
    let far = t0.max(t1);
    let enter = near.max_element().max(tmin);
    let exit = far.min_element().min(tmax);
    enter <= exit
}

fn moller_trumbore(t: &SceneTri, o: Vec3, d: Vec3, tmin: f32, tmax: f32) -> Option<f32> {
    let p = d.cross(t.e2);
    let det = t.e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - t.a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(t.e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let hit = t.e2.dot(q) * inv;
    (hit > tmin && hit < tmax).then_some(hit)
}

/// Cosine-weighted hemisphere directions around +Y from a Fibonacci spiral
/// on the unit disk (Malley's method): equal-weight samples, no RNG.
/// `phase` rotates the spiral so sibling sets don't line up.
fn fibonacci_hemisphere(samples: u32, phase: f32) -> Vec<Vec3> {
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    (0..samples)
        .map(|i| {
            let r = ((i as f32 + 0.5) / samples as f32).sqrt();
            let phi = i as f32 * golden + phase;
            let (s, c) = phi.sin_cos();
            Vec3::new(r * c, (1.0 - r * r).max(0.0).sqrt(), r * s)
        })
        .collect()
}

/// Orthonormal tangent frame `(t, b)` around `n`, chosen deterministically.
fn tangent_frame(n: Vec3) -> (Vec3, Vec3) {
    let helper = if n.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let t = helper.cross(n).normalize();
    (t, n.cross(t))
}

/// Ambient occlusion baked into the object's vertex colors.
///
/// Rays are traced against every non-LOD object in the doc through a BVH.
/// Each vertex shoots its `samples` budget split evenly over its adjacent
/// faces: per face, a cosine-weighted Fibonacci hemisphere around that
/// face's own normal from a point just inside the face. Sampling per face
/// (rather than around the smooth vertex normal) is what keeps hard convex
/// edges bright — no ray ever dips below its own face — while walls meeting
/// at a concave corner are hit at nearly zero distance and go dark. A hit
/// occludes by `1 - t / reach` (reach = 25% of the scene diagonal); back
/// faces never count. The vertex takes the mean over its faces of
/// `1 - occlusion`, and `lerp(1, ao, strength)` multiplies into the
/// existing color.
pub fn bake_ao(doc: &mut Doc, object: &str, samples: u32, strength: f32) -> Result<Diff, OpError> {
    if samples == 0 {
        return Err(OpError::BadParams("samples must be >= 1".into()));
    }
    check_finite("strength", strength, 0.0)?;
    let obj_index = doc
        .objects
        .keys()
        .position(|k| k == object)
        .ok_or_else(|| OpError::UnknownObject(object.into()))? as u32;

    let bvh = SceneBvh::build(doc);
    let reach = bvh.diag * AO_REACH;
    let eps = bvh.diag * AO_EPS;

    let mesh = &doc.objects[object].mesh;
    let face_frames: BTreeMap<FaceId, (Vec3, Vec3)> = mesh
        .faces
        .keys()
        .filter_map(|&f| {
            let n = mesh.face_normal(f).ok()?.try_normalize()?;
            Some((f, (n, mesh.face_centroid(f).ok()?)))
        })
        .collect();
    let mut baked: Vec<(VertId, f32)> = Vec::with_capacity(mesh.verts.len());
    for (&v, faces) in &mesh.vert_faces() {
        let faces: Vec<FaceId> = faces.iter().copied().filter(|f| face_frames.contains_key(f)).collect();
        if faces.is_empty() {
            continue;
        }
        let p = mesh.verts[&v];
        let per_face = samples.div_ceil(faces.len() as u32);
        let mut ao_sum = 0.0f32;
        for (fi, &f) in faces.iter().enumerate() {
            let (n, centroid) = face_frames[&f];
            let inset = (centroid - p).normalize_or_zero() * (eps * AO_INSET);
            let origin = p + inset + n * eps;
            let (t, b) = tangent_frame(n);
            let skip = BTreeSet::from([f]);
            let phase = fi as f32 / faces.len() as f32 * std::f32::consts::TAU;
            let mut occlusion = 0.0f32;
            for d in fibonacci_hemisphere(per_face, phase) {
                let dir = t * d.x + n * d.y + b * d.z;
                if let Some(hit) = bvh.nearest(origin, dir, eps, reach, obj_index, &skip) {
                    occlusion += 1.0 - hit / reach;
                }
            }
            ao_sum += 1.0 - occlusion / per_face as f32;
        }
        baked.push((v, ao_sum / faces.len() as f32));
    }

    let mesh = &mut doc.objects.get_mut(object).expect("checked above").mesh;
    let mut touched = Vec::with_capacity(baked.len());
    for (v, ao) in baked {
        let factor = 1.0 + (ao - 1.0) * strength;
        store(mesh, v, color_of(mesh, v) * factor);
        touched.push(v);
    }
    Ok(color_diff(object, &format!("ao x{samples}"), touched))
}

/// Directional light baked into vertex colors: half-lambert of the vertex
/// normal against `-dir` (so `dir` is where the light travels), tinted by
/// `color`, blended toward by `strength` and multiplied into the existing
/// color. No shadows — `bake_ao` handles occlusion.
pub fn bake_sun(
    doc: &mut Doc,
    object: &str,
    dir: [f32; 3],
    strength: f32,
    color: [f32; 3],
) -> Result<Diff, OpError> {
    check_finite("strength", strength, 0.0)?;
    let light = Vec3::from(dir);
    let light = -light
        .try_normalize()
        .ok_or_else(|| OpError::BadParams(format!("sun dir {dir:?} must be a non-zero vector")))?;
    let tint = Vec3::from(color);
    for (i, c) in color.iter().enumerate() {
        check_finite(&format!("color[{i}]"), *c, 0.0)?;
    }

    let mesh = &mut doc.object_mut(object)?.mesh;
    let normals = vertex_normals(mesh);
    let mut touched = Vec::with_capacity(normals.len());
    for (v, n) in normals {
        let half_lambert = n.dot(light) * 0.5 + 0.5;
        let lit = Vec3::ONE.lerp(tint * half_lambert, strength);
        store(mesh, v, color_of(mesh, v) * lit);
        touched.push(v);
    }
    Ok(color_diff(object, "sun", touched))
}

/// Cheap glow: every light object (one with an emissive material) adds
/// `emissive * emissive_strength * strength * falloff` to the verts of
/// `object` within `radius` of the light's bounds center, where `falloff =
/// (1 - d / radius)^2`. Additive, so it lifts colors that AO/sun darkened.
pub fn bake_glow(
    doc: &mut Doc,
    object: &str,
    lights: &[String],
    radius: f32,
    strength: f32,
) -> Result<Diff, OpError> {
    if !radius.is_finite() || radius <= 0.0 {
        return Err(OpError::BadParams(format!("radius {radius} must be finite and > 0")));
    }
    check_finite("strength", strength, 0.0)?;
    if lights.is_empty() {
        return Err(OpError::BadParams("bake_glow needs at least one light object".into()));
    }
    doc.object(object)?;

    let mut sources: Vec<(Vec3, Vec3)> = Vec::with_capacity(lights.len());
    for name in lights {
        let light = doc.object(name)?;
        let mat_name = light.material.as_deref().ok_or_else(|| {
            OpError::BadParams(format!("light object `{name}` has no material (needs an emissive one)"))
        })?;
        let mat = doc
            .materials
            .get(mat_name)
            .ok_or_else(|| OpError::UnknownMaterial(mat_name.to_owned()))?;
        let emissive = mat.emissive.ok_or_else(|| {
            OpError::BadParams(format!("light object `{name}` material `{mat_name}` is not emissive"))
        })?;
        let (lo, hi) = light
            .mesh
            .bounds()
            .ok_or_else(|| OpError::BadParams(format!("light object `{name}` has no vertices")))?;
        sources.push(((lo + hi) * 0.5, Vec3::from(emissive) * mat.emissive_strength * strength));
    }

    let mesh = &mut doc.objects.get_mut(object).expect("checked above").mesh;
    let verts: Vec<(VertId, Vec3)> = mesh.verts.iter().map(|(&v, &p)| (v, p)).collect();
    let mut touched = Vec::new();
    for (v, p) in verts {
        let mut glow = Vec3::ZERO;
        for &(center, energy) in &sources {
            let d = (p - center).length();
            if d < radius {
                let f = 1.0 - d / radius;
                glow += energy * (f * f);
            }
        }
        if glow != Vec3::ZERO {
            store(mesh, v, color_of(mesh, v) + glow);
            touched.push(v);
        }
    }
    Ok(color_diff(object, &format!("glow from {} lights", sources.len()), touched))
}

/// Blend the selection's vertex colors toward `color` by `strength`.
///
/// With `facing`, only verts touching a face whose normal lies within
/// `max_angle_deg` of `facing` are painted: for face selections that means
/// the selected faces, for vert/edge selections any face around the vert.
pub fn paint_vertex(
    doc: &mut Doc,
    sel: &Selection,
    color: [f32; 3],
    strength: f32,
    facing: Option<[f32; 3]>,
    max_angle_deg: f32,
) -> Result<Diff, OpError> {
    check_finite("strength", strength, 0.0)?;
    check_finite("max_angle_deg", max_angle_deg, 0.0)?;
    for (i, c) in color.iter().enumerate() {
        check_finite(&format!("color[{i}]"), *c, 0.0)?;
    }
    let facing = match facing {
        None => None,
        Some(f) => Some(Vec3::from(f).try_normalize().ok_or_else(|| {
            OpError::BadParams(format!("facing {f:?} must be a non-zero vector"))
        })?),
    };

    let object = sel.object.clone();
    let mesh = &mut doc.object_mut(&object)?.mesh;
    let mut verts = select::to_verts(mesh, sel)?;
    if verts.is_empty() {
        return Err(OpError::BadParams("empty selection".into()));
    }

    if let Some(facing) = facing {
        let cos_max = max_angle_deg.to_radians().cos();
        let passes = |mesh: &Mesh, f: FaceId| mesh.face_normal(f).map_or(false, |n| n.dot(facing) >= cos_max);
        let candidates: BTreeMap<FaceId, Vec<VertId>> = match &sel.elems {
            Elems::Faces(faces) => faces.iter().map(|&f| (f, mesh.faces[&f].verts().collect())).collect(),
            _ => mesh
                .vert_faces()
                .into_iter()
                .filter(|(v, _)| verts.contains(v))
                .flat_map(|(v, faces)| faces.into_iter().map(move |f| (f, v)))
                .fold(BTreeMap::new(), |mut acc: BTreeMap<FaceId, Vec<VertId>>, (f, v)| {
                    acc.entry(f).or_default().push(v);
                    acc
                }),
        };
        let mut kept = BTreeSet::new();
        for (f, vs) in candidates {
            if passes(mesh, f) {
                kept.extend(vs);
            }
        }
        verts = kept;
    }

    let target = Vec3::from(color);
    let mut touched = Vec::with_capacity(verts.len());
    for v in verts {
        store(mesh, v, color_of(mesh, v).lerp(target, strength));
        touched.push(v);
    }
    Ok(color_diff(&object, "paint", touched))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{AlphaMode, Material, Object, TextureRef};
    use dgm_atlas::AssetClass;
    use dgm_mesh::primitives::{plane, prim_box};

    fn lum(c: [f32; 3]) -> f32 {
        (c[0] + c[1] + c[2]) / 3.0
    }

    /// Unit box turned inside out: a room whose corners are concave.
    fn room() -> Mesh {
        let mut m = prim_box(Vec3::splat(2.0)).unwrap();
        for face in m.faces.values_mut() {
            face.corners.reverse();
        }
        m
    }

    #[test]
    fn ao_darkens_room_corners_but_not_an_open_plane() {
        let mut doc = Doc::new(AssetClass::Environment);
        doc.objects.insert("room".into(), Object::new(room()));
        // A large open plane far above the room: nothing in its hemisphere.
        let mut sky = plane(4.0, 4.0).unwrap();
        let up: BTreeSet<VertId> = sky.verts.keys().copied().collect();
        sky.transform_verts(&up, &glam::Mat4::from_translation(Vec3::new(0.0, 40.0, 0.0)));
        doc.objects.insert("sky".into(), Object::new(sky));

        bake_ao(&mut doc, "room", 32, 1.0).unwrap();
        bake_ao(&mut doc, "sky", 32, 1.0).unwrap();

        let room = &doc.objects["room"].mesh;
        let sky = &doc.objects["sky"].mesh;
        assert_eq!(room.colors.len(), room.verts.len());
        assert_eq!(sky.colors.len(), sky.verts.len());
        let corner = room.colors.values().map(|&c| lum(c)).fold(0.0f32, f32::max);
        let open = sky.colors.values().map(|&c| lum(c)).fold(1.0f32, f32::min);
        assert!(open > 0.99, "open plane stays white, got {open}");
        assert!(corner < 0.6, "room corners must be occluded, brightest corner {corner}");
    }

    #[test]
    fn ao_leaves_convex_box_corners_bright_and_composes_multiplicatively() {
        let mut doc = Doc::new(AssetClass::Prop);
        doc.objects.insert("box".into(), Object::new(prim_box(Vec3::ONE).unwrap()));
        bake_ao(&mut doc, "box", 32, 1.0).unwrap();
        let darkest = doc.objects["box"].mesh.colors.values().map(|&c| lum(c)).fold(1.0f32, f32::min);
        assert!(darkest > 0.95, "convex corners must not self-occlude, got {darkest}");

        // Pre-tinted colors are multiplied, not replaced.
        let mut doc = Doc::new(AssetClass::Prop);
        doc.objects.insert("room".into(), Object::new(room()));
        let v0 = *doc.objects["room"].mesh.verts.keys().next().unwrap();
        doc.objects.get_mut("room").unwrap().mesh.colors.insert(v0, [0.5, 1.0, 1.0]);
        bake_ao(&mut doc, "room", 16, 1.0).unwrap();
        let c = doc.objects["room"].mesh.colors[&v0];
        assert!((c[0] * 2.0 - c[1]).abs() < 1e-5, "red channel keeps its half tint: {c:?}");

        // strength 0 is a no-op on values.
        let mut doc = Doc::new(AssetClass::Prop);
        doc.objects.insert("room".into(), Object::new(room()));
        bake_ao(&mut doc, "room", 8, 0.0).unwrap();
        assert!(doc.objects["room"].mesh.colors.values().all(|c| *c == [1.0; 3]));
    }

    #[test]
    fn ao_is_deterministic_and_rejects_bad_params() {
        let mut a = Doc::new(AssetClass::Prop);
        a.objects.insert("room".into(), Object::new(room()));
        let mut b = a.clone();
        bake_ao(&mut a, "room", 24, 0.8).unwrap();
        bake_ao(&mut b, "room", 24, 0.8).unwrap();
        assert_eq!(a.objects["room"].mesh.colors, b.objects["room"].mesh.colors);
        assert!(matches!(bake_ao(&mut a, "room", 0, 1.0), Err(OpError::BadParams(_))));
        assert!(matches!(bake_ao(&mut a, "nope", 8, 1.0), Err(OpError::UnknownObject(_))));
    }

    #[test]
    fn sun_orders_up_facing_above_down_facing() {
        let mut doc = Doc::new(AssetClass::Prop);
        doc.objects.insert("up".into(), Object::new(plane(1.0, 1.0).unwrap()));
        let mut down = plane(1.0, 1.0).unwrap();
        for face in down.faces.values_mut() {
            face.corners.reverse();
        }
        doc.objects.insert("down".into(), Object::new(down));
        let dir = [0.0, -1.0, 0.0];
        bake_sun(&mut doc, "up", dir, 0.5, [1.0, 0.95, 0.85]).unwrap();
        bake_sun(&mut doc, "down", dir, 0.5, [1.0, 0.95, 0.85]).unwrap();
        let up = lum(*doc.objects["up"].mesh.colors.values().next().unwrap());
        let down = lum(*doc.objects["down"].mesh.colors.values().next().unwrap());
        assert!(up > down, "sunlit top {up} must beat shaded bottom {down}");
        // Half-lambert: the shaded side sits at lerp(1, 0, 0.5) = 0.5.
        assert!((down - 0.5).abs() < 1e-5, "down {down}");
        assert!(matches!(bake_sun(&mut doc, "up", [0.0; 3], 0.5, [1.0; 3]), Err(OpError::BadParams(_))));
    }

    #[test]
    fn glow_needs_an_emissive_light_and_falls_off_with_distance() {
        let mut doc = Doc::new(AssetClass::Environment);
        let mut floor = plane(10.0, 10.0).unwrap();
        // Add a center vert so the falloff has something to grade across.
        let c = floor.add_vert(Vec3::ZERO);
        let ring: Vec<VertId> = floor.faces.values().next().unwrap().verts().collect();
        let f0 = *floor.faces.keys().next().unwrap();
        floor.remove_face(f0).unwrap();
        for i in 0..ring.len() {
            floor.add_face(&[c, ring[i], ring[(i + 1) % ring.len()]]).unwrap();
        }
        doc.objects.insert("floor".into(), Object::new(floor));
        let mut crystal = Object::new(prim_box(Vec3::splat(0.2)).unwrap());
        crystal.material = Some("glow".into());
        doc.objects.insert("crystal".into(), crystal);

        let err = bake_glow(&mut doc, "floor", &["crystal".into()], 3.0, 1.0);
        assert!(matches!(err, Err(OpError::UnknownMaterial(_))), "{err:?}");
        doc.materials.insert(
            "glow".into(),
            Material {
                texture: TextureRef::Color { rgba: [255; 4] },
                alpha: AlphaMode::Opaque,
                double_sided: false,
                emissive: None,
                emissive_strength: 1.0,
            },
        );
        let err = bake_glow(&mut doc, "floor", &["crystal".into()], 3.0, 1.0);
        assert!(matches!(err, Err(OpError::BadParams(_))), "{err:?}");
        doc.materials.get_mut("glow").unwrap().emissive = Some([0.2, 0.6, 1.0]);

        // Darken first so additive glow is visible.
        let verts: Vec<VertId> = doc.objects["floor"].mesh.verts.keys().copied().collect();
        for &v in &verts {
            doc.objects.get_mut("floor").unwrap().mesh.colors.insert(v, [0.2; 3]);
        }
        bake_glow(&mut doc, "floor", &["crystal".into()], 3.0, 1.0).unwrap();
        let colors = &doc.objects["floor"].mesh.colors;
        let center = colors[&c];
        assert!((center[2] - 1.0).abs() < 1e-5 && (center[0] - 0.4).abs() < 1e-5, "center {center:?}");
        for v in ring {
            assert_eq!(colors[&v], [0.2; 3], "corners 7 m away sit outside the 3 m radius");
        }
    }

    #[test]
    fn paint_vertex_facing_filter_keeps_only_matching_faces() {
        let mut doc = Doc::new(AssetClass::Prop);
        doc.objects.insert("box".into(), Object::new(prim_box(Vec3::ONE).unwrap()));
        let all_faces: BTreeSet<FaceId> = doc.objects["box"].mesh.faces.keys().copied().collect();
        let sel = Selection { object: "box".into(), elems: Elems::Faces(all_faces) };
        let moss = [0.2, 0.8, 0.3];
        paint_vertex(&mut doc, &sel, moss, 1.0, Some([0.0, 1.0, 0.0]), 45.0).unwrap();
        let mesh = &doc.objects["box"].mesh;
        assert_eq!(mesh.colors.len(), 4, "only the top face's verts get moss");
        for (v, c) in &mesh.colors {
            assert!(mesh.verts[v].y > 0.0, "{v} is not on top");
            assert_eq!(*c, moss);
        }

        // Vert selection with facing: any adjacent face may qualify; a
        // bottom vert touches only side/bottom faces and is skipped.
        let mut doc = Doc::new(AssetClass::Prop);
        doc.objects.insert("box".into(), Object::new(prim_box(Vec3::ONE).unwrap()));
        let all_verts: BTreeSet<VertId> = doc.objects["box"].mesh.verts.keys().copied().collect();
        let sel = Selection { object: "box".into(), elems: Elems::Verts(all_verts) };
        paint_vertex(&mut doc, &sel, moss, 0.5, Some([0.0, 1.0, 0.0]), 45.0).unwrap();
        let mesh = &doc.objects["box"].mesh;
        assert_eq!(mesh.colors.len(), 4);
        let c = *mesh.colors.values().next().unwrap();
        assert!((c[1] - 0.9).abs() < 1e-5, "half blend toward moss from white: {c:?}");

        // No facing: every selected vert.
        let sel = Selection { object: "box".into(), elems: Elems::Verts(mesh.verts.keys().copied().collect()) };
        paint_vertex(&mut doc, &sel, [0.0; 3], 1.0, None, 45.0).unwrap();
        assert_eq!(doc.objects["box"].mesh.colors.len(), 8);
    }
}
