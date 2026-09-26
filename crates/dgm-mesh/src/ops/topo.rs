//! Topology ops: extrude, inset, bevel, loop cut, bridge, merge, dissolve,
//! mirror, decimate. All take resolved element sets, mutate the mesh in
//! place, and report what happened as a [`MeshDelta`].

use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use glam::{DMat3, DVec3, Vec3};

use crate::ids::{EdgeKey, FaceId, VertId};
use crate::mesh::{Corner, Face, Mesh, MeshDelta, MeshError};

/// Unnormalized Newell normal (length = 2x area), for area-weighted averages.
fn newell(mesh: &Mesh, f: FaceId) -> Result<Vec3, MeshError> {
    let face = mesh.face(f)?;
    let k = face.corners.len();
    let mut n = Vec3::ZERO;
    for i in 0..k {
        let a = mesh.pos(face.corners[i].vert)?;
        let b = mesh.pos(face.corners[(i + 1) % k].vert)?;
        n += Vec3::new(
            (a.y - b.y) * (a.z + b.z),
            (a.z - b.z) * (a.x + b.x),
            (a.x - b.x) * (a.y + b.y),
        );
    }
    Ok(n)
}

/// Keep seams whose edges survived; re-point the rest through `map` (vertex
/// old -> new); drop marks whose mapped edge is degenerate or gone.
fn remap_seams(mesh: &mut Mesh, map: &BTreeMap<VertId, VertId>) {
    let edges = mesh.edges();
    let old = std::mem::take(&mut mesh.seams);
    for e in old {
        if edges.contains(&e) {
            mesh.seams.insert(e);
            continue;
        }
        let a = *map.get(&e.0).unwrap_or(&e.0);
        let b = *map.get(&e.1).unwrap_or(&e.1);
        if a != b {
            let k = EdgeKey::new(a, b);
            if edges.contains(&k) {
                mesh.seams.insert(k);
            }
        }
    }
}

/// Extrude `faces` as one patch along their area-weighted average normal by
/// `offset`. The patch keeps its face ids and UVs; side quads are stitched
/// along the patch boundary only.
pub fn extrude(mesh: &mut Mesh, faces: &BTreeSet<FaceId>, offset: f32) -> Result<MeshDelta, MeshError> {
    if faces.is_empty() {
        return Err(MeshError::Invalid("extrude: no faces selected".into()));
    }
    let mut n_sum = Vec3::ZERO;
    for &f in faces {
        n_sum += newell(mesh, f)?;
    }
    let n = n_sum.normalize_or_zero();
    if n == Vec3::ZERO {
        return Err(MeshError::Invalid(
            "extrude: selected faces have no usable average normal".into(),
        ));
    }
    let mut edge_count: BTreeMap<EdgeKey, u32> = BTreeMap::new();
    let mut directed: BTreeMap<EdgeKey, (VertId, VertId)> = BTreeMap::new();
    let mut patch_verts: BTreeSet<VertId> = BTreeSet::new();
    for &f in faces {
        let face = mesh.face(f)?;
        let k = face.corners.len();
        for i in 0..k {
            let a = face.corners[i].vert;
            let b = face.corners[(i + 1) % k].vert;
            patch_verts.insert(a);
            let key = EdgeKey::new(a, b);
            *edge_count.entry(key).or_insert(0) += 1;
            directed.insert(key, (a, b));
        }
    }
    let mut delta = MeshDelta::default();
    let mut dup: BTreeMap<VertId, VertId> = BTreeMap::new();
    for &v in &patch_verts {
        let p = mesh.pos(v)?;
        let nv = mesh.add_vert(p + n * offset);
        dup.insert(v, nv);
        delta.created_verts.push(nv);
    }
    for &f in faces {
        let face = mesh.face_mut(f)?;
        for c in &mut face.corners {
            c.vert = dup[&c.vert];
        }
    }
    for (key, count) in &edge_count {
        if *count != 1 {
            continue;
        }
        let (a, b) = directed[key];
        let fid = mesh.add_face(&[a, b, dup[&b], dup[&a]])?;
        delta.created_faces.push(fid);
    }
    delta.removed_verts.extend(mesh.remove_unused_verts());
    remap_seams(mesh, &dup);
    Ok(delta)
}

/// Inset each selected face independently: shrink it toward its centroid by
/// `thickness`, push it along its normal by `depth`, and ring it with quads.
/// The inner face keeps its id and UVs.
pub fn inset(
    mesh: &mut Mesh,
    faces: &BTreeSet<FaceId>,
    thickness: f32,
    depth: f32,
) -> Result<MeshDelta, MeshError> {
    if faces.is_empty() {
        return Err(MeshError::Invalid("inset: no faces selected".into()));
    }
    if thickness < 0.0 {
        return Err(MeshError::Invalid("inset: thickness must be >= 0".into()));
    }
    let mut plans: Vec<(FaceId, Vec<Corner>, Vec<Vec3>)> = Vec::new();
    for &f in faces {
        let c = mesh.face_centroid(f)?;
        let n = mesh.face_normal(f)?;
        let corners = mesh.face(f)?.corners.clone();
        let mut new_pos = Vec::with_capacity(corners.len());
        for corner in &corners {
            let p = mesh.pos(corner.vert)?;
            let to_c = c - p;
            let d = to_c.length();
            if thickness >= d {
                return Err(MeshError::Invalid(format!(
                    "inset: thickness {thickness} does not fit in face {f} (corner {} is only {d} from the centroid)",
                    corner.vert
                )));
            }
            new_pos.push(p + to_c / d * thickness + n * depth);
        }
        plans.push((f, corners, new_pos));
    }
    let mut delta = MeshDelta::default();
    for (f, old_corners, new_pos) in plans {
        let new_verts: Vec<VertId> = new_pos.into_iter().map(|p| mesh.add_vert(p)).collect();
        delta.created_verts.extend(new_verts.iter().copied());
        {
            let face = mesh.face_mut(f)?;
            for (corner, &nv) in face.corners.iter_mut().zip(&new_verts) {
                corner.vert = nv;
            }
        }
        let k = old_corners.len();
        for i in 0..k {
            let a = old_corners[i].vert;
            let b = old_corners[(i + 1) % k].vert;
            let fid = mesh.add_face(&[a, b, new_verts[(i + 1) % k], new_verts[i]])?;
            delta.created_faces.push(fid);
        }
    }
    Ok(delta)
}

