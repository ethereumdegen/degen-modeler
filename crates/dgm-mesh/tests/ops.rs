//! Behavior tests for topology and deform ops: exact topology counts,
//! validate_mesh cleanliness, and boundary/seam invariants.

use std::collections::BTreeSet;

use dgm_mesh::ops::deform::lattice;
use dgm_mesh::ops::topo::{
    bevel_edges, bridge, decimate_to_target, dissolve, extrude, inset, loop_cut, merge_verts, mirror,
};
use dgm_mesh::primitives::{cylinder, plane, prim_box};
use dgm_mesh::{EdgeKey, FaceId, Mesh, VertId, validate_mesh};
use glam::Vec3;

fn v(i: u32) -> VertId {
    VertId(i)
}

fn f(i: u32) -> FaceId {
    FaceId(i)
}

fn e(a: u32, b: u32) -> EdgeKey {
    EdgeKey::new(VertId(a), VertId(b))
}

fn assert_clean(mesh: &Mesh) {
    let findings = validate_mesh(mesh);
    assert!(findings.is_empty(), "expected clean mesh, got {findings:?}");
}

fn faces(ids: &[u32]) -> BTreeSet<FaceId> {
    ids.iter().map(|&i| f(i)).collect()
}

#[test]
fn extrude_box_top_face() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let d = extrude(&mut m, &faces(&[1]), 0.5).unwrap();
    assert_eq!(m.verts.len(), 12);
    assert_eq!(m.faces.len(), 10);
    assert_eq!(d.created_verts.len(), 4);
    assert_eq!(d.created_faces.len(), 4);
    assert!(d.removed_verts.is_empty());
    // The patch keeps its face id and now sits at y = 1.5.
    for vert in m.face(f(1)).unwrap().verts().collect::<Vec<_>>() {
        assert!((m.pos(vert).unwrap().y - 1.5).abs() < 1e-6);
    }
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
}

#[test]
fn extrude_two_face_patch_has_no_interior_walls() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    // Top (+Y) and +Z faces share edge (6, 7); no side quad may appear there.
    let d = extrude(&mut m, &faces(&[1, 2]), 0.5).unwrap();
    assert_eq!(m.verts.len(), 14);
    assert_eq!(m.faces.len(), 12);
    assert_eq!(d.created_faces.len(), 6);
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
}

#[test]
fn extrude_open_plane_keeps_its_boundary() {
    let mut m = plane(2.0, 2.0).unwrap();
    extrude(&mut m, &faces(&[0]), 1.0).unwrap();
    assert_eq!(m.verts.len(), 8);
    assert_eq!(m.faces.len(), 5);
    assert_eq!(m.boundary_edges().len(), 4);
    assert_clean(&m);
}

#[test]
fn extrude_rejects_empty_selection() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    assert!(extrude(&mut m, &BTreeSet::new(), 0.5).is_err());
}

#[test]
fn inset_box_top_face() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let d = inset(&mut m, &faces(&[1]), 0.2, 0.0).unwrap();
    assert_eq!(m.verts.len(), 12);
    assert_eq!(m.faces.len(), 10);
    assert_eq!(d.created_verts.len(), 4);
    assert_eq!(d.created_faces.len(), 4);
    // The inner face keeps its id, shrinks, and stays in the y = 1 plane.
    assert!(m.face_area(f(1)).unwrap() < 4.0);
    for vert in m.face(f(1)).unwrap().verts().collect::<Vec<_>>() {
        assert!((m.pos(vert).unwrap().y - 1.0).abs() < 1e-6);
    }
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
}

#[test]
fn inset_with_depth_sinks_the_inner_face() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    inset(&mut m, &faces(&[1]), 0.3, -0.4).unwrap();
    for vert in m.face(f(1)).unwrap().verts().collect::<Vec<_>>() {
        assert!((m.pos(vert).unwrap().y - 0.6).abs() < 1e-6);
    }
    assert_clean(&m);
}

