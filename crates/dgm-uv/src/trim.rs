//! Trim binding: fit each selected island into a named trim-region rect.

use std::collections::BTreeSet;

use dgm_atlas::Pack;
use dgm_mesh::{FaceId, Mesh, MeshDelta};
use glam::Vec2;

use crate::UvError;
use crate::geom::{check_faces, island_uv_bbox, uv_delta};
use crate::islands::islands_within;
use crate::unwrap::project_island_to_plane;

const EPS: f32 = 1e-6;

/// Map each island of the selection into the region's UV rect: uniform
/// scale preserving aspect (the larger side fits exactly), centered.
/// Islands stack on the same rect on purpose — that's trim reuse. Islands
/// with no UVs yet are first flattened onto their best-fit plane.
pub fn assign_trim(
    mesh: &mut Mesh,
    faces: &BTreeSet<FaceId>,
    pack: &Pack,
    sheet: &str,
    region: &str,
) -> Result<MeshDelta, UvError> {
    let rect = pack.region_uv(sheet, region).ok_or_else(|| {
        UvError::UnknownRegion { sheet: sheet.into(), region: region.into() }
    })?;
    assign_rect(mesh, faces, rect)
}

/// The same fit into an arbitrary UV rect `[u0, v0, u1, v1]` — how owned
/// (File-texture) atlases get deliberate region layout, e.g. bark on the
/// left half, foliage on the right, before the atlas is even painted.
pub fn assign_rect(
    mesh: &mut Mesh,
    faces: &BTreeSet<FaceId>,
    [u0, v0, u1, v1]: [f32; 4],
) -> Result<MeshDelta, UvError> {
    check_faces(mesh, faces)?;
    let rect_lo = Vec2::new(u0, v0);
    let rect_size = Vec2::new(u1 - u0, v1 - v0);
    if rect_size.min_element() <= 0.0 {
        return Err(UvError::Invalid(format!(
            "assign rect [{u0}, {v0}, {u1}, {v1}] has zero or negative size"
        )));
    }
    for island in islands_within(mesh, faces) {
        let (mut lo, hi) = island_uv_bbox(mesh, &island);
        let mut size = hi - lo;
        if size.max_element() < EPS {
            project_island_to_plane(mesh, &island)?;
            let (l2, h2) = island_uv_bbox(mesh, &island);
            lo = l2;
            size = h2 - l2;
        }
        let size = size.max(Vec2::splat(EPS));
        let scale = (rect_size.x / size.x).min(rect_size.y / size.y);
        let offset = rect_lo + (rect_size - size * scale) * 0.5;
        for &f in &island {
            if let Some(face) = mesh.faces.get_mut(&f) {
                for c in &mut face.corners {
                    c.uv = (c.uv - lo) * scale + offset;
                }
            }
        }
    }
    Ok(uv_delta(faces.iter().copied()))
}
