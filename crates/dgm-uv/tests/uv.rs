//! Behavior tests for the UV toolkit: wrap handling, island invariants,
//! trim fitting, density targets, packing margins, and gate rules.

use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::TAU;
use std::path::PathBuf;

use dgm_atlas::{Band, Pack, PackManifest, Rect, TrimSheet};
use dgm_mesh::primitives::{cylinder, plane, prim_box};
use dgm_mesh::{Corner, EdgeKey, FaceId, Mesh, Severity, VertId};
use dgm_uv::{
    Axis, ProjectKind, TexInfo, UvError, assign_trim, island_of, islands, pack_islands, project,
    set_texel_density, unwrap, uv_findings,
};
use glam::{Vec2, Vec3};

fn uv_area(mesh: &Mesh, f: FaceId) -> f32 {
    let corners = &mesh.face(f).unwrap().corners;
    let n = corners.len();
    let mut s = 0.0f32;
    for i in 0..n {
        let a = corners[i].uv;
        let b = corners[(i + 1) % n].uv;
        s += a.x * b.y - b.x * a.y;
    }
    (s * 0.5).abs()
}

fn island_bbox(mesh: &Mesh, island: &BTreeSet<FaceId>) -> (Vec2, Vec2) {
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for &f in island {
        for c in &mesh.face(f).unwrap().corners {
            lo = lo.min(c.uv);
            hi = hi.max(c.uv);
        }
    }
    (lo, hi)
}

fn boxes_overlap(a: (Vec2, Vec2), b: (Vec2, Vec2), eps: f32) -> bool {
    a.0.x < b.1.x - eps && b.0.x < a.1.x - eps && a.0.y < b.1.y - eps && b.0.y < a.1.y - eps
}

fn test_pack() -> Pack {
    let mut regions = BTreeMap::new();
    regions.insert("wood".to_string(), Rect { x: 64, y: 32, w: 64, h: 32 });
    let mut trims = BTreeMap::new();
    trims.insert(
        "main".to_string(),
        TrimSheet { file: "main.png".into(), size: [256, 128], regions },
    );
    Pack {
        root: PathBuf::from("."),
        manifest: PackManifest {
            name: "t".into(),
            version: "0".into(),
            budgets: BTreeMap::new(),
            texel_density: Band { min: 32.0, max: 512.0 },
            texture_sizes: vec![256],
            palette: vec![],
            hard_edge_angle_deg: 40.0,
            uv_waste_max: 0.15,
            trims,
            rigs: BTreeMap::new(),
            clips: BTreeMap::new(),
            reference_board: None,
        },
    }
}

/// A quad at height `y`, disconnected from anything else, with the given UVs.
fn add_uv_quad(mesh: &mut Mesh, y: f32, uv_lo: Vec2, uv_hi: Vec2) -> FaceId {
    let v = [
        mesh.add_vert(Vec3::new(0.0, y, 0.0)),
        mesh.add_vert(Vec3::new(1.0, y, 0.0)),
        mesh.add_vert(Vec3::new(1.0, y, 1.0)),
        mesh.add_vert(Vec3::new(0.0, y, 1.0)),
    ];
    let uvs = [
        Vec2::new(uv_lo.x, uv_lo.y),
        Vec2::new(uv_hi.x, uv_lo.y),
        Vec2::new(uv_hi.x, uv_hi.y),
        Vec2::new(uv_lo.x, uv_hi.y),
    ];
    mesh.add_face_uv(
        v.iter().zip(uvs).map(|(&vert, uv)| Corner { vert, uv }).collect(),
    )
    .unwrap()
}

// ---- project ----

#[test]
fn cylindrical_project_wraps_without_overlap_and_in_meters() {
    let mut mesh = cylinder(1.0, 2.0, 12, true).unwrap();
    let side: BTreeSet<FaceId> = (0..12).map(FaceId).collect();
    project(&mut mesh, &side, &ProjectKind::Cylindrical { axis: Axis::Y }).unwrap();

    let bbox = island_bbox(&mesh, &side);
    let size = bbox.1 - bbox.0;
    // Meters: full turn at radius 1 is tau wide, height 2 tall.
    assert!((size.x - TAU).abs() < 1e-3, "u span {} != tau", size.x);
    assert!((size.y - 2.0).abs() < 1e-4, "v span {} != 2", size.y);
    // No overlap: the faces tile the bbox exactly, so areas match.
    let sum: f32 = side.iter().map(|&f| uv_area(&mesh, f)).sum();
    let bbox_area = size.x * size.y;
    assert!(
        (sum - bbox_area).abs() < 1e-3 * bbox_area,
        "face uv areas {sum} != bbox area {bbox_area}: wraparound smeared or folded"
    );
}

