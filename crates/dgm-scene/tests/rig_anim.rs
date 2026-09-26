//! RigAnim slice tests: preset fitting, auto weights, weight painting, clip
//! instantiation and keying — proven on the classic pack biped.

use std::collections::BTreeMap;
use std::path::Path;

use dgm_atlas::{AssetClass, Pack};
use dgm_mesh::primitives::prim_box;
use dgm_scene::anim::clip_findings;
use dgm_scene::anim_ops::{clip_apply, clip_delete, clip_key, clip_set_loop};
use dgm_scene::doc::{Doc, Object};
use dgm_scene::op::{BoneFit, PaintMode};
use dgm_scene::rig::{MAX_BONES, MAX_INFLUENCES, rig_findings};
use dgm_scene::rig_ops::{rig_add_bone, rig_apply, rig_auto_weights, rig_paint_weights};
use dgm_scene::select::{Elems, Selection};
use dgm_scene::OpError;
use glam::Vec3;

fn classic() -> Pack {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/classic");
    Pack::load(&root).expect("classic pack loads")
}

/// A 2 m tall box character (bounds y in [-1, 1]) named `hero`.
fn hero_doc() -> Doc {
    let mut doc = Doc::new(AssetClass::Character);
    let mesh = prim_box(Vec3::new(1.0, 2.0, 1.0)).unwrap();
    doc.objects.insert("hero".into(), Object::new(mesh));
    doc
}

fn rigged_hero(pack: &Pack) -> Doc {
    let mut doc = hero_doc();
    rig_apply(&mut doc, pack, "hero", "biped", &BTreeMap::new()).unwrap();
    rig_auto_weights(&mut doc, "hero").unwrap();
    doc
}

