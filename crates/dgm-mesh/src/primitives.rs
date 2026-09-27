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

use std::f32::consts::{PI, TAU};

use glam::{Quat, Vec2, Vec3};

use crate::ids::{EdgeKey, FaceId, VertId};
use crate::mesh::{Corner, Mesh, MeshError};
use crate::ops::organic::displace_noise;

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

/// Swept tube along a polyline: one ring of `segments` verts per path point
/// in a parallel-transported frame (no twist), quads between consecutive
/// rings, both ends open. `radii` is one radius for the whole tube or one
/// per path point.
///
/// Default UVs (meters): u = arc around the ring (the last quad ends at
/// u = 2*pi*r rather than folding back, like `lathe`), v = distance along
/// the path. A single island, no seams.
pub fn tunnel(path: &[Vec3], radii: &[f32], segments: u32) -> Result<Mesh, MeshError> {
    if path.len() < 2 {
        return Err(MeshError::Invalid("tunnel: path needs at least 2 points".into()));
    }
    if segments < 3 {
        return Err(MeshError::Invalid("tunnel: segments must be >= 3".into()));
    }
    if radii.len() != 1 && radii.len() != path.len() {
        return Err(MeshError::Invalid(format!(
            "tunnel: radii must have 1 or {} entries, got {}",
            path.len(),
            radii.len()
        )));
    }
    if radii.iter().any(|r| !(*r > 0.0) || !r.is_finite()) {
        return Err(MeshError::Invalid("tunnel: radii must be > 0".into()));
    }
    if path.iter().any(|p| !p.is_finite()) {
        return Err(MeshError::Invalid("tunnel: path points must be finite".into()));
    }
    let radius = |i: usize| if radii.len() == 1 { radii[0] } else { radii[i] };

    // Unit segment directions; a repeated point has no direction.
    let mut dirs = Vec::with_capacity(path.len() - 1);
    for (i, w) in path.windows(2).enumerate() {
        let d = w[1] - w[0];
        if d.length_squared() < EPS * EPS {
            return Err(MeshError::Invalid(format!("tunnel: path points {i} and {} coincide", i + 1)));
        }
        dirs.push(d.normalize());
    }
    // Point tangents: segment direction at the ends, bisector inside.
    let tangents: Vec<Vec3> = (0..path.len())
        .map(|i| {
            if i == 0 {
                dirs[0]
            } else if i == path.len() - 1 {
                dirs[i - 1]
            } else {
                (dirs[i - 1] + dirs[i]).normalize_or(dirs[i - 1])
            }
        })
        .collect();
    // Parallel transport: start from the world up projected onto the first
    // ring plane, then carry the normal by the minimal rotation between
    // consecutive tangents and re-orthogonalize.
    let t0 = tangents[0];
    let up = if t0.y.abs() < 0.99 { Vec3::Y } else { Vec3::X };
    let mut normal = (up - t0 * up.dot(t0)).normalize();
    let mut frames = Vec::with_capacity(path.len());
    for (i, &t) in tangents.iter().enumerate() {
        if i > 0 {
            let rot = Quat::from_rotation_arc(tangents[i - 1], t);
            let carried = rot * normal;
            normal = (carried - t * carried.dot(t)).normalize_or(normal);
        }
        // (normal, binormal, tangent) is left-handed like lathe's (X, Z, Y),
        // so the lathe quad order below faces outward.
        frames.push((normal, normal.cross(t)));
    }

    let mut mesh = Mesh::new();
    let n = segments as usize;
    let theta = |k: usize| k as f32 / n as f32 * TAU;
    let rings: Vec<Vec<VertId>> = path
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let (nrm, bin) = frames[i];
            let r = radius(i);
            (0..n)
                .map(|k| {
                    let t = theta(k);
                    mesh.add_vert(c + (nrm * t.cos() + bin * t.sin()) * r)
                })
                .collect()
        })
        .collect();
    let mut arc = vec![0.0f32];
    for w in path.windows(2) {
        arc.push(arc.last().unwrap() + (w[1] - w[0]).length());
    }
    for (i, pair) in rings.windows(2).enumerate() {
        let (low, up) = (&pair[0], &pair[1]);
        let (r_low, r_up) = (radius(i), radius(i + 1));
        let (v_low, v_up) = (arc[i], arc[i + 1]);
        for k in 0..n {
            let k1 = (k + 1) % n;
            let (t0, t1) = (theta(k), theta(k + 1));
            mesh.add_face_uv(vec![
                Corner { vert: low[k], uv: Vec2::new(t0 * r_low, v_low) },
                Corner { vert: up[k], uv: Vec2::new(t0 * r_up, v_up) },
                Corner { vert: up[k1], uv: Vec2::new(t1 * r_up, v_up) },
                Corner { vert: low[k1], uv: Vec2::new(t1 * r_low, v_low) },
            ])?;
        }
    }
    spread_islands(&mut mesh);
    Ok(mesh)
}

