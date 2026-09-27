//! Primitive builders. Conventions: +Y up, right-handed, faces wound CCW
//! seen from outside, sizes in meters, geometry centered at the origin
//! (lathe profiles keep their own y).
//!
//! Every primitive ships with **meter-scaled default UVs and seams**, so a
//! fresh primitive already unwraps sanely: revolved side bands unroll by
//! ring arc x profile arc, apex fans and caps are planar discs, and the
//! band/fan/cap boundaries are pre-marked as seams. One
//! `uv_assign_rect`/`uv_assign_trim`/`uv_set_texel_density` away from
//! shippable — no per-face projection surgery required.

use glam::{Vec2, Vec3};

use crate::ids::VertId;
use crate::mesh::{Corner, Mesh, MeshError};

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
    // (indices, dominant axis) — UVs are the two off-axis coordinates in
    // meters, so texel density is uniform out of the box.
    let faces: [([usize; 4], fn(Vec3) -> Vec2); 6] = [
        ([0, 1, 2, 3], |p| Vec2::new(p.x, p.z)), // -Y
        ([4, 7, 6, 5], |p| Vec2::new(p.x, p.z)), // +Y
        ([3, 2, 6, 7], |p| Vec2::new(p.x, -p.y)), // +Z
        ([1, 0, 4, 5], |p| Vec2::new(-p.x, -p.y)), // -Z
        ([2, 1, 5, 6], |p| Vec2::new(-p.z, -p.y)), // +X
        ([0, 3, 7, 4], |p| Vec2::new(p.z, -p.y)), // -X
    ];
    for (quad, uv) in faces {
        let corners = quad
            .map(|i| {
                let vert = v[i];
                let pos = mesh.pos(vert).expect("just added");
                Corner { vert, uv: uv(pos) }
            })
            .to_vec();
        mesh.add_face_uv(corners)?;
    }
    // Every face is its own island.
    let all: Vec<_> = mesh.edges().into_iter().collect();
    mesh.seams.extend(all);
    spread_islands(&mut mesh);
    Ok(mesh)
}

