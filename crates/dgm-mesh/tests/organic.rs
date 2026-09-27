//! Behavior tests for the organic ops and primitives: exact topology counts,
//! validate_mesh cleanliness, manifold/boundary invariants, determinism.

use std::collections::BTreeSet;

use dgm_mesh::ops::organic::{displace_noise, smooth, snap_to_surface, solidify, subdivide};
use dgm_mesh::ops::topo::dissolve;
use dgm_mesh::primitives::{cavern, plane, prim_box, tunnel};
use dgm_mesh::{FaceId, Mesh, VertId, validate_mesh};
use glam::{Vec2, Vec3};

fn assert_clean(mesh: &Mesh) {
    let findings = validate_mesh(mesh);
    assert!(findings.is_empty(), "expected clean mesh, got {findings:?}");
}

fn all_verts(mesh: &Mesh) -> BTreeSet<VertId> {
    mesh.verts.keys().copied().collect()
}

/// Divergence-theorem volume; positive when faces wind outward.
fn signed_volume(mesh: &Mesh) -> f32 {
    mesh.triangulate()
        .iter()
        .map(|t| {
            let face = &mesh.faces[&t.face];
            let [a, b, c] = t.corner_idx.map(|i| mesh.verts[&face.corners[i as usize].vert]);
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

fn uv_area(mesh: &Mesh, f: FaceId) -> f32 {
    let uvs: Vec<Vec2> = mesh.faces[&f].corners.iter().map(|c| c.uv).collect();
    let k = uvs.len();
    (0..k).map(|i| uvs[i].perp_dot(uvs[(i + 1) % k])).sum::<f32>().abs() * 0.5
}

fn assert_uvs_non_degenerate(mesh: &Mesh) {
    for &f in mesh.faces.keys() {
        assert!(uv_area(mesh, f) > 1e-6, "face {f} has a degenerate UV footprint");
    }
    for island in mesh.uv_islands() {
        let mut lo = Vec2::splat(f32::INFINITY);
        let mut hi = Vec2::splat(f32::NEG_INFINITY);
        for f in &island {
            for c in &mesh.faces[f].corners {
                lo = lo.min(c.uv);
                hi = hi.max(c.uv);
            }
        }
        let size = hi - lo;
        assert!(size.x > 1e-4 && size.y > 1e-4, "island {island:?} collapsed to {size}");
    }
}

/// Box with the -Y face removed: five faces, an open square rim.
fn open_lid() -> Mesh {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    dissolve(&mut m, &[FaceId(0)].into_iter().collect()).unwrap();
    assert_eq!(m.boundary_edges().len(), 4);
    m
}

// ---- subdivide ----

#[test]
fn subdivide_box_one_level_counts() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let d = subdivide(&mut m, 1, true).unwrap();
    assert_eq!(m.verts.len(), 26);
    assert_eq!(m.faces.len(), 24);
    assert!(m.faces.values().all(|f| f.corners.len() == 4));
    assert_eq!(d.created_verts.len(), 18);
    assert_eq!(d.created_faces.len(), 24);
    assert_eq!(d.removed_faces.len(), 6);
    assert_eq!(d.moved_verts.len(), 8);
    // Every cube edge was a seam; each now spans two sub-edges, so the six
    // faces still unwrap as six islands.
    assert_eq!(m.seams.len(), 24);
    assert_eq!(m.uv_islands().len(), 6);
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
    // Smoothing pulls the corners inside the original cube.
    for v in 0..8 {
        let p = m.verts[&VertId(v)];
        assert!(p.abs().max_element() < 1.0, "corner {v} at {p} did not move inward");
    }
}

#[test]
fn subdivide_linear_keeps_positions_and_stacks_levels() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let d = subdivide(&mut m, 1, false).unwrap();
    assert!(d.moved_verts.is_empty());
    for v in 0..8 {
        assert_eq!(m.verts[&VertId(v)].abs(), Vec3::ONE);
    }
    let d2 = subdivide(&mut m, 1, false).unwrap();
    // 26 + 24 face points + 48 edge points.
    assert_eq!(m.verts.len(), 98);
    assert_eq!(m.faces.len(), 96);
    assert_eq!(d2.removed_faces.len(), 24);
    assert_clean(&m);
    // Two levels at once give the same topology as two single levels.
    let mut twice = prim_box(Vec3::splat(2.0)).unwrap();
    let d = subdivide(&mut twice, 2, false).unwrap();
    assert_eq!(twice.verts.len(), 98);
    assert_eq!(twice.faces.len(), 96);
    assert_eq!(d.created_faces.len(), 96);
    assert_eq!(d.removed_faces.len(), 6);
}

#[test]
fn subdivide_triangle_and_open_boundary() {
    let mut m = Mesh::new();
    let a = m.add_vert(Vec3::new(0.0, 0.0, 0.0));
    let b = m.add_vert(Vec3::new(2.0, 0.0, 0.0));
    let c = m.add_vert(Vec3::new(0.0, 0.0, 2.0));
    m.add_face(&[a, c, b]).unwrap();
    subdivide(&mut m, 1, true).unwrap();
    assert_eq!(m.verts.len(), 7);
    assert_eq!(m.faces.len(), 3);
    assert_eq!(m.boundary_edges().len(), 6);
    // Boundary corners follow the (6P + a + b)/8 rule, not the interior one.
    assert_eq!(m.verts[&a], Vec3::new(0.25, 0.0, 0.25));
    assert_clean(&m);
}

#[test]
fn subdivide_interpolates_uvs_and_colors() {
    let mut m = plane(2.0, 2.0).unwrap();
    m.colors.insert(VertId(0), [0.0, 0.0, 0.0]);
    subdivide(&mut m, 1, false).unwrap();
    // Corner quad at v0 keeps its UV; the edge point midway to v1 averages
    // black with the implicit white of v1.
    let quad = m.faces.values().find(|f| f.corners[0].vert == VertId(0)).unwrap();
    assert_eq!(quad.corners[0].uv, Vec2::new(-1.0, -1.0));
    assert_eq!(quad.corners[2].uv, Vec2::ZERO);
    let edge_pt = quad.corners[1].vert;
    assert_eq!(m.colors[&edge_pt], [0.5, 0.5, 0.5]);
    let face_pt = quad.corners[2].vert;
    assert_eq!(m.colors[&face_pt], [0.75, 0.75, 0.75]);
    assert_uvs_non_degenerate(&m);
}

#[test]
fn subdivide_rejects_bad_levels() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    assert!(subdivide(&mut m, 0, true).is_err());
    assert!(subdivide(&mut m, 7, true).is_err());
}

