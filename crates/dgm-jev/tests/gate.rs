//! Gate composition: mesh findings are attributed per object and the
//! pass flag reflects hard findings across rule families.

use std::path::Path;

use dgm_atlas::Pack;
use dgm_mesh::primitives::prim_box;
use dgm_scene::doc::Object;
use dgm_scene::{AlphaMode, Clip, Doc, Material, TextureRef};
use glam::Vec3;

fn classic_pack() -> Pack {
    Pack::load(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packs/classic"))).unwrap()
}

#[test]
fn clean_box_passes() {
    let pack = classic_pack();
    let mut doc = Doc::new(dgm_atlas::AssetClass::default());
    doc.objects.insert("crate".into(), Object::new(prim_box(Vec3::splat(1.0)).unwrap()));

    let report = dgm_jev::gate(&doc, &pack);
    assert!(report.pass, "unexpected findings: {:?}", report.findings);
}

#[test]
fn mesh_findings_are_attributed_and_fail_the_gate() {
    let pack = classic_pack();
    let mut doc = Doc::new(dgm_atlas::AssetClass::default());
    let mut mesh = prim_box(Vec3::splat(1.0)).unwrap();
    mesh.add_vert(Vec3::new(9.0, 9.0, 9.0)); // unused vert -> warn
    doc.objects.insert("crate".into(), Object::new(mesh));
    // Orphan clip -> hard anim finding.
    doc.clips.insert(
        "walk".into(),
        Clip { object: "ghost".into(), looped: true, duration: 1.0, channels: Default::default() },
    );

    let report = dgm_jev::gate(&doc, &pack);
    assert!(!report.pass);
    let unused = report.findings.iter().find(|f| f.rule == "mesh.unused_vert").unwrap();
    assert!(unused.message.contains("object `crate`"), "message: {}", unused.message);
    assert!(report.findings.iter().any(|f| f.rule == "anim.orphan_clip"));
}

#[test]
fn unknown_trim_sheet_becomes_missing_sheet_not_overlap() {
    let pack = classic_pack();
    let mut doc = Doc::new(dgm_atlas::AssetClass::default());
    // All-default UVs stack every face at (0,0); on a trim that's fine, but
    // if the broken sheet demoted the object to "untextured" it would draw
    // false uv.overlap Hards. It must surface as uv.missing_sheet instead.
    let mut object = Object::new(prim_box(Vec3::splat(1.0)).unwrap());
    object.material = Some("wood".into());
    doc.objects.insert("crate".into(), object);
    doc.materials.insert(
        "wood".into(),
        Material {
            texture: TextureRef::Trim { sheet: "no_such_sheet".into() },
            alpha: AlphaMode::Opaque,
            double_sided: false,
        },
    );

    let report = dgm_jev::gate(&doc, &pack);
    assert!(!report.pass);
    let missing = report.findings.iter().find(|f| f.rule == "uv.missing_sheet").unwrap();
    assert!(missing.message.contains("no_such_sheet"), "message: {}", missing.message);
    assert!(!report.findings.iter().any(|f| f.rule.starts_with("uv.") && f.rule != "uv.missing_sheet"));
}
