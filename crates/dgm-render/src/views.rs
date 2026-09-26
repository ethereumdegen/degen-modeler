//! Measurement views. Layout is contract: sheets are a 4x2 grid of
//! 45-degree yaw steps at 20 degrees elevation, filmstrips one row of
//! evenly spaced yaw frames, silhouette masks one image per view.

use dgm_atlas::{Band, Pack};
use dgm_mesh::EdgeKey;
use dgm_scene::{Doc, TextureRef};
use glam::{Vec2, Vec3};
use image::{GrayImage, Rgba, RgbaImage};

use crate::camera::{Camera, SV, orbit, project};
use crate::geom::{SceneGeom, TriGeom, scene_geom, seam_edges, world_tris};
use crate::raster::{Target, fill_tri, fill_tri_mask, line2, line3};
use crate::{RenderError, check_px};

pub(crate) const ELEVATION_DEG: f32 = 20.0;
const WIRE: [u8; 3] = [232, 232, 232];
const SEAM_RED: [u8; 4] = [230, 64, 52, 255];
const ISLAND_WIRE: [u8; 4] = [235, 235, 235, 255];
const UV_DARK: [u8; 4] = [30, 30, 35, 255];

/// Fixed camera-space light: over the right shoulder, slightly above.
fn light() -> Vec3 {
    Vec3::new(0.32, 0.57, 0.755).normalize()
}

/// Dim lambert on the flat face normal, front-facing by construction so
/// double-sided/backfacing geometry still reads.
fn shade(cam: &Camera, normal: Vec3) -> f32 {
    let n = (cam.view * normal.extend(0.0)).truncate().normalize_or_zero();
    let n = if n.z < 0.0 { -n } else { n };
    0.34 + 0.62 * n.dot(light()).max(0.0)
}

fn mul(c: [u8; 4], s: f32) -> [u8; 3] {
    [(c[0] as f32 * s) as u8, (c[1] as f32 * s) as u8, (c[2] as f32 * s) as u8]
}

fn project_tri(cam: &Camera, px: u32, tri: &TriGeom) -> Option<[SV; 3]> {
    Some([
        project(cam, px, tri.pos[0], tri.uv[0])?,
        project(cam, px, tri.pos[1], tri.uv[1])?,
        project(cam, px, tri.pos[2], tri.uv[2])?,
    ])
}

fn textured_view(scene: &SceneGeom, cam: &Camera, px: u32, fill_scale: f32) -> Target {
    let mut t = Target::checker(px);
    for obj in &scene.objects {
        for tri in &obj.tris {
            let Some(v) = project_tri(cam, px, tri) else { continue };
            let s = shade(cam, tri.normal) * fill_scale;
            fill_tri(&mut t, &v, |uv| {
                let c = obj.tex.sample(uv);
                if obj.alpha_mask && c[3] < 128 {
                    return None;
                }
                Some(mul(c, s))
            });
        }
    }
    t
}

/// Assemble `cols x rows` square tiles into one sheet.
fn sheet(cols: u32, rows: u32, px: u32, mut tile: impl FnMut(u32) -> RgbaImage) -> RgbaImage {
    let mut out = RgbaImage::new(cols * px, rows * px);
    for i in 0..cols * rows {
        let img = tile(i);
        let (ox, oy) = ((i % cols) * px, (i / cols) * px);
        for (x, y, p) in img.enumerate_pixels() {
            out.put_pixel(ox + x, oy + y, *p);
        }
    }
    out
}

/// 8 views (yaw steps of 45 degrees, 20 degrees elevation) in a 4x2 grid.
pub fn contact_sheet(doc: &Doc, pack: &Pack, view_px: u32) -> Result<RgbaImage, RenderError> {
    check_px(view_px)?;
    let scene = scene_geom(doc, pack, false);
    let (center, radius) = scene.framing();
    Ok(sheet(4, 2, view_px, |i| {
        let cam = orbit(center, radius, i as f32 * 45.0, ELEVATION_DEG);
        textured_view(&scene, &cam, view_px, 1.0).color
    }))
}