/// Ordered face fan around one bevel vertex. `spokes[i]` is the neighbour
/// vertex on the edge between `faces[i]` and `faces[(i + 1) % k]`; a face
/// `faces[i]` therefore runs prev-spoke `spokes[i - 1]` -> u -> `spokes[i]`.
struct Fan {
    faces: Vec<FaceId>,
    spokes: Vec<VertId>,
    sel: Vec<bool>,
}

fn build_fan(mesh: &Mesh, u: VertId, fs: &[FaceId], edges: &BTreeSet<EdgeKey>) -> Result<Fan, MeshError> {
    let mut by_prev: BTreeMap<VertId, FaceId> = BTreeMap::new();
    let mut next_of: BTreeMap<FaceId, VertId> = BTreeMap::new();
    for &f in fs {
        let face = mesh.face(f)?;
        let k = face.corners.len();
        let i = face.corners.iter().position(|c| c.vert == u).expect("face listed at vert");
        let prev = face.corners[(i + k - 1) % k].vert;
        let next = face.corners[(i + 1) % k].vert;
        if by_prev.insert(prev, f).is_some() {
            return Err(MeshError::Invalid(format!(
                "bevel: faces around {u} are inconsistently wound"
            )));
        }
        next_of.insert(f, next);
    }
    let start = *fs.iter().min().expect("non-empty fan");
    let mut faces = Vec::with_capacity(fs.len());
    let mut spokes = Vec::with_capacity(fs.len());
    let mut cur = start;
    loop {
        faces.push(cur);
        let w = next_of[&cur];
        spokes.push(w);
        let Some(&next_face) = by_prev.get(&w) else {
            return Err(MeshError::Invalid(format!(
                "bevel: vertex {u} is not surrounded by a closed manifold fan"
            )));
        };
        if next_face == start {
            break;
        }
        if faces.len() == fs.len() {
            return Err(MeshError::Invalid(format!(
                "bevel: faces around {u} do not form a single fan"
            )));
        }
        cur = next_face;
    }
    if faces.len() != fs.len() {
        return Err(MeshError::Invalid(format!(
            "bevel: faces around {u} do not form a single fan"
        )));
    }
    let sel = spokes.iter().map(|&w| edges.contains(&EdgeKey::new(u, w))).collect();
    Ok(Fan { faces, spokes, sel })
}

