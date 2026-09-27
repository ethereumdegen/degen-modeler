//! Organic ops: Catmull-Clark subdivision, seeded noise displacement,
//! Laplacian smoothing, solidify (shell -> thick volume) and snap-to-surface.
//!
//! Everything here is deterministic: the noise is an integer hash over
//! lattice coordinates and the seed, never an RNG, and every collection that
//! can leak into ids or output iterates in `BTree` order.

use std::collections::{BTreeMap, BTreeSet};

use glam::{Vec2, Vec3};

use super::topo::newell;
use crate::ids::{EdgeKey, FaceId, VertId};
use crate::mesh::{Corner, Mesh, MeshDelta, MeshError};
use crate::validate::{Severity, validate_mesh};

/// Subdivision growth is 4x faces per level; six levels of a 12k-tri
/// environment is already absurd, so refuse anything larger up front.
const MAX_SUBDIVIDE_LEVELS: u32 = 6;
/// Barycentric slack so a ray landing on a shared triangle edge still hits.
const HIT_EPS: f32 = 1e-6;

/// Area-weighted vertex normals: the normalized sum of the unnormalized
/// Newell normals (length = 2x area) of every face around the vertex. Verts
/// without faces get zero.
pub fn vertex_normals(mesh: &Mesh) -> Result<BTreeMap<VertId, Vec3>, MeshError> {
    let mut acc: BTreeMap<VertId, Vec3> = mesh.verts.keys().map(|&v| (v, Vec3::ZERO)).collect();
    for (&fid, face) in &mesh.faces {
        let n = newell(mesh, fid)?;
        for v in face.verts() {
            *acc.entry(v).or_insert(Vec3::ZERO) += n;
        }
    }
    Ok(acc.into_iter().map(|(v, n)| (v, n.normalize_or_zero())).collect())
}

