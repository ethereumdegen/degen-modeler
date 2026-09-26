//! Primitive builders. Conventions: +Y up, right-handed, faces wound CCW
//! seen from outside, sizes in meters, geometry centered at the origin
//! (lathe profiles keep their own y).

use glam::Vec3;

use crate::ids::VertId;
use crate::mesh::{Mesh, MeshError};

const EPS: f32 = 1e-6;

pub fn prim_box(size: Vec3) -> Result<Mesh, MeshError> {
    if size.min_element() <= 0.0 {
        return Err(MeshError::Invalid("box: size must be positive".into()));
    }
    let h = size * 0.5;
    let mut mesh = Mesh::new();
    let p = |x: f32, y: f32, z: f32| Vec3::new(x * h.x, y * h.y, z * h.z);
    let v: Vec<VertId> = [
        p(-1.0, -1.0, -1.0),
        p(1.0, -1.0, -1.0),
        p(1.0, -1.0, 1.0),
        p(-1.0, -1.0, 1.0),
        p(-1.0, 1.0, -1.0),
        p(1.0, 1.0, -1.0),
        p(1.0, 1.0, 1.0),
        p(-1.0, 1.0, 1.0),
    ]
    .into_iter()
    .map(|pos| mesh.add_vert(pos))
    .collect();
    for quad in [
        [0, 1, 2, 3], // -Y
        [4, 7, 6, 5], // +Y
        [3, 2, 6, 7], // +Z
        [1, 0, 4, 5], // -Z
        [2, 1, 5, 6], // +X
        [0, 3, 7, 4], // -X
    ] {
        mesh.add_face(&quad.map(|i| v[i]))?;
    }
    Ok(mesh)
}

pub fn plane(sx: f32, sz: f32) -> Result<Mesh, MeshError> {
    if sx <= 0.0 || sz <= 0.0 {
        return Err(MeshError::Invalid("plane: size must be positive".into()));
    }
    let (hx, hz) = (sx * 0.5, sz * 0.5);
    let mut mesh = Mesh::new();
    let v = [
        mesh.add_vert(Vec3::new(-hx, 0.0, -hz)),
        mesh.add_vert(Vec3::new(-hx, 0.0, hz)),
        mesh.add_vert(Vec3::new(hx, 0.0, hz)),
        mesh.add_vert(Vec3::new(hx, 0.0, -hz)),
    ];
    mesh.add_face(&v)?;
    Ok(mesh)
}

pub fn cylinder(radius: f32, height: f32, segments: u32, caps: bool) -> Result<Mesh, MeshError> {
    lathe(&[[radius, -height * 0.5], [radius, height * 0.5]], segments, caps)
}

pub fn ngon_prism(sides: u32, radius: f32, height: f32) -> Result<Mesh, MeshError> {
    cylinder(radius, height, sides, true)
}

enum Ring {
    Point(VertId),
    Circle(Vec<VertId>),
}

/// Revolve a `(radius, y)` profile around +Y. A profile point with ~zero
/// radius becomes an apex; `caps` closes open circular ends.
pub fn lathe(profile: &[[f32; 2]], segments: u32, caps: bool) -> Result<Mesh, MeshError> {
    if profile.len() < 2 {
        return Err(MeshError::Invalid("lathe: profile needs at least 2 points".into()));
    }
    if segments < 3 {
        return Err(MeshError::Invalid("lathe: segments must be >= 3".into()));
    }
    if profile.iter().any(|p| p[0] < 0.0) {
        return Err(MeshError::Invalid("lathe: radii must be >= 0".into()));
    }
    if profile.windows(2).any(|w| w[0][0] < EPS && w[1][0] < EPS) {
        return Err(MeshError::Invalid("lathe: two consecutive zero-radius points".into()));
    }

    let mut mesh = Mesh::new();
    let n = segments as usize;
    let rings: Vec<Ring> = profile
        .iter()
        .map(|&[r, y]| {
            if r < EPS {
                Ring::Point(mesh.add_vert(Vec3::new(0.0, y, 0.0)))
            } else {
                Ring::Circle(
                    (0..n)
                        .map(|k| {
                            let theta = k as f32 / n as f32 * std::f32::consts::TAU;
                            mesh.add_vert(Vec3::new(r * theta.cos(), y, r * theta.sin()))
                        })
                        .collect(),
                )
            }
        })
        .collect();

    for pair in rings.windows(2) {
        match pair {
            [Ring::Circle(low), Ring::Circle(up)] => {
                for k in 0..n {
                    let k1 = (k + 1) % n;
                    mesh.add_face(&[low[k], up[k], up[k1], low[k1]])?;
                }
            }
            [Ring::Circle(low), Ring::Point(apex)] => {
                for k in 0..n {
                    let k1 = (k + 1) % n;
                    mesh.add_face(&[low[k], *apex, low[k1]])?;
                }
            }
            [Ring::Point(apex), Ring::Circle(up)] => {
                for k in 0..n {
                    let k1 = (k + 1) % n;
                    mesh.add_face(&[*apex, up[k], up[k1]])?;
                }
            }
            _ => unreachable!("consecutive zero-radius points rejected above"),
        }
    }

    if caps {
        if let Some(Ring::Circle(ring)) = rings.first() {
            // Bottom cap: increasing theta gives a -Y normal.
            mesh.add_face(&ring.clone())?;
        }
        if let Some(Ring::Circle(ring)) = rings.last() {
            let mut rev = ring.clone();
            rev.reverse();
            mesh.add_face(&rev)?;
        }
    }
    Ok(mesh)
}