/// Bevel the selected interior manifold edges into strips of `segments`
/// quads at distance `width`, with corner polygons where strips meet.
/// Rebuilt neighbour faces keep their ids and UVs; new faces get zero UVs.
pub fn bevel_edges(
    mesh: &mut Mesh,
    edges: &BTreeSet<EdgeKey>,
    width: f32,
    segments: u32,
) -> Result<MeshDelta, MeshError> {
    if edges.is_empty() {
        return Err(MeshError::Invalid("bevel: no edges selected".into()));
    }
    if !(width > 0.0) {
        return Err(MeshError::Invalid("bevel: width must be positive".into()));
    }
    if segments == 0 {
        return Err(MeshError::Invalid("bevel: segments must be >= 1".into()));
    }
    let ef = mesh.edge_faces();
    for &e in edges {
        match ef.get(&e).map(Vec::len) {
            None => return Err(MeshError::Invalid(format!("bevel: edge {e} is not in the mesh"))),
            Some(2) => {}
            Some(n) => {
                return Err(MeshError::Invalid(format!(
                    "bevel: edge {e} borders {n} faces; only interior manifold edges can be beveled"
                )));
            }
        }
    }
    let bevel_verts: BTreeSet<VertId> = edges.iter().flat_map(|e| [e.0, e.1]).collect();
    let vert_faces = mesh.vert_faces();

    // Phase 1 (read-only): fan + feasibility per bevel vertex, so mutation
    // cannot fail halfway through.
    let mut fans: BTreeMap<VertId, Fan> = BTreeMap::new();
    let mut slide: BTreeMap<EdgeKey, f32> = BTreeMap::new();
    for &u in &bevel_verts {
        let fs = vert_faces.get(&u).ok_or(MeshError::UnknownVert(u))?;
        let fan = build_fan(mesh, u, fs, edges)?;
        for (i, &w) in fan.spokes.iter().enumerate() {
            if !fan.sel[i] {
                *slide.entry(EdgeKey::new(u, w)).or_insert(0.0) += width;
            }
        }
        fans.insert(u, fan);
    }
    for (&e, &s) in &slide {
        let len = (mesh.pos(e.1)? - mesh.pos(e.0)?).length();
        if s >= len {
            return Err(MeshError::Invalid(format!(
                "bevel: width {width} does not fit along edge {e} (length {len})"
            )));
        }
    }
    for (&u, fan) in &fans {
        let k = fan.spokes.len();
        for i in 0..k {
            if fan.sel[(i + k - 1) % k] && fan.sel[i] {
                let pu = mesh.pos(u)?;
                let d1 = (mesh.pos(fan.spokes[(i + k - 1) % k])? - pu).normalize_or_zero();
                let d2 = (mesh.pos(fan.spokes[i])? - pu).normalize_or_zero();
                if (d1 + d2).length() < 1e-5 {
                    return Err(MeshError::Invalid(format!(
                        "bevel: cannot miter the straight-through selected edges at {u}"
                    )));
                }
            }
        }
    }

    // Phase 2: create boundverts / miters / gap profiles, deterministically.
    let mut delta = MeshDelta::default();
    // Boundvert on unselected spoke (u, w), shared by both flanking faces.
    let mut bvs: BTreeMap<(VertId, VertId), VertId> = BTreeMap::new();
    // Miter vert per (u, face) whose both spokes at u are selected.
    let mut miters: BTreeMap<(VertId, FaceId), VertId> = BTreeMap::new();
    // Gap profile intermediates keyed (u, lo, hi) of terminal pair, lo->hi order.
    let mut profiles: BTreeMap<(VertId, VertId, VertId), Vec<VertId>> = BTreeMap::new();
    // Per-face corner replacement sequences at each bevel vertex.
    let mut face_repl: BTreeMap<FaceId, BTreeMap<VertId, Vec<VertId>>> = BTreeMap::new();
    // Full profile ring per (u, selected spoke w), fan order fL -> fR.
    let mut strip_rings: BTreeMap<(VertId, VertId), Vec<VertId>> = BTreeMap::new();
    // Corner polygon ring (fan order) per bevel vertex.
    let mut corner_rings: BTreeMap<VertId, Vec<VertId>> = BTreeMap::new();

    for (&u, fan) in &fans {
        let pu = mesh.pos(u)?;
        let k = fan.spokes.len();
        for i in 0..k {
            if !fan.sel[i] {
                let w = fan.spokes[i];
                let dir = (mesh.pos(w)? - pu).normalize_or_zero();
                let nv = mesh.add_vert(pu + dir * width);
                bvs.insert((u, w), nv);
                delta.created_verts.push(nv);
            }
        }
        // Replacement sequence per face, in traversal order prev-spoke -> next-spoke.
        let mut seqs: Vec<Vec<VertId>> = Vec::with_capacity(k);
        for i in 0..k {
            let prev_sel = fan.sel[(i + k - 1) % k];
            let next_sel = fan.sel[i];
            let seq = match (prev_sel, next_sel) {
                (false, false) => vec![bvs[&(u, fan.spokes[(i + k - 1) % k])], bvs[&(u, fan.spokes[i])]],
                (false, true) => vec![bvs[&(u, fan.spokes[(i + k - 1) % k])]],
                (true, false) => vec![bvs[&(u, fan.spokes[i])]],
                (true, true) => {
                    let d1 = (mesh.pos(fan.spokes[(i + k - 1) % k])? - pu).normalize_or_zero();
                    let d2 = (mesh.pos(fan.spokes[i])? - pu).normalize_or_zero();
                    let bisect = (d1 + d2).normalize_or_zero();
                    let half_sin =
                        ((1.0 - d1.dot(d2).clamp(-1.0, 1.0)) * 0.5).sqrt().max(1e-6);
                    let nv = mesh.add_vert(pu + bisect * (width / half_sin));
                    miters.insert((u, fan.faces[i]), nv);
                    delta.created_verts.push(nv);
                    vec![nv]
                }
            };
            face_repl.entry(fan.faces[i]).or_default().insert(u, seq.clone());
            seqs.push(seq);
        }
        // Gap profiles across each selected spoke, shared by terminal pair.
        for i in 0..k {
            if !fan.sel[i] {
                continue;
            }
            let l = *seqs[i].last().expect("non-empty seq");
            let r = *seqs[(i + 1) % k].first().expect("non-empty seq");
            let (lo, hi) = if l <= r { (l, r) } else { (r, l) };
            if !profiles.contains_key(&(u, lo, hi)) {
                let mut mids = Vec::new();
                let dl = mesh.pos(lo)? - pu;
                let dr = mesh.pos(hi)? - pu;
                let (ul, ur) = (dl.normalize_or_zero(), dr.normalize_or_zero());
                for s in 1..segments {
                    let t = s as f32 / segments as f32;
                    let mixed = ul.lerp(ur, t);
                    let p = if mixed.length() < 1e-4 {
                        pu + dl.lerp(dr, t)
                    } else {
                        pu + mixed.normalize() * dl.length().mul_add(1.0 - t, dr.length() * t)
                    };
                    let nv = mesh.add_vert(p);
                    mids.push(nv);
                    delta.created_verts.push(nv);
                }
                profiles.insert((u, lo, hi), mids);
            }
            let mut mids = profiles[&(u, lo, hi)].clone();
            if l != lo {
                mids.reverse();
            }
            let mut ring = Vec::with_capacity(segments as usize + 1);
            ring.push(l);
            ring.extend(mids);
            ring.push(r);
            strip_rings.insert((u, fan.spokes[i]), ring);
        }
        // Corner ring: all replacement verts plus gap intermediates, fan order.
        let mut ring: Vec<VertId> = Vec::new();
        for i in 0..k {
            for &v in &seqs[i] {
                if ring.last() != Some(&v) {
                    ring.push(v);
                }
            }
            if fan.sel[i] {
                let full = &strip_rings[&(u, fan.spokes[i])];
                for &v in &full[1..full.len() - 1] {
                    ring.push(v);
                }
            }
        }
        while ring.len() > 1 && ring.first() == ring.last() {
            ring.pop();
        }
        corner_rings.insert(u, ring);
    }

    // Phase 3: rebuild every touched face in place (keeps ids and UVs).
    for (&f, repl) in &face_repl {
        let old = mesh.face(f)?.corners.clone();
        let mut corners = Vec::with_capacity(old.len() + 2);
        for c in &old {
            match repl.get(&c.vert) {
                None => corners.push(*c),
                Some(seq) => corners.extend(seq.iter().map(|&v| Corner { vert: v, uv: c.uv })),
            }
        }
        mesh.face_mut(f)?.corners = corners;
        delta.uv_faces.push(f);
    }

    // Phase 4: one strip of quads per selected edge.
    for &e in edges {
        let (u, v) = (e.0, e.1);
        let ring_u = &strip_rings[&(u, v)];
        let ring_v = &strip_rings[&(v, u)];
        let s = segments as usize;
        for i in 0..s {
            let fid = mesh.add_face(&[ring_v[s - i], ring_u[i], ring_u[i + 1], ring_v[s - i - 1]])?;
            delta.created_faces.push(fid);
        }
    }

    // Phase 5: corner polygon per bevel vertex where a real hole remains.
    for (_, ring) in &corner_rings {
        let distinct: BTreeSet<VertId> = ring.iter().copied().collect();
        if ring.len() >= 3 && distinct.len() == ring.len() {
            let rev: Vec<VertId> = ring.iter().rev().copied().collect();
            let fid = mesh.add_face(&rev)?;
            delta.created_faces.push(fid);
        }
    }

    delta.removed_verts.extend(mesh.remove_unused_verts());
    mesh.prune_seams();
    Ok(delta)
}