// ---- displace_noise ----

#[test]
fn displace_noise_is_deterministic_and_seeded() {
    let base = || {
        let mut m = prim_box(Vec3::splat(2.0)).unwrap();
        subdivide(&mut m, 2, true).unwrap();
        m
    };
    let (mut a, mut b, mut c) = (base(), base(), base());
    let sel = all_verts(&a);
    let da = displace_noise(&mut a, &sel, 0.2, 0.7, 42).unwrap();
    displace_noise(&mut b, &sel, 0.2, 0.7, 42).unwrap();
    displace_noise(&mut c, &sel, 0.2, 0.7, 43).unwrap();
    assert_eq!(a.verts, b.verts);
    assert_ne!(a.verts, c.verts);
    assert_eq!(da.moved_verts.len(), a.verts.len());
    let orig = base();
    let mut max_shift = 0.0f32;
    for (v, p) in &a.verts {
        let shift = (*p - orig.verts[v]).length();
        assert!(shift <= 0.2 + 1e-5, "{v} moved {shift} > amplitude");
        max_shift = max_shift.max(shift);
    }
    assert!(max_shift > 0.02, "noise barely moved anything ({max_shift})");
    assert_clean(&a);
}

#[test]
fn displace_noise_moves_along_normal_only() {
    let mut m = plane(4.0, 4.0).unwrap();
    subdivide(&mut m, 2, false).unwrap();
    let sel = all_verts(&m);
    displace_noise(&mut m, &sel, 0.5, 1.0, 1).unwrap();
    // Plane normal is +Y: x/z stay on the grid, y varies.
    for p in m.verts.values() {
        assert!((p.x * 2.0).round() == p.x * 2.0 && (p.z * 2.0).round() == p.z * 2.0, "{p} left the grid");
    }
    assert!(m.verts.values().any(|p| p.y.abs() > 0.05));
    assert!(displace_noise(&mut m, &sel, 0.5, 0.0, 1).is_err());
    assert!(displace_noise(&mut m, &BTreeSet::new(), 0.5, 1.0, 1).is_err());
}

