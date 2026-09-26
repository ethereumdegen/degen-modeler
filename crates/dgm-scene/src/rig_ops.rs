//! rig_apply / rig_auto_weights / rig_paint_weights / rig_add_bone.
//! Owned by the RigAnim slice; see CONTRACT.md.

use std::collections::{BTreeMap, BTreeSet};

use dgm_atlas::Pack;
use dgm_mesh::VertId;
use glam::Vec3;

use crate::doc::Doc;
use crate::error::OpError;
use crate::op::{BoneFit, Diff, PaintMode};
use crate::rig::{Bone, MAX_BONES, MAX_INFLUENCES, Rig};
use crate::select::{self, Selection};

/// Distance floor for inverse-distance weighting: keeps verts sitting exactly
/// on a bone segment finite and dominant without dividing by zero.
const WEIGHT_EPS: f32 = 1e-4;

/// Weights below this are dropped instead of stored (noise on the wire).
const WEIGHT_MIN: f32 = 1e-6;

/// Mirror of `op::DIFF_ELEM_CAP`: keep Diff element lists wire-sized.
const ELEM_CAP: usize = 64;

fn cap_elems(v: Vec<String>) -> Vec<String> {
    if v.len() > ELEM_CAP {
        let extra = v.len() - ELEM_CAP;
        let mut v: Vec<String> = v.into_iter().take(ELEM_CAP).collect();
        v.push(format!("(+{extra} more)"));
        v
    } else {
        v
    }
}

/// Fit a pack rig preset into the object's bounds and install it.
///
/// Preset bones live in normalized space (unit height, feet at y=0): they are
/// scaled uniformly by the mesh bounds height, translated so preset y=0 lands
/// on the bounds min y, and centered on the bounds x/z center. `fit` then
/// overrides individual bones in model space. Any existing rig (including its
/// weights) is replaced.
pub fn rig_apply(
    doc: &mut Doc,
    pack: &Pack,
    object: &str,
    preset: &str,
    fit: &BTreeMap<String, BoneFit>,
) -> Result<Diff, OpError> {
    let preset_def = pack
        .manifest
        .rigs
        .get(preset)
        .ok_or_else(|| OpError::Pack(format!("unknown rig preset `{preset}`")))?;
    if preset_def.bones.len() > MAX_BONES {
        return Err(OpError::Pack(format!(
            "rig preset `{preset}` has {} bones, over the cap of {MAX_BONES}",
            preset_def.bones.len()
        )));
    }

    let obj = doc.object(object)?;
    let (lo, hi) = obj
        .mesh
        .bounds()
        .ok_or_else(|| OpError::BadParams(format!("object `{object}` has no vertices to fit a rig to")))?;
    let height = hi.y - lo.y;
    if height <= 0.0 {
        return Err(OpError::BadParams(format!(
            "object `{object}` has zero height; cannot fit rig `{preset}`"
        )));
    }
    let center = (lo + hi) * 0.5;
    let replaced = obj.rig.is_some();

    let to_model =
        |p: [f32; 3]| Vec3::new(center.x + p[0] * height, lo.y + p[1] * height, center.z + p[2] * height);

    let mut bones = Vec::with_capacity(preset_def.bones.len());
    for pb in &preset_def.bones {
        let parent = match &pb.parent {
            None => None,
            Some(pn) => Some(
                preset_def
                    .bones
                    .iter()
                    .position(|b| &b.name == pn)
                    .ok_or_else(|| {
                        OpError::Pack(format!(
                            "rig preset `{preset}`: bone `{}` parents unknown bone `{pn}`",
                            pb.name
                        ))
                    })? as u16,
            ),
        };
        bones.push(Bone { name: pb.name.clone(), parent, head: to_model(pb.head), tail: to_model(pb.tail) });
    }

    for (name, f) in fit {
        let bone = bones
            .iter_mut()
            .find(|b| &b.name == name)
            .ok_or_else(|| OpError::UnknownBone(name.clone()))?;
        bone.head = Vec3::from(f.head);
        bone.tail = Vec3::from(f.tail);
    }

    let created: Vec<String> = bones.iter().map(|b| b.name.clone()).collect();
    let count = bones.len();
    doc.object_mut(object)?.rig =
        Some(Rig { preset: preset.into(), bones, weights: BTreeMap::new() });

    Ok(Diff {
        summary: format!(
            "{object}: rig `{preset}` applied, {count} bones{}",
            if replaced { " (replaced previous rig, weights cleared)" } else { "" }
        ),
        created: cap_elems(created),
        ..Diff::default()
    })
}

