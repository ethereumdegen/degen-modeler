//! Scene-level gate rules (CONTRACT-19 §SceneGate): objects that
//! interpenetrate, objects that float free, open shells, and inside-out
//! shells. Mesh rules see one object at a time; these see the whole `Doc`.
//!
//! Tags on `Doc.tags` opt an object out: `contact` silences
//! `scene.intersects` for every pair it belongs to; `open` silences
//! `mesh.open_boundary`. LODs inherit their base object's tags and never
//! take part in pair rules.

use std::collections::BTreeSet;

use dgm_atlas::{AssetClass, Pack};
use dgm_mesh::{FaceId, Finding, Mesh};
use dgm_scene::Doc;
use glam::Vec3;

/// Objects closer than this to another object are "touching".
const FLOAT_GAP: f32 = 0.02;
/// Signed distances within this of a plane count as on it; tangent contact
/// (a box resting on a floor, shells sharing a rim) is not penetration.
const PLANE_EPS: f32 = 1e-4;
/// Cap on attributed face ids per object in a `scene.intersects` finding.
const MAX_ELEMS: usize = 8;

/// Every scene rule in a fixed order: intersects, floating, open_boundary,
/// inverted; pairs and objects in name order. The pack is not consulted:
/// severities are class-driven, the parameter keeps parity with the other
/// rule families `gate()` composes.
pub fn scene_findings(doc: &Doc, _pack: &Pack) -> Vec<Finding> {
    let structural = matches!(doc.asset_class, AssetClass::Building | AssetClass::Environment);
    let hard_or_warn = |rule: &str, msg: String| {
        if structural { Finding::hard(rule, msg) } else { Finding::warn(rule, msg) }
    };
    let mut findings = Vec::new();

    let bodies: Vec<Body> = doc
        .objects
        .iter()
        .filter(|(_, o)| o.lod_of.is_none())
        .filter_map(|(name, o)| Body::new(name, &o.mesh))
        .collect();

    // Pair geometry once: face pairs that penetrate and whether the two
    // surfaces come within FLOAT_GAP of each other. `nearest` is a lower
    // bound on the distance to any other object: exact for pairs whose
    // padded bounds overlap, the bounds gap otherwise.
    let mut touching = vec![false; bodies.len()];
    let mut nearest = vec![f32::INFINITY; bodies.len()];
    for i in 0..bodies.len() {
        for j in i + 1..bodies.len() {
            let (a, b) = (&bodies[i], &bodies[j]);
            let gap = box_gap(a.lo, a.hi, b.lo, b.hi);
            let (hits, dist) = if gap > FLOAT_GAP {
                (Vec::new(), gap)
            } else {
                let hits = penetrating_pairs(a, b);
                let dist = if hits.is_empty() { pair_distance(a, b) } else { 0.0 };
                (hits, dist)
            };
            if dist <= FLOAT_GAP {
                touching[i] = true;
                touching[j] = true;
            }
            nearest[i] = nearest[i].min(dist);
            nearest[j] = nearest[j].min(dist);

            if hits.is_empty() || has_tag(doc, a.name, "contact") || has_tag(doc, b.name, "contact") {
                continue;
            }
            let (fa, fb): (BTreeSet<FaceId>, BTreeSet<FaceId>) = hits.iter().copied().unzip();
            let elems = fa
                .iter()
                .take(MAX_ELEMS)
                .map(|f| format!("{}:{f}", a.name))
                .chain(fb.iter().take(MAX_ELEMS).map(|f| format!("{}:{f}", b.name)))
                .collect();
            findings.push(
                hard_or_warn(
                    "scene.intersects",
                    format!(
                        "object `{}` intersects `{}` ({} face pairs); tag one `contact` if intentional",
                        a.name,
                        b.name,
                        hits.len()
                    ),
                )
                .with_elems(elems)
                .with_value(hits.len() as f64),
            );
        }
    }

    if bodies.len() >= 2 {
        for (k, body) in bodies.iter().enumerate() {
            if touching[k] {
                continue;
            }
            findings.push(
                Finding::warn(
                    "scene.floating",
                    format!(
                        "object `{}` touches nothing (nearest object >= {:.3} m away); snap it to a surface",
                        body.name, nearest[k]
                    ),
                )
                .with_value(nearest[k] as f64),
            );
        }
    }

    for (name, object) in &doc.objects {
        let mesh = &object.mesh;
        if mesh.faces.is_empty() {
            continue;
        }
        let edge_faces = mesh.edge_faces();
        let boundary: Vec<_> = edge_faces.iter().filter(|(_, fs)| fs.len() == 1).map(|(e, _)| *e).collect();
        if !boundary.is_empty() {
            if !has_tag(doc, name, "open") {
                findings.push(
                    hard_or_warn(
                        "mesh.open_boundary",
                        format!(
                            "object `{name}`: {} boundary edges (open shell); solidify/close it or tag it `open`",
                            boundary.len()
                        ),
                    )
                    .with_elems(boundary.iter().take(MAX_ELEMS).map(|e| e.to_string()).collect())
                    .with_value(boundary.len() as f64),
                );
            }
            continue;
        }
        if edge_faces.values().any(|fs| fs.len() != 2) {
            continue; // non-manifold: mesh.non_manifold_edge owns it, volume is meaningless
        }
        let volume = signed_volume(mesh);
        if volume < 0.0 {
            findings.push(
                Finding::hard(
                    "mesh.inverted",
                    format!("object `{name}`: closed shell winds inside-out (signed volume {volume:.4})"),
                )
                .with_value(volume as f64),
            );
        }
    }

    findings
}

