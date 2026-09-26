//! clip_apply / clip_key / clip_set_loop / clip_delete.
//! Owned by the RigAnim slice; see CONTRACT.md.

use std::collections::BTreeMap;

use dgm_atlas::Pack;
use glam::{EulerRot, Quat};

use crate::anim::{Channel, Clip};
use crate::doc::Doc;
use crate::error::OpError;
use crate::op::Diff;

/// Two key times closer than this are the same key (upsert, not insert).
const TIME_EPS: f32 = 1e-5;

/// Euler XYZ degrees -> quaternion `[x, y, z, w]`.
fn euler_deg_to_quat(e: [f32; 3]) -> [f32; 4] {
    let q = Quat::from_euler(EulerRot::XYZ, e[0].to_radians(), e[1].to_radians(), e[2].to_radians());
    [q.x, q.y, q.z, q.w]
}

/// Segment of `times` containing `t`: (left, right, alpha), clamped to the
/// ends. `times` must be sorted and non-empty.
fn segment(times: &[f32], t: f32) -> (usize, usize, f32) {
    if t <= times[0] {
        return (0, 0, 0.0);
    }
    let last = times.len() - 1;
    if t >= times[last] {
        return (last, last, 0.0);
    }
    let right = times.iter().position(|&x| x > t).expect("t < last time");
    let left = right - 1;
    (left, right, (t - times[left]) / (times[right] - times[left]))
}

/// Linear sample of a vec3 curve at `t`; `rest` when the curve is empty.
fn sample3(times: &[f32], vals: &[[f32; 3]], t: f32, rest: [f32; 3]) -> [f32; 3] {
    if vals.is_empty() {
        return rest;
    }
    let (a, b, alpha) = segment(times, t);
    let (va, vb) = (vals[a], vals[b]);
    [
        va[0] + (vb[0] - va[0]) * alpha,
        va[1] + (vb[1] - va[1]) * alpha,
        va[2] + (vb[2] - va[2]) * alpha,
    ]
}