/// In a quad containing edge `{a, b}`, the opposite edge oriented so the
/// first vert stays on `a`'s side and the second on `b`'s side.
fn quad_opposite(face: &Face, a: VertId, b: VertId) -> Option<(VertId, VertId)> {
    if face.corners.len() != 4 {
        return None;
    }
    for i in 0..4 {
        let x = face.corners[i].vert;
        let y = face.corners[(i + 1) % 4].vert;
        let z = face.corners[(i + 2) % 4].vert;
        let w = face.corners[(i + 3) % 4].vert;
        if x == a && y == b {
            return Some((w, z));
        }
        if x == b && y == a {
            return Some((z, w));
        }
    }
    None
}

/// Cut the quad ring through `edge` with `cuts` evenly spaced loops.
/// Ring quads are replaced (fresh ids, interpolated UVs); non-quad faces at
/// the ends of an open ring get the cut verts spliced in (no T-junctions).
pub fn loop_cut(mesh: &mut Mesh, edge: EdgeKey, cuts: u32) -> Result<MeshDelta, MeshError> {
    if cuts == 0 {
        return Err(MeshError::Invalid("loop_cut: cuts must be >= 1".into()));
    }
    let ef = mesh.edge_faces();
    let start_faces = ef
        .get(&edge)
        .cloned()
        .ok_or_else(|| MeshError::Invalid(format!("loop_cut: edge {edge} is not in the mesh")))?;

    // Walk the ring of quads. Ring edges stay consistently oriented: the
    // first vert of every oriented edge is on `edge.0`'s side of the ring.
    let mut ring_edges: Vec<(VertId, VertId)> = vec![(edge.0, edge.1)];
    let mut ring_faces: Vec<FaceId> = Vec::new();
    let mut visited: BTreeSet<FaceId> = BTreeSet::new();
    let mut closed = false;
    for (pass, &first_face) in start_faces.iter().enumerate().take(2) {
        if closed {
            break;
        }
        let mut cur = (edge.0, edge.1);
        let mut cur_face = Some(first_face);
        while let Some(f) = cur_face {
            if visited.contains(&f) {
                break;
            }
            let face = mesh.face(f)?;
            let Some(next) = quad_opposite(face, cur.0, cur.1) else {
                break; // non-quad neighbour terminates the ring
            };
            visited.insert(f);
            let nk = EdgeKey::new(next.0, next.1);
            if pass == 0 {
                ring_faces.push(f);
            } else {
                ring_faces.insert(0, f);
            }
            if nk == edge {
                if next != (edge.0, edge.1) {
                    return Err(MeshError::Invalid(format!(
                        "loop_cut: ring through {edge} closes with flipped orientation"
                    )));
                }
                closed = true;
                break;
            }
            if pass == 0 {
                ring_edges.push(next);
            } else {
                ring_edges.insert(0, next);
                // keep the backward walk's face association: faces[i] sits
                // between edges[i] and edges[i + 1]
            }
            cur = next;
            cur_face = ef.get(&nk).and_then(|fs| fs.iter().find(|&&g| g != f).copied());
        }
    }

    let mut delta = MeshDelta::default();
    let n_cuts = cuts as usize;
    // Cut verts per oriented ring edge, ordered from the `.0` side.
    let mut cut_verts: Vec<Vec<VertId>> = Vec::with_capacity(ring_edges.len());
    let mut seam_splits: Vec<(EdgeKey, Vec<EdgeKey>)> = Vec::new();
    for &(a, b) in &ring_edges {
        let (pa, pb) = (mesh.pos(a)?, mesh.pos(b)?);
        let mut vs = Vec::with_capacity(n_cuts);
        for j in 1..=n_cuts {
            let t = j as f32 / (cuts + 1) as f32;
            vs.push(mesh.add_vert(pa.lerp(pb, t)));
        }
        delta.created_verts.extend(vs.iter().copied());
        let key = EdgeKey::new(a, b);
        if mesh.seams.contains(&key) {
            let chain: Vec<VertId> =
                std::iter::once(a).chain(vs.iter().copied()).chain(std::iter::once(b)).collect();
            let subs = chain.windows(2).map(|w| EdgeKey::new(w[0], w[1])).collect();
            seam_splits.push((key, subs));
        }
        cut_verts.push(vs);
    }

    // Replace each ring quad with cuts + 1 slices, UVs lerped per face.
    for (i, &f) in ring_faces.iter().enumerate() {
        let ea = ring_edges[i];
        let old = mesh.remove_face(f)?;
        delta.removed_faces.push(f);
        let rot = {
            let idx = (0..4)
                .find(|&j| {
                    let x = old.corners[j].vert;
                    let y = old.corners[(j + 1) % 4].vert;
                    EdgeKey::new(x, y) == EdgeKey::new(ea.0, ea.1)
                })
                .expect("ring face contains its ring edge");
            [0, 1, 2, 3].map(|j| old.corners[(idx + j) % 4])
        };
        let forward = rot[0].vert == ea.0; // face traverses a -> b on edge i
        // Corner rows from the a-side: q along edge i, r along edge i + 1.
        let (qa, qb, ra, rb) = if forward {
            (rot[0], rot[1], rot[3], rot[2])
        } else {
            (rot[1], rot[0], rot[2], rot[3])
        };
        let row = |side_a: Corner, side_b: Corner, verts: &[VertId]| -> Vec<Corner> {
            let mut out = vec![side_a];
            for (j, &v) in verts.iter().enumerate() {
                let t = (j + 1) as f32 / (cuts + 1) as f32;
                out.push(Corner { vert: v, uv: side_a.uv.lerp(side_b.uv, t) });
            }
            out.push(side_b);
            out
        };
        let q = row(qa, qb, &cut_verts[i]);
        let r = row(ra, rb, &cut_verts[(i + 1) % cut_verts.len()]);
        for j in 0..=n_cuts {
            let corners = if forward {
                vec![q[j], q[j + 1], r[j + 1], r[j]]
            } else {
                vec![q[j + 1], q[j], r[j], r[j + 1]]
            };
            let fid = mesh.add_face_uv(corners)?;
            delta.created_faces.push(fid);
        }
    }

    // Open ends: splice the cut verts into whatever face sits across the
    // terminal edge so no T-junctions remain.
    if !closed {
        for (end_idx, &(a, b)) in [(0usize, &ring_edges[0]), (ring_edges.len() - 1, ring_edges.last().unwrap())] {
            let key = EdgeKey::new(a, b);
            let Some(fs) = ef.get(&key) else { continue };
            for &g in fs {
                if visited.contains(&g) {
                    continue;
                }
                let old = mesh.face(g)?.corners.clone();
                let k = old.len();
                let mut corners: Vec<Corner> = Vec::with_capacity(k + n_cuts);
                for j in 0..k {
                    let c = old[j];
                    let d = old[(j + 1) % k];
                    corners.push(c);
                    let pair = (c.vert, d.vert);
                    if pair == (a, b) || pair == (b, a) {
                        let vs = &cut_verts[end_idx];
                        let iter: Vec<usize> = if pair == (a, b) {
                            (0..vs.len()).collect()
                        } else {
                            (0..vs.len()).rev().collect()
                        };
                        for &vi in &iter {
                            let t = (vi + 1) as f32 / (cuts + 1) as f32;
                            let (ua, ub) = if pair == (a, b) { (c.uv, d.uv) } else { (d.uv, c.uv) };
                            corners.push(Corner { vert: vs[vi], uv: ua.lerp(ub, t) });
                        }
                    }
                }
                mesh.face_mut(g)?.corners = corners;
                delta.uv_faces.push(g);
            }
        }
    }

    for (old, subs) in seam_splits {
        mesh.seams.remove(&old);
        mesh.seams.extend(subs);
    }
    mesh.prune_seams();
    Ok(delta)
}

