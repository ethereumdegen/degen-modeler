//! Tiny deterministic software rasterizer: z-buffered triangle fill,
//! depth-biased 3D lines (wireframes), plain 2D lines and coverage fills.

use glam::Vec2;
use image::{GrayImage, Luma, Rgba, RgbaImage};

use crate::camera::SV;

pub(crate) const CHECKER_A: [u8; 4] = [26, 26, 31, 255];
pub(crate) const CHECKER_B: [u8; 4] = [37, 37, 44, 255];

/// One square view tile: colour + z-buffer.
pub(crate) struct Target {
    pub px: u32,
    pub color: RgbaImage,
    pub depth: Vec<f32>,
}

impl Target {
    /// Tile pre-filled with the dark 2x2-pixel checker background.
    pub fn checker(px: u32) -> Self {
        let mut color = RgbaImage::new(px, px);
        for (x, y, p) in color.enumerate_pixels_mut() {
            let c = if ((x / 2) + (y / 2)) % 2 == 0 { CHECKER_A } else { CHECKER_B };
            *p = Rgba(c);
        }
        Self { px, color, depth: vec![f32::INFINITY; px as usize * px as usize] }
    }
}

fn orient(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

/// Z-buffered fill; both windings rasterize (no backface cull, the z-buffer
/// sorts). `px_fn` gets the perspective-correct UV and the perspective-
/// correct barycentric weights (corner order of `v`) and returns the pixel
/// colour, or `None` to discard (alpha mask).
pub(crate) fn fill_tri(
    t: &mut Target,
    v: &[SV; 3],
    mut px_fn: impl FnMut(Vec2, [f32; 3]) -> Option<[u8; 3]>,
) {
    let area = orient(v[0].x, v[0].y, v[1].x, v[1].y, v[2].x, v[2].y);
    if area.abs() < 1e-8 {
        return;
    }
    let inv_area = 1.0 / area;
    let dim = t.px as f32;
    let min_x = v.iter().map(|s| s.x).fold(f32::INFINITY, f32::min).floor().max(0.0) as u32;
    let max_x = v.iter().map(|s| s.x).fold(f32::NEG_INFINITY, f32::max).ceil().clamp(0.0, dim) as u32;
    let min_y = v.iter().map(|s| s.y).fold(f32::INFINITY, f32::min).floor().max(0.0) as u32;
    let max_y = v.iter().map(|s| s.y).fold(f32::NEG_INFINITY, f32::max).ceil().clamp(0.0, dim) as u32;
    for y in min_y..max_y {
        let cy = y as f32 + 0.5;
        for x in min_x..max_x {
            let cx = x as f32 + 0.5;
            let w0 = orient(v[1].x, v[1].y, v[2].x, v[2].y, cx, cy) * inv_area;
            let w1 = orient(v[2].x, v[2].y, v[0].x, v[0].y, cx, cy) * inv_area;
            let w2 = orient(v[0].x, v[0].y, v[1].x, v[1].y, cx, cy) * inv_area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = w0 * v[0].z + w1 * v[1].z + w2 * v[2].z;
            let idx = (y * t.px + x) as usize;
            if z >= t.depth[idx] {
                continue;
            }
            let iw = w0 * v[0].inv_w + w1 * v[1].inv_w + w2 * v[2].inv_w;
            let uv = (w0 * v[0].uow + w1 * v[1].uow + w2 * v[2].uow) / iw;
            let bary = [w0 * v[0].inv_w / iw, w1 * v[1].inv_w / iw, w2 * v[2].inv_w / iw];
            if let Some(rgb) = px_fn(uv, bary) {
                t.depth[idx] = z;
                t.color.put_pixel(x, y, Rgba([rgb[0], rgb[1], rgb[2], 255]));
            }
        }
    }
}

/// Depth-tested 1px line with a bias pulling it toward the camera, so edges
/// sit on top of their own faces but stay hidden behind nearer geometry.
pub(crate) fn line3(t: &mut Target, a: &SV, b: &SV, color: [u8; 3], bias: f32) {
    let steps = (b.x - a.x).abs().max((b.y - a.y).abs()).ceil().clamp(1.0, 16384.0) as i32;
    for i in 0..=steps {
        let s = i as f32 / steps as f32;
        let x = (a.x + (b.x - a.x) * s).floor();
        let y = (a.y + (b.y - a.y) * s).floor();
        if x < 0.0 || y < 0.0 || x >= t.px as f32 || y >= t.px as f32 {
            continue;
        }
        let (xi, yi) = (x as u32, y as u32);
        let z = a.z + (b.z - a.z) * s;
        if z - bias <= t.depth[(yi * t.px + xi) as usize] {
            t.color.put_pixel(xi, yi, Rgba([color[0], color[1], color[2], 255]));
        }
    }
}

/// Plain 2D line (UV layouts).
pub(crate) fn line2(img: &mut RgbaImage, a: Vec2, b: Vec2, color: [u8; 4]) {
    let steps = (b.x - a.x).abs().max((b.y - a.y).abs()).ceil().clamp(1.0, 16384.0) as i32;
    for i in 0..=steps {
        let s = i as f32 / steps as f32;
        let x = (a.x + (b.x - a.x) * s).floor();
        let y = (a.y + (b.y - a.y) * s).floor();
        if x >= 0.0 && y >= 0.0 && x < img.width() as f32 && y < img.height() as f32 {
            img.put_pixel(x as u32, y as u32, Rgba(color));
        }
    }
}

/// Coverage fill: paints the triangle white into a mask, no depth.
pub(crate) fn fill_tri_mask(img: &mut GrayImage, v: &[(f32, f32); 3]) {
    let area = orient(v[0].0, v[0].1, v[1].0, v[1].1, v[2].0, v[2].1);
    if area.abs() < 1e-8 {
        return;
    }
    let inv_area = 1.0 / area;
    let (w, h) = (img.width() as f32, img.height() as f32);
    let min_x = v.iter().map(|s| s.0).fold(f32::INFINITY, f32::min).floor().max(0.0) as u32;
    let max_x = v.iter().map(|s| s.0).fold(f32::NEG_INFINITY, f32::max).ceil().clamp(0.0, w) as u32;
    let min_y = v.iter().map(|s| s.1).fold(f32::INFINITY, f32::min).floor().max(0.0) as u32;
    let max_y = v.iter().map(|s| s.1).fold(f32::NEG_INFINITY, f32::max).ceil().clamp(0.0, h) as u32;
    for y in min_y..max_y {
        let cy = y as f32 + 0.5;
        for x in min_x..max_x {
            let cx = x as f32 + 0.5;
            let w0 = orient(v[1].0, v[1].1, v[2].0, v[2].1, cx, cy) * inv_area;
            let w1 = orient(v[2].0, v[2].1, v[0].0, v[0].1, cx, cy) * inv_area;
            let w2 = orient(v[0].0, v[0].1, v[1].0, v[1].1, cx, cy) * inv_area;
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                img.put_pixel(x, y, Luma([255]));
            }
        }
    }
}
