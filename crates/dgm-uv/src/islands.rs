//! UV islands: faces connected across shared non-seam edges.

use std::collections::{BTreeMap, BTreeSet};

use dgm_mesh::{EdgeKey, FaceId, Mesh};

/// All UV islands, ordered by their smallest face id.
pub fn islands(mesh: &Mesh) -> Vec<BTreeSet<FaceId>> {
    let all: BTreeSet<FaceId> = mesh.faces.keys().copied().collect();
    islands_within(mesh, &all)
}

/// The island containing `face`; empty when the face doesn't exist.
pub fn island_of(mesh: &Mesh, face: FaceId) -> BTreeSet<FaceId> {
    if !mesh.faces.contains_key(&face) {
        return BTreeSet::new();
    }
    let all: BTreeSet<FaceId> = mesh.faces.keys().copied().collect();
    flood(mesh, &mesh.edge_faces(), face, &all)
}

/// Islands of the sub-mesh induced by `faces` (the flood fill never leaves
/// the set), ordered by smallest face id. Callers validate the ids.
pub(crate) fn islands_within(mesh: &Mesh, faces: &BTreeSet<FaceId>) -> Vec<BTreeSet<FaceId>> {
    let edge_faces = mesh.edge_faces();
    let mut seen: BTreeSet<FaceId> = BTreeSet::new();
    let mut out = Vec::new();
    for &start in faces {
        if seen.contains(&start) {
            continue;
        }
        let island = flood(mesh, &edge_faces, start, faces);
        seen.extend(island.iter().copied());
        out.push(island);
    }
    out
}

/// Mirrors `dgm-scene`'s `IslandOf` selection flood fill.
fn flood(
    mesh: &Mesh,
    edge_faces: &BTreeMap<EdgeKey, Vec<FaceId>>,
    start: FaceId,
    allowed: &BTreeSet<FaceId>,
) -> BTreeSet<FaceId> {
    let mut seen = BTreeSet::from([start]);
    let mut stack = vec![start];
    while let Some(f) = stack.pop() {
        let Some(face) = mesh.faces.get(&f) else { continue };
        for e in face.edges() {
            if mesh.seams.contains(&e) {
                continue;
            }
            if let Some(neighbours) = edge_faces.get(&e) {
                for &nf in neighbours {
                    if allowed.contains(&nf) && seen.insert(nf) {
                        stack.push(nf);
                    }
                }
            }
        }
    }
    seen
}