/// One boundary loop from an edge set: every edge must border exactly one
/// face; the loop is returned as a single closed cycle in face-traversal
/// order, starting at its smallest vertex id.
fn boundary_loop(mesh: &Mesh, edges: &BTreeSet<EdgeKey>, which: &str) -> Result<Vec<VertId>, MeshError> {
    if edges.len() < 3 {
        return Err(MeshError::Invalid(format!(
            "bridge: {which} loop needs at least 3 edges"
        )));
    }
    let ef = mesh.edge_faces();
    let mut succ: BTreeMap<VertId, VertId> = BTreeMap::new();
    for &e in edges {
        let fs = ef
            .get(&e)
            .ok_or_else(|| MeshError::Invalid(format!("bridge: edge {e} is not in the mesh")))?;
        if fs.len() != 1 {
            return Err(MeshError::Invalid(format!(
                "bridge: edge {e} is not a boundary edge ({} faces)",
                fs.len()
            )));
        }
        let face = mesh.face(fs[0])?;
        let k = face.corners.len();
        let (u, v) = (0..k)
            .map(|i| (face.corners[i].vert, face.corners[(i + 1) % k].vert))
            .find(|&(u, v)| EdgeKey::new(u, v) == e)
            .expect("face listed for edge contains it");
        if succ.insert(u, v).is_some() {
            return Err(MeshError::Invalid(format!(
                "bridge: {which} loop revisits vertex {u}"
            )));
        }
    }
    let start = *succ.keys().next().expect("non-empty loop");
    let mut cyc = vec![start];
    let mut cur = succ[&start];
    while cur != start {
        if !succ.contains_key(&cur) || cyc.len() > succ.len() {
            return Err(MeshError::Invalid(format!(
                "bridge: {which} loop is not a single closed cycle"
            )));
        }
        cyc.push(cur);
        cur = succ[&cur];
    }
    if cyc.len() != succ.len() {
        return Err(MeshError::Invalid(format!(
            "bridge: {which} loop is not a single closed cycle"
        )));
    }
    Ok(cyc)
}

/// Bridge two equal-length boundary loops with a ring of quads. The loop
/// pairing offset is chosen to minimize total span length, deterministically.
pub fn bridge(mesh: &mut Mesh, a: &BTreeSet<EdgeKey>, b: &BTreeSet<EdgeKey>) -> Result<MeshDelta, MeshError> {
    let ca = boundary_loop(mesh, a, "first")?;
    let cb = boundary_loop(mesh, b, "second")?;
    if ca.len() != cb.len() {
        return Err(MeshError::Invalid(format!(
            "bridge: loops differ in length ({} vs {} edges)",
            ca.len(),
            cb.len()
        )));
    }
    let sb: BTreeSet<VertId> = cb.iter().copied().collect();
    if ca.iter().any(|v| sb.contains(v)) {
        return Err(MeshError::Invalid("bridge: loops share vertices".into()));
    }
    let n = ca.len();
    let pa: Vec<Vec3> = ca.iter().map(|&v| mesh.pos(v)).collect::<Result<_, _>>()?;
    let pb: Vec<Vec3> = cb.iter().map(|&v| mesh.pos(v)).collect::<Result<_, _>>()?;
    let mut best = (f32::INFINITY, 0usize);
    for c in 0..n {
        let sum: f32 = (0..n).map(|i| (pa[i] - pb[(c + n - i) % n]).length_squared()).sum();
        if sum < best.0 {
            best = (sum, c);
        }
    }
    let c = best.1;
    let mut delta = MeshDelta::default();
    for i in 0..n {
        let x = ca[i];
        let y = ca[(i + n - 1) % n];
        let mx = cb[(c + n - i) % n];
        let my = cb[(c + n - i + 1) % n];
        let fid = mesh.add_face(&[x, y, my, mx])?;
        delta.created_faces.push(fid);
    }
    Ok(delta)
}

fn uf_find(parent: &mut BTreeMap<VertId, VertId>, v: VertId) -> VertId {
    let mut root = v;
    while parent[&root] != root {
        root = parent[&root];
    }
    let mut cur = v;
    while parent[&cur] != root {
        let next = parent[&cur];
        parent.insert(cur, root);
        cur = next;
    }
    root
}