#[test]
fn planar_project_is_world_scaled() {
    let mut mesh = plane(2.0, 1.0).unwrap();
    let faces: BTreeSet<FaceId> = mesh.faces.keys().copied().collect();
    project(&mut mesh, &faces, &ProjectKind::Planar { axis: Axis::Y }).unwrap();
    let a = uv_area(&mesh, FaceId(0));
    assert!((a - 2.0).abs() < 1e-5, "uv area {a} != world area 2");
}

#[test]
fn project_rejects_bad_selection() {
    let mut mesh = plane(1.0, 1.0).unwrap();
    let err = project(&mut mesh, &BTreeSet::new(), &ProjectKind::Box).unwrap_err();
    assert!(matches!(err, UvError::EmptySelection));
    let missing = BTreeSet::from([FaceId(99)]);
    let err = project(&mut mesh, &missing, &ProjectKind::Box).unwrap_err();
    assert!(matches!(err, UvError::UnknownFace(FaceId(99))));
}

// ---- islands ----

#[test]
fn islands_split_on_seams() {
    let mut mesh = prim_box(Vec3::splat(1.0)).unwrap();
    assert_eq!(islands(&mesh).len(), 1);
    let ring: Vec<EdgeKey> = mesh.face(FaceId(0)).unwrap().edges();
    mesh.seams.extend(ring);
    let isl = islands(&mesh);
    assert_eq!(isl.len(), 2);
    assert_eq!(island_of(&mesh, FaceId(0)), BTreeSet::from([FaceId(0)]));
    assert_eq!(island_of(&mesh, FaceId(3)).len(), 5);
    assert!(island_of(&mesh, FaceId(42)).is_empty());
}

// ---- unwrap ----

#[test]
fn unwrap_cylinder_islands_disjoint_in_unit_square() {
    let mut mesh = cylinder(1.0, 2.0, 12, true).unwrap();
    // Seams: both cap rims + one vertical cut. Bottom ring verts are 0..12,
    // top ring 12..24 (lathe emits rings in profile order).
    for k in 0..12u32 {
        let k1 = (k + 1) % 12;
        mesh.seams.insert(EdgeKey::new(VertId(k), VertId(k1)));
        mesh.seams.insert(EdgeKey::new(VertId(12 + k), VertId(12 + k1)));
    }
    mesh.seams.insert(EdgeKey::new(VertId(0), VertId(12)));

    unwrap(&mut mesh).unwrap();

    let isl = islands(&mesh);
    assert_eq!(isl.len(), 3, "side sheet + two caps");
    let boxes: Vec<_> = isl.iter().map(|i| island_bbox(&mesh, i)).collect();
    for (lo, hi) in &boxes {
        assert!(lo.x >= -1e-5 && lo.y >= -1e-5 && hi.x <= 1.0 + 1e-5 && hi.y <= 1.0 + 1e-5);
    }
    for a in 0..boxes.len() {
        for b in a + 1..boxes.len() {
            assert!(
                !boxes_overlap(boxes[a], boxes[b], 1e-6),
                "islands {a} and {b} overlap: {:?} vs {:?}",
                boxes[a],
                boxes[b]
            );
        }
    }
}

// ---- assign_trim ----

#[test]
fn assign_trim_lands_inside_region_rect() {
    let mut mesh = prim_box(Vec3::new(1.0, 1.0, 1.0)).unwrap();
    let faces: BTreeSet<FaceId> = mesh.faces.keys().copied().collect();
    let pack = test_pack();
    assign_trim(&mut mesh, &faces, &pack, "main", "wood").unwrap();
    // Region "wood" in UV space of the 256x128 sheet: [0.25..0.5] x [0.25..0.5].
    for face in mesh.faces.values() {
        for c in &face.corners {
            assert!(
                c.uv.x >= 0.25 - 1e-4 && c.uv.x <= 0.5 + 1e-4,
                "u {} outside region",
                c.uv.x
            );
            assert!(
                c.uv.y >= 0.25 - 1e-4 && c.uv.y <= 0.5 + 1e-4,
                "v {} outside region",
                c.uv.y
            );
        }
    }
    // Aspect fit: the larger side spans the rect.
    let bbox = island_bbox(&mesh, &faces);
    let size = bbox.1 - bbox.0;
    assert!((size.max_element() - 0.25).abs() < 1e-4);

    let err = assign_trim(&mut mesh, &faces, &pack, "main", "nope").unwrap_err();
    assert!(matches!(err, UvError::UnknownRegion { .. }));
}

// ---- set_texel_density ----

