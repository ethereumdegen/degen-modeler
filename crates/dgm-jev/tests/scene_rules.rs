//! Scene-level gate rules against the cave failure modes that motivated
//! them: a dome rim pushed through the floor, a floating cone, an open
//! shell nobody declared open, an inside-out shell.

use std::collections::BTreeSet;
use std::path::Path;

use dgm_atlas::{AssetClass, Pack};
use dgm_jev::scene_findings;
use dgm_mesh::primitives::{lathe, plane, prim_box};
use dgm_mesh::{Finding, Mesh, Severity};
use dgm_scene::doc::Object;
use dgm_scene::Doc;
use glam::{Mat4, Vec3};

fn classic_pack() -> Pack {
    Pack::load(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packs/classic"))).unwrap()
}

fn shifted(mut mesh: Mesh, by: Vec3) -> Mesh {
    let all: BTreeSet<_> = mesh.verts.keys().copied().collect();
    mesh.transform_verts(&all, &Mat4::from_translation(by));
    mesh
}

fn tag(doc: &mut Doc, object: &str, tag: &str) {
    doc.tags.entry(object.into()).or_default().insert(tag.into());
}

fn rule<'a>(findings: &'a [Finding], rule: &str) -> Vec<&'a Finding> {
    findings.iter().filter(|f| f.rule == rule).collect()
}

/// Today's cave, as an agent first builds it: an open lathe dome whose rim
/// sits 0.2 m below a floor plane, a cone hovering above the apex, and a
/// boulder box half through the dome wall. Class Building.
fn cave_fixture() -> Doc {
    let mut doc = Doc::new(AssetClass::Building);
    let dome = lathe(&[[0.0, 1.5], [1.2, 0.8], [1.5, -0.2]], 12, false).unwrap();
    doc.objects.insert("dome".into(), Object::new(dome));
    doc.objects.insert("floor".into(), Object::new(plane(6.0, 6.0).unwrap()));
    let cone = lathe(&[[0.5, 0.0], [0.0, 1.0]], 8, true).unwrap();
    doc.objects.insert("cone".into(), Object::new(shifted(cone, Vec3::new(0.0, 2.5, 0.0))));
    let boulder = prim_box(Vec3::splat(1.0)).unwrap();
    doc.objects.insert("boulder".into(), Object::new(shifted(boulder, Vec3::new(1.3, 0.3, 0.0))));
    doc
}

#[test]
fn cave_fixture_trips_intersects_open_boundary_hard_and_floating_warn() {
    let pack = classic_pack();
    let doc = cave_fixture();
    let findings = scene_findings(&doc, &pack);

    let intersects = rule(&findings, "scene.intersects");
    assert!(intersects.iter().all(|f| f.severity == Severity::Hard));
    let pairs: Vec<&str> = intersects.iter().map(|f| f.message.as_str()).collect();
    assert!(pairs.iter().any(|m| m.contains("`dome` intersects `floor`")), "{pairs:?}");
    assert!(pairs.iter().any(|m| m.contains("`boulder` intersects `dome`")), "{pairs:?}");
    let dome_floor = intersects.iter().find(|f| f.message.contains("`dome` intersects `floor`")).unwrap();
    let (dome_faces, floor_faces): (Vec<_>, Vec<_>) =
        dome_floor.elems.iter().partition(|e| e.starts_with("dome:f"));
    assert_eq!(dome_faces.len(), 8, "capped at 8 per object: {:?}", dome_floor.elems);
    assert!(!floor_faces.is_empty() && floor_faces.iter().all(|e| e.starts_with("floor:f")));

    let open = rule(&findings, "mesh.open_boundary");
    let dome_open = open.iter().find(|f| f.message.contains("`dome`")).expect("dome rim is open");
    assert_eq!(dome_open.severity, Severity::Hard);
    assert_eq!(dome_open.value, Some(12.0));
    assert!(open.iter().any(|f| f.message.contains("`floor`")));

    let floating = rule(&findings, "scene.floating");
    assert_eq!(floating.len(), 1, "{floating:?}");
    assert_eq!(floating[0].severity, Severity::Warn);
    assert!(floating[0].message.contains("`cone`"));
    assert!(floating[0].value.unwrap() > 0.9, "cone sits 1 m above the apex: {:?}", floating[0].value);

    assert!(rule(&findings, "mesh.inverted").is_empty());
    assert!(!dgm_jev::gate(&doc, &pack).pass);
}

