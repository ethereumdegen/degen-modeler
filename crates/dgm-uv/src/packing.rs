//! Island packing: deterministic shelf packing (tallest first) with a
//! binary-searched global scale. Margins are measured in final UV units, so
//! `margin_px / texture_px` really is that many pixels between islands and
//! from every atlas border. Relative island scale is preserved, keeping
//! texel density uniform across islands.

use std::collections::BTreeSet;

use dgm_mesh::{FaceId, Mesh, MeshDelta};
use glam::Vec2;

use crate::UvError;
use crate::geom::{island_uv_bbox, uv_delta};
use crate::islands::islands;

/// Degenerate (zero-extent) islands are packed as this pseudo-size.
const MIN_SIZE: f32 = 1e-4;

/// Repack all UV islands into `[0,1]^2` with `margin_px` pixels (at
/// `texture_px`) between islands and from the borders.
pub fn pack_islands(
    mesh: &mut Mesh,
    margin_px: u32,
    texture_px: u32,
) -> Result<MeshDelta, UvError> {
    if texture_px == 0 {
        return Err(UvError::Invalid("pack: texture_px must be > 0".into()));
    }
    let isl = islands(mesh);
    if isl.is_empty() {
        return Ok(MeshDelta::default());
    }
    pack_all_islands(mesh, &isl, margin_px as f32 / texture_px as f32)?;
    Ok(uv_delta(mesh.faces.keys().copied()))
}

/// Scale + translate the given islands into `[0,1]^2` (used by `unwrap` and
/// [`pack_islands`]).
pub(crate) fn pack_all_islands(
    mesh: &mut Mesh,
    islands: &[BTreeSet<FaceId>],
    margin: f32,
) -> Result<(), UvError> {
    let bboxes: Vec<(Vec2, Vec2)> = islands.iter().map(|i| island_uv_bbox(mesh, i)).collect();
    let sizes: Vec<Vec2> = bboxes.iter().map(|(lo, hi)| *hi - *lo).collect();
    let placed = pack_into_unit(&sizes, margin)?;
    for (k, island) in islands.iter().enumerate() {
        let lo = bboxes[k].0;
        let origin = placed.origins[k];
        for &f in island {
            if let Some(face) = mesh.faces.get_mut(&f) {
                for c in &mut face.corners {
                    c.uv = (c.uv - lo) * placed.scale + origin;
                }
            }
        }
    }
    Ok(())
}

pub(crate) struct Placement {
    pub scale: f32,
    /// Final UV position of each island's bbox minimum, input order.
    pub origins: Vec<Vec2>,
}

/// Shelf-pack rects of `scale * sizes[i]` into `[0,1]^2` at the largest
/// scale that fits, with `margin` between rects and from the borders.
pub(crate) fn pack_into_unit(sizes: &[Vec2], margin: f32) -> Result<Placement, UvError> {
    if sizes.is_empty() {
        return Ok(Placement { scale: 1.0, origins: Vec::new() });
    }
    if !(0.0..0.5).contains(&margin) {
        return Err(UvError::Invalid(format!("pack: margin {margin} leaves no atlas space")));
    }
    let sizes: Vec<Vec2> = sizes.iter().map(|s| s.max(Vec2::splat(MIN_SIZE))).collect();
    // Padded rects (island + margin right/bottom) go into a bin of side
    // `1 - margin`; islands land at padded origin + margin. That yields
    // >= margin between neighbours and from all four borders.
    let bin = 1.0 - margin;
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|&a, &b| sizes[b].y.total_cmp(&sizes[a].y).then(a.cmp(&b)));
    let try_pack = |scale: f32| -> Option<Vec<Vec2>> {
        let mut origins = vec![Vec2::ZERO; sizes.len()];
        let (mut x, mut y, mut shelf) = (0.0f32, 0.0f32, 0.0f32);
        for &i in &order {
            let w = sizes[i].x * scale + margin;
            let h = sizes[i].y * scale + margin;
            if w > bin || h > bin {
                return None;
            }
            if x + w > bin && x > 0.0 {
                y += shelf;
                x = 0.0;
                shelf = 0.0;
            }
            if y + h > bin {
                return None;
            }
            origins[i] = Vec2::new(x + margin, y + margin);
            shelf = shelf.max(h);
            x += w;
        }
        Some(origins)
    };
    let max_dim = sizes.iter().map(|s| s.max_element()).fold(0.0f32, f32::max);
    let mut hi = (bin - margin) / max_dim;
    if let Some(origins) = try_pack(hi) {
        return Ok(Placement { scale: hi, origins });
    }
    let mut lo = 0.0f32;
    for _ in 0..48 {
        let mid = 0.5 * (lo + hi);
        if try_pack(mid).is_some() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    match try_pack(lo) {
        Some(origins) if lo > 0.0 => Ok(Placement { scale: lo, origins }),
        _ => Err(UvError::Invalid(format!(
            "pack: cannot fit {} islands with margin {margin}",
            sizes.len()
        ))),
    }
}
