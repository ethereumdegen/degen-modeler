//! Whole-mesh unwrap: seams -> islands -> per-island best-fit plane
//! projection (meters) -> packed into `[0,1]^2` with a small default margin.

use std::collections::{BTreeMap, BTreeSet};

use dgm_mesh::{FaceId, Mesh, MeshDelta, VertId};
use glam::{Vec2, Vec3};

use crate::UvError;
use crate::geom::uv_delta;
use crate::islands::islands;
use crate::packing::pack_all_islands;

/// Margin used by `unwrap` (4 px on a 1024 atlas); [`crate::pack_islands`]
/// repacks with caller-chosen margins.
const DEFAULT_MARGIN: f32 = 4.0 / 1024.0;

/// Re-unwrap every island of the mesh. Relative island scale survives the
/// packing, so texel density stays uniform across islands.
pub fn unwrap(mesh: &mut Mesh) -> Result<MeshDelta, UvError> {
    let isl = islands(mesh);
    if isl.is_empty() {
        return Ok(MeshDelta::default());
    }
    for island in &isl {
        project_island_to_plane(mesh, island)?;
    }
    pack_all_islands(mesh, &isl, DEFAULT_MARGIN)?;
    Ok(uv_delta(mesh.faces.keys().copied()))
}

/// Project one island onto its best-fit plane, UVs in meters around the
/// island centroid. Plane normal = area-weighted face normal; when that
/// cancels out (closed shells), the largest face's normal wins.
pub(crate) fn project_island_to_plane(
    mesh: &mut Mesh,
    island: &BTreeSet<FaceId>,
) -> Result<(), UvError> {
    let mut weighted = Vec3::ZERO;
    for &f in island {
        weighted += mesh.face_normal(f)? * mesh.face_area(f)?;
    }
    let mut normal = weighted.normalize_or_zero();
    if normal == Vec3::ZERO {
        let mut best_area = 0.0f32;
        for &f in island {
            let area = mesh.face_area(f)?;
            if area > best_area {
                let n = mesh.face_normal(f)?;
                if n != Vec3::ZERO {
                    best_area = area;
                    normal = n;
                }
            }
        }
        if normal == Vec3::ZERO {
            normal = Vec3::Y;
        }
    }
    // Basis: reference = axis least aligned with the normal (first wins on
    // ties), t = reference x n, b = n x t; u = t.q, v = -(b.q) so the image
    // v axis points down.
    let reference = [Vec3::X, Vec3::Y, Vec3::Z]
        .into_iter()
        .min_by(|a, b| a.dot(normal).abs().total_cmp(&b.dot(normal).abs()))
        .expect("three candidate axes");
    let t = reference.cross(normal).normalize();
    let b = normal.cross(t);

    let verts: BTreeSet<VertId> =
        island.iter().flat_map(|&f| mesh.faces[&f].verts().collect::<Vec<_>>()).collect();
    let centroid =
        verts.iter().map(|&v| mesh.verts[&v]).sum::<Vec3>() / verts.len() as f32;
    let uvs: BTreeMap<VertId, Vec2> = verts
        .iter()
        .map(|&v| {
            let q = mesh.verts[&v] - centroid;
            (v, Vec2::new(t.dot(q), -b.dot(q)))
        })
        .collect();
    for &f in island {
        if let Some(face) = mesh.faces.get_mut(&f) {
            for c in &mut face.corners {
                c.uv = uvs[&c.vert];
            }
        }
    }
    Ok(())
}
