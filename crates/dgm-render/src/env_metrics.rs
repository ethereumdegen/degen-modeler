//! Environment metrics: object connectivity (does every piece touch the
//! rest?) and texture repetition (how many times does one sheet tile across
//! the surface it covers?).

use std::collections::BTreeMap;

use dgm_atlas::Pack;
use dgm_scene::{Doc, TextureRef};
use glam::Vec3;

use crate::RenderError;
use crate::geom::world_tris;

/// Two objects "touch" when any vertex of one lies within this distance of
/// the other's surface, or an edge of one crosses the other's surface.
pub(crate) const CONTACT_EPS: f32 = 0.02;
/// Grid resolution (cells per axis) for the triangle lookup.
const GRID_N: usize = 24;
/// Tile repeats at which `repetition` saturates to 1.0 (log-scaled: one
/// tile = 0, 2^REP_SATURATE_LOG2 tiles = 1).
const REP_SATURATE_LOG2: f32 = 6.0;

// ---- connectivity -----------------------------------------------------------

struct TriGrid {
    tris: Vec<[Vec3; 3]>,
    lo: Vec3,
    inv_cell: Vec3,
    cells: Vec<Vec<u32>>,
}

impl TriGrid {
    fn new(tris: Vec<[Vec3; 3]>) -> Self {
        let mut lo = Vec3::INFINITY;
        let mut hi = Vec3::NEG_INFINITY;
        for t in &tris {
            for p in t {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
        }
        let extent = (hi - lo).max(Vec3::splat(1e-6));
        let inv_cell = Vec3::splat(GRID_N as f32) / extent;
        let mut cells = vec![Vec::new(); GRID_N * GRID_N * GRID_N];
        for (i, t) in tris.iter().enumerate() {
            let tlo = t[0].min(t[1]).min(t[2]);
            let thi = t[0].max(t[1]).max(t[2]);
            let (a, b) = (Self::cell_of(lo, inv_cell, tlo), Self::cell_of(lo, inv_cell, thi));
            for x in a[0]..=b[0] {
                for y in a[1]..=b[1] {
                    for z in a[2]..=b[2] {
                        cells[(x * GRID_N + y) * GRID_N + z].push(i as u32);
                    }
                }
            }
        }
        Self { tris, lo, inv_cell, cells }
    }

    fn cell_of(lo: Vec3, inv_cell: Vec3, p: Vec3) -> [usize; 3] {
        let c = ((p - lo) * inv_cell).floor();
        let clamp = |v: f32| v.clamp(0.0, (GRID_N - 1) as f32) as usize;
        [clamp(c.x), clamp(c.y), clamp(c.z)]
    }

    /// Triangle indices whose cells overlap the box `[lo, hi]`; a triangle
    /// spanning several cells is yielded once per cell (harmless: callers
    /// stop at the first hit).
    fn candidates(&self, lo: Vec3, hi: Vec3, mut f: impl FnMut(&[Vec3; 3]) -> bool) -> bool {
        let a = Self::cell_of(self.lo, self.inv_cell, lo);
        let b = Self::cell_of(self.lo, self.inv_cell, hi);
        for x in a[0]..=b[0] {
            for y in a[1]..=b[1] {
                for z in a[2]..=b[2] {
                    for &i in &self.cells[(x * GRID_N + y) * GRID_N + z] {
                        if f(&self.tris[i as usize]) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn bounds(&self) -> (Vec3, Vec3) {
        let hi = self.lo + Vec3::splat(GRID_N as f32) / self.inv_cell;
        (self.lo, hi)
    }

    /// True when `p` is within `eps` of any triangle.
    fn near_point(&self, p: Vec3, eps: f32) -> bool {
        let e = Vec3::splat(eps);
        let eps2 = eps * eps;
        self.candidates(p - e, p + e, |t| point_tri_dist2(p, t) <= eps2)
    }

    /// True when segment `a-b` crosses any triangle.
    fn crosses(&self, a: Vec3, b: Vec3) -> bool {
        self.candidates(a.min(b), a.max(b), |t| segment_hits_tri(a, b, t))
    }
}

/// Squared distance from `p` to the closest point on the triangle
/// (Ericson, Real-Time Collision Detection 5.1.5).
fn point_tri_dist2(p: Vec3, t: &[Vec3; 3]) -> f32 {
    let [a, b, c] = *t;
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length_squared();
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length_squared();
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (p - (a + ab * v)).length_squared();
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length_squared();
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (p - (a + ac * w)).length_squared();
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (p - (b + (c - b) * w)).length_squared();
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (p - (a + ab * v + ac * w)).length_squared()
}

/// Möller–Trumbore, segment parameter clamped to `[0, 1]`.
fn segment_hits_tri(a: Vec3, b: Vec3, t: &[Vec3; 3]) -> bool {
    let dir = b - a;
    let e1 = t[1] - t[0];
    let e2 = t[2] - t[0];
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return false;
    }
    let inv = 1.0 / det;
    let s = a - t[0];
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    let tt = e2.dot(q) * inv;
    (0.0..=1.0).contains(&tt)
}

fn bbox_overlap(a: (Vec3, Vec3), b: (Vec3, Vec3), eps: f32) -> bool {
    let e = Vec3::splat(eps);
    (a.0 - e).cmple(b.1 + e).all() && (b.0 - e).cmple(a.1 + e).all()
}

/// Fraction of non-LOD, non-empty objects that touch another one: a vertex
/// within [`CONTACT_EPS`] of the other's surface, or an edge crossing it.
/// 1.0 with fewer than two such objects (nothing can be disconnected).
pub(crate) fn connectivity(doc: &Doc) -> f32 {
    let objects: Vec<(&dgm_mesh::Mesh, TriGrid)> = doc
        .objects
        .values()
        .filter(|o| o.lod_of.is_none() && !o.mesh.faces.is_empty())
        .map(|o| (&o.mesh, TriGrid::new(world_tris(&o.mesh))))
        .collect();
    if objects.len() < 2 {
        return 1.0;
    }
    let mut touched = vec![false; objects.len()];
    for i in 0..objects.len() {
        for j in (i + 1)..objects.len() {
            if touched[i] && touched[j] {
                continue;
            }
            let (ma, ga) = &objects[i];
            let (mb, gb) = &objects[j];
            if !bbox_overlap(ga.bounds(), gb.bounds(), CONTACT_EPS) {
                continue;
            }
            if objects_touch(ma, gb) || objects_touch(mb, ga) {
                touched[i] = true;
                touched[j] = true;
            }
        }
    }
    touched.iter().filter(|&&t| t).count() as f32 / objects.len() as f32
}

fn objects_touch(mesh: &dgm_mesh::Mesh, other: &TriGrid) -> bool {
    let (olo, ohi) = other.bounds();
    let e = Vec3::splat(CONTACT_EPS);
    if mesh
        .verts
        .values()
        .filter(|p| p.cmpge(olo - e).all() && p.cmple(ohi + e).all())
        .any(|&p| other.near_point(p, CONTACT_EPS))
    {
        return true;
    }
    mesh.edges().into_iter().any(|edge| {
        let (a, b) = (mesh.verts[&edge.0], mesh.verts[&edge.1]);
        bbox_overlap((a.min(b), a.max(b)), (olo, ohi), 0.0) && other.crosses(a, b)
    })
}

// ---- repetition -------------------------------------------------------------

/// How visibly a texture repeats: for every textured sheet (trim or file),
/// sum the UV area of every face mapped onto it — in whole-sheet units —
/// i.e. how many copies of the sheet the eye sees across that surface.
/// `repetition = clamp(log2(max repeats) / 6)`: one tile reads 0, 64
/// tiles read 1. Deterministic and render-free; this is the quantity that
/// made one 512² rock sheet read as wallpaper over a 25 m cave.
/// `None` when nothing is textured.
pub(crate) fn repetition(doc: &Doc, _pack: &Pack) -> Result<Option<f32>, RenderError> {
    let mut repeats: BTreeMap<String, f32> = BTreeMap::new();
    for obj in doc.objects.values() {
        if obj.lod_of.is_some() {
            continue;
        }
        let Some(mat) = obj.material.as_ref().and_then(|m| doc.materials.get(m)) else {
            continue;
        };
        let key = match &mat.texture {
            TextureRef::Trim { sheet } => format!("trim:{sheet}"),
            TextureRef::File { path } => format!("file:{path}"),
            TextureRef::Color { .. } => continue,
        };
        let area: f32 = obj.mesh.faces.values().map(crate::geom::uv_poly_area).sum();
        *repeats.entry(key).or_default() += area;
    }
    Ok(repeats
        .values()
        .copied()
        .reduce(f32::max)
        .map(|r| (r.max(1.0).log2() / REP_SATURATE_LOG2).clamp(0.0, 1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_triangle_distance_regions() {
        let t = [Vec3::ZERO, Vec3::X, Vec3::Z];
        assert!((point_tri_dist2(Vec3::new(0.25, 0.5, 0.25), &t) - 0.25).abs() < 1e-6); // face
        assert!((point_tri_dist2(Vec3::new(-1.0, 0.0, -1.0), &t) - 2.0).abs() < 1e-6); // vertex
        assert!((point_tri_dist2(Vec3::new(0.5, 0.0, -1.0), &t) - 1.0).abs() < 1e-6); // edge
    }

    #[test]
    fn segment_triangle_crossing() {
        let t = [Vec3::ZERO, Vec3::X, Vec3::Z];
        assert!(segment_hits_tri(Vec3::new(0.2, 1.0, 0.2), Vec3::new(0.2, -1.0, 0.2), &t));
        assert!(!segment_hits_tri(Vec3::new(0.2, 1.0, 0.2), Vec3::new(0.2, 0.5, 0.2), &t));
        assert!(!segment_hits_tri(Vec3::new(2.0, 1.0, 2.0), Vec3::new(2.0, -1.0, 2.0), &t));
    }

}
