//! Clip sampling (linear, looped) and CPU skinning against the doc rig.
//!
//! Pure math on workspace `glam` 0.30; the Bevy layer converts through
//! arrays. Semantics match the glTF export (dgm-gltf): a bone node's rest
//! local transform is the head offset from its parent; sampled rotations and
//! scales *replace* the rest component, while sampled translations are
//! *deltas added to* the rest offset (the exporter bakes rest + delta into
//! absolute glTF node translations). Skin matrix = animated world * inverse
//! bind, with the inverse bind derived from the rest heads.

use dgm_mesh::VertId;
use dgm_scene::rig::Rig;
use dgm_scene::{Channel, Clip};
use glam::{Mat4, Quat, Vec3};

/// Map a wall-clock playhead into the clip's key range.
pub fn clip_time(t: f32, duration: f32, looped: bool) -> f32 {
    if duration <= 0.0 {
        return 0.0;
    }
    if looped {
        t.rem_euclid(duration)
    } else {
        t.clamp(0.0, duration)
    }
}

/// `(i0, i1, alpha)` for linear interpolation of sorted `times` at `t`.
fn key_span(times: &[f32], t: f32) -> (usize, usize, f32) {
    match times.iter().position(|&k| k > t) {
        Some(0) => (0, 0, 0.0),
        None => (times.len() - 1, times.len() - 1, 0.0),
        Some(i) => {
            let (t0, t1) = (times[i - 1], times[i]);
            let span = t1 - t0;
            let alpha = if span > 0.0 { (t - t0) / span } else { 0.0 };
            (i - 1, i, alpha)
        }
    }
}

/// Linearly sample one channel at `t` (already wrapped into the key range).
/// Returns `(translation, rotation, scale)`; `None` where the channel has no
/// curve of that kind.
pub fn sample_channel(ch: &Channel, t: f32) -> (Option<Vec3>, Option<Quat>, Option<Vec3>) {
    if ch.times.is_empty() {
        return (None, None, None);
    }
    let (i0, i1, a) = key_span(&ch.times, t);
    let vec3_at = |curve: &Option<Vec<[f32; 3]>>| -> Option<Vec3> {
        let curve = curve.as_ref()?;
        let v0 = Vec3::from_array(*curve.get(i0)?);
        let v1 = Vec3::from_array(*curve.get(i1)?);
        Some(v0.lerp(v1, a))
    };
    let rot = ch.rotations.as_ref().and_then(|curve| {
        let q0 = Quat::from_array(*curve.get(i0)?).normalize();
        let q1 = Quat::from_array(*curve.get(i1)?).normalize();
        Some(q0.slerp(q1, a))
    });
    (vec3_at(&ch.translations), rot, vec3_at(&ch.scales))
}

/// Per-bone skinning matrices (animated world * inverse bind) for `rig`
/// posed by `posed = (clip, wrapped_time)`; rest pose (all identity) when
/// `posed` is `None`.
pub fn skin_matrices(rig: &Rig, posed: Option<(&Clip, f32)>) -> Vec<Mat4> {
    let n = rig.bones.len();
    let mut locals = Vec::with_capacity(n);
    for bone in &rig.bones {
        let parent_head = bone
            .parent
            .and_then(|p| rig.bones.get(p as usize))
            .map(|b| b.head)
            .unwrap_or(Vec3::ZERO);
        let rest_offset = bone.head - parent_head;
        let mut translation = rest_offset;
        let mut rotation = Quat::IDENTITY;
        let mut scale = Vec3::ONE;
        if let Some((clip, t)) = posed
            && let Some(ch) = clip.channels.get(&bone.name)
        {
            let (tr, rot, sc) = sample_channel(ch, t);
            if let Some(v) = tr {
                translation = rest_offset + v;
            }
            if let Some(q) = rot {
                rotation = q;
            }
            if let Some(v) = sc {
                scale = v;
            }
        }
        locals.push(Mat4::from_scale_rotation_translation(scale, rotation, translation));
    }

    // Resolve worlds without assuming parent-before-child order; a cycle
    // (impossible for a gate-clean rig) degrades to the local transform.
    let mut worlds: Vec<Option<Mat4>> = vec![None; n];
    for i in 0..n {
        resolve_world(rig, &locals, &mut worlds, i, 0);
    }
    (0..n)
        .map(|i| worlds[i].unwrap_or(locals[i]) * Mat4::from_translation(-rig.bones[i].head))
        .collect()
}