fn has_tag(doc: &Doc, name: &str, tag: &str) -> bool {
    let tagged = |n: &str| doc.tags.get(n).is_some_and(|t| t.contains(tag));
    tagged(name) || doc.objects.get(name).and_then(|o| o.lod_of.as_deref()).is_some_and(tagged)
}

/// Divergence-theorem volume over the fan triangulation; positive for
/// outward winding.
fn signed_volume(mesh: &Mesh) -> f32 {
    let mut six_v = 0.0f32;
    for face in mesh.faces.values() {
        let p: Vec<Vec3> = face.verts().map(|v| mesh.verts[&v]).collect();
        for i in 1..p.len() - 1 {
            six_v += p[0].dot(p[i].cross(p[i + 1]));
        }
    }
    six_v / 6.0
}

struct TriGeom {
    face: FaceId,
    p: [Vec3; 3],
    lo: Vec3,
    hi: Vec3,
}

struct Body<'a> {
    name: &'a str,
    verts: Vec<Vec3>,
    tris: Vec<TriGeom>,
    lo: Vec3,
    hi: Vec3,
}

impl<'a> Body<'a> {
    fn new(name: &'a str, mesh: &Mesh) -> Option<Self> {
        let (lo, hi) = mesh.bounds()?;
        if mesh.faces.is_empty() {
            return None;
        }
        let tris = mesh
            .triangulate()
            .into_iter()
            .map(|t| {
                let face = &mesh.faces[&t.face];
                let p = t.corner_idx.map(|i| mesh.verts[&face.corners[i as usize].vert]);
                TriGeom { face: t.face, p, lo: p[0].min(p[1]).min(p[2]), hi: p[0].max(p[1]).max(p[2]) }
            })
            .collect();
        Some(Self { name, verts: mesh.verts.values().copied().collect(), tris, lo, hi })
    }

    /// Triangles whose (padded) bounds reach into `lo..hi`.
    fn tris_near(&self, lo: Vec3, hi: Vec3, pad: f32) -> Vec<&TriGeom> {
        self.tris.iter().filter(|t| boxes_overlap(t.lo, t.hi, lo, hi, pad)).collect()
    }
}

fn boxes_overlap(alo: Vec3, ahi: Vec3, blo: Vec3, bhi: Vec3, pad: f32) -> bool {
    (alo - Vec3::splat(pad)).cmple(bhi).all() && (blo - Vec3::splat(pad)).cmple(ahi).all()
}

/// Distance between two axis-aligned boxes (0 when they overlap).
fn box_gap(alo: Vec3, ahi: Vec3, blo: Vec3, bhi: Vec3) -> f32 {
    (alo - bhi).max(blo - ahi).max(Vec3::ZERO).length()
}