pub fn plane(sx: f32, sz: f32) -> Result<Mesh, MeshError> {
    if sx <= 0.0 || sz <= 0.0 {
        return Err(MeshError::Invalid("plane: size must be positive".into()));
    }
    let (hx, hz) = (sx * 0.5, sz * 0.5);
    let mut mesh = Mesh::new();
    let pts = [
        Vec3::new(-hx, 0.0, -hz),
        Vec3::new(-hx, 0.0, hz),
        Vec3::new(hx, 0.0, hz),
        Vec3::new(hx, 0.0, -hz),
    ];
    let corners = pts
        .map(|p| Corner { vert: mesh.add_vert(p), uv: Vec2::new(p.x, p.z) })
        .to_vec();
    mesh.add_face_uv(corners)?;
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
///
/// Default UVs (meters): side bands unroll as u = theta * ring radius,
/// v = cumulative profile arc length; apex fans and caps are planar
/// (x, z) discs. Seams pre-mark the fan/cap boundary rings so each band
/// is already its own island.
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
    let theta = |k: usize| k as f32 / n as f32 * std::f32::consts::TAU;
    let rings: Vec<Ring> = profile
        .iter()
        .map(|&[r, y]| {
            if r < EPS {
                Ring::Point(mesh.add_vert(Vec3::new(0.0, y, 0.0)))
            } else {
                Ring::Circle(
                    (0..n)
                        .map(|k| {
                            let t = theta(k);
                            mesh.add_vert(Vec3::new(r * t.cos(), y, r * t.sin()))
                        })
                        .collect(),
                )
            }
        })
        .collect();

    // Cumulative profile arc length, the v coordinate of the side bands.
    let mut arc = vec![0.0f32];
    for w in profile.windows(2) {
        let d = Vec2::new(w[1][0] - w[0][0], w[1][1] - w[0][1]).length();
        arc.push(arc.last().unwrap() + d);
    }

    let disc_uv = |mesh: &Mesh, vert: VertId| {
        let p = mesh.pos(vert).expect("lathe vert");
        Vec2::new(p.x, p.z)
    };

    for (i, pair) in rings.windows(2).enumerate() {
        let (r_low, r_up) = (profile[i][0], profile[i + 1][0]);
        let (v_low, v_up) = (arc[i], arc[i + 1]);
        match pair {
            [Ring::Circle(low), Ring::Circle(up)] => {
                for k in 0..n {
                    let k1 = (k + 1) % n;
                    // theta of the far column is k+1 even when it wraps, so
                    // the unroll is continuous and the last quad ends at
                    // u = 2*pi*r instead of folding back to 0.
                    let (t0, t1) = (theta(k), theta(k + 1));
                    mesh.add_face_uv(vec![
                        Corner { vert: low[k], uv: Vec2::new(t0 * r_low, v_low) },
                        Corner { vert: up[k], uv: Vec2::new(t0 * r_up, v_up) },
                        Corner { vert: up[k1], uv: Vec2::new(t1 * r_up, v_up) },
                        Corner { vert: low[k1], uv: Vec2::new(t1 * r_low, v_low) },
                    ])?;
                }
            }
            [Ring::Circle(low), Ring::Point(apex)] => {
                for k in 0..n {
                    let k1 = (k + 1) % n;
                    mesh.add_face_uv(vec![
                        Corner { vert: low[k], uv: disc_uv(&mesh, low[k]) },
                        Corner { vert: *apex, uv: Vec2::ZERO },
                        Corner { vert: low[k1], uv: disc_uv(&mesh, low[k1]) },
                    ])?;
                }
            }
            [Ring::Point(apex), Ring::Circle(up)] => {
                for k in 0..n {
                    let k1 = (k + 1) % n;
                    mesh.add_face_uv(vec![
                        Corner { vert: *apex, uv: Vec2::ZERO },
                        Corner { vert: up[k], uv: disc_uv(&mesh, up[k]) },
                        Corner { vert: up[k1], uv: disc_uv(&mesh, up[k1]) },
                    ])?;
                }
            }
            _ => unreachable!("consecutive zero-radius points rejected above"),
        }
    }

    if caps {
        if let Some(Ring::Circle(ring)) = rings.first() {
            // Bottom cap: increasing theta gives a -Y normal.
            let corners =
                ring.iter().map(|&v| Corner { vert: v, uv: disc_uv(&mesh, v) }).collect();
            mesh.add_face_uv(corners)?;
        }
        if let Some(Ring::Circle(ring)) = rings.last() {
            let mut rev: Vec<VertId> = ring.clone();
            rev.reverse();
            let corners =
                rev.into_iter().map(|v| Corner { vert: v, uv: disc_uv(&mesh, v) }).collect();
            mesh.add_face_uv(corners)?;
        }
    }

    // Seams: every ring that borders an apex fan, plus cap boundaries, so
    // fans/caps split from the side bands as their own islands.
    let mut seam_rings: Vec<&Vec<VertId>> = Vec::new();
    for (i, ring) in rings.iter().enumerate() {
        if let Ring::Circle(circle) = ring {
            let next_is_apex =
                rings.get(i + 1).map(|r| matches!(r, Ring::Point(_))).unwrap_or(false);
            let prev_is_apex =
                i.checked_sub(1).map(|j| matches!(rings[j], Ring::Point(_))).unwrap_or(false);
            let is_cap_boundary = caps && (i == 0 || i == rings.len() - 1);
            if next_is_apex || prev_is_apex || is_cap_boundary {
                seam_rings.push(circle);
            }
        }
    }
    for ring in seam_rings {
        for k in 0..n {
            mesh.seams.insert(crate::ids::EdgeKey::new(ring[k], ring[(k + 1) % n]));
        }
    }
    spread_islands(&mut mesh);
    Ok(mesh)
}

/// Islands share one meter-space plane after building; slide each island
/// to its own u-lane (5% gutters) so no two default islands overlap — an
/// owned-atlas material is gate-clean from the first op.
fn spread_islands(mesh: &mut Mesh) {
    let islands = mesh.uv_islands();
    if islands.len() < 2 {
        return;
    }
    let mut cursor = 0.0f32;
    for island in islands {
        let mut lo = Vec2::splat(f32::INFINITY);
        let mut hi = Vec2::splat(f32::NEG_INFINITY);
        for &f in &island {
            for c in &mesh.faces[&f].corners {
                lo = lo.min(c.uv);
                hi = hi.max(c.uv);
            }
        }
        let size = (hi - lo).max(Vec2::splat(EPS));
        let shift = Vec2::new(cursor - lo.x, -lo.y);
        for &f in &island {
            if let Some(face) = mesh.faces.get_mut(&f) {
                for c in &mut face.corners {
                    c.uv += shift;
                }
            }
        }
        cursor += size.x * 1.05 + EPS;
    }
}
