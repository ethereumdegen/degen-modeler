//! Behaviour tests: golden contact sheet, raycast hit/miss geometry,
//! stretch/density metrics on a two-face stretched mesh, and the layout
//! contract of the sheet views.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::PathBuf;

use dgm_atlas::{AssetClass, Band, Pack, PackManifest, TrimSheet};
use dgm_mesh::{Corner, Mesh, primitives::prim_box};
use dgm_render::{
    contact_sheet, filmstrip, heatmap, metrics, raycast, silhouette_masks, uv_layout,
    wireframe_sheet,
};
use dgm_scene::{AlphaMode, Doc, Material, Object, TextureRef};
use glam::{Vec2, Vec3};

fn pack(trims: BTreeMap<String, TrimSheet>) -> Pack {
    Pack {
        // No PNGs live here: trim files resolve as missing on purpose.
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests"),
        manifest: PackManifest {
            name: "test".into(),
            version: "0".into(),
            budgets: BTreeMap::new(),
            texel_density: Band { min: 100.0, max: 450.0 },
            texture_sizes: vec![256],
            palette: vec!["#808080".into()],
            hard_edge_angle_deg: 40.0,
            uv_waste_max: 0.15,
            trims,
            rigs: BTreeMap::new(),
            clips: BTreeMap::new(),
            reference_board: None,
        },
    }
}

fn box_doc() -> Doc {
    let mut doc = Doc::new(AssetClass::Prop);
    doc.materials.insert(
        "paint".into(),
        Material {
            texture: TextureRef::Color { rgba: [168, 111, 61, 255] },
            alpha: AlphaMode::Opaque,
            double_sided: false,
            emissive: None,
            emissive_strength: 1.0,
        },
    );
    let mut obj = Object::new(prim_box(Vec3::ONE).unwrap());
    obj.material = Some("paint".into());
    doc.objects.insert("box".into(), obj);
    doc
}

fn quad(mesh: &mut Mesh, x0: f32, uv0: Vec2, uv_size: f32) {
    let v = [
        mesh.add_vert(Vec3::new(x0, 0.0, 0.0)),
        mesh.add_vert(Vec3::new(x0 + 1.0, 0.0, 0.0)),
        mesh.add_vert(Vec3::new(x0 + 1.0, 0.0, 1.0)),
        mesh.add_vert(Vec3::new(x0, 0.0, 1.0)),
    ];
    let uvs = [
        uv0,
        uv0 + Vec2::new(uv_size, 0.0),
        uv0 + Vec2::new(uv_size, uv_size),
        uv0 + Vec2::new(0.0, uv_size),
    ];
    mesh.add_face_uv(
        v.iter().zip(uvs).map(|(&vert, uv)| Corner { vert, uv }).collect(),
    )
    .unwrap();
}

#[test]
fn golden_contact_sheet_box() {
    let doc = box_doc();
    let img = contact_sheet(&doc, &pack(BTreeMap::new()), 128).unwrap();
    assert_eq!((img.width(), img.height()), (512, 256), "4x2 grid of 128px tiles");
    let mut bytes = Vec::new();
    img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    let golden =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/contact_sheet_box.png");
    if std::env::var_os("DGM_BLESS").is_some() {
        std::fs::write(&golden, &bytes).unwrap();
        return;
    }
    let want = std::fs::read(&golden).expect("golden missing; bless with DGM_BLESS=1");
    assert!(
        bytes == want,
        "contact sheet drifted from golden ({} vs {} bytes)",
        bytes.len(),
        want.len()
    );
}

#[test]
fn raycast_hit_and_miss() {
    let doc = box_doc();
    // Straight at the +Z face of the unit box from z=5: enters at z=0.5.
    let hit = raycast(&doc, "box", Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -2.0))
        .expect("ray through the box must hit");
    assert!((hit.distance - 4.5).abs() < 1e-4, "distance {}", hit.distance);
    assert!((hit.point - Vec3::new(0.0, 0.0, 0.5)).length() < 1e-4, "point {}", hit.point);
    let n = doc.objects["box"].mesh.face_normal(hit.face).unwrap();
    assert!((n - Vec3::Z).length() < 1e-4, "nearest hit must be the +Z face, normal {n}");

    // Parallel ray offset past the box: miss.
    assert!(raycast(&doc, "box", Vec3::new(5.0, 5.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).is_none());
    // Pointing away: miss (no negative-t hits).
    assert!(raycast(&doc, "box", Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, 1.0)).is_none());
    // Unknown object and degenerate direction: miss.
    assert!(raycast(&doc, "nope", Vec3::ZERO, Vec3::Z).is_none());
    assert!(raycast(&doc, "box", Vec3::ZERO, Vec3::ZERO).is_none());
}