fn resolve_world(rig: &Rig, locals: &[Mat4], worlds: &mut Vec<Option<Mat4>>, i: usize, depth: usize) -> Mat4 {
    if let Some(w) = worlds[i] {
        return w;
    }
    let w = match rig.bones[i].parent {
        Some(p) if (p as usize) < rig.bones.len() && depth <= rig.bones.len() => {
            resolve_world(rig, locals, worlds, p as usize, depth + 1) * locals[i]
        }
        _ => locals[i],
    };
    worlds[i] = Some(w);
    w
}

/// Skin one vertex position + normal by the rig's weights. Unweighted
/// vertices pass through untouched.
pub fn skin_vertex(rig: &Rig, mats: &[Mat4], vert: VertId, pos: Vec3, normal: Vec3) -> (Vec3, Vec3) {
    let Some(weights) = rig.weights.get(&vert) else {
        return (pos, normal);
    };
    let mut p = Vec3::ZERO;
    let mut nrm = Vec3::ZERO;
    let mut total = 0.0;
    for &(bone, w) in weights {
        if let Some(m) = mats.get(bone as usize) {
            p += m.transform_point3(pos) * w;
            nrm += m.transform_vector3(normal) * w;
            total += w;
        }
    }
    if total <= 1e-6 {
        return (pos, normal);
    }
    (p / total, (nrm / total).normalize_or(normal))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use dgm_scene::rig::Bone;

    use super::*;

    fn channel(times: Vec<f32>) -> Channel {
        Channel { times, rotations: None, translations: None, scales: None }
    }

    #[test]
    fn clip_time_wraps_looped_and_clamps_oneshot() {
        assert!((clip_time(1.25, 1.0, true) - 0.25).abs() < 1e-6);
        assert!((clip_time(-0.25, 1.0, true) - 0.75).abs() < 1e-6);
        assert_eq!(clip_time(1.25, 1.0, false), 1.0);
        assert_eq!(clip_time(-0.5, 1.0, false), 0.0);
        assert_eq!(clip_time(3.0, 0.0, true), 0.0);
    }

    #[test]
    fn translation_lerps_between_keys() {
        let mut ch = channel(vec![0.0, 1.0]);
        ch.translations = Some(vec![[0.0, 0.0, 0.0], [2.0, 4.0, -2.0]]);
        let (tr, rot, sc) = sample_channel(&ch, 0.5);
        assert_eq!(tr.unwrap(), Vec3::new(1.0, 2.0, -1.0));
        assert!(rot.is_none());
        assert!(sc.is_none());
    }

    #[test]
    fn sampling_clamps_outside_key_range() {
        let mut ch = channel(vec![0.25, 0.75]);
        ch.translations = Some(vec![[1.0, 0.0, 0.0], [3.0, 0.0, 0.0]]);
        assert_eq!(sample_channel(&ch, 0.0).0.unwrap().x, 1.0);
        assert_eq!(sample_channel(&ch, 0.9).0.unwrap().x, 3.0);
    }

    #[test]
    fn rotation_slerps_halfway() {
        let mut ch = channel(vec![0.0, 1.0]);
        let q90 = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        ch.rotations = Some(vec![Quat::IDENTITY.to_array(), q90.to_array()]);
        let rot = sample_channel(&ch, 0.5).1.unwrap();
        let expect = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        assert!(rot.angle_between(expect) < 1e-4, "got {rot:?}");
    }

    fn one_bone_rig(head: Vec3, weights: &[(u32, Vec<(u16, f32)>)]) -> Rig {
        Rig {
            preset: "test".into(),
            bones: vec![Bone { name: "root".into(), parent: None, head, tail: head + Vec3::Y }],
            weights: weights.iter().map(|(v, w)| (VertId(*v), w.clone())).collect(),
        }
    }

    #[test]
    fn rest_pose_matrices_are_identity() {
        let rig = Rig {
            preset: "test".into(),
            bones: vec![
                Bone { name: "root".into(), parent: None, head: Vec3::ZERO, tail: Vec3::Y },
                Bone { name: "spine".into(), parent: Some(0), head: Vec3::Y, tail: Vec3::Y * 2.0 },
            ],
            weights: BTreeMap::new(),
        };
        for m in skin_matrices(&rig, None) {
            assert!(m.abs_diff_eq(Mat4::IDENTITY, 1e-6), "{m:?}");
        }
    }

    #[test]
    fn root_rotation_spins_vertex_around_bone_head() {
        let rig = one_bone_rig(Vec3::ZERO, &[(0, vec![(0, 1.0)])]);
        let q90 = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let mut ch = channel(vec![0.0]);
        ch.rotations = Some(vec![q90.to_array()]);
        let clip = Clip {
            object: "o".into(),
            looped: true,
            duration: 1.0,
            channels: [("root".to_string(), ch)].into_iter().collect(),
        };
        let mats = skin_matrices(&rig, Some((&clip, 0.0)));
        let (p, n) = skin_vertex(&rig, &mats, VertId(0), Vec3::new(1.0, 0.0, 0.0), Vec3::X);
        assert!(p.abs_diff_eq(Vec3::new(0.0, 0.0, -1.0), 1e-5), "{p:?}");
        assert!(n.abs_diff_eq(Vec3::new(0.0, 0.0, -1.0), 1e-5), "{n:?}");
    }

    #[test]
    fn child_bone_inherits_parent_translation() {
        let rig = Rig {
            preset: "test".into(),
            bones: vec![
                Bone { name: "root".into(), parent: None, head: Vec3::ZERO, tail: Vec3::Y },
                Bone { name: "arm".into(), parent: Some(0), head: Vec3::Y, tail: Vec3::Y * 2.0 },
            ],
            weights: [(VertId(0), vec![(1u16, 1.0f32)])].into_iter().collect(),
        };
        let mut ch = channel(vec![0.0]);
        ch.translations = Some(vec![[1.0, 0.0, 0.0]]); // root shifts +X
        let clip = Clip {
            object: "o".into(),
            looped: true,
            duration: 1.0,
            channels: [("root".to_string(), ch)].into_iter().collect(),
        };
        let mats = skin_matrices(&rig, Some((&clip, 0.0)));
        // Vertex at the arm head follows the root's +X shift.
        let (p, _) = skin_vertex(&rig, &mats, VertId(0), Vec3::Y, Vec3::Y);
        assert!(p.abs_diff_eq(Vec3::new(1.0, 1.0, 0.0), 1e-5), "{p:?}");
    }

    #[test]
    fn translation_keys_are_deltas_from_rest() {
        // Bone head off the origin: a +X delta must shift the vertex by
        // exactly +X (delta semantics), not teleport the bone to x=1
        // (absolute semantics) — matches the dgm-gltf exporter.
        let rig = one_bone_rig(Vec3::Y, &[(0, vec![(0, 1.0)])]);
        let mut ch = channel(vec![0.0]);
        ch.translations = Some(vec![[1.0, 0.0, 0.0]]);
        let clip = Clip {
            object: "o".into(),
            looped: true,
            duration: 1.0,
            channels: [("root".to_string(), ch)].into_iter().collect(),
        };
        let mats = skin_matrices(&rig, Some((&clip, 0.0)));
        let (p, _) = skin_vertex(&rig, &mats, VertId(0), Vec3::Y, Vec3::Y);
        assert!(p.abs_diff_eq(Vec3::new(1.0, 1.0, 0.0), 1e-5), "{p:?}");
    }

    #[test]
    fn unweighted_vertex_passes_through() {
        let rig = one_bone_rig(Vec3::ZERO, &[]);
        let q90 = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let mut ch = channel(vec![0.0]);
        ch.rotations = Some(vec![q90.to_array()]);
        let clip = Clip {
            object: "o".into(),
            looped: false,
            duration: 1.0,
            channels: [("root".to_string(), ch)].into_iter().collect(),
        };
        let mats = skin_matrices(&rig, Some((&clip, 0.0)));
        let (p, _) = skin_vertex(&rig, &mats, VertId(7), Vec3::X, Vec3::X);
        assert_eq!(p, Vec3::X);
    }
}
