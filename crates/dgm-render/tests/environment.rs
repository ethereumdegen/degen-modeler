//! Environment views + metrics: interior sheet layout and near-plane
//! clipping, connectivity over touching/crossing/floating objects, and the
//! repetition metric on a tiled texture vs a flat colour.

use std::collections::BTreeMap;
use std::path::PathBuf;

use dgm_atlas::{AssetClass, Band, Pack, PackManifest};
use dgm_mesh::{Mesh, primitives::{plane, prim_box}};
use dgm_render::{interior_sheet, metrics};
use dgm_scene::{AlphaMode, Doc, Material, Object, TextureRef};
use glam::Vec3;
use image::{Rgba, RgbaImage};

const CHECKER: [[u8; 4]; 2] = [[26, 26, 31, 255], [37, 37, 44, 255]];

fn pack_at(root: PathBuf) -> Pack {
    Pack {
        root,
        manifest: PackManifest {
            name: "test".into(),
            version: "0".into(),
            budgets: BTreeMap::new(),
            texel_density: Band { min: 100.0, max: 450.0 },
            texture_sizes: vec![256],
            palette: vec!["#808080".into()],
            hard_edge_angle_deg: 40.0,
            uv_waste_max: 0.15,
            trims: BTreeMap::new(),
            rigs: BTreeMap::new(),
            clips: BTreeMap::new(),
            reference_board: None,
        },
    }
}

fn pack() -> Pack {
    pack_at(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests"))
}

fn material(texture: TextureRef) -> Material {
    Material { texture, alpha: AlphaMode::Opaque, double_sided: false, emissive: None, emissive_strength: 1.0 }
}

fn shifted(mut mesh: Mesh, by: Vec3) -> Mesh {
    for p in mesh.verts.values_mut() {
        *p += by;
    }
    mesh
}

fn doc_with(objects: Vec<(&str, Mesh)>) -> Doc {
    let mut doc = Doc::new(AssetClass::Environment);
    doc.materials.insert("paint".into(), material(TextureRef::Color { rgba: [168, 111, 61, 255] }));
    for (name, mesh) in objects {
        let mut obj = Object::new(mesh);
        obj.material = Some("paint".into());
        doc.objects.insert(name.into(), obj);
    }
    doc
}

fn is_background(p: [u8; 4]) -> bool {
    CHECKER.contains(&p)
}

fn brightest(img: &RgbaImage) -> u32 {
    img.pixels().map(|p| p.0[0] as u32 + p.0[1] as u32 + p.0[2] as u32).max().unwrap()
}

#[test]
fn interior_sheet_layout_and_near_clipping() {
    let doc = doc_with(vec![("room", prim_box(Vec3::ONE).unwrap())]);
    let p = pack();
    let px = 64;
    let sheet = interior_sheet(&doc, &p, px).unwrap();
    assert_eq!((sheet.width(), sheet.height()), (128, 128), "2x2 tiles");

    // Inside a closed box every camera looks at a wall: tile centers are
    // covered in all four views.
    for (tx, ty) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let c = sheet.get_pixel(tx * px + px / 2, ty * px + px / 2).0;
        assert!(!is_background(c), "tile ({tx},{ty}) center is background");
    }
    // Entrance view (tile 0): the side walls cross the near plane. Only
    // clipping keeps them, and only they reach the tile's left/right edge
    // (the far wall covers ~75% of the width at this FOV).
    for x in [1, px - 2] {
        let c = sheet.get_pixel(x, px / 2).0;
        assert!(!is_background(c), "entrance view lost the clipped side wall at x={x}");
    }
    // Empty scene still renders (unit-box framing), all background.
    let empty = interior_sheet(&Doc::new(AssetClass::Prop), &p, 16).unwrap();
    assert!(empty.pixels().all(|q| is_background(q.0)));
    assert!(interior_sheet(&doc, &p, 0).is_err());
}