fn check_verts(mesh: &Mesh, verts: &BTreeSet<VertId>, op: &str) -> Result<(), MeshError> {
    if verts.is_empty() {
        return Err(MeshError::Invalid(format!("{op}: no vertices selected")));
    }
    for &v in verts {
        mesh.pos(v)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// subdivide
// ---------------------------------------------------------------------------

/// Catmull-Clark subdivision of every face (any polygon: an n-gon becomes n
/// quads). `smooth = false` keeps every point where the linear midpoint
/// split puts it. Seams split onto their two sub-edges; UVs and vertex
/// colors are interpolated; original vertex ids survive (smoothing only
/// moves them).
pub fn subdivide(mesh: &mut Mesh, levels: u32, smooth: bool) -> Result<MeshDelta, MeshError> {
    if levels == 0 || levels > MAX_SUBDIVIDE_LEVELS {
        return Err(MeshError::Invalid(format!(
            "subdivide: levels must be 1..={MAX_SUBDIVIDE_LEVELS}, got {levels}"
        )));
    }
    if mesh.faces.is_empty() {
        return Err(MeshError::Invalid("subdivide: mesh has no faces".into()));
    }
    let mut created_verts = Vec::new();
    let mut created_faces = BTreeSet::new();
    let mut removed_faces = BTreeSet::new();
    let mut moved = BTreeSet::new();
    for _ in 0..levels {
        subdivide_once(mesh, smooth, &mut created_verts, &mut created_faces, &mut removed_faces, &mut moved)?;
    }
    Ok(MeshDelta {
        created_verts,
        created_faces: created_faces.into_iter().collect(),
        removed_faces: removed_faces.into_iter().collect(),
        moved_verts: moved.into_iter().collect(),
        ..MeshDelta::default()
    })
}

fn color_of(mesh: &Mesh, v: VertId) -> Vec3 {
    mesh.colors.get(&v).map(|c| Vec3::from(*c)).unwrap_or(Vec3::ONE)
}

fn subdivide_once(
    mesh: &mut Mesh,
    smooth: bool,
    created_verts: &mut Vec<VertId>,
    created_faces: &mut BTreeSet<FaceId>,
    removed_faces: &mut BTreeSet<FaceId>,
    moved: &mut BTreeSet<VertId>,
) -> Result<(), MeshError> {
    let edge_faces = mesh.edge_faces();
    let has_colors = !mesh.colors.is_empty();

    let mut centroid: BTreeMap<FaceId, Vec3> = BTreeMap::new();
    for &f in mesh.faces.keys() {
        centroid.insert(f, mesh.face_centroid(f)?);
    }

    // Edge points: interior edges average the endpoints with both face
    // points; boundary (and non-manifold) edges use the plain midpoint.
    let mut edge_pos: BTreeMap<EdgeKey, Vec3> = BTreeMap::new();
    for (e, fs) in &edge_faces {
        let (a, b) = (mesh.pos(e.0)?, mesh.pos(e.1)?);
        let p = if smooth && fs.len() == 2 {
            (a + b + centroid[&fs[0]] + centroid[&fs[1]]) * 0.25
        } else {
            (a + b) * 0.5
        };
        edge_pos.insert(*e, p);
    }

    // Smoothed originals, computed entirely from pre-split positions.
    let mut new_pos: BTreeMap<VertId, Vec3> = BTreeMap::new();
    if smooth {
        let vert_faces = mesh.vert_faces();
        let mut vert_edges: BTreeMap<VertId, Vec<EdgeKey>> = BTreeMap::new();
        for e in edge_faces.keys() {
            vert_edges.entry(e.0).or_default().push(*e);
            vert_edges.entry(e.1).or_default().push(*e);
        }
        for (&v, edges) in &vert_edges {
            let p = mesh.pos(v)?;
            let boundary: Vec<VertId> = edges
                .iter()
                .filter(|e| edge_faces[e].len() == 1)
                .map(|e| if e.0 == v { e.1 } else { e.0 })
                .collect();
            let q = match boundary.len() {
                0 => {
                    let n = edges.len() as f32;
                    let fs = &vert_faces[&v];
                    let f_avg = fs.iter().map(|f| centroid[f]).sum::<Vec3>() / fs.len() as f32;
                    let mut r_sum = Vec3::ZERO;
                    for e in edges {
                        r_sum += (mesh.pos(e.0)? + mesh.pos(e.1)?) * 0.5;
                    }
                    let r_avg = r_sum / n;
                    (f_avg + 2.0 * r_avg + (n - 3.0) * p) / n
                }
                2 => (6.0 * p + mesh.pos(boundary[0])? + mesh.pos(boundary[1])?) / 8.0,
                // Pinched / non-manifold boundary vertex: leave it alone.
                _ => p,
            };
            if q != p {
                new_pos.insert(v, q);
            }
        }
    }

    let old_faces: Vec<FaceId> = mesh.faces.keys().copied().collect();
    let mut face_vert: BTreeMap<FaceId, VertId> = BTreeMap::new();
    for &f in &old_faces {
        let nv = mesh.add_vert(centroid[&f]);
        if has_colors {
            let face = &mesh.faces[&f];
            let c = face.verts().map(|v| color_of(mesh, v)).sum::<Vec3>() / face.corners.len() as f32;
            mesh.colors.insert(nv, c.to_array());
        }
        face_vert.insert(f, nv);
        created_verts.push(nv);
    }
    let mut edge_vert: BTreeMap<EdgeKey, VertId> = BTreeMap::new();
    for (e, p) in &edge_pos {
        let nv = mesh.add_vert(*p);
        if has_colors {
            let c = (color_of(mesh, e.0) + color_of(mesh, e.1)) * 0.5;
            mesh.colors.insert(nv, c.to_array());
        }
        edge_vert.insert(*e, nv);
        created_verts.push(nv);
    }

    // One quad per corner: corner -> next edge point -> face point -> previous
    // edge point, which keeps the parent's winding.
    for &f in &old_faces {
        let face = mesh.remove_face(f)?;
        let k = face.corners.len();
        let fv = face_vert[&f];
        let uv_c = face.corners.iter().map(|c| c.uv).sum::<Vec2>() / k as f32;
        for i in 0..k {
            let prev = face.corners[(i + k - 1) % k];
            let cur = face.corners[i];
            let next = face.corners[(i + 1) % k];
            let e_next = edge_vert[&EdgeKey::new(cur.vert, next.vert)];
            let e_prev = edge_vert[&EdgeKey::new(prev.vert, cur.vert)];
            let nf = mesh.add_face_uv(vec![
                Corner { vert: cur.vert, uv: cur.uv },
                Corner { vert: e_next, uv: (cur.uv + next.uv) * 0.5 },
                Corner { vert: fv, uv: uv_c },
                Corner { vert: e_prev, uv: (prev.uv + cur.uv) * 0.5 },
            ])?;
            created_faces.insert(nf);
        }
        if !created_faces.remove(&f) {
            removed_faces.insert(f);
        }
    }

    let old_seams = std::mem::take(&mut mesh.seams);
    for s in old_seams {
        if let Some(&ev) = edge_vert.get(&s) {
            mesh.seams.insert(EdgeKey::new(s.0, ev));
            mesh.seams.insert(EdgeKey::new(ev, s.1));
        }
    }

    for (v, p) in new_pos {
        mesh.set_pos(v, p)?;
        moved.insert(v);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// displace_noise
// ---------------------------------------------------------------------------

/// splitmix-style integer hash of a lattice point and the seed -> [0, 1).
fn lattice_hash(x: i32, y: i32, z: i32, seed: u64) -> f32 {
    let mut h = seed ^ 0x9E37_79B9_7F4A_7C15;
    for c in [x, y, z] {
        h ^= c as u32 as u64;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 31;
    }
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 29;
    ((h >> 40) as u32) as f32 / (1u32 << 24) as f32
}

/// Trilinear value noise with a smoothstep fade, in [0, 1].
fn value_noise(p: Vec3, seed: u64) -> f32 {
    let f = p.floor();
    let i = f.as_ivec3();
    let t = p - f;
    let s = t * t * (Vec3::splat(3.0) - 2.0 * t);
    let corner = |dx: i32, dy: i32, dz: i32| lattice_hash(i.x + dx, i.y + dy, i.z + dz, seed);
    let lerp = |a: f32, b: f32, w: f32| a + (b - a) * w;
    let x00 = lerp(corner(0, 0, 0), corner(1, 0, 0), s.x);
    let x10 = lerp(corner(0, 1, 0), corner(1, 1, 0), s.x);
    let x01 = lerp(corner(0, 0, 1), corner(1, 0, 1), s.x);
    let x11 = lerp(corner(0, 1, 1), corner(1, 1, 1), s.x);
    let y0 = lerp(x00, x10, s.y);
    let y1 = lerp(x01, x11, s.y);
    lerp(y0, y1, s.z)
}

/// Three-octave fractal value noise remapped to [-1, 1]. Each octave hashes
/// with its own seed so the layers do not simply rescale each other.
pub fn fbm(p: Vec3, seed: u64) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for octave in 0..3u64 {
        sum += amp * value_noise(p * freq, seed.wrapping_add(octave.wrapping_mul(0x51_7C_C1_B7_27_22_0A_95)));
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm * 2.0 - 1.0
}

/// Move each selected vertex along its area-weighted normal by
/// `amplitude * fbm(pos / scale, seed)`. Normals come from the pre-move
/// positions, so the result does not depend on iteration order.
pub fn displace_noise(
    mesh: &mut Mesh,
    verts: &BTreeSet<VertId>,
    amplitude: f32,
    scale: f32,
    seed: u64,
) -> Result<MeshDelta, MeshError> {
    check_verts(mesh, verts, "displace_noise")?;
    if !(scale > 0.0) || !scale.is_finite() {
        return Err(MeshError::Invalid(format!("displace_noise: scale must be > 0, got {scale}")));
    }
    if !amplitude.is_finite() {
        return Err(MeshError::Invalid(format!("displace_noise: amplitude must be finite, got {amplitude}")));
    }
    let normals = vertex_normals(mesh)?;
    let mut delta = MeshDelta::default();
    for &v in verts {
        let p = mesh.pos(v)?;
        let d = normals[&v] * (amplitude * fbm(p / scale, seed));
        if d != Vec3::ZERO {
            mesh.set_pos(v, p + d)?;
            delta.moved_verts.push(v);
        }
    }
    Ok(delta)
}

// ---------------------------------------------------------------------------
// smooth
// ---------------------------------------------------------------------------

/// Laplacian smoothing: each selected interior vertex moves `factor` of the
/// way toward the centroid of its edge neighbours, `iterations` times
/// (Jacobi: every step reads the previous step's positions). Boundary
/// vertices are pinned so open rims and silhouettes keep their shape.
pub fn smooth(mesh: &mut Mesh, verts: &BTreeSet<VertId>, iterations: u32, factor: f32) -> Result<MeshDelta, MeshError> {
    check_verts(mesh, verts, "smooth")?;
    if iterations == 0 {
        return Err(MeshError::Invalid("smooth: iterations must be >= 1".into()));
    }
    if !(0.0..=1.0).contains(&factor) {
        return Err(MeshError::Invalid(format!("smooth: factor must be within 0..1, got {factor}")));
    }
    let mut adj: BTreeMap<VertId, Vec<VertId>> = BTreeMap::new();
    let mut boundary: BTreeSet<VertId> = BTreeSet::new();
    for (e, fs) in mesh.edge_faces() {
        adj.entry(e.0).or_default().push(e.1);
        adj.entry(e.1).or_default().push(e.0);
        if fs.len() == 1 {
            boundary.insert(e.0);
            boundary.insert(e.1);
        }
    }
    let movable: Vec<VertId> =
        verts.iter().copied().filter(|v| !boundary.contains(v) && adj.contains_key(v)).collect();
    let start: BTreeMap<VertId, Vec3> = movable.iter().map(|&v| (v, mesh.verts[&v])).collect();
    for _ in 0..iterations {
        let mut next = Vec::with_capacity(movable.len());
        for &v in &movable {
            let ns = &adj[&v];
            let c = ns.iter().map(|n| mesh.verts[n]).sum::<Vec3>() / ns.len() as f32;
            next.push((v, mesh.verts[&v].lerp(c, factor)));
        }
        for (v, p) in next {
            mesh.set_pos(v, p)?;
        }
    }
    let mut delta = MeshDelta::default();
    for (v, p) in start {
        if mesh.verts[&v] != p {
            delta.moved_verts.push(v);
        }
    }
    Ok(delta)
}

// ---------------------------------------------------------------------------
// solidify
// ---------------------------------------------------------------------------

/// Directed boundary chains in face-winding order (`a -> b` means the lone
/// face runs a then b). Closed loops repeat their start vertex at the end;
/// a chain that dead-ends (non-manifold input) is returned as-is.
fn boundary_chains(mesh: &Mesh) -> Vec<Vec<VertId>> {
    let edge_faces = mesh.edge_faces();
    let mut next: BTreeMap<VertId, Vec<VertId>> = BTreeMap::new();
    for face in mesh.faces.values() {
        let k = face.corners.len();
        for i in 0..k {
            let (a, b) = (face.corners[i].vert, face.corners[(i + 1) % k].vert);
            if edge_faces[&EdgeKey::new(a, b)].len() == 1 {
                next.entry(a).or_default().push(b);
            }
        }
    }
    for outs in next.values_mut() {
        outs.sort();
    }
    let mut chains = Vec::new();
    while let Some((&start, _)) = next.iter().next() {
        let mut chain = vec![start];
        let mut cur = start;
        loop {
            let Some(outs) = next.get_mut(&cur) else { break };
            let to = outs.remove(0);
            if outs.is_empty() {
                next.remove(&cur);
            }
            chain.push(to);
            if to == start {
                break;
            }
            cur = to;
        }
        chains.push(chain);
    }
    chains
}

/// Give an open shell thickness: a flipped copy of every face offset along
/// the vertex normals, stitched to the original along its boundary with rim
/// quads. `thickness > 0` grows inward (the copy is the inner skin); a
/// negative thickness pushes the original surface outward instead and keeps
/// the copy where the shell was, so normals face out either way. UVs, seams
/// and colors are copied onto the inner skin; rim quads unroll by boundary
/// arc length x thickness and are seamed off as their own island. The
/// result must pass `validate_mesh` with no hard finding or the mesh is left
/// untouched.
pub fn solidify(mesh: &mut Mesh, thickness: f32) -> Result<MeshDelta, MeshError> {
    if !thickness.is_finite() || thickness == 0.0 {
        return Err(MeshError::Invalid(format!("solidify: thickness must be non-zero, got {thickness}")));
    }
    if mesh.faces.is_empty() {
        return Err(MeshError::Invalid("solidify: mesh has no faces".into()));
    }
    let normals = vertex_normals(mesh)?;
    let inward = thickness > 0.0;
    let mut out = mesh.clone();
    let mut delta = MeshDelta::default();
    let mut map: BTreeMap<VertId, VertId> = BTreeMap::new();
    for (&v, &p) in &mesh.verts {
        let shifted = p - normals[&v] * thickness;
        let inner = if inward {
            shifted
        } else {
            out.set_pos(v, shifted)?;
            delta.moved_verts.push(v);
            p
        };
        let nv = out.add_vert(inner);
        if let Some(c) = mesh.colors.get(&v) {
            out.colors.insert(nv, *c);
        }
        map.insert(v, nv);
        delta.created_verts.push(nv);
    }
    for face in mesh.faces.values() {
        let corners = face.corners.iter().rev().map(|c| Corner { vert: map[&c.vert], uv: c.uv }).collect();
        delta.created_faces.push(out.add_face_uv(corners)?);
    }
    for s in &mesh.seams {
        out.seams.insert(EdgeKey::new(map[&s.0], map[&s.1]));
    }

    let depth = thickness.abs();
    for chain in boundary_chains(mesh) {
        let mut u = 0.0f32;
        for w in chain.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (ia, ib) = (map[&a], map[&b]);
            let u_next = u + (out.verts[&b] - out.verts[&a]).length();
            // b, a, a', b' faces away from the shell for either sign.
            let fid = out.add_face_uv(vec![
                Corner { vert: b, uv: Vec2::new(u_next, 0.0) },
                Corner { vert: a, uv: Vec2::new(u, 0.0) },
                Corner { vert: ia, uv: Vec2::new(u, depth) },
                Corner { vert: ib, uv: Vec2::new(u_next, depth) },
            ])?;
            delta.created_faces.push(fid);
            out.seams.insert(EdgeKey::new(a, b));
            out.seams.insert(EdgeKey::new(ia, ib));
            u = u_next;
        }
    }

    if let Some(hard) = validate_mesh(&out).into_iter().find(|f| f.severity == Severity::Hard) {
        return Err(MeshError::Invalid(format!("solidify: result is not a clean manifold: {}", hard.message)));
    }
    *mesh = out;
    Ok(delta)
}

// ---------------------------------------------------------------------------
// snap_to_surface
// ---------------------------------------------------------------------------

/// Moller-Trumbore; returns the signed ray parameter of a hit on the
/// (slightly fattened) triangle.
fn ray_tri(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(-HIT_EPS..=1.0 + HIT_EPS).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < -HIT_EPS || u + v > 1.0 + HIT_EPS {
        return None;
    }
    Some(e2.dot(q) * inv)
}

/// Drop each selected vertex onto `target`: cast along `+dir` and `-dir`
/// against every target triangle and move to the nearest hit. Vertices that
/// miss stay put; it is an error only when nothing hits (the message lists
/// the misses).
pub fn snap_to_surface(
    mesh: &mut Mesh,
    verts: &BTreeSet<VertId>,
    target: &Mesh,
    dir: Vec3,
) -> Result<MeshDelta, MeshError> {
    check_verts(mesh, verts, "snap_to_surface")?;
    if !dir.is_finite() || dir.length_squared() <= 0.0 {
        return Err(MeshError::Invalid(format!("snap_to_surface: dir must be a non-zero vector, got {dir}")));
    }
    if target.faces.is_empty() {
        return Err(MeshError::Invalid("snap_to_surface: target has no faces".into()));
    }
    let d = dir.normalize();
    let tris: Vec<[Vec3; 3]> = target
        .triangulate()
        .into_iter()
        .map(|t| {
            let face = &target.faces[&t.face];
            t.corner_idx.map(|i| target.verts[&face.corners[i as usize].vert])
        })
        .collect();
    let mut delta = MeshDelta::default();
    let mut missed = Vec::new();
    for &v in verts {
        let o = mesh.pos(v)?;
        let mut best: Option<f32> = None;
        for tri in &tris {
            if let Some(t) = ray_tri(o, d, tri[0], tri[1], tri[2]) {
                if best.is_none_or(|b| t.abs() < b.abs()) {
                    best = Some(t);
                }
            }
        }
        match best {
            Some(t) => {
                let p = o + d * t;
                if p != o {
                    mesh.set_pos(v, p)?;
                    delta.moved_verts.push(v);
                }
            }
            None => missed.push(v.to_string()),
        }
    }
    if missed.len() == verts.len() {
        return Err(MeshError::Invalid(format!(
            "snap_to_surface: no ray along +-{d} from {} hits the target",
            missed.join(", ")
        )));
    }
    Ok(delta)
}