#[test]
fn inset_thickness_too_large_names_the_face() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let err = inset(&mut m, &faces(&[1]), 2.0, 0.0).unwrap_err();
    assert!(err.to_string().contains("f1"), "got: {err}");
    // Validation happens before mutation.
    assert_eq!(m.verts.len(), 8);
    assert_eq!(m.faces.len(), 6);
}

#[test]
fn bevel_one_box_edge_single_segment() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    // Vertical edge shared by the +Z and +X faces.
    let d = bevel_edges(&mut m, &BTreeSet::from([e(2, 6)]), 0.25, 1).unwrap();
    assert_eq!(m.verts.len(), 10);
    assert_eq!(m.faces.len(), 7);
    assert_eq!(d.created_verts.len(), 4);
    assert_eq!(d.created_faces.len(), 1);
    assert_eq!(d.removed_verts, vec![v(2), v(6)]);
    // Faces at the old edge ends become pentagons; the flanks stay quads.
    assert_eq!(m.face(f(0)).unwrap().corners.len(), 5);
    assert_eq!(m.face(f(1)).unwrap().corners.len(), 5);
    assert_eq!(m.face(f(2)).unwrap().corners.len(), 4);
    assert_eq!(m.face(f(4)).unwrap().corners.len(), 4);
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
}

#[test]
fn bevel_two_segments_rounds_the_corners() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let d = bevel_edges(&mut m, &BTreeSet::from([e(2, 6)]), 0.25, 2).unwrap();
    // 4 boundverts + 1 profile vert per end; strip of 2 quads + 2 corner tris.
    assert_eq!(m.verts.len(), 12);
    assert_eq!(m.faces.len(), 10);
    assert_eq!(d.created_verts.len(), 6);
    assert_eq!(d.created_faces.len(), 4);
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
}

#[test]
fn bevel_full_vertical_loop_of_the_box() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let sel: BTreeSet<EdgeKey> = [e(0, 4), e(1, 5), e(2, 6), e(3, 7)].into();
    bevel_edges(&mut m, &sel, 0.25, 1).unwrap();
    // Every vertical edge chamfered: 8 corner verts replaced by 16, four
    // strips added, top/bottom become octagons.
    assert_eq!(m.verts.len(), 16);
    assert_eq!(m.faces.len(), 10);
    assert_eq!(m.face(f(0)).unwrap().corners.len(), 8);
    assert_eq!(m.face(f(1)).unwrap().corners.len(), 8);
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
}

#[test]
fn bevel_rejects_bad_input() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    // Not an edge of the box (face diagonal).
    let err = bevel_edges(&mut m, &BTreeSet::from([e(0, 6)]), 0.25, 1).unwrap_err();
    assert!(err.to_string().contains("e0:6"), "got: {err}");
    // Width larger than the neighbouring edges.
    assert!(bevel_edges(&mut m, &BTreeSet::from([e(2, 6)]), 3.0, 1).is_err());
    assert!(bevel_edges(&mut m, &BTreeSet::from([e(2, 6)]), 0.25, 0).is_err());
    // Boundary edges cannot be beveled.
    let mut p = plane(2.0, 2.0).unwrap();
    let rim = p.boundary_edges()[0];
    assert!(bevel_edges(&mut p, &BTreeSet::from([rim]), 0.1, 1).is_err());
}

#[test]
fn loop_cut_closed_ring_on_cylinder() {
    let mut m = cylinder(1.0, 2.0, 8, true).unwrap();
    // Vertical side edge between the bottom and top rings.
    let d = loop_cut(&mut m, e(0, 8), 1).unwrap();
    assert_eq!(m.verts.len(), 24);
    assert_eq!(m.faces.len(), 18); // 16 side quads + 2 untouched caps
    assert_eq!(d.created_verts.len(), 8);
    assert_eq!(d.removed_faces.len(), 8);
    assert_eq!(d.created_faces.len(), 16);
    for &nv in &d.created_verts {
        assert!(m.pos(nv).unwrap().y.abs() < 1e-6, "cut loop should sit at y = 0");
    }
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
}