#[test]
fn set_texel_density_hits_target_within_one_percent() {
    let mut mesh = plane(2.0, 1.0).unwrap();
    let faces: BTreeSet<FaceId> = mesh.faces.keys().copied().collect();
    project(&mut mesh, &faces, &ProjectKind::Planar { axis: Axis::Y }).unwrap();
    // Push it off-target first so the op has real work to do.
    set_texel_density(&mut mesh, &faces, 77.0, 512).unwrap();
    set_texel_density(&mut mesh, &faces, 128.0, 512).unwrap();
    let f = FaceId(0);
    let density = 512.0 * (uv_area(&mesh, f) / mesh.face_area(f).unwrap()).sqrt();
    assert!(
        (density - 128.0).abs() <= 128.0 * 0.01,
        "density {density} not within 1% of 128"
    );
}

#[test]
fn set_texel_density_needs_uvs() {
    let mut mesh = plane(1.0, 1.0).unwrap();
    let faces: BTreeSet<FaceId> = mesh.faces.keys().copied().collect();
    let err = set_texel_density(&mut mesh, &faces, 128.0, 512).unwrap_err();
    assert!(matches!(err, UvError::Invalid(_)));
}

// ---- pack_islands ----

#[test]
fn pack_islands_no_overlap_and_margin_respected() {
    let mut mesh = prim_box(Vec3::new(1.0, 2.0, 0.5)).unwrap();
    let faces: BTreeSet<FaceId> = mesh.faces.keys().copied().collect();
    mesh.seams = mesh.edges(); // every face its own island
    project(&mut mesh, &faces, &ProjectKind::Box).unwrap();
    pack_islands(&mut mesh, 4, 256).unwrap();

    let margin = 4.0 / 256.0;
    let isl = islands(&mesh);
    assert_eq!(isl.len(), 6);
    let boxes: Vec<_> = isl.iter().map(|i| island_bbox(&mesh, i)).collect();
    for (lo, hi) in &boxes {
        assert!(
            lo.x >= margin - 1e-4
                && lo.y >= margin - 1e-4
                && hi.x <= 1.0 - margin + 1e-4
                && hi.y <= 1.0 - margin + 1e-4,
            "island bbox {lo:?}..{hi:?} violates border margin"
        );
    }
    for a in 0..boxes.len() {
        for b in a + 1..boxes.len() {
            let ((alo, ahi), (blo, bhi)) = (boxes[a], boxes[b]);
            let sep = (blo.x - ahi.x)
                .max(alo.x - bhi.x)
                .max(blo.y - ahi.y)
                .max(alo.y - bhi.y);
            assert!(
                sep >= margin - 1e-4,
                "islands {a} and {b} closer than margin: sep {sep}"
            );
        }
    }
    // Relative island scale preserved: the two 1x2 faces still have twice
    // the uv area of the two 1x0.5 faces... compare extremes instead: all
    // area ratios must match world ratios.
    let a0 = uv_area(&mesh, FaceId(0));
    let w0 = mesh.face_area(FaceId(0)).unwrap();
    for &f in &faces {
        let ratio = uv_area(&mesh, f) / mesh.face_area(f).unwrap();
        let base = a0 / w0;
        assert!((ratio / base - 1.0).abs() < 1e-3, "island scale drifted on {f}");
    }
}

// ---- uv_findings ----

#[test]
fn overlap_flagged_unless_mirror_declared() {
    let mut mesh = Mesh::new();
    let _f0 = add_uv_quad(&mut mesh, 0.0, Vec2::new(0.1, 0.1), Vec2::new(0.6, 0.6));
    let f1 = add_uv_quad(&mut mesh, 2.0, Vec2::new(0.1, 0.1), Vec2::new(0.6, 0.6));
    let band = Band { min: 1.0, max: 100_000.0 };
    let meshes: BTreeMap<String, (&Mesh, Option<TexInfo>)> =
        BTreeMap::from([("hut".to_string(), (&mesh, Some(TexInfo { px: 256, trim: false })))]);

    let findings = uv_findings(&meshes, &BTreeMap::new(), band, 1.0);
    let overlap = findings.iter().find(|f| f.rule == "uv.overlap");
    let overlap = overlap.expect("stacked islands must be flagged");
    assert_eq!(overlap.severity, Severity::Hard);
    assert!(overlap.elems.contains(&"f0".to_string()));

    let mirrors = BTreeMap::from([("hut".to_string(), BTreeSet::from([f1]))]);
    let findings = uv_findings(&meshes, &mirrors, band, 1.0);
    assert!(
        !findings.iter().any(|f| f.rule == "uv.overlap"),
        "declared mirror set must exempt the overlap"
    );
}

