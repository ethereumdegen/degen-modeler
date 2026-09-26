//! Numeric scene metrics. Field names are contract; see CONTRACT.md
//! §RenderMetrics. Global aggregates pool the faces of every non-LOD
//! object; `per_object` covers every object (LODs included).

use std::collections::BTreeMap;

use dgm_atlas::Pack;
use dgm_mesh::Mesh;
use dgm_scene::Doc;
use glam::{Vec2, Vec3};
use image::GrayImage;
use serde::Serialize;

use crate::RenderError;
use crate::geom::{ObjGeom, Tex, TexKind, edge_uvs, scene_geom, seam_edges, world_tris};
use crate::raster::fill_tri_mask;
use crate::views::mask_views;

/// Grid edge for UV occupancy / used-texel sampling.
const COVER_PX: u32 = 256;
/// Views/resolution for the LOD mask-drift comparison.
const DRIFT_VIEWS: u32 = 4;
const DRIFT_PX: u32 = 128;

#[derive(Debug, Clone, Serialize)]
pub struct Metrics {
    /// Texture colour delta sampled across UV seam edges, 0-1.
    pub seam_contrast_max: f32,
    pub seam_contrast_mean: f32,
    /// Rasterized UV coverage of [0,1]^2 for non-trim materials; `null`
    /// when every textured object sits on a trim sheet.
    pub uv_occupancy: Option<f32>,
    /// Per-face 3D/UV area ratio deviation from the median (>= 1).
    pub stretch_max: f32,
    pub stretch_mean: f32,
    /// Mean nearest-palette RGB distance of used texels, 0-1.
    pub palette_distance: f32,
    /// max/min face texel density (1.0 when fewer than two textured faces).
    pub density_spread: f32,
    /// Mean silhouette pixel diff between LOD0 and the highest LOD, 0-1;
    /// `null` without LODs.
    pub mask_drift: Option<f32>,
    pub per_object: BTreeMap<String, ObjectMetrics>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObjectMetrics {
    pub tris: u32,
    pub seam_contrast_max: f32,
    pub seam_contrast_mean: f32,
    pub stretch_max: f32,
    pub stretch_mean: f32,
    pub density_min: Option<f32>,
    pub density_max: Option<f32>,
    /// True when the material references texture bytes not on disk (the
    /// renderer fell back to flat 0x808080).
    pub missing_texture: bool,
}

pub fn metrics(doc: &Doc, pack: &Pack) -> Result<Metrics, RenderError> {
    let scene = scene_geom(doc, pack, true);
    let mut per_object = BTreeMap::new();
    let mut g_seam: Vec<f32> = Vec::new();
    let mut g_ratio: Vec<f32> = Vec::new();
    let mut g_density: Vec<f32> = Vec::new();
    let mut palette_acc = 0.0f64;
    let mut palette_n = 0u64;
    let palette = pack.palette_rgb();
    let mut occupancy_grid = GrayImage::new(COVER_PX, COVER_PX);
    let mut any_non_trim = false;

    for obj in &scene.objects {
        let mesh = &doc.objects[&obj.name].mesh;
        let seam = seam_deltas(mesh, &obj.tex);
        let ratios: Vec<f32> = obj
            .faces
            .values()
            .filter(|f| f.world_area > 1e-12 && f.uv_area > 1e-12)
            .map(|f| f.world_area / f.uv_area)
            .collect();
        let densities: Vec<f32> = obj.faces.values().filter_map(|f| f.density).collect();
        let (stretch_max, stretch_mean) = stretch_stats(&ratios);
        per_object.insert(
            obj.name.clone(),
            ObjectMetrics {
                tris: mesh.tri_count(),
                seam_contrast_max: fold_max(&seam),
                seam_contrast_mean: mean(&seam),
                stretch_max,
                stretch_mean,
                density_min: densities.iter().copied().reduce(f32::min),
                density_max: densities.iter().copied().reduce(f32::max),
                missing_texture: obj.missing_texture,
            },
        );

        if obj.is_lod {
            continue;
        }
        g_seam.extend_from_slice(&seam);
        g_ratio.extend_from_slice(&ratios);
        g_density.extend_from_slice(&densities);
        if obj.kind != TexKind::Trim && !mesh.faces.is_empty() {
            any_non_trim = true;
            rasterize_uv_coverage(mesh, &mut occupancy_grid);
        }
        sample_palette(obj, &palette, &mut palette_acc, &mut palette_n);
    }

    let (stretch_max, stretch_mean) = stretch_stats(&g_ratio);
    let density_spread = match (
        g_density.iter().copied().reduce(f32::min),
        g_density.iter().copied().reduce(f32::max),
    ) {
        (Some(lo), Some(hi)) if lo > 0.0 => hi / lo,
        _ => 1.0,
    };
    let uv_occupancy = any_non_trim.then(|| {
        let covered = occupancy_grid.pixels().filter(|p| p.0[0] > 0).count();
        covered as f32 / (COVER_PX * COVER_PX) as f32
    });

    Ok(Metrics {
        seam_contrast_max: fold_max(&g_seam),
        seam_contrast_mean: mean(&g_seam),
        uv_occupancy,
        stretch_max,
        stretch_mean,
        palette_distance: if palette_n == 0 { 0.0 } else { (palette_acc / palette_n as f64) as f32 },
        density_spread,
        mask_drift: mask_drift(doc),
        per_object,
    })
}

fn fold_max(v: &[f32]) -> f32 {
    v.iter().copied().fold(0.0, f32::max)
}

fn mean(v: &[f32]) -> f32 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 }
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) * 0.5 })
}