#[test]
fn stretch_ordering_and_density_on_two_face_mesh() {
    // Two 1x1m quads on one mesh: face A maps to a 0.25^2 UV patch
    // (world/uv ratio 16), face B to 0.5^2 (ratio 4). Median 10 gives
    // deviations {1.6, 2.5}; densities on a 256px sheet are 64 and 128.
    let mut mesh = Mesh::new();
    quad(&mut mesh, 0.0, Vec2::new(0.0, 0.0), 0.25);
    quad(&mut mesh, 2.0, Vec2::new(0.5, 0.5), 0.5);

    let mut doc = Doc::new(AssetClass::Prop);
    doc.materials.insert(
        "trim".into(),
        Material {
            texture: TextureRef::Trim { sheet: "t".into() },
            alpha: AlphaMode::Opaque,
            double_sided: false,
            emissive: None,
            emissive_strength: 1.0,
        },
    );
    let mut obj = Object::new(mesh);
    obj.material = Some("trim".into());
    doc.objects.insert("quad2".into(), obj);

    let mut trims = BTreeMap::new();
    trims.insert(
        "t".into(),
        TrimSheet { file: "missing.png".into(), size: [256, 256], regions: BTreeMap::new() },
    );
    let m = metrics(&doc, &pack(trims)).unwrap();

    assert!((m.stretch_max - 2.5).abs() < 1e-3, "stretch_max {}", m.stretch_max);
    assert!((m.stretch_mean - 2.05).abs() < 1e-3, "stretch_mean {}", m.stretch_mean);
    assert!(m.stretch_max > m.stretch_mean, "max deviation dominates the mean");
    assert!((m.density_spread - 2.0).abs() < 1e-3, "density_spread {}", m.density_spread);
    assert!(m.uv_occupancy.is_none(), "all-trim scene has no occupancy");
    assert!(m.mask_drift.is_none(), "no LODs, no drift");

    let o = &m.per_object["quad2"];
    assert_eq!(o.tris, 4);
    assert!(o.missing_texture, "trim PNG is absent and must be recorded");
    assert!((o.density_min.unwrap() - 64.0).abs() < 1e-2, "density_min {:?}", o.density_min);
    assert!((o.density_max.unwrap() - 128.0).abs() < 1e-2, "density_max {:?}", o.density_max);
    assert!((o.stretch_max - m.stretch_max).abs() < 1e-6, "single object drives the scene");
}

#[test]
fn occupancy_and_seam_contrast_on_color_quad() {
    // One quad with UVs covering [0, 0.25]^2 of a Color material:
    // occupancy is 1/16 of UV space, and a solid colour has no seams.
    let mut mesh = Mesh::new();
    quad(&mut mesh, 0.0, Vec2::new(0.0, 0.0), 0.25);
    let mut doc = Doc::new(AssetClass::Prop);
    doc.materials.insert(
        "flat".into(),
        Material {
            texture: TextureRef::Color { rgba: [200, 60, 40, 255] },
            alpha: AlphaMode::Opaque,
            double_sided: false,
            emissive: None,
            emissive_strength: 1.0,
        },
    );
    let mut obj = Object::new(mesh);
    obj.material = Some("flat".into());
    doc.objects.insert("q".into(), obj);

    let m = metrics(&doc, &pack(BTreeMap::new())).unwrap();
    let occ = m.uv_occupancy.expect("colour material counts as non-trim");
    assert!((occ - 0.0625).abs() < 1e-3, "occupancy {occ}");
    assert_eq!(m.seam_contrast_max, 0.0);
    assert_eq!(m.seam_contrast_mean, 0.0);
    assert!(m.palette_distance > 0.0, "[200,60,40] sits away from the gray palette");
}

#[test]
fn view_layouts_and_visibility() {
    let doc = box_doc();
    let p = pack(BTreeMap::new());

    let strip = filmstrip(&doc, &p, 5, 64).unwrap();
    assert_eq!((strip.width(), strip.height()), (320, 64), "one row of 5 frames");

    let wf = wireframe_sheet(&doc, &p, 64).unwrap();
    assert_eq!((wf.width(), wf.height()), (256, 128));
    let hm = heatmap(&doc, &p, 64).unwrap();
    assert_eq!((hm.width(), hm.height()), (256, 128));

    let masks = silhouette_masks(&doc, "box", 3, 64).unwrap();
    assert_eq!(masks.len(), 3);
    for m in &masks {
        assert_eq!(m.get_pixel(32, 32).0[0], 255, "box must cover the tile center");
        assert_eq!(m.get_pixel(0, 0).0[0], 0, "corners stay background");
    }

    let uv = uv_layout(&doc, &p, "box").unwrap();
    assert_eq!((uv.width(), uv.height()), (512, 512));
    assert!(silhouette_masks(&doc, "ghost", 1, 32).is_err(), "unknown object errors");
}

