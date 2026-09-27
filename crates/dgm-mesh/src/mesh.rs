use std::collections::{BTreeMap, BTreeSet};

use glam::{Mat4, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::ids::{EdgeKey, FaceId, VertId};

/// One face corner: a vertex reference plus its UV for this face.
///
/// UVs live on corners, not vertices, so seams and trim-sheet reuse work
/// without duplicating geometry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Corner {
    pub vert: VertId,
    #[serde(default)]
    pub uv: Vec2,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Face {
    pub corners: Vec<Corner>,
}

impl Face {
    pub fn verts(&self) -> impl Iterator<Item = VertId> + '_ {
        self.corners.iter().map(|c| c.vert)
    }

    /// The face's edges in corner order (wrapping), as normalized keys.
    pub fn edges(&self) -> Vec<EdgeKey> {
        let n = self.corners.len();
        (0..n)
            .map(|i| EdgeKey::new(self.corners[i].vert, self.corners[(i + 1) % n].vert))
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MeshError {
    #[error("unknown vertex {0}")]
    UnknownVert(VertId),
    #[error("unknown face {0}")]
    UnknownFace(FaceId),
    #[error("a face needs at least 3 vertices")]
    FaceTooSmall,
    #[error("face repeats vertex {0}")]
    RepeatedVert(VertId),
    #[error("{0}")]
    Invalid(String),
}

/// One triangle of a triangulated face: indexes into the face's corners.
#[derive(Debug, Clone, Copy)]
pub struct Tri {
    pub face: FaceId,
    pub corner_idx: [u16; 3],
}

/// What a topology op did, for diffs and attribution.
#[derive(Debug, Default, Clone)]
pub struct MeshDelta {
    pub created_verts: Vec<VertId>,
    pub created_faces: Vec<FaceId>,
    pub removed_verts: Vec<VertId>,
    pub removed_faces: Vec<FaceId>,
    pub moved_verts: Vec<VertId>,
    /// Faces whose UVs changed.
    pub uv_faces: Vec<FaceId>,
}

impl MeshDelta {
    pub fn merge(&mut self, other: MeshDelta) {
        self.created_verts.extend(other.created_verts);
        self.created_faces.extend(other.created_faces);
        self.removed_verts.extend(other.removed_verts);
        self.removed_faces.extend(other.removed_faces);
        self.moved_verts.extend(other.moved_verts);
        self.uv_faces.extend(other.uv_faces);
    }
}

/// Indexed polygon mesh with stable ids and per-corner UVs.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Mesh {
    next_vert: u32,
    next_face: u32,
    pub verts: BTreeMap<VertId, Vec3>,
    pub faces: BTreeMap<FaceId, Face>,
    /// UV seam edges (marked by ops, respected by unwrap/islands).
    pub seams: BTreeSet<EdgeKey>,
}

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_vert(&mut self, pos: Vec3) -> VertId {
        let id = VertId(self.next_vert);
        self.next_vert += 1;
        self.verts.insert(id, pos);
        id
    }

    pub fn add_face(&mut self, verts: &[VertId]) -> Result<FaceId, MeshError> {
        self.add_face_uv(
            verts
                .iter()
                .map(|&v| Corner { vert: v, uv: Vec2::ZERO })
                .collect(),
        )
    }

    pub fn add_face_uv(&mut self, corners: Vec<Corner>) -> Result<FaceId, MeshError> {
        if corners.len() < 3 {
            return Err(MeshError::FaceTooSmall);
        }
        let mut seen = BTreeSet::new();
        for c in &corners {
            if !self.verts.contains_key(&c.vert) {
                return Err(MeshError::UnknownVert(c.vert));
            }
            if !seen.insert(c.vert) {
                return Err(MeshError::RepeatedVert(c.vert));
            }
        }
        let id = FaceId(self.next_face);
        self.next_face += 1;
        self.faces.insert(id, Face { corners });
        Ok(id)
    }

    pub fn pos(&self, v: VertId) -> Result<Vec3, MeshError> {
        self.verts.get(&v).copied().ok_or(MeshError::UnknownVert(v))
    }

    pub fn set_pos(&mut self, v: VertId, pos: Vec3) -> Result<(), MeshError> {
        *self.verts.get_mut(&v).ok_or(MeshError::UnknownVert(v))? = pos;
        Ok(())
    }

    pub fn face(&self, f: FaceId) -> Result<&Face, MeshError> {
        self.faces.get(&f).ok_or(MeshError::UnknownFace(f))
    }

    pub fn face_mut(&mut self, f: FaceId) -> Result<&mut Face, MeshError> {
        self.faces.get_mut(&f).ok_or(MeshError::UnknownFace(f))
    }

    pub fn remove_face(&mut self, f: FaceId) -> Result<Face, MeshError> {
        self.faces.remove(&f).ok_or(MeshError::UnknownFace(f))
    }

    /// Drop vertices no face references. Returns what was removed.
    pub fn remove_unused_verts(&mut self) -> Vec<VertId> {
        let used: BTreeSet<VertId> = self.faces.values().flat_map(|f| f.verts()).collect();
        let unused: Vec<VertId> = self.verts.keys().copied().filter(|v| !used.contains(v)).collect();
        for v in &unused {
            self.verts.remove(v);
        }
        unused
    }

    /// Drop seam marks whose edge no longer exists.
    pub fn prune_seams(&mut self) {
        let edges = self.edges();
        self.seams.retain(|e| edges.contains(e));
    }

    pub fn edges(&self) -> BTreeSet<EdgeKey> {
        self.faces.values().flat_map(|f| f.edges()).collect()
    }

    pub fn edge_faces(&self) -> BTreeMap<EdgeKey, Vec<FaceId>> {
        let mut map: BTreeMap<EdgeKey, Vec<FaceId>> = BTreeMap::new();
        for (&fid, face) in &self.faces {
            for e in face.edges() {
                map.entry(e).or_default().push(fid);
            }
        }
        map
    }

    pub fn vert_faces(&self) -> BTreeMap<VertId, Vec<FaceId>> {
        let mut map: BTreeMap<VertId, Vec<FaceId>> = BTreeMap::new();
        for (&fid, face) in &self.faces {
            for v in face.verts() {
                map.entry(v).or_default().push(fid);
            }
        }
        map
    }

    /// Edges bordering exactly one face.
    pub fn boundary_edges(&self) -> Vec<EdgeKey> {
        self.edge_faces()
            .into_iter()
            .filter(|(_, fs)| fs.len() == 1)
            .map(|(e, _)| e)
            .collect()
    }

    /// UV islands: faces connected across shared non-seam edges, in
    /// deterministic (lowest-face-id) order. The canonical flood fill the
    /// selection queries and the UV toolkit mirror.
    pub fn uv_islands(&self) -> Vec<BTreeSet<FaceId>> {
        let edge_faces = self.edge_faces();
        let mut unvisited: BTreeSet<FaceId> = self.faces.keys().copied().collect();
        let mut islands = Vec::new();
        while let Some(&start) = unvisited.iter().next() {
            let mut island = BTreeSet::new();
            let mut stack = vec![start];
            unvisited.remove(&start);
            while let Some(f) = stack.pop() {
                island.insert(f);
                for e in self.faces[&f].edges() {
                    if self.seams.contains(&e) {
                        continue;
                    }
                    if let Some(neighbours) = edge_faces.get(&e) {
                        for nf in neighbours {
                            if unvisited.remove(nf) {
                                stack.push(*nf);
                            }
                        }
                    }
                }
            }
            islands.push(island);
        }
        islands
    }

    /// Newell's method; robust for non-convex planar-ish ngons.
    pub fn face_normal(&self, f: FaceId) -> Result<Vec3, MeshError> {
        let face = self.face(f)?;
        let mut n = Vec3::ZERO;
        let k = face.corners.len();
        for i in 0..k {
            let a = self.pos(face.corners[i].vert)?;
            let b = self.pos(face.corners[(i + 1) % k].vert)?;
            n += Vec3::new(
                (a.y - b.y) * (a.z + b.z),
                (a.z - b.z) * (a.x + b.x),
                (a.x - b.x) * (a.y + b.y),
            );
        }
        Ok(n.normalize_or_zero())
    }

    pub fn face_area(&self, f: FaceId) -> Result<f32, MeshError> {
        let face = self.face(f)?;
        let mut n = Vec3::ZERO;
        let k = face.corners.len();
        for i in 0..k {
            let a = self.pos(face.corners[i].vert)?;
            let b = self.pos(face.corners[(i + 1) % k].vert)?;
            n += a.cross(b);
        }
        Ok(n.length() * 0.5)
    }

    pub fn face_centroid(&self, f: FaceId) -> Result<Vec3, MeshError> {
        let face = self.face(f)?;
        let mut c = Vec3::ZERO;
        for corner in &face.corners {
            c += self.pos(corner.vert)?;
        }
        Ok(c / face.corners.len() as f32)
    }

    /// Triangle count after fan triangulation (what the budget counts).
    pub fn tri_count(&self) -> u32 {
        self.faces.values().map(|f| f.corners.len() as u32 - 2).sum()
    }

    /// Deterministic fan triangulation, face id order.
    pub fn triangulate(&self) -> Vec<Tri> {
        let mut tris = Vec::with_capacity(self.tri_count() as usize);
        for (&fid, face) in &self.faces {
            for i in 1..face.corners.len() as u16 - 1 {
                tris.push(Tri { face: fid, corner_idx: [0, i, i + 1] });
            }
        }
        tris
    }

    /// Per-corner normals with a hard-edge angle threshold (degrees):
    /// a neighbouring face contributes to a corner's normal only when its
    /// normal is within the threshold of this face's normal.
    pub fn corner_normals(&self, hard_angle_deg: f32) -> BTreeMap<(FaceId, u16), Vec3> {
        let cos = hard_angle_deg.to_radians().cos();
        let normals: BTreeMap<FaceId, Vec3> = self
            .faces
            .keys()
            .map(|&f| (f, self.face_normal(f).unwrap_or(Vec3::Y)))
            .collect();
        let vert_faces = self.vert_faces();
        let mut out = BTreeMap::new();
        for (&fid, face) in &self.faces {
            let n0 = normals[&fid];
            for (i, corner) in face.corners.iter().enumerate() {
                let mut acc = Vec3::ZERO;
                if let Some(neighbours) = vert_faces.get(&corner.vert) {
                    for nf in neighbours {
                        let n = normals[nf];
                        if n.dot(n0) >= cos {
                            acc += n;
                        }
                    }
                }
                let n = acc.normalize_or(n0);
                out.insert((fid, i as u16), n);
            }
        }
        out
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.verts.values();
        let first = *it.next()?;
        let (mut lo, mut hi) = (first, first);
        for &p in it {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        Some((lo, hi))
    }

    pub fn transform_verts(&mut self, verts: &BTreeSet<VertId>, m: &Mat4) {
        for v in verts {
            if let Some(p) = self.verts.get_mut(v) {
                *p = m.transform_point3(*p);
            }
        }
    }
}