#[test]
fn findings_are_deterministic() {
    let pack = classic_pack();
    let doc = cave_fixture();
    let a = serde_json::to_string(&scene_findings(&doc, &pack)).unwrap();
    let b = serde_json::to_string(&scene_findings(&doc, &pack)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn contact_and_open_tags_silence_their_rules() {
    let pack = classic_pack();
    let mut doc = cave_fixture();
    tag(&mut doc, "floor", "contact");
    tag(&mut doc, "boulder", "contact");
    tag(&mut doc, "dome", "open");
    tag(&mut doc, "floor", "open");

    let findings = scene_findings(&doc, &pack);
    let rules: Vec<&str> = findings.iter().map(|f| f.rule.as_str()).collect();
    assert_eq!(rules, vec!["scene.floating"], "{findings:?}");
}

#[test]
fn closed_touching_manifolds_pass_untagged() {
    let pack = classic_pack();
    let mut doc = Doc::new(AssetClass::Environment);
    // Two closed boxes stacked face-to-face: tangent contact is neither a
    // penetration nor floating, and no tag is needed.
    doc.objects.insert("base".into(), Object::new(prim_box(Vec3::splat(1.0)).unwrap()));
    let top = shifted(prim_box(Vec3::splat(1.0)).unwrap(), Vec3::new(0.25, 1.0, 0.25));
    doc.objects.insert("top".into(), Object::new(top));

    let findings = scene_findings(&doc, &pack);
    assert!(findings.is_empty(), "{findings:?}");
    assert!(dgm_jev::gate(&doc, &pack).pass);
}

#[test]
fn single_object_is_never_floating() {
    let pack = classic_pack();
    let mut doc = Doc::new(AssetClass::Environment);
    doc.objects.insert("cave".into(), Object::new(prim_box(Vec3::splat(4.0)).unwrap()));
    assert!(scene_findings(&doc, &pack).is_empty());
}

#[test]
fn inverted_box_trips_mesh_inverted_only_when_closed() {
    let pack = classic_pack();
    let mut inverted = prim_box(Vec3::splat(1.0)).unwrap();
    for face in inverted.faces.values_mut() {
        face.corners.reverse();
    }
    let mut doc = Doc::new(AssetClass::Prop);
    doc.objects.insert("crate".into(), Object::new(inverted.clone()));
    let findings = scene_findings(&doc, &pack);
    let inv = rule(&findings, "mesh.inverted");
    assert_eq!(inv.len(), 1, "{findings:?}");
    assert_eq!(inv[0].severity, Severity::Hard);
    assert!(inv[0].value.unwrap() < 0.0);

    // Same winding with a face removed: it is an open shell, not inverted.
    let lid = *inverted.faces.keys().next().unwrap();
    inverted.remove_face(lid).unwrap();
    doc.objects.insert("crate".into(), Object::new(inverted));
    let findings = scene_findings(&doc, &pack);
    assert!(rule(&findings, "mesh.inverted").is_empty(), "{findings:?}");
    let open = rule(&findings, "mesh.open_boundary");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].severity, Severity::Warn, "Prop class downgrades to Warn");
}

#[test]
fn non_structural_classes_only_warn() {
    let pack = classic_pack();
    let mut doc = cave_fixture();
    doc.asset_class = AssetClass::Prop;
    let findings = scene_findings(&doc, &pack);
    assert!(!findings.is_empty());
    assert!(findings.iter().all(|f| f.severity == Severity::Warn), "{findings:?}");
}

#[test]
fn lods_skip_pair_rules_and_inherit_tags() {
    let pack = classic_pack();
    let mut doc = cave_fixture();
    tag(&mut doc, "dome", "open");
    // A LOD sitting exactly on its base would trip intersects against
    // everything the base does; LODs must stay out of pair rules and read
    // the base object's `open` tag.
    let mut lod = Object::new(lathe(&[[0.0, 1.5], [1.2, 0.8], [1.5, -0.2]], 6, false).unwrap());
    lod.lod_of = Some("dome".into());
    lod.lod_level = 1;
    doc.objects.insert("dome_lod1".into(), lod);

    let findings = scene_findings(&doc, &pack);
    assert!(
        !findings.iter().any(|f| f.message.contains("dome_lod1") || f.elems.iter().any(|e| e.starts_with("dome_lod1"))),
        "{findings:?}"
    );
}
