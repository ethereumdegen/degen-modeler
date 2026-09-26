//! Projection operators. UVs come out world-scaled: 1 UV unit == 1 meter
//! (cylindrical uses arc length at the selection's mean radius), so a later
//! [`crate::set_texel_density`] maps them onto a texture without guessing.
//!
//! Image-space orientation (v down, matching glTF): each planar/box mapping
//! is chosen so a face viewed from its projection direction reads unmirrored.

use std::collections::BTreeSet;
use std::f32::consts::{PI, TAU};

use dgm_mesh::{FaceId, Mesh, MeshDelta, VertId};
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::UvError;
use crate::geom::{check_faces, uv_delta};

/// Mirrors `dgm_scene::Axis` (identical serde shape); dispatch adapts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
    Z,
}

/// Mirrors `dgm_scene::ProjectKind` (identical serde shape); dispatch adapts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectKind {
    Planar { axis: Axis },
    Box,
    Cylindrical { axis: Axis },
}

const RADIUS_EPS: f32 = 1e-6;

/// Write projected, meter-scaled UVs onto the selected faces' corners.
pub fn project(
    mesh: &mut Mesh,
    faces: &BTreeSet<FaceId>,
    kind: &ProjectKind,
) -> Result<MeshDelta, UvError> {
    check_faces(mesh, faces)?;
    match kind {
        ProjectKind::Planar { axis } => {
            for &f in faces {
                let uvs: Vec<Vec2> = face_positions(mesh, f)
                    .into_iter()
                    .map(|p| planar_uv(*axis, p))
                    .collect();
                set_face_uvs(mesh, f, &uvs);
            }
        }
        ProjectKind::Box => {
            for &f in faces {
                let normal = mesh.face_normal(f)?;
                let uvs: Vec<Vec2> = face_positions(mesh, f)
                    .into_iter()
                    .map(|p| box_uv(normal, p))
                    .collect();
                set_face_uvs(mesh, f, &uvs);
            }
        }
        ProjectKind::Cylindrical { axis } => {
            let verts: BTreeSet<VertId> = faces
                .iter()
                .flat_map(|&f| mesh.faces[&f].verts().collect::<Vec<_>>())
                .collect();
            let radius = verts
                .iter()
                .map(|&v| {
                    let (r1, r2, _) = cyl_coords(*axis, mesh.verts[&v]);
                    (r1 * r1 + r2 * r2).sqrt()
                })
                .sum::<f32>()
                / verts.len() as f32;
            if radius < RADIUS_EPS {
                return Err(UvError::Invalid(
                    "cylindrical projection: selection radius is ~zero".into(),
                ));
            }
            for &f in faces {
                let coords: Vec<(f32, f32, f32)> = face_positions(mesh, f)
                    .into_iter()
                    .map(|p| cyl_coords(*axis, p))
                    .collect();
                let mut thetas: Vec<f32> =
                    coords.iter().map(|&(r1, r2, _)| r2.atan2(r1)).collect();
                // Faces spanning the -pi/pi cut get their negative angles
                // lifted by tau so they don't smear across the whole map.
                let (min, max) = thetas
                    .iter()
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &t| {
                        (lo.min(t), hi.max(t))
                    });
                if max - min > PI {
                    for t in &mut thetas {
                        if *t < 0.0 {
                            *t += TAU;
                        }
                    }
                }
                let uvs: Vec<Vec2> = thetas
                    .iter()
                    .zip(&coords)
                    .map(|(&t, &(_, _, h))| Vec2::new(radius * t, -h))
                    .collect();
                set_face_uvs(mesh, f, &uvs);
            }
        }
    }
    Ok(uv_delta(faces.iter().copied()))
}

fn face_positions(mesh: &Mesh, f: FaceId) -> Vec<Vec3> {
    mesh.faces[&f].corners.iter().map(|c| mesh.verts[&c.vert]).collect()
}

fn set_face_uvs(mesh: &mut Mesh, f: FaceId, uvs: &[Vec2]) {
    let face = mesh.faces.get_mut(&f).expect("face checked by caller");
    for (c, &uv) in face.corners.iter_mut().zip(uvs) {
        c.uv = uv;
    }
}

/// View from the +axis side, v down: X -> (-z, -y), Y -> (x, z), Z -> (x, -y).
fn planar_uv(axis: Axis, p: Vec3) -> Vec2 {
    match axis {
        Axis::X => Vec2::new(-p.z, -p.y),
        Axis::Y => Vec2::new(p.x, p.z),
        Axis::Z => Vec2::new(p.x, -p.y),
    }
}

/// Planar projection along the dominant signed axis of the face normal,
/// mirrored per sign so every face reads unmirrored from outside.
fn box_uv(n: Vec3, p: Vec3) -> Vec2 {
    let a = n.abs();
    if a.x >= a.y && a.x >= a.z {
        if n.x >= 0.0 { Vec2::new(-p.z, -p.y) } else { Vec2::new(p.z, -p.y) }
    } else if a.y >= a.z {
        if n.y >= 0.0 { Vec2::new(p.x, p.z) } else { Vec2::new(p.x, -p.z) }
    } else if n.z >= 0.0 {
        Vec2::new(p.x, -p.y)
    } else {
        Vec2::new(-p.x, -p.y)
    }
}

/// (radial-1, radial-2, height) coordinates for a cylinder around `axis`.
fn cyl_coords(axis: Axis, p: Vec3) -> (f32, f32, f32) {
    match axis {
        Axis::X => (p.y, p.z, p.x),
        Axis::Y => (p.x, p.z, p.y),
        Axis::Z => (p.x, p.y, p.z),
    }
}