/// Shortest-arc normalized-lerp sample of a quaternion curve at `t`.
fn sample_quat(times: &[f32], vals: &[[f32; 4]], t: f32) -> [f32; 4] {
    if vals.is_empty() {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let (a, b, alpha) = segment(times, t);
    let qa = Quat::from_array(vals[a]);
    let mut qb = Quat::from_array(vals[b]);
    if qa.dot(qb) < 0.0 {
        qb = -qb;
    }
    let q = (qa.lerp(qb, alpha)).normalize();
    [q.x, q.y, q.z, q.w]
}

/// Instantiate a pack clip template onto a rigged object as `doc.clips[name]`.
///
/// The template must target the object's rig preset. Key times and duration
/// scale by `1/speed`; euler XYZ degree rotations become quaternions;
/// normalized translations scale by the fitted rig height (mesh bounds
/// height). Re-applying over an existing clip name replaces it.
pub fn clip_apply(
    doc: &mut Doc,
    pack: &Pack,
    name: &str,
    object: &str,
    template: &str,
    speed: f32,
) -> Result<Diff, OpError> {
    if !speed.is_finite() || speed <= 0.0 {
        return Err(OpError::BadParams(format!("speed {speed} must be finite and > 0")));
    }
    let tpl = pack
        .manifest
        .clips
        .get(template)
        .ok_or_else(|| OpError::Pack(format!("unknown clip template `{template}`")))?;

    let obj = doc.object(object)?;
    let rig = obj
        .rig
        .as_ref()
        .ok_or_else(|| OpError::BadParams(format!("object `{object}` has no rig; run rig_apply first")))?;
    if tpl.rig != rig.preset {
        return Err(OpError::BadParams(format!(
            "clip template `{template}` targets rig `{}` but object `{object}` is rigged with `{}`",
            tpl.rig, rig.preset
        )));
    }
    let (lo, hi) = obj
        .mesh
        .bounds()
        .ok_or_else(|| OpError::BadParams(format!("object `{object}` has no vertices")))?;
    let height = hi.y - lo.y;

    let inv = 1.0 / speed;
    let mut channels = BTreeMap::new();
    for (bone, tch) in &tpl.channels {
        if rig.bone_index(bone).is_none() {
            return Err(OpError::UnknownBone(format!("{bone} (clip template `{template}`)")));
        }
        channels.insert(
            bone.clone(),
            Channel {
                times: tch.times.iter().map(|t| t * inv).collect(),
                rotations: tch
                    .rotations_euler_deg
                    .as_ref()
                    .map(|rs| rs.iter().map(|&e| euler_deg_to_quat(e)).collect()),
                translations: tch
                    .translations
                    .as_ref()
                    .map(|ts| ts.iter().map(|t| [t[0] * height, t[1] * height, t[2] * height]).collect()),
                scales: tch.scales.clone(),
            },
        );
    }

    let bones = channels.len();
    let duration = tpl.duration * inv;
    let replaced = doc.clips.contains_key(name);
    doc.clips
        .insert(name.into(), Clip { object: object.into(), looped: tpl.looped, duration, channels });

    let mut diff = Diff::summary(format!(
        "clip `{name}`: `{template}` on {object}, {duration:.3}s, {bones} bone channels{}",
        if replaced { " (replaced)" } else { "" }
    ));
    if replaced {
        diff.modified.push(name.into());
    } else {
        diff.created.push(name.into());
    }
    Ok(diff)
}

/// Upsert one key into a clip channel.
///
/// `rot` is euler XYZ degrees. A key at an existing time overwrites the
/// provided components. A new time is inserted in sorted order; curve arrays
/// the key does not provide get the curve's value sampled at that time, and a
/// provided component whose curve does not exist yet creates it filled with
/// rest values (identity rotation, zero translation, unit scale) for the
/// existing times. Keys beyond the clip duration extend it.
pub fn clip_key(
    doc: &mut Doc,
    clip: &str,
    bone: &str,
    time: f32,
    rot: Option<[f32; 3]>,
    tr: Option<[f32; 3]>,
    sc: Option<[f32; 3]>,
) -> Result<Diff, OpError> {
    if rot.is_none() && tr.is_none() && sc.is_none() {
        return Err(OpError::BadParams(
            "clip_key needs at least one of rotation/translation/scale".into(),
        ));
    }
    if !time.is_finite() || time < 0.0 {
        return Err(OpError::BadParams(format!("key time {time} must be finite and >= 0")));
    }

    let object =
        doc.clips.get(clip).ok_or_else(|| OpError::UnknownClip(clip.into()))?.object.clone();
    let rig = doc
        .object(&object)?
        .rig
        .as_ref()
        .ok_or_else(|| OpError::BadParams(format!("clip `{clip}`: object `{object}` has no rig")))?;
    if rig.bone_index(bone).is_none() {
        return Err(OpError::UnknownBone(bone.into()));
    }

    let c = doc.clips.get_mut(clip).expect("clip checked above");
    let ch = c.channels.entry(bone.into()).or_default();

    // Curves for provided components must exist; backfill rest for old keys.
    if rot.is_some() && ch.rotations.is_none() {
        ch.rotations = Some(vec![[0.0, 0.0, 0.0, 1.0]; ch.times.len()]);
    }
    if tr.is_some() && ch.translations.is_none() {
        ch.translations = Some(vec![[0.0, 0.0, 0.0]; ch.times.len()]);
    }
    if sc.is_some() && ch.scales.is_none() {
        ch.scales = Some(vec![[1.0, 1.0, 1.0]; ch.times.len()]);
    }

    match ch.times.iter().position(|&t| (t - time).abs() <= TIME_EPS) {
        Some(i) => {
            if let Some(e) = rot {
                ch.rotations.as_mut().expect("created above")[i] = euler_deg_to_quat(e);
            }
            if let Some(v) = tr {
                ch.translations.as_mut().expect("created above")[i] = v;
            }
            if let Some(v) = sc {
                ch.scales.as_mut().expect("created above")[i] = v;
            }
        }
        None => {
            let i = ch.times.iter().position(|&t| t > time).unwrap_or(ch.times.len());
            // Sample unprovided curves before touching times so existing
            // motion keeps its shape through the new key.
            let rot_v = match rot {
                Some(e) => Some(euler_deg_to_quat(e)),
                None => ch.rotations.as_ref().map(|c| sample_quat(&ch.times, c, time)),
            };
            let tr_v = match tr {
                Some(v) => Some(v),
                None => {
                    ch.translations.as_ref().map(|c| sample3(&ch.times, c, time, [0.0, 0.0, 0.0]))
                }
            };
            let sc_v = match sc {
                Some(v) => Some(v),
                None => ch.scales.as_ref().map(|c| sample3(&ch.times, c, time, [1.0, 1.0, 1.0])),
            };
            ch.times.insert(i, time);
            if let (Some(curve), Some(v)) = (ch.rotations.as_mut(), rot_v) {
                curve.insert(i, v);
            }
            if let (Some(curve), Some(v)) = (ch.translations.as_mut(), tr_v) {
                curve.insert(i, v);
            }
            if let (Some(curve), Some(v)) = (ch.scales.as_mut(), sc_v) {
                curve.insert(i, v);
            }
        }
    }
    c.duration = c.duration.max(time);

    Ok(Diff {
        summary: format!("clip `{clip}`: key `{bone}` @ {time:.3}s"),
        modified: vec![format!("{clip}/{bone}@{time:.3}")],
        ..Diff::default()
    })
}

pub fn clip_set_loop(doc: &mut Doc, clip: &str, looped: bool) -> Result<Diff, OpError> {
    let c = doc.clips.get_mut(clip).ok_or_else(|| OpError::UnknownClip(clip.into()))?;
    c.looped = looped;
    Ok(Diff {
        summary: format!("clip `{clip}`: looped = {looped}"),
        modified: vec![clip.into()],
        ..Diff::default()
    })
}

pub fn clip_delete(doc: &mut Doc, clip: &str) -> Result<Diff, OpError> {
    doc.clips.remove(clip).ok_or_else(|| OpError::UnknownClip(clip.into()))?;
    Ok(Diff {
        summary: format!("clip `{clip}` deleted"),
        removed: vec![clip.into()],
        ..Diff::default()
    })
}
