//! Perspective pinhole orbit camera, auto-framed on a bounding sphere.

use glam::{Mat4, Vec2, Vec3};

const FOV_Y_DEG: f32 = 45.0;
/// Slack around the bounding sphere so silhouettes never kiss the border.
const MARGIN: f32 = 1.12;

pub(crate) struct Camera {
    pub view: Mat4,
    pub viewproj: Mat4,
}

/// Orbit `center` at the distance that fits a sphere of `radius` into a
/// square viewport. Yaw spins around +Y (0 = looking from +Z), elevation
/// lifts the eye above the horizon.
pub(crate) fn orbit(center: Vec3, radius: f32, yaw_deg: f32, elev_deg: f32) -> Camera {
    let r = radius.max(1e-3);
    let fov = FOV_Y_DEG.to_radians();
    let dist = r / (fov * 0.5).sin() * MARGIN;
    let (sy, cy) = yaw_deg.to_radians().sin_cos();
    let (se, ce) = elev_deg.to_radians().sin_cos();
    let eye = center + Vec3::new(ce * sy, se, ce * cy) * dist;
    let view = Mat4::look_at_rh(eye, center, Vec3::Y);
    let near = (dist - 2.0 * r).max(dist * 0.05);
    let far = dist + 2.0 * r;
    Camera { view, viewproj: Mat4::perspective_rh(fov, 1.0, near, far) * view }
}

/// A projected ("screen") vertex: pixel x/y, NDC depth, and the
/// perspective-correct interpolation terms.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SV {
    pub x: f32,
    pub y: f32,
    /// NDC depth in [0,1], screen-linear; smaller is closer.
    pub z: f32,
    pub inv_w: f32,
    /// uv * inv_w, for perspective-correct texturing.
    pub uow: Vec2,
}

/// `None` when the point is behind the eye; the framed orbit camera never
/// produces that for scene geometry, so whole triangles are simply skipped.
pub(crate) fn project(cam: &Camera, px: u32, p: Vec3, uv: Vec2) -> Option<SV> {
    let clip = cam.viewproj * p.extend(1.0);
    if clip.w <= 1e-4 {
        return None;
    }
    let inv_w = 1.0 / clip.w;
    Some(SV {
        x: (clip.x * inv_w * 0.5 + 0.5) * px as f32,
        y: (0.5 - clip.y * inv_w * 0.5) * px as f32,
        z: clip.z * inv_w,
        inv_w,
        uow: uv * inv_w,
    })
}