// ---- smooth ----

#[test]
fn smooth_pins_boundary_and_relaxes_interior() {
    let mut m = plane(4.0, 4.0).unwrap();
    subdivide(&mut m, 2, false).unwrap();
    // Bump the centre vertex (the level-1 face point) straight up.
    let centre = m.verts.iter().find(|(_, p)| **p == Vec3::ZERO).map(|(v, _)| *v).unwrap();
    m.set_pos(centre, Vec3::new(0.0, 1.0, 0.0)).unwrap();
    let before = m.clone();
    let sel = all_verts(&m);
    let d = smooth(&mut m, &sel, 3, 0.5).unwrap();
    assert!(m.verts[&centre].y < 0.5 && m.verts[&centre].y > 0.0);
    let boundary: BTreeSet<VertId> = m.boundary_edges().iter().flat_map(|e| [e.0, e.1]).collect();
    assert_eq!(boundary.len(), 16);
    for v in &boundary {
        assert_eq!(m.verts[v], before.verts[v], "boundary {v} moved");
        assert!(!d.moved_verts.contains(v));
    }
    assert!(d.moved_verts.contains(&centre));
    assert_clean(&m);
    assert!(smooth(&mut m, &sel, 0, 0.5).is_err());
    assert!(smooth(&mut m, &sel, 1, 1.5).is_err());
}

// ---- solidify ----

#[test]
fn solidify_open_lid_is_closed_manifold() {
    let mut m = open_lid();
    let d = solidify(&mut m, 0.1).unwrap();
    assert_eq!(m.verts.len(), 16);
    assert_eq!(m.faces.len(), 14);
    assert_eq!(d.created_verts.len(), 8);
    assert_eq!(d.created_faces.len(), 9);
    assert!(d.moved_verts.is_empty());
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
    assert!(signed_volume(&m) > 0.0, "shell winds inside-out");
    // Inner skin sits inside the original box; the open rim's verts have
    // no -Y face, so they slide inward at y = -1 rather than lifting.
    for &v in &d.created_verts {
        let p = m.verts[&v];
        assert!(p.x.abs() < 1.0 && p.z.abs() < 1.0 && p.y.abs() <= 1.0, "{v} at {p} is not inside");
    }
    // The box's 12 seams copied onto the inner skin; the rim loops (4 outer
    // + 4 inner) are already among them.
    assert_eq!(m.seams.len(), 24);
    // Original + inner + rim islands, all with real UV footprints.
    assert_eq!(m.uv_islands().len(), 11);
    assert_uvs_non_degenerate(&m);
}

#[test]
fn solidify_negative_thickness_grows_outward() {
    let mut m = open_lid();
    let d = solidify(&mut m, -0.1).unwrap();
    assert_eq!(d.moved_verts.len(), 8);
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
    assert!(signed_volume(&m) > 0.0);
    // The copy stayed on the original surface; originals moved out.
    for &v in &d.created_verts {
        assert_eq!(m.verts[&v].abs(), Vec3::ONE);
    }
    for v in 0..8 {
        assert!(m.verts[&VertId(v)].abs().max_element() > 1.0);
    }
}

#[test]
fn solidify_copies_colors_and_rejects_zero() {
    let mut m = open_lid();
    m.colors.insert(VertId(4), [0.2, 0.3, 0.4]);
    let d = solidify(&mut m, 0.2).unwrap();
    let copies: Vec<_> = d.created_verts.iter().filter(|v| m.colors.get(v) == Some(&[0.2, 0.3, 0.4])).collect();
    assert_eq!(copies.len(), 1);
    let mut z = open_lid();
    assert!(solidify(&mut z, 0.0).is_err());
    assert_eq!(z.faces.len(), 5);
}

// ---- snap_to_surface ----