/// Noise-displaced ellipsoid (a UV sphere scaled by `radii`) centered at the
/// origin with outward normals; `rings` latitude bands between the two
/// apexes. Faces lying entirely at or below `floor_y` are removed so the
/// floor is open, then every vertex is displaced along its normal by
/// `displace_noise(noise * min radius, 0.5 * min radius, seed)`.
///
/// Default UVs (meters): the side bands unroll spherically, u = theta * mean
/// equatorial radius, v = phi * mean meridian radius; apex fans are planar
/// (x, z) discs seamed off from the bands like `lathe`; islands spread.
pub fn cavern(
    radii: Vec3,
    segments: u32,
    rings: u32,
    floor_y: Option<f32>,
    noise: f32,
    seed: u64,
) -> Result<Mesh, MeshError> {
    if !radii.is_finite() || radii.min_element() <= 0.0 {
        return Err(MeshError::Invalid("cavern: radii must be positive".into()));
    }
    if segments < 3 {
        return Err(MeshError::Invalid("cavern: segments must be >= 3".into()));
    }
    if rings < 2 {
        return Err(MeshError::Invalid("cavern: rings must be >= 2".into()));
    }
    if !(noise >= 0.0) || !noise.is_finite() {
        return Err(MeshError::Invalid(format!("cavern: noise must be >= 0, got {noise}")));
    }
    let n = segments as usize;
    let m = rings as usize;
    let theta = |k: usize| k as f32 / n as f32 * TAU;
    let phi = |j: usize| j as f32 / m as f32 * PI;
    let r_eq = (radii.x + radii.z) * 0.5;
    let r_mer = (r_eq + radii.y) * 0.5;

    let mut mesh = Mesh::new();
    let bottom = mesh.add_vert(Vec3::new(0.0, -radii.y, 0.0));
    let circles: Vec<Vec<VertId>> = (1..m)
        .map(|j| {
            let (s, c) = phi(j).sin_cos();
            (0..n)
                .map(|k| {
                    let t = theta(k);
                    mesh.add_vert(Vec3::new(radii.x * s * t.cos(), -radii.y * c, radii.z * s * t.sin()))
                })
                .collect()
        })
        .collect();
    let top = mesh.add_vert(Vec3::new(0.0, radii.y, 0.0));
    let disc_uv = |mesh: &Mesh, vert: VertId| {
        let p = mesh.pos(vert).expect("cavern vert");
        Vec2::new(p.x, p.z)
    };

    let first = &circles[0];
    for k in 0..n {
        let k1 = (k + 1) % n;
        mesh.add_face_uv(vec![
            Corner { vert: bottom, uv: Vec2::ZERO },
            Corner { vert: first[k], uv: disc_uv(&mesh, first[k]) },
            Corner { vert: first[k1], uv: disc_uv(&mesh, first[k1]) },
        ])?;
    }
    for (j, pair) in circles.windows(2).enumerate() {
        let (low, up) = (&pair[0], &pair[1]);
        let (v_low, v_up) = (phi(j + 1) * r_mer, phi(j + 2) * r_mer);
        for k in 0..n {
            let k1 = (k + 1) % n;
            let (u0, u1) = (theta(k) * r_eq, theta(k + 1) * r_eq);
            mesh.add_face_uv(vec![
                Corner { vert: low[k], uv: Vec2::new(u0, v_low) },
                Corner { vert: up[k], uv: Vec2::new(u0, v_up) },
                Corner { vert: up[k1], uv: Vec2::new(u1, v_up) },
                Corner { vert: low[k1], uv: Vec2::new(u1, v_low) },
            ])?;
        }
    }
    let last = &circles[m - 2];
    for k in 0..n {
        let k1 = (k + 1) % n;
        mesh.add_face_uv(vec![
            Corner { vert: last[k], uv: disc_uv(&mesh, last[k]) },
            Corner { vert: top, uv: Vec2::ZERO },
            Corner { vert: last[k1], uv: disc_uv(&mesh, last[k1]) },
        ])?;
    }
    for ring in [first, last] {
        for k in 0..n {
            mesh.seams.insert(EdgeKey::new(ring[k], ring[(k + 1) % n]));
        }
    }

    if let Some(fy) = floor_y {
        let drop: Vec<FaceId> = mesh
            .faces
            .iter()
            .filter(|(_, f)| f.verts().all(|v| mesh.verts[&v].y <= fy))
            .map(|(&id, _)| id)
            .collect();
        if drop.len() == mesh.faces.len() {
            return Err(MeshError::Invalid(format!("cavern: floor_y {fy} removes every face")));
        }
        for f in drop {
            mesh.remove_face(f)?;
        }
        mesh.remove_unused_verts();
        mesh.prune_seams();
    }
    if noise > 0.0 {
        let all = mesh.verts.keys().copied().collect();
        let r_min = radii.min_element();
        displace_noise(&mut mesh, &all, noise * r_min, 0.5 * r_min, seed)?;
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
