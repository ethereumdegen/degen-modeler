//! `uv.*` gate rules over plain per-object inputs. This crate sits below
//! `dgm-scene`, so instead of a `Doc` the caller (dgm-jev's `gate`, via
//! integration) passes each object's mesh, its [`TexInfo`], and the union
//! of its declared mirror-set faces. See the crate docs for the adaptation
//! table.
//!
//! Rules (stable ids):
//! - `uv.overlap` (Hard): two island bboxes overlap and neither island
//!   carries a declared mirror face. Trim-bound objects are skipped —
//!   stacking islands on a trim region is the whole point of trims.
//! - `uv.out_of_bounds` (Hard): corner UVs outside `[0,1]` on a non-trim
//!   textured object.
//! - `uv.texel_band` (Hard): face texel density outside the pack band;
//!   needs a texture size, so objects without one are skipped.
//! - `uv.stretch` (Warn): face 3D/UV area ratio beyond 2x the object
//!   median.
//! - `uv.waste` (Warn): unused fraction of an owned (non-trim) atlas above
//!   the pack's `uv_waste_max`.

use std::collections::{BTreeMap, BTreeSet};

use dgm_atlas::Band;
use dgm_mesh::{FaceId, Finding, Mesh};
use glam::Vec2;

use crate::geom::{face_uv_area, island_uv_bbox};
use crate::islands::islands;

/// What the gate needs to know about an object's bound texture.
///
/// Adapt from `dgm_scene::Material`: `Trim { sheet }` -> `TexInfo { px: max
/// sheet edge, trim: true }`, `File { .. }` -> `TexInfo { px: image edge,
/// trim: false }`, `Color { .. }` or no material -> `None`.
#[derive(Debug, Clone, Copy)]
pub struct TexInfo {
    pub px: u32,
    pub trim: bool,
}

const EPS: f32 = 1e-5;
const AREA_EPS: f32 = 1e-12;
const MAX_ELEMS: usize = 8;
/// Relative slack on the band edges so exact-target densities never flap.
const BAND_TOL: f32 = 1e-3;

/// Run every `uv.*` rule over the given objects (name order; rule order
/// fixed per object), returning deterministic findings.
pub fn uv_findings(
    meshes: &BTreeMap<String, (&Mesh, Option<TexInfo>)>,
    mirror_faces: &BTreeMap<String, BTreeSet<FaceId>>,
    band: Band,
    uv_waste_max: f32,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for (name, &(mesh, tex)) in meshes {
        let is_trim = tex.is_some_and(|t| t.trim);
        if !is_trim {
            overlap_rule(name, mesh, mirror_faces.get(name), &mut out);
        }
        if let Some(t) = tex {
            if !t.trim {
                out_of_bounds_rule(name, mesh, &mut out);
            }
            texel_band_rule(name, mesh, t.px, band, &mut out);
        }
        stretch_rule(name, mesh, &mut out);
        if let Some(t) = tex
            && !t.trim
        {
            waste_rule(name, mesh, uv_waste_max, &mut out);
        }
    }
    out
}

fn overlap_rule(
    object: &str,
    mesh: &Mesh,
    mirrors: Option<&BTreeSet<FaceId>>,
    out: &mut Vec<Finding>,
) {
    let isl = islands(mesh);
    let boxes: Vec<(Vec2, Vec2)> = isl.iter().map(|i| island_uv_bbox(mesh, i)).collect();
    for a in 0..isl.len() {
        for b in a + 1..isl.len() {
            let ((alo, ahi), (blo, bhi)) = (boxes[a], boxes[b]);
            let overlap = alo.x < bhi.x - EPS
                && blo.x < ahi.x - EPS
                && alo.y < bhi.y - EPS
                && blo.y < ahi.y - EPS;
            if !overlap {
                continue;
            }
            let declared = mirrors.is_some_and(|m| {
                isl[a].iter().any(|f| m.contains(f)) || isl[b].iter().any(|f| m.contains(f))
            });
            if declared {
                continue;
            }
            let (fa, fb) =
                (*isl[a].first().expect("non-empty"), *isl[b].first().expect("non-empty"));
            out.push(
                Finding::hard(
                    "uv.overlap",
                    format!(
                        "UV islands at {fa} and {fb} overlap on `{object}` and no mirror set declares them"
                    ),
                )
                .with_elems(vec![fa.to_string(), fb.to_string()]),
            );
        }
    }
}