/// (max, mean) deviation factor of each ratio from the median; 1.0 = no
/// stretch spread, and the neutral answer when there is no UV data.
fn stretch_stats(ratios: &[f32]) -> (f32, f32) {
    let Some(m) = median(ratios.to_vec()) else {
        return (1.0, 1.0);
    };
    if m <= 0.0 {
        return (1.0, 1.0);
    }
    let mut max = 1.0f32;
    let mut acc = 0.0f32;
    for &r in ratios {
        let d = (r / m).max(m / r);
        max = max.max(d);
        acc += d;
    }
    (max, acc / ratios.len() as f32)
}

/// Per-seam-edge mean colour delta (0-1): 4 samples along the edge on each
/// side, nudged 8% toward the face's UV centroid so nearest sampling lands
/// inside the face's own texels.
fn seam_deltas(mesh: &Mesh, tex: &Tex) -> Vec<f32> {
    let seams = seam_edges(mesh);
    let edge_faces = mesh.edge_faces();
    let mut out = Vec::new();
    for edge in &seams {
        let Some(faces) = edge_faces.get(edge) else { continue };
        if faces.len() != 2 {
            continue;
        }
        let (fa, fb) = (&mesh.faces[&faces[0]], &mesh.faces[&faces[1]]);
        let (Some(a), Some(b)) = (edge_uvs(fa, *edge), edge_uvs(fb, *edge)) else {
            continue;
        };
        let ca = uv_centroid(fa);
        let cb = uv_centroid(fb);
        let mut acc = 0.0;
        for t in [0.2, 0.4, 0.6, 0.8] {
            let ua = a.0.lerp(a.1, t) * 0.92 + ca * 0.08;
            let ub = b.0.lerp(b.1, t) * 0.92 + cb * 0.08;
            acc += color_dist(tex.sample(ua), tex.sample(ub));
        }
        out.push(acc / 4.0);
    }
    out
}

fn uv_centroid(face: &dgm_mesh::Face) -> Vec2 {
    let mut c = Vec2::ZERO;
    for corner in &face.corners {
        c += corner.uv;
    }
    c / face.corners.len() as f32
}