#[test]
fn loop_cut_two_cuts_splits_seam_marks() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    // Isolate the behavior under test from the primitive's default seams.
    m.seams.clear();
    m.seams.insert(e(0, 4));
    let d = loop_cut(&mut m, e(0, 4), 2).unwrap();
    assert_eq!(m.verts.len(), 16);
    assert_eq!(m.faces.len(), 14); // top + bottom + 4 ring quads * 3 slices
    assert_eq!(d.created_verts.len(), 8);
    // The seam mark survives as the three sub-edges of the cut edge
    // (the two cut verts on it are the first ids handed out: v8, v9).
    assert_eq!(m.seams, BTreeSet::from([e(0, 8), e(8, 9), e(4, 9)]));
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
}

#[test]
fn loop_cut_open_ring_splices_terminal_faces() {
    // The ring through a bottom rim edge crosses one side quad up to the top
    // rim and stops at both caps, which must gain the cut verts (no
    // T-junctions): each 8-gon cap becomes a 9-gon.
    let mut m = cylinder(1.0, 2.0, 8, true).unwrap();
    let d = loop_cut(&mut m, e(0, 1), 1).unwrap();
    assert_eq!(d.created_verts.len(), 2);
    assert_eq!(d.uv_faces.len(), 2);
    assert!(m.faces.values().any(|f| f.corners.len() == 9));
    assert_clean(&m);
    assert!(m.boundary_edges().is_empty());
}

#[test]
fn loop_cut_rejects_bad_input() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    assert!(loop_cut(&mut m, e(0, 4), 0).is_err());
    assert!(loop_cut(&mut m, e(0, 6), 1).is_err());
}

/// Two coaxial open tubes (6 segments) with facing rims, plus those rims.
fn two_tubes() -> (Mesh, BTreeSet<EdgeKey>, BTreeSet<EdgeKey>) {
    let n = 6u32;
    let mut m = Mesh::new();
    let ring = |m: &mut Mesh, y: f32| -> Vec<VertId> {
        (0..n)
            .map(|k| {
                let th = k as f32 / n as f32 * std::f32::consts::TAU;
                m.add_vert(Vec3::new(th.cos(), y, th.sin()))
            })
            .collect()
    };
    let r0 = ring(&mut m, -2.0);
    let r1 = ring(&mut m, -1.0);
    let r2 = ring(&mut m, 1.0);
    let r3 = ring(&mut m, 2.0);
    for (lo, up) in [(&r0, &r1), (&r2, &r3)] {
        for k in 0..n as usize {
            let k1 = (k + 1) % n as usize;
            m.add_face(&[lo[k], up[k], up[k1], lo[k1]]).unwrap();
        }
    }
    let rim = |ring: &[VertId]| -> BTreeSet<EdgeKey> {
        (0..ring.len()).map(|k| EdgeKey::new(ring[k], ring[(k + 1) % ring.len()])).collect()
    };
    (m, rim(&r1), rim(&r2))
}

#[test]
fn bridge_two_rims_with_quads() {
    let (mut m, rim_a, rim_b) = two_tubes();
    assert_eq!(m.boundary_edges().len(), 24);
    let d = bridge(&mut m, &rim_a, &rim_b).unwrap();
    assert_eq!(d.created_faces.len(), 6);
    assert_eq!(m.faces.len(), 18);
    // The two bridged rims are now interior; only the far rims stay open.
    assert_eq!(m.boundary_edges().len(), 12);
    assert_clean(&m);
    // Straight pairing: bridge quads connect vertically aligned rim verts
    // rather than corkscrewing around the axis.
    for &nf in &d.created_faces {
        let c: Vec<VertId> = m.face(nf).unwrap().verts().collect();
        for (top, bot) in [(c[0], c[3]), (c[1], c[2])] {
            let (a, b) = (m.pos(top).unwrap(), m.pos(bot).unwrap());
            assert!((a.x - b.x).abs() < 1e-6 && (a.z - b.z).abs() < 1e-6, "twisted quad {nf}");
        }
    }
}