/// Merge selected vertices that lie within `distance` of each other into
/// their cluster's lowest id, placed at the cluster centroid. Faces that
/// collapse below 3 distinct corners (or pinch into bow-ties) are removed.
pub fn merge_verts(mesh: &mut Mesh, verts: &BTreeSet<VertId>, distance: f32) -> Result<MeshDelta, MeshError> {
    if distance < 0.0 {
        return Err(MeshError::Invalid("merge: distance must be >= 0".into()));
    }
    let mut pos: BTreeMap<VertId, Vec3> = BTreeMap::new();
    for &v in verts {
        pos.insert(v, mesh.pos(v)?);
    }
    let ids: Vec<VertId> = verts.iter().copied().collect();
    let mut parent: BTreeMap<VertId, VertId> = ids.iter().map(|&v| (v, v)).collect();
    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            if pos[&ids[i]].distance(pos[&ids[j]]) <= distance {
                let (ra, rb) = (uf_find(&mut parent, ids[i]), uf_find(&mut parent, ids[j]));
                if ra != rb {
                    let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
                    parent.insert(hi, lo);
                }
            }
        }
    }
    let mut clusters: BTreeMap<VertId, Vec<VertId>> = BTreeMap::new();
    for &v in &ids {
        let r = uf_find(&mut parent, v);
        clusters.entry(r).or_default().push(v);
    }
    let mut delta = MeshDelta::default();
    let mut map: BTreeMap<VertId, VertId> = BTreeMap::new();
    for (&rep, members) in &clusters {
        if members.len() < 2 {
            continue;
        }
        let centroid =
            members.iter().map(|v| pos[v]).fold(Vec3::ZERO, |acc, p| acc + p) / members.len() as f32;
        mesh.set_pos(rep, centroid)?;
        delta.moved_verts.push(rep);
        for &m in members {
            if m != rep {
                map.insert(m, rep);
            }
        }
    }
    if map.is_empty() {
        return Ok(delta);
    }
    let face_ids: Vec<FaceId> = mesh.faces.keys().copied().collect();
    for f in face_ids {
        let face = mesh.face(f)?;
        if !face.corners.iter().any(|c| map.contains_key(&c.vert)) {
            continue;
        }
        let mapped: Vec<Corner> = face
            .corners
            .iter()
            .map(|c| Corner { vert: *map.get(&c.vert).unwrap_or(&c.vert), uv: c.uv })
            .collect();
        let mut out: Vec<Corner> = Vec::with_capacity(mapped.len());
        for c in mapped {
            if out.last().map(|l| l.vert) != Some(c.vert) {
                out.push(c);
            }
        }
        while out.len() > 1 && out.first().map(|c| c.vert) == out.last().map(|c| c.vert) {
            out.pop();
        }
        let distinct: BTreeSet<VertId> = out.iter().map(|c| c.vert).collect();
        if out.len() < 3 || distinct.len() != out.len() {
            mesh.remove_face(f)?;
            delta.removed_faces.push(f);
        } else {
            mesh.face_mut(f)?.corners = out;
        }
    }
    remap_seams(mesh, &map);
    delta.removed_verts.extend(mesh.remove_unused_verts());
    Ok(delta)
}

/// Delete the selected faces and clean up whatever geometry they stranded.
pub fn dissolve(mesh: &mut Mesh, faces: &BTreeSet<FaceId>) -> Result<MeshDelta, MeshError> {
    if faces.is_empty() {
        return Err(MeshError::Invalid("dissolve: no faces selected".into()));
    }
    for &f in faces {
        mesh.face(f)?;
    }
    let mut delta = MeshDelta::default();
    for &f in faces {
        mesh.remove_face(f)?;
        delta.removed_faces.push(f);
    }
    delta.removed_verts.extend(mesh.remove_unused_verts());
    mesh.prune_seams();
    Ok(delta)
}

/// Mirror the mesh across the plane through the origin perpendicular to
/// `axis` (0 = X, 1 = Y, 2 = Z). Verts within `merge_distance` of the plane
/// snap onto it and are shared; mirrored faces get flipped winding.
pub fn mirror(mesh: &mut Mesh, axis: usize, merge_distance: f32) -> Result<MeshDelta, MeshError> {
    if axis > 2 {
        return Err(MeshError::Invalid(format!(
            "mirror: axis {axis} out of range (0 = X, 1 = Y, 2 = Z)"
        )));
    }
    if merge_distance < 0.0 {
        return Err(MeshError::Invalid("mirror: merge_distance must be >= 0".into()));
    }
    let mut delta = MeshDelta::default();
    let orig_verts: Vec<(VertId, Vec3)> = mesh.verts.iter().map(|(&v, &p)| (v, p)).collect();
    let mut map: BTreeMap<VertId, VertId> = BTreeMap::new();
    for (v, p) in orig_verts {
        if p[axis].abs() <= merge_distance {
            if p[axis] != 0.0 {
                let mut q = p;
                q[axis] = 0.0;
                mesh.set_pos(v, q)?;
                delta.moved_verts.push(v);
            }
            map.insert(v, v);
        } else {
            let mut q = p;
            q[axis] = -q[axis];
            let nv = mesh.add_vert(q);
            map.insert(v, nv);
            delta.created_verts.push(nv);
        }
    }
    let orig_faces: Vec<(FaceId, Face)> = mesh.faces.iter().map(|(&f, face)| (f, face.clone())).collect();
    for (_, face) in orig_faces {
        if face.verts().all(|v| map[&v] == v) {
            continue; // face lies on the plane; a mirror copy would double it
        }
        let corners: Vec<Corner> =
            face.corners.iter().rev().map(|c| Corner { vert: map[&c.vert], uv: c.uv }).collect();
        let nf = mesh.add_face_uv(corners)?;
        delta.created_faces.push(nf);
    }
    let old_seams: Vec<EdgeKey> = mesh.seams.iter().copied().collect();
    for e in old_seams {
        let (Some(&a), Some(&b)) = (map.get(&e.0), map.get(&e.1)) else { continue };
        if a != b {
            mesh.seams.insert(EdgeKey::new(a, b));
        }
    }
    mesh.prune_seams();
    Ok(delta)
}