#[test]
fn snap_drops_vert_onto_plane_exactly() {
    let floor = plane(4.0, 4.0).unwrap();
    let mut m = Mesh::new();
    let hi = m.add_vert(Vec3::new(0.3, 2.0, -0.7));
    let lo = m.add_vert(Vec3::new(-1.0, -3.0, 1.0));
    let off = m.add_vert(Vec3::new(5.0, 1.0, 5.0));
    let d = snap_to_surface(&mut m, &[hi, lo, off].into_iter().collect(), &floor, Vec3::NEG_Y).unwrap();
    assert_eq!(m.verts[&hi], Vec3::new(0.3, 0.0, -0.7));
    assert_eq!(m.verts[&lo], Vec3::new(-1.0, 0.0, 1.0));
    assert_eq!(m.verts[&off], Vec3::new(5.0, 1.0, 5.0));
    assert_eq!(d.moved_verts, vec![hi, lo]);
    let err = snap_to_surface(&mut m, &[off].into_iter().collect(), &floor, Vec3::NEG_Y).unwrap_err();
    assert!(err.to_string().contains("v2"), "{err}");
    assert!(snap_to_surface(&mut m, &[hi].into_iter().collect(), &floor, Vec3::ZERO).is_err());
}

#[test]
fn snap_picks_nearest_hit_in_either_direction() {
    // Two horizontal quads at y = 1 and y = -4; a vert at y = 0 goes up.
    let mut target = plane(4.0, 4.0).unwrap();
    let low = {
        let mut p = plane(4.0, 4.0).unwrap();
        let all = all_verts(&p);
        p.transform_verts(&all, &glam::Mat4::from_translation(Vec3::new(0.0, -4.0, 0.0)));
        p
    };
    let all = all_verts(&target);
    target.transform_verts(&all, &glam::Mat4::from_translation(Vec3::Y));
    target.append(&low);
    let mut m = Mesh::new();
    let v = m.add_vert(Vec3::new(0.5, 0.0, 0.5));
    snap_to_surface(&mut m, &[v].into_iter().collect(), &target, Vec3::NEG_Y).unwrap();
    assert_eq!(m.verts[&v].y, 1.0);
}

// ---- append ----

#[test]
fn append_remaps_ids_and_carries_attributes() {
    let mut a = prim_box(Vec3::splat(1.0)).unwrap();
    let mut b = prim_box(Vec3::splat(1.0)).unwrap();
    b.colors.insert(VertId(3), [0.1, 0.2, 0.3]);
    let seams_before = a.seams.len();
    let map = a.append(&b);
    assert_eq!(a.verts.len(), 16);
    assert_eq!(a.faces.len(), 12);
    assert_eq!(map.len(), 8);
    assert!(map.values().all(|v| v.0 >= 8));
    assert_eq!(a.seams.len(), seams_before + b.seams.len());
    assert_eq!(a.colors.get(&map[&VertId(3)]), Some(&[0.1, 0.2, 0.3]));
    assert!(a.colors.get(&VertId(3)).is_none());
    // UVs survive verbatim on the appended faces.
    let last = a.faces.values().last().unwrap();
    let src = b.faces.values().last().unwrap();
    assert_eq!(last.corners.iter().map(|c| c.uv).collect::<Vec<_>>(), src.corners.iter().map(|c| c.uv).collect::<Vec<_>>());
    assert_clean(&a);
}

// ---- tunnel ----

#[test]
fn tunnel_counts_and_open_ends() {
    let path = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::new(4.0, 1.0, 1.0),
        Vec3::new(4.0, 1.0, 4.0),
    ];
    let m = tunnel(&path, &[0.5], 6).unwrap();
    assert_eq!(m.verts.len(), 24);
    assert_eq!(m.faces.len(), 18);
    assert!(m.faces.values().all(|f| f.corners.len() == 4));
    assert_eq!(m.boundary_edges().len(), 12);
    assert!(m.seams.is_empty());
    assert_eq!(m.uv_islands().len(), 1);
    assert_clean(&m);
    assert_uvs_non_degenerate(&m);
    // Every ring vertex sits at the radius from its path point.
    for (i, c) in path.iter().enumerate() {
        for k in 0..6 {
            let p = m.verts[&VertId((i * 6 + k) as u32)];
            assert!(((p - *c).length() - 0.5).abs() < 1e-5, "ring {i} vert {k} at {p}");
        }
    }
    // Outward winding: the first ring's quads face away from the axis.
    let n = m.face_normal(FaceId(0)).unwrap();
    let centroid = m.face_centroid(FaceId(0)).unwrap();
    let radial = Vec3::new(0.0, centroid.y, centroid.z).normalize();
    assert!(n.dot(radial) > 0.5, "normal {n} vs radial {radial}");
}