/// Same grid; edges as depth-tested 1px lines over a faint fill.
pub fn wireframe_sheet(doc: &Doc, pack: &Pack, view_px: u32) -> Result<RgbaImage, RenderError> {
    check_px(view_px)?;
    let scene = scene_geom(doc, pack, false);
    let (center, radius) = scene.framing();
    Ok(sheet(4, 2, view_px, |i| {
        let cam = orbit(center, radius, i as f32 * 45.0, ELEVATION_DEG);
        let mut t = textured_view(&scene, &cam, view_px, 0.35);
        for obj in &scene.objects {
            for &(a, b) in &obj.wires {
                let (Some(sa), Some(sb)) = (
                    project(&cam, view_px, a, Vec2::ZERO),
                    project(&cam, view_px, b, Vec2::ZERO),
                ) else {
                    continue;
                };
                line3(&mut t, &sa, &sb, WIRE, 2e-3);
            }
        }
        t.color
    }))
}

fn heat_color(density: Option<f32>, band: &Band) -> [u8; 4] {
    match density {
        None => [150, 150, 150, 255],
        Some(d) if d < band.min => [64, 96, 224, 255],
        Some(d) if d > band.max => [224, 64, 56, 255],
        Some(_) => [72, 200, 96, 255],
    }
}

/// Face texel density vs the pack band: blue under, green in, red over;
/// gray where density is unknown (no texture).
pub fn heatmap(doc: &Doc, pack: &Pack, view_px: u32) -> Result<RgbaImage, RenderError> {
    check_px(view_px)?;
    let scene = scene_geom(doc, pack, false);
    let (center, radius) = scene.framing();
    let band = pack.manifest.texel_density;
    Ok(sheet(4, 2, view_px, |i| {
        let cam = orbit(center, radius, i as f32 * 45.0, ELEVATION_DEG);
        let mut t = Target::checker(view_px);
        for obj in &scene.objects {
            for tri in &obj.tris {
                let Some(v) = project_tri(&cam, view_px, tri) else { continue };
                let s = shade(&cam, tri.normal);
                let c = mul(heat_color(obj.faces[&tri.face].density, &band), s);
                fill_tri(&mut t, &v, |_| Some(c));
            }
        }
        t.color
    }))
}

/// Turntable: `frames` yaw steps of 360/frames degrees in one row.
pub fn filmstrip(
    doc: &Doc,
    pack: &Pack,
    frames: u32,
    view_px: u32,
) -> Result<RgbaImage, RenderError> {
    check_px(view_px)?;
    if frames == 0 {
        return Err(RenderError::Invalid("filmstrip needs at least 1 frame".into()));
    }
    let scene = scene_geom(doc, pack, false);
    let (center, radius) = scene.framing();
    let step = 360.0 / frames as f32;
    Ok(sheet(frames, 1, view_px, |i| {
        let cam = orbit(center, radius, i as f32 * step, ELEVATION_DEG);
        textured_view(&scene, &cam, view_px, 1.0).color
    }))
}

/// Render `tris` white-on-black from `views` evenly spaced yaws.
pub(crate) fn mask_views(
    tris: &[[Vec3; 3]],
    center: Vec3,
    radius: f32,
    views: u32,
    px: u32,
) -> Vec<GrayImage> {
    let mut out = Vec::with_capacity(views as usize);
    for i in 0..views {
        let cam = orbit(center, radius, i as f32 * 360.0 / views as f32, ELEVATION_DEG);
        let mut img = GrayImage::new(px, px);
        for tri in tris {
            let (Some(a), Some(b), Some(c)) = (
                project(&cam, px, tri[0], Vec2::ZERO),
                project(&cam, px, tri[1], Vec2::ZERO),
                project(&cam, px, tri[2], Vec2::ZERO),
            ) else {
                continue;
            };
            fill_tri_mask(&mut img, &[(a.x, a.y), (b.x, b.y), (c.x, c.y)]);
        }
        out.push(img);
    }
    out
}

/// The object filled white on black, one mask per view, framed on the
/// object's own bounds.
pub fn silhouette_masks(
    doc: &Doc,
    object: &str,
    views: u32,
    px: u32,
) -> Result<Vec<GrayImage>, RenderError> {
    check_px(px)?;
    let obj = doc
        .objects
        .get(object)
        .ok_or_else(|| RenderError::UnknownObject(object.into()))?;
    if views == 0 {
        return Ok(Vec::new());
    }
    let (lo, hi) = obj.mesh.bounds().unwrap_or((Vec3::splat(-0.5), Vec3::splat(0.5)));
    let center = (lo + hi) * 0.5;
    let radius = ((hi - lo).length() * 0.5).max(1e-3);
    Ok(mask_views(&world_tris(&obj.mesh), center, radius, views, px))
}