/// Symmetric 4x4 error quadric (f64 internally; still fully deterministic).
#[derive(Clone, Copy, Default)]
struct Quadric {
    m: [[f64; 4]; 4],
}

impl Quadric {
    fn add_plane(&mut self, n: DVec3, d: f64, weight: f64) {
        let v = [n.x, n.y, n.z, d];
        for i in 0..4 {
            for j in 0..4 {
                self.m[i][j] += weight * v[i] * v[j];
            }
        }
    }

    fn add(&mut self, other: &Quadric) {
        for i in 0..4 {
            for j in 0..4 {
                self.m[i][j] += other.m[i][j];
            }
        }
    }

    fn error(&self, p: DVec3) -> f64 {
        let v = [p.x, p.y, p.z, 1.0];
        let mut e = 0.0;
        for i in 0..4 {
            for j in 0..4 {
                e += self.m[i][j] * v[i] * v[j];
            }
        }
        e.max(0.0)
    }

    /// Point minimizing the quadric, if the 3x3 system is well conditioned.
    fn optimal(&self) -> Option<DVec3> {
        let a = DMat3::from_cols(
            DVec3::new(self.m[0][0], self.m[1][0], self.m[2][0]),
            DVec3::new(self.m[0][1], self.m[1][1], self.m[2][1]),
            DVec3::new(self.m[0][2], self.m[1][2], self.m[2][2]),
        );
        if a.determinant().abs() < 1e-10 {
            return None;
        }
        Some(a.inverse() * -DVec3::new(self.m[0][3], self.m[1][3], self.m[2][3]))
    }
}

/// Heap entry: interior collapses first, then cheapest error, then edge id.
#[derive(PartialEq)]
struct Cand {
    feature: bool,
    cost: f64,
    edge: EdgeKey,
}

impl Eq for Cand {}

impl Ord for Cand {
    fn cmp(&self, other: &Self) -> Ordering {
        self.feature
            .cmp(&other.feature)
            .then(self.cost.total_cmp(&other.cost))
            .then(self.edge.cmp(&other.edge))
    }
}

impl PartialOrd for Cand {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct Decimator {
    quadrics: BTreeMap<VertId, Quadric>,
    vert_faces: BTreeMap<VertId, BTreeSet<FaceId>>,
    constrained: BTreeSet<VertId>,
    feature_edges: BTreeSet<EdgeKey>,
}

impl Decimator {
    fn edge_alive(&self, mesh: &Mesh, e: EdgeKey) -> bool {
        self.vert_faces.get(&e.0).is_some_and(|fs| {
            fs.iter().any(|f| mesh.faces.get(f).is_some_and(|face| face.verts().any(|v| v == e.1)))
        })
    }

    /// Plan a collapse: kept vert, dropped vert, new position, cost.
    fn candidate(&self, mesh: &Mesh, e: EdgeKey) -> Option<(Cand, VertId, VertId, DVec3)> {
        let (u, v) = (e.0, e.1);
        let (cu, cv) = (self.constrained.contains(&u), self.constrained.contains(&v));
        if cu && cv && !self.feature_edges.contains(&e) {
            return None; // would weld two distinct feature curves together
        }
        let pu = mesh.verts.get(&u).copied()?.as_dvec3();
        let pv = mesh.verts.get(&v).copied()?.as_dvec3();
        let mut q = self.quadrics.get(&u).copied().unwrap_or_default();
        q.add(self.quadrics.get(&v).unwrap_or(&Quadric::default()));
        let (kept, gone, p) = match (cu, cv) {
            (true, false) => (u, v, pu),
            (false, true) => (v, u, pv),
            (true, true) => {
                let mid = (pu + pv) * 0.5;
                let best = [pu, pv, mid]
                    .into_iter()
                    .min_by(|a, b| q.error(*a).total_cmp(&q.error(*b)))
                    .expect("non-empty");
                (u, v, best)
            }
            (false, false) => {
                let p = q.optimal().unwrap_or_else(|| {
                    let mid = (pu + pv) * 0.5;
                    [pu, pv, mid]
                        .into_iter()
                        .min_by(|a, b| q.error(*a).total_cmp(&q.error(*b)))
                        .expect("non-empty")
                });
                (u, v, p)
            }
        };
        Some((Cand { feature: cu || cv, cost: q.error(p), edge: e }, kept, gone, p))
    }

