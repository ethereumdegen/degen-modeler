//! Deform ops: free-form lattice displacement over the mesh bounds.

use glam::Vec3;

use crate::mesh::{Mesh, MeshDelta, MeshError};

/// Deform every vertex by trilinearly interpolating `displacements` over a
/// `dims = [nx, ny, nz]` control grid stretched across the mesh bounds.
///
/// Control points are indexed x-fastest: `idx = i + nx * (j + ny * k)`, with
/// `i` along +X, `j` along +Y, `k` along +Z, `[0, 0, 0]` at the bounds
/// minimum. Axes with zero extent use the grid's low plane.
pub fn lattice(mesh: &mut Mesh, dims: [u32; 3], displacements: &[Vec3]) -> Result<MeshDelta, MeshError> {
    let [nx, ny, nz] = dims.map(|d| d as usize);
    if nx < 2 || ny < 2 || nz < 2 {
        return Err(MeshError::Invalid(format!(
            "lattice: dims must each be >= 2, got [{}, {}, {}]",
            dims[0], dims[1], dims[2]
        )));
    }
    let expected = nx
        .checked_mul(ny)
        .and_then(|p| p.checked_mul(nz))
        .ok_or_else(|| MeshError::Invalid("lattice: dims overflow".into()))?;
    if displacements.len() != expected {
        return Err(MeshError::Invalid(format!(
            "lattice: expected {expected} displacements for a {}x{}x{} grid, got {}",
            dims[0],
            dims[1],
            dims[2],
            displacements.len()
        )));
    }
    let Some((lo, hi)) = mesh.bounds() else {
        return Err(MeshError::Invalid("lattice: mesh has no vertices".into()));
    };
    let ext = hi - lo;

    // Per-axis cell index + intra-cell fraction for a normalized coordinate.
    let cell = |t: f32, n: usize| -> (usize, f32) {
        let f = (t * (n - 1) as f32).clamp(0.0, (n - 1) as f32);
        let i = (f.floor() as usize).min(n - 2);
        (i, f - i as f32)
    };

    let mut delta = MeshDelta::default();
    let ids: Vec<_> = mesh.verts.keys().copied().collect();
    for v in ids {
        let p = mesh.pos(v)?;
        let t = Vec3::new(
            if ext.x > 0.0 { (p.x - lo.x) / ext.x } else { 0.0 },
            if ext.y > 0.0 { (p.y - lo.y) / ext.y } else { 0.0 },
            if ext.z > 0.0 { (p.z - lo.z) / ext.z } else { 0.0 },
        );
        let (i, fx) = cell(t.x, nx);
        let (j, fy) = cell(t.y, ny);
        let (k, fz) = cell(t.z, nz);
        let at = |di: usize, dj: usize, dk: usize| displacements[(i + di) + nx * ((j + dj) + ny * (k + dk))];
        let d = at(0, 0, 0).lerp(at(1, 0, 0), fx).lerp(at(0, 1, 0).lerp(at(1, 1, 0), fx), fy).lerp(
            at(0, 0, 1).lerp(at(1, 0, 1), fx).lerp(at(0, 1, 1).lerp(at(1, 1, 1), fx), fy),
            fz,
        );
        if d != Vec3::ZERO {
            mesh.set_pos(v, p + d)?;
            delta.moved_verts.push(v);
        }
    }
    Ok(delta)
}