fn out_of_bounds_rule(object: &str, mesh: &Mesh, out: &mut Vec<Finding>) {
    let bad: Vec<FaceId> = mesh
        .faces
        .iter()
        .filter(|(_, face)| {
            face.corners.iter().any(|c| {
                c.uv.x < -EPS || c.uv.x > 1.0 + EPS || c.uv.y < -EPS || c.uv.y > 1.0 + EPS
            })
        })
        .map(|(&f, _)| f)
        .collect();
    if bad.is_empty() {
        return;
    }
    out.push(
        Finding::hard(
            "uv.out_of_bounds",
            format!("{} faces ({}) have UVs outside [0,1] on `{object}`", bad.len(), preview(&bad)),
        )
        .with_elems(elems(&bad)),
    );
}

fn texel_band_rule(object: &str, mesh: &Mesh, px: u32, band: Band, out: &mut Vec<Finding>) {
    let mut bad: Vec<FaceId> = Vec::new();
    let mut worst = (0.0f32, 0.0f32); // (deviation, density)
    for (&f, face) in &mesh.faces {
        let world = mesh.face_area(f).unwrap_or(0.0);
        if world < AREA_EPS {
            continue; // mesh.degenerate_face owns that case
        }
        let density = px as f32 * (face_uv_area(face) / world).sqrt();
        let deviation = if density < band.min * (1.0 - BAND_TOL) {
            band.min - density
        } else if density > band.max * (1.0 + BAND_TOL) {
            density - band.max
        } else {
            continue;
        };
        bad.push(f);
        if deviation > worst.0 {
            worst = (deviation, density);
        }
    }
    if bad.is_empty() {
        return;
    }
    out.push(
        Finding::hard(
            "uv.texel_band",
            format!(
                "{} faces ({}) are outside the texel band [{}..{}] tex/m on `{object}`, worst {:.1}",
                bad.len(),
                preview(&bad),
                band.min,
                band.max,
                worst.1
            ),
        )
        .with_elems(elems(&bad))
        .with_value(worst.1 as f64),
    );
}

fn stretch_rule(object: &str, mesh: &Mesh, out: &mut Vec<Finding>) {
    let mut ratios: Vec<(FaceId, f32)> = Vec::new();
    for (&f, face) in &mesh.faces {
        let world = mesh.face_area(f).unwrap_or(0.0);
        let uv = face_uv_area(face);
        if world < AREA_EPS || uv < AREA_EPS {
            continue;
        }
        ratios.push((f, world / uv));
    }
    if ratios.len() < 2 {
        return;
    }
    let mut sorted: Vec<f32> = ratios.iter().map(|&(_, r)| r).collect();
    sorted.sort_by(f32::total_cmp);
    let mid = sorted.len() / 2;
    let median = if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        0.5 * (sorted[mid - 1] + sorted[mid])
    };
    if median < f32::MIN_POSITIVE {
        return;
    }
    let mut bad: Vec<FaceId> = Vec::new();
    let mut worst = 1.0f32;
    for (f, r) in ratios {
        let spread = (r / median).max(median / r);
        if spread > 2.0 {
            bad.push(f);
            worst = worst.max(spread);
        }
    }
    if bad.is_empty() {
        return;
    }
    out.push(
        Finding::warn(
            "uv.stretch",
            format!(
                "{} faces ({}) stretch beyond 2x the median 3D/UV area ratio on `{object}`",
                bad.len(),
                preview(&bad)
            ),
        )
        .with_elems(elems(&bad))
        .with_value(worst as f64),
    );
}

fn waste_rule(object: &str, mesh: &Mesh, uv_waste_max: f32, out: &mut Vec<Finding>) {
    let used: f32 = mesh.faces.values().map(face_uv_area).sum();
    let waste = (1.0 - used).clamp(0.0, 1.0);
    if waste <= uv_waste_max {
        return;
    }
    out.push(
        Finding::warn(
            "uv.waste",
            format!("atlas waste {waste:.2} exceeds {uv_waste_max:.2} on `{object}`"),
        )
        .with_value(waste as f64),
    );
}

fn elems(faces: &[FaceId]) -> Vec<String> {
    faces.iter().take(MAX_ELEMS).map(|f| f.to_string()).collect()
}

fn preview(faces: &[FaceId]) -> String {
    let mut s = elems(faces).join(", ");
    if faces.len() > MAX_ELEMS {
        s.push_str(", …");
    }
    s
}