/// Shortest distance from `p` to the segment `a..b`.
fn point_segment_distance(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Inverse-distance skin weights: per vertex, take the up-to-[`MAX_INFLUENCES`]
/// nearest bone segments (ties broken by bone index), weight `1/(d+eps)`,
/// normalized to sum 1. Fully deterministic; replaces all existing weights.
pub fn rig_auto_weights(doc: &mut Doc, object: &str) -> Result<Diff, OpError> {
    let obj = doc.object(object)?;
    let rig = obj
        .rig
        .as_ref()
        .ok_or_else(|| OpError::BadParams(format!("object `{object}` has no rig; run rig_apply first")))?;
    let segments: Vec<(Vec3, Vec3)> = rig.bones.iter().map(|b| (b.head, b.tail)).collect();
    if segments.is_empty() {
        return Err(OpError::BadParams(format!("object `{object}`: rig has no bones")));
    }

    let mut weights: BTreeMap<VertId, Vec<(u16, f32)>> = BTreeMap::new();
    for (&v, &p) in &obj.mesh.verts {
        let mut dists: Vec<(f32, u16)> = segments
            .iter()
            .enumerate()
            .map(|(i, &(a, b))| (point_segment_distance(p, a, b), i as u16))
            .collect();
        dists.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        dists.truncate(MAX_INFLUENCES);

        let mut influences: Vec<(u16, f32)> =
            dists.iter().map(|&(d, i)| (i, 1.0 / (d + WEIGHT_EPS))).collect();
        let sum: f32 = influences.iter().map(|&(_, w)| w).sum();
        for w in &mut influences {
            w.1 /= sum;
        }
        influences.sort_by_key(|&(i, _)| i);
        weights.insert(v, influences);
    }

    let modified: Vec<String> = weights.keys().map(|v| v.to_string()).collect();
    let (verts, bones) = (weights.len(), segments.len());
    doc.object_mut(object)?.rig.as_mut().expect("rig checked above").weights = weights;

    Ok(Diff {
        summary: format!("{object}: auto weights on {verts} verts across {bones} bones"),
        modified: cap_elems(modified),
        ..Diff::default()
    })
}

/// Weight of `bone` at `v` in a weight map, 0 when absent.
fn weight_of(weights: &BTreeMap<VertId, Vec<(u16, f32)>>, v: VertId, bone: u16) -> f32 {
    weights
        .get(&v)
        .and_then(|ws| ws.iter().find(|&&(b, _)| b == bone).map(|&(_, w)| w))
        .unwrap_or(0.0)
}

/// Set `bone` to `w` in a per-vert influence list, keeping the vert
/// normalized: other influences are rescaled to fill `1-w`, near-zero entries
/// dropped, the list clamped to [`MAX_INFLUENCES`] by dropping the smallest
/// weights (ties keep the lower bone index), and the survivors renormalized.
fn apply_weight(entry: &mut Vec<(u16, f32)>, bone: u16, w: f32) {
    let w = w.clamp(0.0, 1.0);
    entry.retain(|&(b, _)| b != bone);
    let others: f32 = entry.iter().map(|&(_, x)| x).sum();
    if others > 0.0 {
        let scale = (1.0 - w) / others;
        for e in entry.iter_mut() {
            e.1 *= scale;
        }
    }
    if w > 0.0 {
        entry.push((bone, w));
    }
    entry.retain(|&(_, x)| x > WEIGHT_MIN);
    if entry.len() > MAX_INFLUENCES {
        entry.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        entry.truncate(MAX_INFLUENCES);
    }
    let sum: f32 = entry.iter().map(|&(_, x)| x).sum();
    if sum > 0.0 {
        for e in entry.iter_mut() {
            e.1 /= sum;
        }
    }
    entry.sort_by_key(|&(b, _)| b);
}

/// Paint skin weights on the selection's vertices.
///
/// `value` is the target weight (0..1). With `falloff > 0` the effect fades
/// linearly to zero at `falloff` meters from the selection centroid.
/// Modes: `Add` adds `value`, `Replace` blends the weight toward `value`,
/// `Smooth` moves it toward the average of the vert and its topological
/// (edge) neighbours for that bone. Every touched vert is renormalized and
/// clamped to [`MAX_INFLUENCES`] influences by dropping the smallest.
pub fn rig_paint_weights(
    doc: &mut Doc,
    sel: &Selection,
    bone: &str,
    value: f32,
    falloff: f32,
    mode: PaintMode,
) -> Result<Diff, OpError> {
    if !value.is_finite() || value < 0.0 {
        return Err(OpError::BadParams(format!("weight value {value} must be finite and >= 0")));
    }
    if !falloff.is_finite() || falloff < 0.0 {
        return Err(OpError::BadParams(format!("falloff {falloff} must be finite and >= 0")));
    }

    let object = sel.object.clone();
    let obj = doc.object(&object)?;
    let rig = obj
        .rig
        .as_ref()
        .ok_or_else(|| OpError::BadParams(format!("object `{object}` has no rig; run rig_apply first")))?;
    let bone_idx = rig.bone_index(bone).ok_or_else(|| OpError::UnknownBone(bone.into()))?;

    let verts = select::to_verts(&obj.mesh, sel)?;
    if verts.is_empty() {
        return Err(OpError::BadParams("empty selection".into()));
    }

    let mut centroid = Vec3::ZERO;
    for &v in &verts {
        centroid += obj.mesh.pos(v)?;
    }
    centroid /= verts.len() as f32;

    let mut neighbours: BTreeMap<VertId, BTreeSet<VertId>> = BTreeMap::new();
    if mode == PaintMode::Smooth {
        for e in obj.mesh.edges() {
            neighbours.entry(e.0).or_default().insert(e.1);
            neighbours.entry(e.1).or_default().insert(e.0);
        }
    }

    // All targets are computed from this snapshot so smoothing never reads a
    // half-updated neighbour: order independent, deterministic.
    let snapshot = &rig.weights;
    let mut updates: Vec<(VertId, Vec<(u16, f32)>)> = Vec::with_capacity(verts.len());
    for &v in &verts {
        let p = obj.mesh.pos(v)?;
        let t = if falloff > 0.0 { (1.0 - p.distance(centroid) / falloff).max(0.0) } else { 1.0 };
        let old = weight_of(snapshot, v, bone_idx);
        let target = match mode {
            PaintMode::Add => old + value * t,
            PaintMode::Replace => old + (value - old) * t,
            PaintMode::Smooth => {
                let mut acc = old;
                let mut n = 1.0;
                if let Some(nbs) = neighbours.get(&v) {
                    for &nb in nbs {
                        acc += weight_of(snapshot, nb, bone_idx);
                        n += 1.0;
                    }
                }
                let avg = acc / n;
                old + (avg - old) * (value.min(1.0) * t)
            }
        };

        let mut entry = snapshot.get(&v).cloned().unwrap_or_default();
        apply_weight(&mut entry, bone_idx, target);
        updates.push((v, entry));
    }

    let modified: Vec<String> = updates.iter().map(|(v, _)| v.to_string()).collect();
    let count = updates.len();
    let rig = doc.object_mut(&object)?.rig.as_mut().expect("rig checked above");
    for (v, entry) in updates {
        if entry.is_empty() {
            rig.weights.remove(&v);
        } else {
            rig.weights.insert(v, entry);
        }
    }

    Ok(Diff {
        summary: format!("{object}: painted `{bone}` ({mode:?}) on {count} verts"),
        modified: cap_elems(modified),
        ..Diff::default()
    })
}

/// Append a bone (model space) under an existing parent; enforces the
/// [`MAX_BONES`] cap and unique bone names.
pub fn rig_add_bone(
    doc: &mut Doc,
    object: &str,
    name: &str,
    parent: &str,
    head: [f32; 3],
    tail: [f32; 3],
) -> Result<Diff, OpError> {
    let obj = doc.object_mut(object)?;
    let rig = obj
        .rig
        .as_mut()
        .ok_or_else(|| OpError::BadParams(format!("object `{object}` has no rig; run rig_apply first")))?;
    if rig.bones.iter().any(|b| b.name == name) {
        return Err(OpError::BadParams(format!("object `{object}` already has a bone `{name}`")));
    }
    let parent_idx = rig.bone_index(parent).ok_or_else(|| OpError::UnknownBone(parent.into()))?;
    if rig.bones.len() >= MAX_BONES {
        return Err(OpError::BadParams(format!(
            "object `{object}` is at the bone cap ({MAX_BONES}); cannot add `{name}`"
        )));
    }
    rig.bones.push(Bone {
        name: name.into(),
        parent: Some(parent_idx),
        head: Vec3::from(head),
        tail: Vec3::from(tail),
    });
    Ok(Diff {
        summary: format!("{object}: bone `{name}` added under `{parent}`"),
        created: vec![name.into()],
        ..Diff::default()
    })
}
