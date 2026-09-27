//! Interior views: cameras placed *inside* the scene bounds, for walkable
//! spaces the orbit sheet cannot show. Geometry crossing the near plane is
//! clipped in clip space (the orbit views simply skip such triangles, which
//! is never wrong from outside but leaves holes from inside).

use dgm_atlas::Pack;
use dgm_scene::Doc;
use glam::{Mat4, Vec2, Vec3, Vec4};
use image::RgbaImage;

use crate::camera::{Camera, SV};
use crate::geom::{SceneGeom, TriGeom, scene_geom};
use crate::raster::{Target, fill_tri};
use crate::views::{sheet, shade, tint_mul};
use crate::{RenderError, check_px};

/// Wider than the orbit camera: interiors are read at arm's length.
const FOV_Y_DEG: f32 = 70.0;
/// Near plane as a fraction of the framing radius, floored so tiny scenes
/// still clip sanely.
const NEAR_FRACTION: f32 = 0.01;
const NEAR_MIN: f32 = 1e-3;
/// How far inside the +Z face the entrance camera stands (fraction of the
/// Z extent), so the shell's own wall does not sit on the near plane.
const ENTRANCE_INSET: f32 = 0.05;

/// One vertex in clip space with its interpolated attributes.
#[derive(Clone, Copy)]
struct ClipVert {
    clip: Vec4,
    uv: Vec2,
    color: Vec3,
}

impl ClipVert {
    fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            clip: self.clip.lerp(other.clip, t),
            uv: self.uv.lerp(other.uv, t),
            color: self.color.lerp(other.color, t),
        }
    }

    /// Screen vertex; only valid after near clipping (w > 0 guaranteed).
    fn screen(&self, px: u32) -> SV {
        let inv_w = 1.0 / self.clip.w;
        SV {
            x: (self.clip.x * inv_w * 0.5 + 0.5) * px as f32,
            y: (0.5 - self.clip.y * inv_w * 0.5) * px as f32,
            z: self.clip.z * inv_w,
            inv_w,
            uow: self.uv * inv_w,
        }
    }
}

/// Look-at camera with explicit near/far; `glam::perspective_rh` maps the
/// near plane to `z_clip == 0`, which is what [`clip_near`] tests.
fn look_at(eye: Vec3, target: Vec3, up: Vec3, near: f32, far: f32) -> Camera {
    let view = Mat4::look_at_rh(eye, target, up);
    let proj = Mat4::perspective_rh(FOV_Y_DEG.to_radians(), 1.0, near, far);
    Camera { view, viewproj: proj * view }
}

/// Sutherland–Hodgman against the near plane (`z_clip >= 0`). Returns 0, 3
/// or 4 vertices.
fn clip_near(tri: [ClipVert; 3]) -> Vec<ClipVert> {
    let mut out = Vec::with_capacity(4);
    for i in 0..3 {
        let a = tri[i];
        let b = tri[(i + 1) % 3];
        let (da, db) = (a.clip.z, b.clip.z);
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            out.push(a.lerp(b, t));
        }
    }
    out
}

/// Textured view with near-plane clipping; vertex colors ride through the
/// clip as interpolated corner attributes.
fn interior_view(scene: &SceneGeom, cam: &Camera, px: u32) -> Target {
    let mut t = Target::checker(px);
    for obj in &scene.objects {
        for tri in &obj.tris {
            let s = shade(cam, tri.normal);
            let poly = clip_near(to_clip(cam, tri));
            if poly.len() < 3 {
                continue;
            }
            for i in 1..poly.len() - 1 {
                let corners = [poly[0], poly[i], poly[i + 1]];
                let v = [corners[0].screen(px), corners[1].screen(px), corners[2].screen(px)];
                fill_tri(&mut t, &v, |uv, bary| {
                    let c = obj.tex.sample(uv);
                    if obj.alpha_mask && c[3] < 128 {
                        return None;
                    }
                    let tint = corners[0].color * bary[0]
                        + corners[1].color * bary[1]
                        + corners[2].color * bary[2];
                    Some(tint_mul(c, tint, s))
                });
            }
        }
    }
    t
}

fn to_clip(cam: &Camera, tri: &TriGeom) -> [ClipVert; 3] {
    std::array::from_fn(|i| ClipVert {
        clip: cam.viewproj * tri.pos[i].extend(1.0),
        uv: tri.uv[i],
        color: tri.color[i],
    })
}

/// The four interior cameras for `bounds`, in sheet order: entrance (+Z
/// edge looking in), center looking +Z, center looking -Z, center looking
/// up. All share one near/far pair derived from the framing radius.
pub(crate) fn interior_cameras(lo: Vec3, hi: Vec3) -> [Camera; 4] {
    let center = (lo + hi) * 0.5;
    let radius = ((hi - lo).length() * 0.5).max(1e-3);
    let near = (radius * NEAR_FRACTION).max(NEAR_MIN);
    let far = radius * 4.0;
    let entrance = Vec3::new(center.x, center.y, hi.z - (hi.z - lo.z) * ENTRANCE_INSET);
    [
        look_at(entrance, Vec3::new(center.x, center.y, lo.z), Vec3::Y, near, far),
        look_at(center, center + Vec3::Z, Vec3::Y, near, far),
        look_at(center, center - Vec3::Z, Vec3::Y, near, far),
        look_at(center, center + Vec3::Y, Vec3::Z, near, far),
    ]
}

/// 2x2 sheet of interior views: from the +Z edge looking in, from the
/// center looking +Z and -Z, and from the center looking up. Empty scenes
/// frame the same unit box as the orbit views.
pub fn interior_sheet(doc: &Doc, pack: &Pack, view_px: u32) -> Result<RgbaImage, RenderError> {
    check_px(view_px)?;
    let scene = scene_geom(doc, pack, false);
    let (lo, hi) = scene.bounds.unwrap_or((Vec3::splat(-0.5), Vec3::splat(0.5)));
    let cams = interior_cameras(lo, hi);
    Ok(sheet(2, 2, view_px, |i| interior_view(&scene, &cams[i as usize], view_px).color))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cv(z: f32) -> ClipVert {
        ClipVert { clip: Vec4::new(0.0, 0.0, z, 1.0), uv: Vec2::new(z, 0.0), color: Vec3::splat(z) }
    }

    #[test]
    fn near_clip_keeps_inside_drops_outside_splits_crossing() {
        assert_eq!(clip_near([cv(1.0), cv(2.0), cv(3.0)]).len(), 3);
        assert!(clip_near([cv(-1.0), cv(-2.0), cv(-3.0)]).is_empty());
        // One vertex behind: quad (two kept + two intersections).
        let poly = clip_near([cv(-1.0), cv(1.0), cv(1.0)]);
        assert_eq!(poly.len(), 4);
        assert!(poly.iter().all(|v| v.clip.z >= 0.0));
        // Attributes interpolate with the position.
        let mid = poly.iter().find(|v| v.clip.z.abs() < 1e-6).unwrap();
        assert!((mid.uv.x).abs() < 1e-6 && mid.color.x.abs() < 1e-6);
        // Two behind: triangle.
        assert_eq!(clip_near([cv(-1.0), cv(-1.0), cv(1.0)]).len(), 3);
    }
}