/// Euclidean RGB distance normalized to 0-1.
fn color_dist(a: [u8; 4], b: [u8; 4]) -> f32 {
    let d = Vec3::new(
        (a[0] as f32 - b[0] as f32) / 255.0,
        (a[1] as f32 - b[1] as f32) / 255.0,
        (a[2] as f32 - b[2] as f32) / 255.0,
    );
    d.length() / 3f32.sqrt()
}

fn rasterize_uv_coverage(mesh: &Mesh, grid: &mut GrayImage) {
    let s = COVER_PX as f32;
    for face in mesh.faces.values() {
        for i in 1..face.corners.len() - 1 {
            let (a, b, c) =
                (face.corners[0].uv, face.corners[i].uv, face.corners[i + 1].uv);
            fill_tri_mask(grid, &[(a.x * s, a.y * s), (b.x * s, b.y * s), (c.x * s, c.y * s)]);
        }
    }
}

/// "Used texels": for image textures, the texels under the object's UV
/// coverage on a 256^2 grid; for Color materials the colour itself.
/// Missing/absent textures contribute nothing (their gray fallback is not
/// an artistic choice).
fn sample_palette(obj: &ObjGeom, palette: &[[u8; 3]], acc: &mut f64, n: &mut u64) {
    if palette.is_empty() || obj.missing_texture {
        return;
    }
    match (&obj.tex, obj.kind) {
        (Tex::Flat(c), TexKind::Color) => {
            *acc += nearest_palette(*c, palette) as f64;
            *n += 1;
        }
        (Tex::Image(_), _) => {
            let mut cover = GrayImage::new(COVER_PX, COVER_PX);
            let s = COVER_PX as f32;
            for tri in &obj.tris {
                fill_tri_mask(
                    &mut cover,
                    &[
                        (tri.uv[0].x * s, tri.uv[0].y * s),
                        (tri.uv[1].x * s, tri.uv[1].y * s),
                        (tri.uv[2].x * s, tri.uv[2].y * s),
                    ],
                );
            }
            for (x, y, p) in cover.enumerate_pixels() {
                if p.0[0] == 0 {
                    continue;
                }
                let uv = Vec2::new((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
                *acc += nearest_palette(obj.tex.sample(uv), palette) as f64;
                *n += 1;
            }
        }
        _ => {}
    }
}

fn nearest_palette(c: [u8; 4], palette: &[[u8; 3]]) -> f32 {
    palette
        .iter()
        .map(|p| color_dist(c, [p[0], p[1], p[2], 255]))
        .fold(f32::INFINITY, f32::min)
}

/// Mean silhouette pixel diff between each LOD0 object and its highest
/// LOD, over 4 shared views framed on their union bounds.
fn mask_drift(doc: &Doc) -> Option<f32> {
    let mut drifts = Vec::new();
    for (name, base) in &doc.objects {
        if base.lod_of.is_some() {
            continue;
        }
        let Some(lod) = doc
            .objects
            .values()
            .filter(|o| o.lod_of.as_deref() == Some(name))
            .max_by_key(|o| o.lod_level)
        else {
            continue;
        };
        let (Some((bl, bh)), Some((ll, lh))) = (base.mesh.bounds(), lod.mesh.bounds()) else {
            continue;
        };
        let (lo, hi) = (bl.min(ll), bh.max(lh));
        let center = (lo + hi) * 0.5;
        let radius = ((hi - lo).length() * 0.5).max(1e-3);
        let a = mask_views(&world_tris(&base.mesh), center, radius, DRIFT_VIEWS, DRIFT_PX);
        let b = mask_views(&world_tris(&lod.mesh), center, radius, DRIFT_VIEWS, DRIFT_PX);
        let mut acc = 0.0f64;
        let mut n = 0u64;
        for (ia, ib) in a.iter().zip(&b) {
            for (pa, pb) in ia.pixels().zip(ib.pixels()) {
                acc += (pa.0[0] as f64 - pb.0[0] as f64).abs() / 255.0;
                n += 1;
            }
        }
        drifts.push((acc / n as f64) as f32);
    }
    if drifts.is_empty() { None } else { Some(mean(&drifts)) }
}
