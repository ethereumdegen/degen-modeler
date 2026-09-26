//! Texel density: scale islands so face density hits a target.
//!
//! Density of a face = `sqrt(uv_area * texture_px^2 / world_area)`, i.e.
//! `texture_px` times the linear UV/world scale, in texels per meter.

use std::collections::BTreeSet;

use dgm_mesh::{FaceId, Mesh, MeshDelta};

use crate::UvError;
use crate::geom::{check_faces, face_uv_area, island_uv_bbox, uv_delta};
use crate::islands::islands_within;

const AREA_EPS: f32 = 1e-12;

/// Uniformly rescale each island of the selection (about its own bbox
/// minimum, so it stays put) so its density equals `texels_per_meter` on a
/// `texture_px` texture. Faces then meet the target exactly wherever the
/// projection is unstretched.
pub fn set_texel_density(
    mesh: &mut Mesh,
    faces: &BTreeSet<FaceId>,
    texels_per_meter: f32,
    texture_px: u32,
) -> Result<MeshDelta, UvError> {
    check_faces(mesh, faces)?;
    if texture_px == 0 {
        return Err(UvError::Invalid("texel density: texture_px must be > 0".into()));
    }
    if !texels_per_meter.is_finite() || texels_per_meter <= 0.0 {
        return Err(UvError::Invalid(format!(
            "texel density: target {texels_per_meter} must be a positive number"
        )));
    }
    for island in islands_within(mesh, faces) {
        let anchor = *island.first().expect("islands are never empty");
        let mut uv_area = 0.0f32;
        let mut world_area = 0.0f32;
        for &f in &island {
            uv_area += face_uv_area(mesh.face(f)?);
            world_area += mesh.face_area(f)?;
        }
        if world_area < AREA_EPS {
            return Err(UvError::Invalid(format!("island at {anchor} has ~zero world area")));
        }
        if uv_area < AREA_EPS {
            return Err(UvError::Invalid(format!(
                "island at {anchor} has no UV area; project or unwrap first"
            )));
        }
        let density = texture_px as f32 * (uv_area / world_area).sqrt();
        let k = texels_per_meter / density;
        let (lo, _) = island_uv_bbox(mesh, &island);
        for &f in &island {
            if let Some(face) = mesh.faces.get_mut(&f) {
                for c in &mut face.corners {
                    c.uv = lo + (c.uv - lo) * k;
                }
            }
        }
    }
    Ok(uv_delta(faces.iter().copied()))
}