/// The object's texture dimmed, its UV wires bright, seam edges red.
pub fn uv_layout(doc: &Doc, pack: &Pack, object: &str) -> Result<RgbaImage, RenderError> {
    let obj = doc
        .objects
        .get(object)
        .ok_or_else(|| RenderError::UnknownObject(object.into()))?;
    let mat = obj.material.as_deref().and_then(|m| doc.materials.get(m));
    let mut img = match mat.map(|m| &m.texture) {
        Some(TextureRef::Trim { sheet }) => match pack.manifest.trims.get(sheet) {
            Some(trim) => uv_canvas(
                trim.size[0],
                trim.size[1],
                image::ImageReader::open(pack.root.join(&trim.file))
                    .ok()
                    .and_then(|r| r.decode().ok())
                    .map(|d| d.to_rgba8()),
                UV_DARK,
            ),
            None => uv_canvas(512, 512, None, UV_DARK),
        },
        Some(TextureRef::File { path }) => {
            let loaded = image::ImageReader::open(path)
                .ok()
                .and_then(|r| r.decode().ok())
                .map(|d| d.to_rgba8());
            match loaded {
                Some(tex) => {
                    let (w, h) = (tex.width(), tex.height());
                    uv_canvas(w, h, Some(tex), UV_DARK)
                }
                None => uv_canvas(512, 512, None, UV_DARK),
            }
        }
        Some(TextureRef::Color { rgba }) => {
            let bg = [
                (rgba[0] as f32 * 0.22) as u8,
                (rgba[1] as f32 * 0.22) as u8,
                (rgba[2] as f32 * 0.22) as u8,
                255,
            ];
            uv_canvas(512, 512, None, bg)
        }
        None => uv_canvas(512, 512, None, UV_DARK),
    };
    let (w, h) = (img.width() as f32, img.height() as f32);
    let seams = seam_edges(&obj.mesh);
    for face in obj.mesh.faces.values() {
        let n = face.corners.len();
        for i in 0..n {
            let a = &face.corners[i];
            let b = &face.corners[(i + 1) % n];
            let color = if seams.contains(&EdgeKey::new(a.vert, b.vert)) {
                SEAM_RED
            } else {
                ISLAND_WIRE
            };
            line2(
                &mut img,
                Vec2::new(a.uv.x * w, a.uv.y * h),
                Vec2::new(b.uv.x * w, b.uv.y * h),
                color,
            );
        }
    }
    Ok(img)
}

/// Canvas for the UV layout: the texture dimmed to 35%, or a flat fill.
fn uv_canvas(w: u32, h: u32, tex: Option<RgbaImage>, fill: [u8; 4]) -> RgbaImage {
    let mut img = RgbaImage::new(w.max(1), h.max(1));
    match tex {
        Some(tex) if tex.width() == img.width() && tex.height() == img.height() => {
            for (x, y, p) in img.enumerate_pixels_mut() {
                let c = tex.get_pixel(x, y).0;
                *p = Rgba([
                    (c[0] as f32 * 0.35) as u8,
                    (c[1] as f32 * 0.35) as u8,
                    (c[2] as f32 * 0.35) as u8,
                    255,
                ]);
            }
        }
        Some(tex) => {
            // Size drifted from the manifest: nearest-scale it under the wires.
            let (tw, th) = (tex.width() as f32, tex.height() as f32);
            let (iw, ih) = (img.width() as f32, img.height() as f32);
            for (x, y, p) in img.enumerate_pixels_mut() {
                let tx = ((x as f32 / iw * tw) as u32).min(tex.width() - 1);
                let ty = ((y as f32 / ih * th) as u32).min(tex.height() - 1);
                let c = tex.get_pixel(tx, ty).0;
                *p = Rgba([
                    (c[0] as f32 * 0.35) as u8,
                    (c[1] as f32 * 0.35) as u8,
                    (c[2] as f32 * 0.35) as u8,
                    255,
                ]);
            }
        }
        None => {
            for p in img.pixels_mut() {
                *p = Rgba(fill);
            }
        }
    }
    img
}