#[test]
fn tunnel_per_point_radii_and_errors() {
    let path = [Vec3::ZERO, Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, 6.0, 0.0)];
    let m = tunnel(&path, &[1.0, 0.5, 1.0], 8).unwrap();
    assert_eq!(m.verts.len(), 24);
    assert!((m.verts[&VertId(8)].length() - (3.0f32 * 3.0 + 0.25).sqrt()).abs() < 1e-5);
    assert_clean(&m);
    assert_uvs_non_degenerate(&m);
    assert!(tunnel(&path[..1], &[1.0], 8).is_err());
    assert!(tunnel(&path, &[1.0, 1.0], 8).is_err());
    assert!(tunnel(&path, &[0.0], 8).is_err());
    assert!(tunnel(&path, &[1.0], 2).is_err());
    assert!(tunnel(&[Vec3::ZERO, Vec3::ZERO], &[1.0], 8).is_err());
}

// ---- cavern ----

#[test]
fn cavern_counts_floor_cut_and_noise() {
    let m = cavern(Vec3::new(3.0, 2.0, 3.0), 8, 4, Some(-1.0), 0.2, 7).unwrap();
    // 2 apexes + 3 circles of 8; the bottom fan (apex + circle at y=-1.41)
    // sits entirely below the floor and goes, taking the apex with it.
    assert_eq!(m.verts.len(), 25);
    assert_eq!(m.faces.len(), 24);
    assert_eq!(m.boundary_edges().len(), 8);
    assert_clean(&m);
    assert_uvs_non_degenerate(&m);
    // Top fan + band islands (bottom fan is gone); the cut ring keeps its
    // seam marks on what are now boundary edges.
    assert_eq!(m.uv_islands().len(), 2);
    assert_eq!(m.seams.len(), 16);
    // Noise moved verts by at most noise * min radius.
    let clean = cavern(Vec3::new(3.0, 2.0, 3.0), 8, 4, Some(-1.0), 0.0, 7).unwrap();
    let mut max_shift = 0.0f32;
    for (v, p) in &m.verts {
        let shift = (*p - clean.verts[v]).length();
        assert!(shift <= 0.4 + 1e-5);
        max_shift = max_shift.max(shift);
    }
    assert!(max_shift > 0.01);
    assert_eq!(m.verts, cavern(Vec3::new(3.0, 2.0, 3.0), 8, 4, Some(-1.0), 0.2, 7).unwrap().verts);
    // Outward normals: closed version has positive volume.
    let closed = cavern(Vec3::new(3.0, 2.0, 3.0), 8, 4, None, 0.0, 0).unwrap();
    assert_eq!(closed.verts.len(), 26);
    assert_eq!(closed.faces.len(), 32);
    assert!(closed.boundary_edges().is_empty());
    assert!(signed_volume(&closed) > 0.0);
    assert_eq!(closed.uv_islands().len(), 3);
    assert_clean(&closed);
    assert_uvs_non_degenerate(&closed);
}

#[test]
fn cavern_rejects_bad_params() {
    assert!(cavern(Vec3::new(1.0, 0.0, 1.0), 8, 4, None, 0.0, 0).is_err());
    assert!(cavern(Vec3::ONE, 2, 4, None, 0.0, 0).is_err());
    assert!(cavern(Vec3::ONE, 8, 1, None, 0.0, 0).is_err());
    assert!(cavern(Vec3::ONE, 8, 4, None, -0.1, 0).is_err());
    assert!(cavern(Vec3::ONE, 8, 4, Some(1.0), 0.0, 0).is_err());
}