#[test]
fn bridge_rejects_bad_loops() {
    let (mut m, rim_a, rim_b) = two_tubes();
    // A loop cannot bridge to itself (shared vertices).
    assert!(bridge(&mut m, &rim_a, &rim_a.clone()).is_err());
    // An incomplete cycle is rejected.
    let mut broken = rim_b.clone();
    let dropped = *broken.iter().next().unwrap();
    broken.remove(&dropped);
    assert!(bridge(&mut m, &rim_a, &broken).is_err());
    // Interior edges are rejected.
    let mut boxed = prim_box(Vec3::splat(2.0)).unwrap();
    let interior: BTreeSet<EdgeKey> = [e(0, 1), e(1, 2), e(2, 3), e(0, 3)].into();
    assert!(bridge(&mut boxed, &interior, &interior.clone()).is_err());
}

#[test]
fn merge_top_of_box_into_pyramid() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let sel: BTreeSet<VertId> = [v(4), v(5), v(6), v(7)].into();
    let d = merge_verts(&mut m, &sel, 10.0).unwrap();
    assert_eq!(m.verts.len(), 5);
    assert_eq!(m.faces.len(), 5);
    // Cluster representative is the lowest id, at the cluster centroid.
    assert_eq!(d.moved_verts, vec![v(4)]);
    assert_eq!(m.pos(v(4)).unwrap(), Vec3::new(0.0, 1.0, 0.0));
    // The top quad degenerated and was dropped; the sides became triangles.
    assert_eq!(d.removed_faces, vec![f(1)]);
    assert_eq!(m.tri_count(), 6);
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
}

#[test]
fn merge_respects_distance() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let sel: BTreeSet<VertId> = m.verts.keys().copied().collect();
    let d = merge_verts(&mut m, &sel, 0.1).unwrap();
    assert!(d.moved_verts.is_empty() && d.removed_faces.is_empty());
    assert_eq!(m.verts.len(), 8);
    assert_eq!(m.faces.len(), 6);
    assert_clean(&m);
}

#[test]
fn dissolve_opens_the_box() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let d = dissolve(&mut m, &faces(&[1])).unwrap();
    assert_eq!(d.removed_faces, vec![f(1)]);
    assert!(d.removed_verts.is_empty(), "top verts still used by the sides");
    assert_eq!(m.faces.len(), 5);
    assert_eq!(m.verts.len(), 8);
    assert_eq!(m.boundary_edges().len(), 4);
    assert_clean(&m);
}

#[test]
fn dissolve_prunes_stranded_verts() {
    let mut m = plane(2.0, 2.0).unwrap();
    let d = dissolve(&mut m, &faces(&[0])).unwrap();
    assert_eq!(d.removed_verts.len(), 4);
    assert!(m.verts.is_empty() && m.faces.is_empty());
}

#[test]
fn mirror_half_box_into_full_box() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    // Shift onto x >= 0, drop the wall on the mirror plane, then mirror.
    let all: BTreeSet<VertId> = m.verts.keys().copied().collect();
    m.transform_verts(&all, &glam::Mat4::from_translation(Vec3::X));
    dissolve(&mut m, &faces(&[5])).unwrap();
    let d = mirror(&mut m, 0, 1e-3).unwrap();
    assert_eq!(d.created_verts.len(), 4);
    assert_eq!(d.created_faces.len(), 5);
    assert!(d.moved_verts.is_empty(), "on-plane verts were already exact");
    assert_eq!(m.verts.len(), 12);
    assert_eq!(m.faces.len(), 10);
    let (lo, hi) = m.bounds().unwrap();
    assert_eq!((lo, hi), (Vec3::new(-2.0, -1.0, -1.0), Vec3::new(2.0, 1.0, 1.0)));
    assert!(m.boundary_edges().is_empty(), "midline must weld shut");
    assert_clean(&m);
}

#[test]
fn mirror_snaps_near_plane_verts() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let all: BTreeSet<VertId> = m.verts.keys().copied().collect();
    m.transform_verts(&all, &glam::Mat4::from_translation(Vec3::X * 1.01));
    dissolve(&mut m, &faces(&[5])).unwrap();
    let d = mirror(&mut m, 0, 0.05).unwrap();
    assert_eq!(d.moved_verts.len(), 4);
    for &mv in &d.moved_verts {
        assert_eq!(m.pos(mv).unwrap().x, 0.0);
    }
    assert!(m.boundary_edges().is_empty());
    assert_clean(&m);
}