#[test]
fn interior_views_multiply_vertex_colors() {
    let lit = doc_with(vec![("room", prim_box(Vec3::ONE).unwrap())]);
    let mut dark = doc_with(vec![("room", prim_box(Vec3::ONE).unwrap())]);
    let mesh = &mut dark.objects.get_mut("room").unwrap().mesh;
    for id in mesh.verts.keys().copied().collect::<Vec<_>>() {
        mesh.colors.insert(id, [0.25, 0.25, 0.25]);
    }
    let p = pack();
    let bright = brightest(&interior_sheet(&lit, &p, 32).unwrap());
    let dim = brightest(&interior_sheet(&dark, &p, 32).unwrap());
    assert!(dim * 3 < bright, "tinted {dim} vs untinted {bright}");
}

#[test]
fn connectivity_counts_touching_and_crossing_objects() {
    let p = pack();
    let unit = || prim_box(Vec3::ONE).unwrap();

    // One object: nothing can be disconnected.
    assert_eq!(metrics(&doc_with(vec![("a", unit())]), &p).unwrap().connectivity, 1.0);

    // Two boxes 0.5 m apart: neither touches.
    let apart = doc_with(vec![("a", unit()), ("b", shifted(unit(), Vec3::new(1.5, 0.0, 0.0)))]);
    assert_eq!(metrics(&apart, &p).unwrap().connectivity, 0.0);

    // Face-to-face contact (within 0.02 m): both touch.
    let kiss = doc_with(vec![("a", unit()), ("b", shifted(unit(), Vec3::new(1.015, 0.0, 0.0)))]);
    assert_eq!(metrics(&kiss, &p).unwrap().connectivity, 1.0);

    // A wide floor plane slicing through the box (no vertex near the other's
    // surface, edges cross): both touch; a third box floats -> 2/3.
    let sliced = doc_with(vec![
        ("box", unit()),
        ("floor", plane(3.0, 3.0).unwrap()),
        ("float", shifted(unit(), Vec3::new(0.0, 3.0, 0.0))),
    ]);
    let c = metrics(&sliced, &p).unwrap().connectivity;
    assert!((c - 2.0 / 3.0).abs() < 1e-6, "connectivity {c}");

    // LODs never count.
    let mut lod = kiss;
    let mut l = Object::new(shifted(unit(), Vec3::new(0.0, 5.0, 0.0)));
    l.lod_of = Some("a".into());
    lod.objects.insert("a_lod1".into(), l);
    assert_eq!(metrics(&lod, &p).unwrap().connectivity, 1.0);
}

/// Repetition = how many copies of a sheet cover the surface. A big wall
/// tiled at meter-scale UVs must score far above the same wall with its UVs
/// squeezed into one tile; untextured scenes have nothing to repeat.
#[test]
fn repetition_counts_sheet_copies_across_the_surface() {
    let dir = std::env::temp_dir().join(format!("dgm-render-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    RgbaImage::from_pixel(64, 64, Rgba([90, 80, 100, 255])).save(dir.join("rock.png")).unwrap();
    let p = pack_at(dir.join("pack"));

    // 8 m x 8 m plane: default meter-scale UVs span 64 sheet copies.
    let wall = || dgm_mesh::primitives::plane(8.0, 8.0).unwrap();
    let mut tiled = doc_with(vec![("wall", wall())]);
    tiled.materials.insert("rock".into(), material(TextureRef::File { path: "rock.png".into() }));
    tiled.objects.get_mut("wall").unwrap().material = Some("rock".into());

    let mut single = tiled.clone();
    for face in single.objects.get_mut("wall").unwrap().mesh.faces.values_mut() {
        for c in &mut face.corners {
            c.uv = (c.uv + glam::Vec2::splat(4.0)) / 8.0; // squeeze into [0,1]^2
        }
    }
    let untextured = doc_with(vec![("wall", wall())]);

    let r_tiled = metrics(&tiled, &p).unwrap().repetition.unwrap();
    let r_single = metrics(&single, &p).unwrap().repetition.unwrap();
    std::fs::remove_dir_all(&dir).ok();

    assert!((r_tiled - 1.0).abs() < 1e-4, "64 copies saturate: {r_tiled}");
    assert!(r_single.abs() < 1e-4, "one copy reads zero: {r_single}");
    assert_eq!(metrics(&untextured, &p).unwrap().repetition, None);
}