/// The brightest pixel of a tile, as (r, g, b) — box fixtures always have
/// a face brighter than the checker background.
fn max_pixel(img: &image::RgbaImage) -> [u8; 3] {
    let mut best = [0u8; 3];
    for p in img.pixels() {
        let [r, g, b, _] = p.0;
        if r as u32 + g as u32 + b as u32 > best[0] as u32 + best[1] as u32 + best[2] as u32 {
            best = [r, g, b];
        }
    }
    best
}

#[test]
fn vertex_colors_darken_beauty_and_filmstrip_but_not_heatmap() {
    let p = pack(BTreeMap::new());
    let plain = box_doc();
    let mut tinted = box_doc();
    let mesh = &mut tinted.objects.get_mut("box").unwrap().mesh;
    let verts: Vec<_> = mesh.verts.keys().copied().collect();
    for v in verts {
        mesh.colors.insert(v, [0.5, 0.5, 0.5]);
    }

    let a = max_pixel(&contact_sheet(&plain, &p, 64).unwrap());
    let b = max_pixel(&contact_sheet(&tinted, &p, 64).unwrap());
    assert!(b[0] < a[0] && b[1] < a[1] && b[2] < a[2], "sheet: {b:?} must be darker than {a:?}");
    // Half tint halves the sample before the shade term: within rounding.
    assert!((b[0] as i32 - a[0] as i32 / 2).abs() <= 1, "expected ~half of {a:?}, got {b:?}");

    let a = max_pixel(&filmstrip(&plain, &p, 3, 64).unwrap());
    let b = max_pixel(&filmstrip(&tinted, &p, 3, 64).unwrap());
    assert!(b[0] < a[0], "filmstrip: {b:?} must be darker than {a:?}");

    assert_eq!(
        heatmap(&plain, &p, 64).unwrap().as_raw(),
        heatmap(&tinted, &p, 64).unwrap().as_raw(),
        "heatmap ignores vertex colors"
    );
    assert_ne!(
        wireframe_sheet(&plain, &p, 64).unwrap().as_raw(),
        wireframe_sheet(&tinted, &p, 64).unwrap().as_raw(),
        "wireframe fill carries the tint"
    );
}

#[test]
fn vertex_colors_interpolate_across_a_triangle() {
    // One quad in the XY plane facing +Z with a black->white gradient
    // along X: the rendered tile must be darker on the left than the right.
    let p = pack(BTreeMap::new());
    let mut doc = Doc::new(AssetClass::Prop);
    let mut mesh = Mesh::new();
    let v = [
        mesh.add_vert(Vec3::new(-1.0, -1.0, 0.0)),
        mesh.add_vert(Vec3::new(1.0, -1.0, 0.0)),
        mesh.add_vert(Vec3::new(1.0, 1.0, 0.0)),
        mesh.add_vert(Vec3::new(-1.0, 1.0, 0.0)),
    ];
    mesh.add_face(&v).unwrap();
    mesh.colors.insert(v[0], [0.0; 3]);
    mesh.colors.insert(v[3], [0.0; 3]);
    doc.objects.insert("grad".into(), Object::new(mesh));
    let strip = filmstrip(&doc, &p, 1, 64).unwrap();
    let left = strip.get_pixel(20, 32).0;
    let right = strip.get_pixel(44, 32).0;
    assert!(left[0] + 40 < right[0], "left {left:?} should be darker than right {right:?}");
}

#[test]
fn lit_range_reports_luminance_span() {
    let p = pack(BTreeMap::new());
    let plain = box_doc();
    assert!(metrics(&plain, &p).unwrap().lit_range.is_none(), "no bake → null");

    let mut doc = box_doc();
    let mesh = &mut doc.objects.get_mut("box").unwrap().mesh;
    let first = *mesh.verts.keys().next().unwrap();
    mesh.colors.insert(first, [0.0, 0.0, 0.0]);
    let n = mesh.verts.len() as f32;
    let lr = metrics(&doc, &p).unwrap().lit_range.expect("baked object → range");
    assert_eq!(lr[0], 0.0);
    assert_eq!(lr[1], 1.0, "uncoloured verts count as white");
    assert!((lr[2] - (n - 1.0) / n).abs() < 1e-5, "mean {}", lr[2]);
}