/// Every (face of a, face of b) whose triangles genuinely pass through each
/// other, in (a face, b face) order.
fn penetrating_pairs(a: &Body, b: &Body) -> Vec<(FaceId, FaceId)> {
    let mut out: BTreeSet<(FaceId, FaceId)> = BTreeSet::new();
    let ta = a.tris_near(b.lo, b.hi, 0.0);
    let tb = b.tris_near(a.lo, a.hi, 0.0);
    for x in &ta {
        for y in &tb {
            if boxes_overlap(x.lo, x.hi, y.lo, y.hi, 0.0) && tri_tri_penetrate(&x.p, &y.p) {
                out.insert((x.face, y.face));
            }
        }
    }
    out.into_iter().collect()
}

/// Möller's interval test, restricted to proper penetration: each triangle
/// must have vertices strictly on both sides of the other's plane, so
/// coplanar, edge-on-face and vertex-on-face contact all read as touching.
fn tri_tri_penetrate(a: &[Vec3; 3], b: &[Vec3; 3]) -> bool {
    let nb = (b[1] - b[0]).cross(b[2] - b[0]);
    let da = a.map(|p| snap(nb.dot(p - b[0])));
    if !straddles(&da) {
        return false;
    }
    let na = (a[1] - a[0]).cross(a[2] - a[0]);
    let db = b.map(|p| snap(na.dot(p - a[0])));
    if !straddles(&db) {
        return false;
    }
    let dir = na.cross(nb);
    let axis = dir.abs().max_position();
    let pa = a.map(|p| p[axis]);
    let pb = b.map(|p| p[axis]);
    let (a0, a1) = interval(&pa, &da);
    let (b0, b1) = interval(&pb, &db);
    a0.max(b0) < a1.min(b1)
}

fn snap(d: f32) -> f32 {
    if d.abs() < PLANE_EPS { 0.0 } else { d }
}

fn straddles(d: &[f32; 3]) -> bool {
    d.iter().any(|&x| x > 0.0) && d.iter().any(|&x| x < 0.0)
}

/// Parameter interval of the triangle's cut along the intersection line.
/// Requires `straddles(d)`: the lone vertex is the one alone on its side
/// (zeros join the majority), so both denominators are non-zero.
fn interval(p: &[f32; 3], d: &[f32; 3]) -> (f32, f32) {
    let lone = (0..3)
        .find(|&i| d[i] != 0.0 && (0..3).all(|j| j == i || d[j] * d[i] <= 0.0))
        .expect("straddling triangle has a lone-side vertex");
    let (j, k) = ((lone + 1) % 3, (lone + 2) % 3);
    let t1 = p[lone] + (p[j] - p[lone]) * d[lone] / (d[lone] - d[j]);
    let t2 = p[lone] + (p[k] - p[lone]) * d[lone] / (d[lone] - d[k]);
    (t1.min(t2), t1.max(t2))
}

/// Min vertex-to-triangle distance in both directions, stopping early once
/// the pair is known to touch (result <= FLOAT_GAP).
fn pair_distance(a: &Body, b: &Body) -> f32 {
    let mut best = f32::INFINITY;
    for (verts, other) in [(&a.verts, b), (&b.verts, a)] {
        let tris = other.tris_near(verts_lo(verts), verts_hi(verts), FLOAT_GAP);
        for &v in verts {
            for t in &tris {
                if !boxes_overlap(t.lo, t.hi, v, v, best.min(FLOAT_GAP)) {
                    continue;
                }
                best = best.min(point_tri_distance(v, &t.p));
                if best <= FLOAT_GAP {
                    return best;
                }
            }
        }
    }
    best
}

fn verts_lo(v: &[Vec3]) -> Vec3 {
    v.iter().copied().fold(Vec3::INFINITY, Vec3::min)
}

fn verts_hi(v: &[Vec3]) -> Vec3 {
    v.iter().copied().fold(Vec3::NEG_INFINITY, Vec3::max)
}

/// Ericson, Real-Time Collision Detection §5.1.5 (closest point on triangle).
fn point_tri_distance(p: Vec3, t: &[Vec3; 3]) -> f32 {
    let [a, b, c] = *t;
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length();
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length();
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (p - (a + ab * v)).length();
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length();
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (p - (a + ac * w)).length();
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (p - (b + (c - b) * w)).length();
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    (p - (a + ab * v + ac * w)).length()
}
