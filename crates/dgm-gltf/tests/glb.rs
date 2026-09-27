//! Behavioral tests for the deterministic GLB writer: byte determinism,
//! golden hash, welding topology, skin/animation payloads, and the `gltf`
//! crate round trip.

use std::collections::BTreeMap;
use std::path::Path;

use dgm_atlas::{AssetClass, Pack};
use dgm_gltf::{ExportOptions, export_glb, import_summary};
use dgm_mesh::primitives::{cylinder, prim_box};
use dgm_scene::{AlphaMode, Bone, Channel, Clip, Doc, Material, Object, Rig, TextureRef};
use glam::Vec3;
use serde_json::json;

/// Pinned FNV-1a 64 of the fixture export. Any drift means the bytes are no
/// longer reproducible; update only for an intentional format change.
const GOLDEN_FNV1A: &str = "86786a8b1c92e4d7";

fn classic() -> Pack {
    Pack::load(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packs/classic"))).unwrap()
}

fn biped_rig(pack: &Pack) -> Rig {
    let preset = &pack.manifest.rigs["biped"];
    let index: BTreeMap<&str, u16> =
        preset.bones.iter().enumerate().map(|(i, b)| (b.name.as_str(), i as u16)).collect();
    let bones = preset
        .bones
        .iter()
        .map(|b| Bone {
            name: b.name.clone(),
            parent: b.parent.as_deref().map(|p| index[p]),
            head: Vec3::from(b.head),
            tail: Vec3::from(b.tail),
        })
        .collect();
    Rig { preset: "biped".into(), bones, weights: BTreeMap::new() }
}

/// Box + trim-textured cylinder + biped-rigged box + one clip.
fn fixture(pack: &Pack) -> Doc {
    let mut doc = Doc::new(AssetClass::Prop);

    let mut bx = Object::new(prim_box(Vec3::ONE).unwrap());
    bx.material = Some("paint".into());
    doc.objects.insert("box".into(), bx);
    doc.materials.insert(
        "paint".into(),
        Material {
            texture: TextureRef::Color { rgba: [200, 60, 40, 255] },
            alpha: AlphaMode::Opaque,
            double_sided: false,
            emissive: None,
            emissive_strength: 1.0,
        },
    );

    let mut cyl = Object::new(cylinder(0.4, 1.2, 8, true).unwrap());
    cyl.material = Some("wood".into());
    doc.objects.insert("cyl".into(), cyl);
    doc.materials.insert(
        "wood".into(),
        Material {
            texture: TextureRef::Trim { sheet: "wood".into() },
            alpha: AlphaMode::Mask,
            double_sided: true,
            emissive: None,
            emissive_strength: 1.0,
        },
    );

    let mut hero = Object::new(prim_box(Vec3::new(0.6, 1.0, 0.3)).unwrap());
    let mut rig = biped_rig(pack);
    rig.weights = hero
        .mesh
        .verts
        .iter()
        .map(|(&v, pos)| {
            // Lower half binds to hips, upper half splits hips/spine.
            if pos.y > 0.0 { (v, vec![(0u16, 0.5f32), (1u16, 0.5f32)]) } else { (v, vec![(0u16, 1.0f32)]) }
        })
        .collect();
    hero.rig = Some(rig);
    doc.objects.insert("hero".into(), hero);

    let mut channels = BTreeMap::new();
    channels.insert(
        "hips".into(),
        Channel {
            times: vec![0.0, 0.5, 1.0],
            rotations: Some(vec![
                [0.0, 0.0, 0.0, 1.0],
                [0.0, 0.382_683_43, 0.0, 0.923_879_5],
                [0.0, 0.0, 0.0, 1.0],
            ]),
            translations: Some(vec![[0.0, 0.0, 0.0], [0.0, 0.08, 0.0], [0.0, 0.0, 0.0]]),
            scales: None,
        },
    );
    doc.clips.insert(
        "idle".into(),
        Clip { object: "hero".into(), looped: true, duration: 1.0, channels },
    );
    doc
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[test]
fn export_is_deterministic_and_matches_golden() {
    let pack = classic();
    let doc = fixture(&pack);
    let opts = ExportOptions::default();
    let a = export_glb(&doc, &pack, &opts).unwrap();
    let b = export_glb(&doc, &pack, &opts).unwrap();
    assert_eq!(a, b, "same doc must export byte-identical GLBs");
    assert_eq!(format!("{:016x}", fnv1a(&a)), GOLDEN_FNV1A, "GLB bytes drifted from golden");
}

#[test]
fn import_summary_counts_match_doc() {
    let pack = classic();
    let doc = fixture(&pack);
    let bytes = export_glb(&doc, &pack, &ExportOptions::default()).unwrap();
    let summary = import_summary(&bytes).unwrap();

    let bone_count = pack.manifest.rigs["biped"].bones.len() as u64;
    let tris: u32 = doc.objects.values().map(|o| o.mesh.tri_count()).sum();
    assert_eq!(summary["nodes"], 3 + bone_count, "3 object nodes + one node per bone");
    assert_eq!(summary["meshes"], 3u64);
    assert_eq!(summary["prims"], 3u64);
    assert_eq!(summary["tris"], tris as u64);
    assert_eq!(summary["materials"], 2u64);
    assert_eq!(summary["textures"], 1u64, "wood trim sheet embeds once");
    assert_eq!(summary["skins"], 1u64);
    assert_eq!(summary["animations"], 1u64);
    assert_eq!(summary["unlit"], true);
}

#[test]
fn welding_splits_hard_edges_and_merges_smooth_ones() {
    let pack = classic(); // hard_edge_angle_deg = 40
    // Box: every dihedral is 90° -> no cross-face weld: 6 faces * 4 corners.
    let mut doc = Doc::new(AssetClass::Prop);
    doc.objects.insert("box".into(), Object::new(prim_box(Vec3::ONE).unwrap()));
    // 12-segment cylinder: side dihedral 30° < 40° -> ring verts weld across
    // the two side quads (24 side verts); caps stay split (2 * 12).
    doc.objects.insert("smooth".into(), Object::new(cylinder(0.5, 1.0, 12, true).unwrap()));
    let bytes = export_glb(&doc, &pack, &ExportOptions::default()).unwrap();

    let (document, buffers, _) = gltf::import_slice(&bytes).unwrap();
    let count = |name: &str| {
        let node = document.nodes().find(|n| n.name() == Some(name)).unwrap();
        let prim = node.mesh().unwrap().primitives().next().unwrap();
        let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
        (
            reader.read_positions().unwrap().len(),
            reader.read_indices().unwrap().into_u32().len(),
        )
    };
    assert_eq!(count("box"), (24, 36));
    // 48 ring verts + 2 extra from the primitives' default-UV wrap column
    // (u = 0 vs u = 2*pi*r on the same position/normal).
    assert_eq!(count("smooth"), (50, (12 * 2 + 2 * 10) * 3));
}

#[test]
fn skin_and_animation_survive_reimport() {
    let pack = classic();
    let doc = fixture(&pack);
    let bytes = export_glb(&doc, &pack, &ExportOptions::default()).unwrap();
    let (document, buffers, _) = gltf::import_slice(&bytes).unwrap();
    let data = |b: gltf::Buffer<'_>| buffers.get(b.index()).map(|d| &d.0[..]);

    let hero = document.nodes().find(|n| n.name() == Some("hero")).unwrap();
    let skin = hero.skin().unwrap();
    let preset = &pack.manifest.rigs["biped"];
    assert_eq!(skin.joints().count(), preset.bones.len());
    let joint_names: Vec<_> = skin.joints().map(|j| j.name().unwrap().to_owned()).collect();
    let preset_names: Vec<_> = preset.bones.iter().map(|b| b.name.clone()).collect();
    assert_eq!(joint_names, preset_names, "joints follow rig bone order");
    // IBM of the root bone is translate(-head).
    let ibms: Vec<[[f32; 4]; 4]> =
        skin.reader(data).read_inverse_bind_matrices().unwrap().collect();
    let hips_head = preset.bones[0].head;
    assert!((ibms[0][3][0] + hips_head[0]).abs() < 1e-6);
    assert!((ibms[0][3][1] + hips_head[1]).abs() < 1e-6);
    assert!((ibms[0][3][2] + hips_head[2]).abs() < 1e-6);

    // Weights are VEC4, normalized, joints u8-ranged.
    let prim = hero.mesh().unwrap().primitives().next().unwrap();
    let reader = prim.reader(data);
    for w in reader.read_weights(0).unwrap().into_f32() {
        let sum: f32 = w.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "weights must sum to 1, got {sum}");
    }

    // Material flags: MASK + cutoff 0.5 + doubleSided on the wood trim.
    let wood = document.materials().find(|m| m.name() == Some("wood")).unwrap();
    assert_eq!(wood.alpha_mode(), gltf::material::AlphaMode::Mask);
    assert_eq!(wood.alpha_cutoff(), Some(0.5));
    assert!(wood.double_sided());
    assert!(wood.unlit());
    let paint = document.materials().find(|m| m.name() == Some("paint")).unwrap();
    let factor = paint.pbr_metallic_roughness().base_color_factor();
    assert!((factor[0] - 200.0 / 255.0).abs() < 1e-6);

    // Animation: rotation + translation channels on the hips node;
    // translation keys are rest + delta (hips is a root: rest == head).
    let anim = document.animations().next().unwrap();
    assert_eq!(anim.name(), Some("idle"));
    let channels: Vec<_> = anim.channels().collect();
    assert_eq!(channels.len(), 2);
    let mut saw_translation = false;
    for ch in &channels {
        assert_eq!(ch.target().node().name(), Some("hips"));
        assert_eq!(ch.sampler().interpolation(), gltf::animation::Interpolation::Linear);
        let reader = ch.reader(data);
        let times: Vec<f32> = reader.read_inputs().unwrap().collect();
        assert_eq!(times, vec![0.0, 0.5, 1.0]);
        if let gltf::animation::util::ReadOutputs::Translations(t) =
            reader.read_outputs().unwrap()
        {
            saw_translation = true;
            let t: Vec<[f32; 3]> = t.collect();
            assert!((t[0][1] - hips_head[1]).abs() < 1e-6, "key 0: rest translation");
            assert!((t[1][1] - (hips_head[1] + 0.08)).abs() < 1e-6, "key 1: rest + delta");
        }
    }
    assert!(saw_translation);
}

#[test]
fn big_meshes_use_u32_indices() {
    let pack = classic();
    let mut doc = Doc::new(AssetClass::Prop);
    // 33k open-ended segments -> 66k welded verts > u16 range.
    doc.objects.insert("big".into(), Object::new(cylinder(0.5, 1.0, 33_000, false).unwrap()));
    let bytes = export_glb(&doc, &pack, &ExportOptions::default()).unwrap();
    let (document, _, _) = gltf::import_slice(&bytes).unwrap();
    let prim = document.meshes().next().unwrap().primitives().next().unwrap();
    let indices = prim.indices().unwrap();
    assert_eq!(indices.data_type(), gltf::accessor::DataType::U32);
    assert_eq!(indices.count(), 33_000 * 2 * 3);
}

#[test]
fn small_meshes_use_u16_indices() {
    let pack = classic();
    let mut doc = Doc::new(AssetClass::Prop);
    doc.objects.insert("box".into(), Object::new(prim_box(Vec3::ONE).unwrap()));
    let bytes = export_glb(&doc, &pack, &ExportOptions::default()).unwrap();
    let (document, _, _) = gltf::import_slice(&bytes).unwrap();
    let prim = document.meshes().next().unwrap().primitives().next().unwrap();
    assert_eq!(prim.indices().unwrap().data_type(), gltf::accessor::DataType::U16);
}

#[test]
fn file_texture_resolves_against_base_dir() {
    let pack = classic();
    let mut doc = Doc::new(AssetClass::Prop);
    let mut bx = Object::new(prim_box(Vec3::ONE).unwrap());
    bx.material = Some("painted".into());
    doc.objects.insert("box".into(), bx);
    doc.materials.insert(
        "painted".into(),
        Material {
            texture: TextureRef::File { path: "board.png".into() },
            alpha: AlphaMode::Opaque,
            double_sided: false,
            emissive: None,
            emissive_strength: 1.0,
        },
    );
    let opts = ExportOptions { base_dir: pack.root.clone(), embed_report: None };
    let bytes = export_glb(&doc, &pack, &opts).unwrap();
    let summary = import_summary(&bytes).unwrap();
    assert_eq!(summary["textures"], 1u64);
}

#[test]
fn embed_report_lands_in_root_extras() {
    let pack = classic();
    let doc = fixture(&pack);
    let opts = ExportOptions {
        base_dir: Default::default(),
        embed_report: Some(json!({ "gate": "clean", "revision": 7 })),
    };
    let bytes = export_glb(&doc, &pack, &opts).unwrap();
    // Parse the GLB JSON chunk by hand: header is 12 bytes, then the JSON
    // chunk length + type.
    let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    assert_eq!(&bytes[16..20], b"JSON");
    let root: serde_json::Value = serde_json::from_slice(&bytes[20..20 + json_len]).unwrap();
    assert_eq!(root["extras"]["gate"], "clean");
    assert_eq!(root["extras"]["revision"], 7);
    assert_eq!(root["asset"]["version"], "2.0");
}

#[test]
fn errors_name_the_offenders() {
    let pack = classic();

    let mut doc = Doc::new(AssetClass::Prop);
    let mut bx = Object::new(prim_box(Vec3::ONE).unwrap());
    bx.material = Some("ghost".into());
    doc.objects.insert("box".into(), bx);
    let err = export_glb(&doc, &pack, &ExportOptions::default()).unwrap_err();
    assert!(err.to_string().contains("ghost"), "unknown material: {err}");

    let mut doc = Doc::new(AssetClass::Prop);
    doc.materials.insert(
        "bad".into(),
        Material {
            texture: TextureRef::Trim { sheet: "nope".into() },
            alpha: AlphaMode::Opaque,
            double_sided: false,
            emissive: None,
            emissive_strength: 1.0,
        },
    );
    let err = export_glb(&doc, &pack, &ExportOptions::default()).unwrap_err();
    assert!(err.to_string().contains("nope"), "unknown sheet: {err}");

    let mut doc = fixture(&pack);
    doc.clips.get_mut("idle").unwrap().channels.insert("tail".into(), Channel {
        times: vec![0.0],
        rotations: Some(vec![[0.0, 0.0, 0.0, 1.0]]),
        translations: None,
        scales: None,
    });
    let err = export_glb(&doc, &pack, &ExportOptions::default()).unwrap_err();
    assert!(err.to_string().contains("tail"), "unknown clip bone: {err}");
}

/// Baked vertex colors and emissive materials must reach the file in a
/// form a real glTF reader accepts (COLOR_0 u8 VEC4 normalized; emissive
/// factor + strength extension).
#[test]
fn vertex_colors_and_emissive_survive_reimport() {
    let pack = classic();
    let mut doc = Doc::new(AssetClass::Environment);
    let mut mesh = dgm_mesh::primitives::prim_box(Vec3::splat(1.0)).unwrap();
    let first = *mesh.verts.keys().next().unwrap();
    mesh.colors.insert(first, [0.25, 0.5, 0.75]);
    let mut obj = dgm_scene::Object::new(mesh);
    obj.material = Some("crystal".into());
    doc.objects.insert("gem".into(), obj);
    doc.materials.insert(
        "crystal".into(),
        Material {
            texture: TextureRef::Color { rgba: [60, 180, 255, 255] },
            alpha: AlphaMode::Opaque,
            double_sided: false,
            emissive: Some([0.2, 0.7, 1.0]),
            emissive_strength: 3.0,
        },
    );
    let glb = export_glb(&doc, &pack, &ExportOptions::default()).unwrap();
    let s = import_summary(&glb).unwrap();
    assert_eq!(s["vertex_colors"], true);
    assert_eq!(s["emissive_materials"], 1);
    let text = String::from_utf8_lossy(&glb);
    assert!(text.contains("KHR_materials_emissive_strength"));

    // A doc without colors must not grow a COLOR_0 stream.
    doc.objects.get_mut("gem").unwrap().mesh.colors.clear();
    let s = import_summary(&export_glb(&doc, &pack, &ExportOptions::default()).unwrap()).unwrap();
    assert_eq!(s["vertex_colors"], false);
}