    fn neighbours(&self, mesh: &Mesh, v: VertId) -> BTreeSet<VertId> {
        let mut out = BTreeSet::new();
        if let Some(fs) = self.vert_faces.get(&v) {
            for f in fs {
                if let Some(face) = mesh.faces.get(f) {
                    out.extend(face.verts().filter(|&w| w != v));
                }
            }
        }
        out
    }
}

fn dvec(p: Vec3) -> DVec3 {
    DVec3::new(p.x as f64, p.y as f64, p.z as f64)
}

/// Greedy quadric-error edge collapse until the fan-triangulated count is at
/// most `target_tris`. Ngons are triangulated first. Boundary and seam
/// vertices are pinned so silhouettes and UV islands survive.
pub fn decimate_to_target(mesh: &mut Mesh, target_tris: u32) -> Result<MeshDelta, MeshError> {
    // Work on a copy so an unreachable target leaves the mesh untouched.
    let mut work = mesh.clone();
    let delta = decimate_impl(&mut work, target_tris)?;
    *mesh = work;
    Ok(delta)
}

fn decimate_impl(mesh: &mut Mesh, target_tris: u32) -> Result<MeshDelta, MeshError> {
    let mut delta = MeshDelta::default();
    if mesh.tri_count() <= target_tris {
        return Ok(delta);
    }
    let ngons: Vec<FaceId> = mesh
        .faces
        .iter()
        .filter(|(_, face)| face.corners.len() > 3)
        .map(|(&f, _)| f)
        .collect();
    for f in ngons {
        let face = mesh.remove_face(f)?;
        delta.removed_faces.push(f);
        for i in 1..face.corners.len() - 1 {
            let nf = mesh.add_face_uv(vec![face.corners[0], face.corners[i], face.corners[i + 1]])?;
            delta.created_faces.push(nf);
        }
    }

    let mut st = Decimator {
        quadrics: BTreeMap::new(),
        vert_faces: BTreeMap::new(),
        constrained: BTreeSet::new(),
        feature_edges: BTreeSet::new(),
    };
    for (&f, face) in &mesh.faces {
        for v in face.verts() {
            st.vert_faces.entry(v).or_default().insert(f);
        }
        let a = dvec(mesh.pos(face.corners[0].vert)?);
        let b = dvec(mesh.pos(face.corners[1].vert)?);
        let c = dvec(mesh.pos(face.corners[2].vert)?);
        let cross = (b - a).cross(c - a);
        let area2 = cross.length();
        if area2 > 1e-14 {
            let n = cross / area2;
            for v in face.verts() {
                st.quadrics.entry(v).or_default().add_plane(n, -n.dot(a), area2 * 0.5);
            }
        }
    }
    for e in mesh.boundary_edges() {
        st.feature_edges.insert(e);
    }
    st.feature_edges.extend(mesh.seams.iter().copied());
    for &e in &st.feature_edges {
        st.constrained.insert(e.0);
        st.constrained.insert(e.1);
    }

    let mut heap: BinaryHeap<Reverse<Cand>> = BinaryHeap::new();
    for e in mesh.edges() {
        if let Some((cand, _, _, _)) = st.candidate(mesh, e) {
            heap.push(Reverse(cand));
        }
    }

    let mut collapse_map: BTreeMap<VertId, VertId> = BTreeMap::new();
    let mut tris = mesh.tri_count();
    while tris > target_tris {
        let Some(Reverse(top)) = heap.pop() else {
            return Err(MeshError::Invalid(format!(
                "decimate: cannot reach {target_tris} tris without breaking the mesh (stuck at {tris})"
            )));
        };
        if !st.edge_alive(mesh, top.edge) {
            continue;
        }
        let Some((cand, kept, gone, p)) = st.candidate(mesh, top.edge) else {
            continue;
        };
        if cand != top {
            continue; // stale entry; the fresh one is elsewhere in the heap
        }
        // Link condition: shared neighbours must be exactly the shared faces'
        // third corners, or the collapse pinches the surface.
        let shared_faces: Vec<FaceId> = st.vert_faces[&kept]
            .intersection(&st.vert_faces[&gone])
            .copied()
            .collect();
        let mut third: BTreeSet<VertId> = BTreeSet::new();
        for f in &shared_faces {
            for v in mesh.face(*f)?.verts() {
                if v != kept && v != gone {
                    third.insert(v);
                }
            }
        }
        let common: BTreeSet<VertId> = st
            .neighbours(mesh, kept)
            .intersection(&st.neighbours(mesh, gone))
            .copied()
            .collect();
        if common != third {
            continue;
        }
        // Normal-flip / degeneracy guard on every surviving incident face.
        let mut flips = false;
        for &f in st.vert_faces[&kept].union(&st.vert_faces[&gone]) {
            if shared_faces.contains(&f) {
                continue;
            }
            let face = mesh.face(f)?;
            let mut before = [DVec3::ZERO; 3];
            let mut after = [DVec3::ZERO; 3];
            for (i, c) in face.corners.iter().enumerate().take(3) {
                let q = dvec(mesh.pos(c.vert)?);
                before[i] = q;
                after[i] = if c.vert == kept || c.vert == gone { p } else { q };
            }
            let nb = (before[1] - before[0]).cross(before[2] - before[0]);
            let na = (after[1] - after[0]).cross(after[2] - after[0]);
            if na.length() < 1e-7 || nb.dot(na) <= 0.0 {
                flips = true;
                break;
            }
        }
        if flips {
            continue;
        }
        // Commit the collapse.
        for f in shared_faces {
            let face = mesh.remove_face(f)?;
            delta.removed_faces.push(f);
            tris -= face.corners.len() as u32 - 2;
            for v in face.verts() {
                if let Some(fs) = st.vert_faces.get_mut(&v) {
                    fs.remove(&f);
                }
            }
        }
        let gone_faces: Vec<FaceId> = st.vert_faces.get(&gone).map(|fs| fs.iter().copied().collect()).unwrap_or_default();
        for f in gone_faces {
            for c in &mut mesh.face_mut(f)?.corners {
                if c.vert == gone {
                    c.vert = kept;
                }
            }
            st.vert_faces.entry(kept).or_default().insert(f);
        }
        st.vert_faces.remove(&gone);
        let new_pos = Vec3::new(p.x as f32, p.y as f32, p.z as f32);
        if mesh.pos(kept)? != new_pos {
            mesh.set_pos(kept, new_pos)?;
            delta.moved_verts.push(kept);
        }
        let gq = st.quadrics.remove(&gone).unwrap_or_default();
        st.quadrics.entry(kept).or_default().add(&gq);
        if st.constrained.remove(&gone) {
            st.constrained.insert(kept);
        }
        let feature_touching: Vec<EdgeKey> =
            st.feature_edges.iter().copied().filter(|e| e.0 == gone || e.1 == gone).collect();
        for e in feature_touching {
            st.feature_edges.remove(&e);
            let other = if e.0 == gone { e.1 } else { e.0 };
            if other != kept {
                st.feature_edges.insert(EdgeKey::new(kept, other));
            }
        }
        collapse_map.insert(gone, kept);
        for w in st.neighbours(mesh, kept) {
            if let Some((cand, _, _, _)) = st.candidate(mesh, EdgeKey::new(kept, w)) {
                heap.push(Reverse(cand));
            }
        }
    }

    // Resolve chained collapses for seam remapping.
    let mut resolved: BTreeMap<VertId, VertId> = BTreeMap::new();
    for &g in collapse_map.keys() {
        let mut cur = g;
        while let Some(&next) = collapse_map.get(&cur) {
            cur = next;
        }
        resolved.insert(g, cur);
    }
    remap_seams(mesh, &resolved);
    delta.removed_verts.extend(mesh.remove_unused_verts());
    delta.moved_verts.sort_unstable();
    delta.moved_verts.dedup();
    Ok(delta)
}