fn all_verts_sel(doc: &Doc) -> Selection {
    Selection {
        object: "hero".into(),
        elems: Elems::Verts(doc.objects["hero"].mesh.verts.keys().copied().collect()),
    }
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn rig_apply_fits_biped_into_2m_box() {
    let pack = classic();
    let mut doc = hero_doc();
    let diff = rig_apply(&mut doc, &pack, "hero", "biped", &BTreeMap::new()).unwrap();
    assert!(diff.summary.contains("17 bones"), "summary: {}", diff.summary);

    let rig = doc.objects["hero"].rig.as_ref().unwrap();
    assert_eq!(rig.preset, "biped");
    assert_eq!(rig.bones.len(), 17);

    // Normalized bones scaled x2 and dropped onto bounds min y = -1.
    let hips = &rig.bones[rig.bone_index("hips").unwrap() as usize];
    assert!(approx(hips.head.y, -1.0 + 0.55 * 2.0), "hips head {:?}", hips.head);
    assert!(approx(hips.head.x, 0.0) && approx(hips.head.z, 0.0));
    let head = &rig.bones[rig.bone_index("head").unwrap() as usize];
    assert!(approx(head.tail.y, 1.0), "head tail should reach bounds top, got {:?}", head.tail);
    let hand = &rig.bones[rig.bone_index("hand_l").unwrap() as usize];
    assert!(approx(hand.tail.x, 0.53 * 2.0), "hand_l tail {:?}", hand.tail);

    // Parent names resolved to indices.
    let spine = &rig.bones[rig.bone_index("spine").unwrap() as usize];
    assert_eq!(spine.parent, rig.bone_index("hips"));
    assert_eq!(rig.bones[0].parent, None, "hips is the root");

    // Freshly applied rig has no weights: exactly the unweighted finding.
    let findings = rig_findings(&doc);
    assert_eq!(findings.len(), 1, "findings: {findings:?}");
    assert_eq!(findings[0].rule, "rig.unweighted_verts");
}

#[test]
fn rig_apply_fit_overrides_in_model_space() {
    let pack = classic();
    let mut doc = hero_doc();
    let fit = BTreeMap::from([(
        "head".to_string(),
        BoneFit { head: [0.0, 0.8, 0.1], tail: [0.0, 1.3, 0.1] },
    )]);
    rig_apply(&mut doc, &pack, "hero", "biped", &fit).unwrap();
    let rig = doc.objects["hero"].rig.as_ref().unwrap();
    let head = &rig.bones[rig.bone_index("head").unwrap() as usize];
    assert_eq!(head.head, Vec3::new(0.0, 0.8, 0.1));
    assert_eq!(head.tail, Vec3::new(0.0, 1.3, 0.1));

    let err = rig_apply(&mut doc, &pack, "hero", "biped", &BTreeMap::from([(
        "no_such_bone".to_string(),
        BoneFit { head: [0.0; 3], tail: [0.0; 3] },
    )]));
    assert!(matches!(err, Err(OpError::UnknownBone(_))));
}

#[test]
fn rig_apply_replaces_rig_and_clears_weights() {
    let pack = classic();
    let mut doc = rigged_hero(&pack);
    assert!(!doc.objects["hero"].rig.as_ref().unwrap().weights.is_empty());
    rig_apply(&mut doc, &pack, "hero", "biped", &BTreeMap::new()).unwrap();
    assert!(doc.objects["hero"].rig.as_ref().unwrap().weights.is_empty());
}

#[test]
fn auto_weights_normalized_capped_and_deterministic() {
    let pack = classic();
    let doc = rigged_hero(&pack);
    assert!(rig_findings(&doc).is_empty(), "findings: {:?}", rig_findings(&doc));

    let rig = doc.objects["hero"].rig.as_ref().unwrap();
    assert_eq!(rig.weights.len(), 8, "every box vert weighted");
    for (v, ws) in &rig.weights {
        assert!(!ws.is_empty() && ws.len() <= MAX_INFLUENCES, "{v}: {ws:?}");
        let sum: f32 = ws.iter().map(|&(_, w)| w).sum();
        assert!(approx(sum, 1.0), "{v} weights sum to {sum}");
        assert!(ws.windows(2).all(|w| w[0].0 < w[1].0), "{v} influences sorted by bone");
    }

    // Byte-for-byte deterministic across a fresh replay.
    let doc2 = rigged_hero(&pack);
    let (a, b) = (
        serde_json::to_string(&doc.objects["hero"].rig).unwrap(),
        serde_json::to_string(&doc2.objects["hero"].rig).unwrap(),
    );
    assert_eq!(a, b);
}

#[test]
fn paint_weights_replace_add_and_smooth_stay_normalized() {
    let pack = classic();
    let mut doc = rigged_hero(&pack);
    let sel = all_verts_sel(&doc);

    // Replace at full value: the bone owns every selected vert outright.
    rig_paint_weights(&mut doc, &sel, "head", 1.0, 0.0, PaintMode::Replace).unwrap();
    {
        let rig = doc.objects["hero"].rig.as_ref().unwrap();
        let head = rig.bone_index("head").unwrap();
        for (v, ws) in &rig.weights {
            assert_eq!(ws.as_slice(), &[(head, 1.0)], "{v}: {ws:?}");
        }
    }

    // Rebuild real weights, then add a partial influence.
    rig_auto_weights(&mut doc, "hero").unwrap();
    rig_paint_weights(&mut doc, &sel, "head", 0.5, 0.0, PaintMode::Add).unwrap();
    assert!(rig_findings(&doc).is_empty(), "add broke gate: {:?}", rig_findings(&doc));
    {
        let rig = doc.objects["hero"].rig.as_ref().unwrap();
        let head = rig.bone_index("head").unwrap();
        for (v, ws) in &rig.weights {
            assert!(ws.len() <= MAX_INFLUENCES, "{v}: {ws:?}");
            let sum: f32 = ws.iter().map(|&(_, w)| w).sum();
            assert!(approx(sum, 1.0), "{v} weights sum to {sum}");
            assert!(
                ws.iter().any(|&(b, w)| b == head && w > 0.0),
                "{v} should carry the painted bone: {ws:?}"
            );
        }
    }

    // Smooth: averaged with topological neighbours, still normalized.
    rig_auto_weights(&mut doc, "hero").unwrap();
    let before = doc.objects["hero"].rig.as_ref().unwrap().weights.clone();
    rig_paint_weights(&mut doc, &sel, "head", 1.0, 0.0, PaintMode::Smooth).unwrap();
    assert!(rig_findings(&doc).is_empty(), "smooth broke gate: {:?}", rig_findings(&doc));
    let rig = doc.objects["hero"].rig.as_ref().unwrap();
    let head = rig.bone_index("head").unwrap();
    let weight_of = |m: &BTreeMap<_, Vec<(u16, f32)>>, v, b| -> f32 {
        m.get(&v)
            .and_then(|ws: &Vec<(u16, f32)>| ws.iter().find(|&&(x, _)| x == b).map(|&(_, w)| w))
            .unwrap_or(0.0)
    };
    let mut changed = false;
    for (&v, ws) in &rig.weights {
        assert!(ws.len() <= MAX_INFLUENCES);
        let sum: f32 = ws.iter().map(|&(_, w)| w).sum();
        assert!(approx(sum, 1.0), "{v} weights sum to {sum} after smooth");
        changed |= !approx(weight_of(&before, v, head), weight_of(&rig.weights, v, head));
    }
    assert!(changed, "smooth should move at least one head weight on an asymmetric bone");
}

#[test]
fn add_bone_appends_under_parent_and_enforces_cap() {
    let pack = classic();
    let mut doc = rigged_hero(&pack);
    rig_add_bone(&mut doc, "hero", "tail", "hips", [0.0, 0.1, -0.5], [0.0, 0.0, -0.9]).unwrap();
    {
        let rig = doc.objects["hero"].rig.as_ref().unwrap();
        let tail = &rig.bones[17];
        assert_eq!(tail.name, "tail");
        assert_eq!(tail.parent, rig.bone_index("hips"));
        assert_eq!(tail.head, Vec3::new(0.0, 0.1, -0.5));
    }

    let err = rig_add_bone(&mut doc, "hero", "tail", "hips", [0.0; 3], [1.0; 3]);
    assert!(matches!(err, Err(OpError::BadParams(_))), "duplicate name must fail");
    let err = rig_add_bone(&mut doc, "hero", "t2", "no_such", [0.0; 3], [1.0; 3]);
    assert!(matches!(err, Err(OpError::UnknownBone(_))));

    for i in 0..(MAX_BONES - 18) {
        rig_add_bone(&mut doc, "hero", &format!("seg{i}"), "tail", [0.0; 3], [0.0, 0.0, -1.0])
            .unwrap();
    }
    assert_eq!(doc.objects["hero"].rig.as_ref().unwrap().bones.len(), MAX_BONES);
    let err = rig_add_bone(&mut doc, "hero", "one_too_many", "tail", [0.0; 3], [1.0; 3]);
    assert!(matches!(err, Err(OpError::BadParams(_))), "bone cap must hold");
}

#[test]
fn clip_apply_walk_scales_times_rotations_translations() {
    let pack = classic();
    let mut doc = rigged_hero(&pack);

    clip_apply(&mut doc, &pack, "walk", "hero", "walk", 1.0).unwrap();
    assert!(clip_findings(&doc).is_empty(), "findings: {:?}", clip_findings(&doc));
    {
        let clip = &doc.clips["walk"];
        assert_eq!(clip.object, "hero");
        assert!(clip.looped);
        assert!(approx(clip.duration, 1.0));
        let thigh = &clip.channels["thigh_l"];
        assert_eq!(thigh.times, vec![0.0, 0.5, 1.0]);
        // 25 deg about X, XYZ order -> [sin 12.5, 0, 0, cos 12.5].
        let q = thigh.rotations.as_ref().unwrap()[0];
        let half = 12.5_f32.to_radians();
        assert!(approx(q[0], half.sin()) && approx(q[1], 0.0) && approx(q[2], 0.0));
        assert!(approx(q[3], half.cos()), "quat {q:?}");
    }

    // Speed 2: everything halves.
    clip_apply(&mut doc, &pack, "walk_fast", "hero", "walk", 2.0).unwrap();
    let fast = &doc.clips["walk_fast"];
    assert!(approx(fast.duration, 0.5));
    assert_eq!(fast.channels["thigh_l"].times, vec![0.0, 0.25, 0.5]);

    // Idle hips bob is normalized -0.01: scaled by the 2 m fitted height.
    clip_apply(&mut doc, &pack, "idle", "hero", "idle", 1.0).unwrap();
    let bob = doc.clips["idle"].channels["hips"].translations.as_ref().unwrap()[1];
    assert!(approx(bob[1], -0.02), "hips bob {bob:?}");
    assert!(clip_findings(&doc).is_empty());
}

#[test]
fn clip_apply_rejects_bad_inputs() {
    let pack = classic();
    let mut doc = rigged_hero(&pack);
    assert!(matches!(
        clip_apply(&mut doc, &pack, "w", "hero", "no_such_template", 1.0),
        Err(OpError::Pack(_))
    ));
    assert!(matches!(
        clip_apply(&mut doc, &pack, "w", "hero", "walk", 0.0),
        Err(OpError::BadParams(_))
    ));
    // Rig preset mismatch.
    doc.objects.get_mut("hero").unwrap().rig.as_mut().unwrap().preset = "other".into();
    assert!(matches!(
        clip_apply(&mut doc, &pack, "w", "hero", "walk", 1.0),
        Err(OpError::BadParams(_))
    ));
    // No rig at all.
    doc.objects.get_mut("hero").unwrap().rig = None;
    assert!(matches!(
        clip_apply(&mut doc, &pack, "w", "hero", "walk", 1.0),
        Err(OpError::BadParams(_))
    ));
}

#[test]
fn clip_key_upserts_sorted_and_backfills_curves() {
    let pack = classic();
    let mut doc = rigged_hero(&pack);
    clip_apply(&mut doc, &pack, "walk", "hero", "walk", 1.0).unwrap();

    // New time on a rotation-only channel, keying a translation: the
    // translation curve is backfilled with rest, the rotation sampled.
    clip_key(&mut doc, "walk", "thigh_l", 0.25, None, Some([0.0, 0.1, 0.0]), None).unwrap();
    {
        let ch = &doc.clips["walk"].channels["thigh_l"];
        assert_eq!(ch.times, vec![0.0, 0.25, 0.5, 1.0]);
        let tr = ch.translations.as_ref().unwrap();
        assert_eq!(tr.len(), 4);
        assert_eq!(tr[0], [0.0, 0.0, 0.0], "existing keys got rest translation");
        assert_eq!(tr[1], [0.0, 0.1, 0.0]);
        let rots = ch.rotations.as_ref().unwrap();
        assert_eq!(rots.len(), 4, "rotation curve tracks the new key count");
        // Sampled halfway between +25 and -25 deg about X: identity-ish.
        assert!(approx(rots[1][0], 0.0) && approx(rots[1][3], 1.0), "sampled {:?}", rots[1]);
    }
    assert!(clip_findings(&doc).is_empty(), "findings: {:?}", clip_findings(&doc));

    // Upsert at an existing time replaces in place, no growth.
    clip_key(&mut doc, "walk", "thigh_l", 0.25, Some([90.0, 0.0, 0.0]), None, None).unwrap();
    {
        let ch = &doc.clips["walk"].channels["thigh_l"];
        assert_eq!(ch.times.len(), 4);
        let q = ch.rotations.as_ref().unwrap()[1];
        let half = 45.0_f32.to_radians();
        assert!(approx(q[0], half.sin()) && approx(q[3], half.cos()), "quat {q:?}");
    }

    // A brand-new channel on another rig bone, and a key past the end
    // extends the duration; the gate stays clean.
    clip_key(&mut doc, "walk", "neck", 1.25, Some([0.0, 10.0, 0.0]), None, None).unwrap();
    assert!(approx(doc.clips["walk"].duration, 1.25));
    assert_eq!(doc.clips["walk"].channels["neck"].times, vec![1.25]);
    assert!(clip_findings(&doc).is_empty(), "findings: {:?}", clip_findings(&doc));

    // Errors: unknown clip, unknown bone, empty key.
    assert!(matches!(
        clip_key(&mut doc, "nope", "neck", 0.0, Some([0.0; 3]), None, None),
        Err(OpError::UnknownClip(_))
    ));
    assert!(matches!(
        clip_key(&mut doc, "walk", "no_bone", 0.0, Some([0.0; 3]), None, None),
        Err(OpError::UnknownBone(_))
    ));
    assert!(matches!(
        clip_key(&mut doc, "walk", "neck", 0.0, None, None, None),
        Err(OpError::BadParams(_))
    ));
}

#[test]
fn clip_set_loop_and_delete() {
    let pack = classic();
    let mut doc = rigged_hero(&pack);
    clip_apply(&mut doc, &pack, "walk", "hero", "walk", 1.0).unwrap();

    clip_set_loop(&mut doc, "walk", false).unwrap();
    assert!(!doc.clips["walk"].looped);
    assert!(matches!(clip_set_loop(&mut doc, "nope", true), Err(OpError::UnknownClip(_))));

    let diff = clip_delete(&mut doc, "walk").unwrap();
    assert_eq!(diff.removed, vec!["walk".to_string()]);
    assert!(doc.clips.is_empty());
    assert!(matches!(clip_delete(&mut doc, "walk"), Err(OpError::UnknownClip(_))));
}