#[test]
fn mirror_rejects_bad_axis() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    assert!(mirror(&mut m, 3, 0.0).is_err());
}

#[test]
fn decimate_closed_cylinder_to_budget() {
    let mut m = cylinder(1.0, 2.0, 12, true).unwrap();
    assert_eq!(m.tri_count(), 44);
    let d = decimate_to_target(&mut m, 24).unwrap();
    assert!(m.tri_count() <= 24, "still at {} tris", m.tri_count());
    assert!(!d.removed_faces.is_empty());
    assert!(m.boundary_edges().is_empty(), "decimation must keep the surface closed");
    assert_clean(&m);
}

#[test]
fn decimate_is_a_noop_under_budget() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let d = decimate_to_target(&mut m, 12).unwrap();
    assert!(d.removed_faces.is_empty() && d.created_faces.is_empty());
    assert_eq!(m.faces.len(), 6);
}

#[test]
fn decimate_pins_boundary_loops() {
    // An open tube's rims are its silhouette; they must survive decimation.
    let mut m = cylinder(1.0, 2.0, 12, false).unwrap();
    assert_eq!(m.tri_count(), 24);
    let before = m.boundary_edges().len();
    let _ = decimate_to_target(&mut m, 16);
    // Whether or not the budget was reachable, every boundary vertex must
    // still lie exactly in one of the two rim planes (y = +-1).
    for e in m.boundary_edges() {
        for vert in [e.0, e.1] {
            let p = m.pos(vert).unwrap();
            assert!((p.y.abs() - 1.0).abs() < 1e-6, "rim vert drifted: {p}");
        }
    }
    assert!(m.boundary_edges().len() <= before);
    let findings = validate_mesh(&m);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn decimate_errors_when_target_unreachable() {
    // Rim verts are pinned, so an open tube can never decimate to zero.
    let mut m = cylinder(1.0, 2.0, 12, false).unwrap();
    assert!(decimate_to_target(&mut m, 0).is_err());
    // The failed attempt must leave the mesh untouched.
    assert_eq!(m.tri_count(), 24);
    assert_eq!(m.verts.len(), 24);
}

#[test]
fn lattice_uniform_displacement_translates() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let d = lattice(&mut m, [2, 2, 2], &[Vec3::new(0.5, 0.0, 0.0); 8]).unwrap();
    assert_eq!(d.moved_verts.len(), 8);
    let (lo, hi) = m.bounds().unwrap();
    assert_eq!((lo, hi), (Vec3::new(-0.5, -1.0, -1.0), Vec3::new(1.5, 1.0, 1.0)));
    assert_clean(&m);
}

#[test]
fn lattice_single_corner_pull_is_trilinear() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let mut disp = [Vec3::ZERO; 8];
    disp[7] = Vec3::new(0.0, 1.0, 0.0); // control point [1, 1, 1], idx 1 + 2 * (1 + 2 * 1)
    let d = lattice(&mut m, [2, 2, 2], &disp).unwrap();
    // Only the (+1, +1, +1) box corner has full weight on that control point.
    assert_eq!(d.moved_verts, vec![v(6)]);
    assert_eq!(m.pos(v(6)).unwrap(), Vec3::new(1.0, 2.0, 1.0));
    assert_eq!(m.pos(v(0)).unwrap(), Vec3::new(-1.0, -1.0, -1.0));
}

#[test]
fn lattice_rejects_bad_grid() {
    let mut m = prim_box(Vec3::splat(2.0)).unwrap();
    let err = lattice(&mut m, [2, 2, 2], &[Vec3::ZERO; 7]).unwrap_err();
    assert!(err.to_string().contains("expected 8"), "got: {err}");
    assert!(lattice(&mut m, [1, 2, 2], &[Vec3::ZERO; 4]).is_err());
}