#[test]
fn out_of_bounds_only_for_non_trim() {
    let mut mesh = Mesh::new();
    add_uv_quad(&mut mesh, 0.0, Vec2::new(0.5, 0.5), Vec2::new(1.5, 0.9));
    let band = Band { min: 1.0, max: 100_000.0 };

    let non_trim: BTreeMap<String, (&Mesh, Option<TexInfo>)> =
        BTreeMap::from([("hut".to_string(), (&mesh, Some(TexInfo { px: 256, trim: false })))]);
    let findings = uv_findings(&non_trim, &BTreeMap::new(), band, 1.0);
    let oob = findings.iter().find(|f| f.rule == "uv.out_of_bounds").expect("u > 1 must flag");
    assert_eq!(oob.severity, Severity::Hard);
    assert_eq!(oob.elems, vec!["f0".to_string()]);

    let trim: BTreeMap<String, (&Mesh, Option<TexInfo>)> =
        BTreeMap::from([("hut".to_string(), (&mesh, Some(TexInfo { px: 256, trim: true })))]);
    let findings = uv_findings(&trim, &BTreeMap::new(), band, 1.0);
    assert!(!findings.iter().any(|f| f.rule == "uv.out_of_bounds"));
}

#[test]
fn texel_band_needs_texture_and_flags_outliers() {
    let mut mesh = Mesh::new();
    // 1x1 world quad with a full [0,1] uv square: density == px == 256.
    add_uv_quad(&mut mesh, 0.0, Vec2::ZERO, Vec2::ONE);

    let tight = Band { min: 300.0, max: 400.0 };
    let meshes: BTreeMap<String, (&Mesh, Option<TexInfo>)> =
        BTreeMap::from([("hut".to_string(), (&mesh, Some(TexInfo { px: 256, trim: false })))]);
    let findings = uv_findings(&meshes, &BTreeMap::new(), tight, 1.0);
    let band_hit = findings.iter().find(|f| f.rule == "uv.texel_band").expect("256 < 300");
    assert_eq!(band_hit.severity, Severity::Hard);
    assert!((band_hit.value.unwrap() - 256.0).abs() < 1.0);

    let wide = Band { min: 100.0, max: 300.0 };
    let findings = uv_findings(&meshes, &BTreeMap::new(), wide, 1.0);
    assert!(!findings.iter().any(|f| f.rule == "uv.texel_band"));

    // No material -> no texture size -> rule skipped entirely.
    let bare: BTreeMap<String, (&Mesh, Option<TexInfo>)> =
        BTreeMap::from([("hut".to_string(), (&mesh, None))]);
    let findings = uv_findings(&bare, &BTreeMap::new(), tight, 1.0);
    assert!(!findings.iter().any(|f| f.rule == "uv.texel_band"));
}

#[test]
fn stretch_warns_beyond_double_median() {
    let mut mesh = Mesh::new();
    for i in 0..4 {
        add_uv_quad(
            &mut mesh,
            i as f32,
            Vec2::new(0.0, i as f32 * 0.2),
            Vec2::new(0.2, i as f32 * 0.2 + 0.2),
        );
    }
    // Fifth quad: tiny uv footprint -> 3D/UV ratio far above the median.
    let squeezed =
        add_uv_quad(&mut mesh, 4.0, Vec2::new(0.9, 0.9), Vec2::new(0.92, 0.92));
    let band = Band { min: 0.1, max: 1_000_000.0 };
    let meshes: BTreeMap<String, (&Mesh, Option<TexInfo>)> =
        BTreeMap::from([("hut".to_string(), (&mesh, None))]);
    let findings = uv_findings(&meshes, &BTreeMap::new(), band, 1.0);
    let stretch = findings.iter().find(|f| f.rule == "uv.stretch").expect("outlier must warn");
    assert_eq!(stretch.severity, Severity::Warn);
    assert_eq!(stretch.elems, vec![squeezed.to_string()]);
}

#[test]
fn waste_warns_on_sparse_owned_atlas() {
    let mut mesh = Mesh::new();
    add_uv_quad(&mut mesh, 0.0, Vec2::ZERO, Vec2::new(0.3, 0.3)); // 9% used
    let band = Band { min: 0.1, max: 1_000_000.0 };
    let meshes: BTreeMap<String, (&Mesh, Option<TexInfo>)> =
        BTreeMap::from([("hut".to_string(), (&mesh, Some(TexInfo { px: 256, trim: false })))]);
    let findings = uv_findings(&meshes, &BTreeMap::new(), band, 0.15);
    let waste = findings.iter().find(|f| f.rule == "uv.waste").expect("91% waste > 15%");
    assert_eq!(waste.severity, Severity::Warn);
    assert!(waste.value.unwrap() > 0.9);
    // Generous allowance -> silent.
    let findings = uv_findings(&meshes, &BTreeMap::new(), band, 0.95);
    assert!(!findings.iter().any(|f| f.rule == "uv.waste"));
}
