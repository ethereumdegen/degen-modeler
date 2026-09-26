//! Semantic selections. An op targets either a named saved selection or an
//! inline query; nothing on the wire is ever a pixel.

use std::collections::BTreeSet;

use dgm_mesh::{EdgeKey, FaceId, Mesh, VertId};
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::doc::Doc;
use crate::error::OpError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElemKind {
    Verts,
    Faces,
    Edges,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "ids", rename_all = "snake_case")]
pub enum Elems {
    Verts(BTreeSet<VertId>),
    Faces(BTreeSet<FaceId>),
    Edges(BTreeSet<EdgeKey>),
}

impl Elems {
    pub fn len(&self) -> usize {
        match self {
            Elems::Verts(s) => s.len(),
            Elems::Faces(s) => s.len(),
            Elems::Edges(s) => s.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn kind(&self) -> ElemKind {
        match self {
            Elems::Verts(_) => ElemKind::Verts,
            Elems::Faces(_) => ElemKind::Faces,
            Elems::Edges(_) => ElemKind::Edges,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Selection {
    pub object: String,
    pub elems: Elems,
}

fn default_facing_angle() -> f32 {
    45.0
}
fn default_kind_faces() -> ElemKind {
    ElemKind::Faces
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "q", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectQuery {
    All { object: String, #[serde(default = "default_kind_faces")] kind: ElemKind },
    Verts { object: String, ids: Vec<u32> },
    Faces { object: String, ids: Vec<u32> },
    Edges { object: String, pairs: Vec<[u32; 2]> },
    /// Faces whose normal is within `max_angle_deg` of `dir`.
    FacesFacing { object: String, dir: [f32; 3], #[serde(default = "default_facing_angle")] max_angle_deg: f32 },
    /// Edges bordering exactly one face.
    BoundaryLoop { object: String },
    /// Faces connected to `face` without crossing a UV seam.
    IslandOf { object: String, face: u32 },
    /// Faces of the single object bound to `material`.
    ByMaterial { material: String },
    /// Elements fully inside the axis-aligned box.
    InBox { object: String, min: [f32; 3], max: [f32; 3], #[serde(default = "default_kind_faces")] kind: ElemKind },
}

/// A selection reference: a saved name, or an inline query.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SelRef {
    Named(String),
    Query(SelectQuery),
}

pub fn resolve(doc: &Doc, sel: &SelRef) -> Result<Selection, OpError> {
    match sel {
        SelRef::Named(name) => doc
            .selections
            .get(name)
            .cloned()
            .ok_or_else(|| OpError::UnknownSelection(name.clone())),
        SelRef::Query(q) => resolve_query(doc, q),
    }
}

pub fn resolve_query(doc: &Doc, q: &SelectQuery) -> Result<Selection, OpError> {
    let sel = match q {
        SelectQuery::All { object, kind } => {
            let mesh = &doc.object(object)?.mesh;
            let elems = match kind {
                ElemKind::Verts => Elems::Verts(mesh.verts.keys().copied().collect()),
                ElemKind::Faces => Elems::Faces(mesh.faces.keys().copied().collect()),
                ElemKind::Edges => Elems::Edges(mesh.edges()),
            };
            Selection { object: object.clone(), elems }
        }
        SelectQuery::Verts { object, ids } => {
            let mesh = &doc.object(object)?.mesh;
            let mut set = BTreeSet::new();
            for &id in ids {
                let v = VertId(id);
                mesh.pos(v)?;
                set.insert(v);
            }
            Selection { object: object.clone(), elems: Elems::Verts(set) }
        }
        SelectQuery::Faces { object, ids } => {
            let mesh = &doc.object(object)?.mesh;
            let mut set = BTreeSet::new();
            for &id in ids {
                let f = FaceId(id);
                mesh.face(f)?;
                set.insert(f);
            }
            Selection { object: object.clone(), elems: Elems::Faces(set) }
        }
        SelectQuery::Edges { object, pairs } => {
            let mesh = &doc.object(object)?.mesh;
            let all = mesh.edges();
            let mut set = BTreeSet::new();
            for &[a, b] in pairs {
                let e = EdgeKey::new(VertId(a), VertId(b));
                if !all.contains(&e) {
                    return Err(OpError::BadParams(format!("no edge {e}")));
                }
                set.insert(e);
            }
            Selection { object: object.clone(), elems: Elems::Edges(set) }
        }
        SelectQuery::FacesFacing { object, dir, max_angle_deg } => {
            let mesh = &doc.object(object)?.mesh;
            let dir = Vec3::from(*dir).normalize_or_zero();
            if dir == Vec3::ZERO {
                return Err(OpError::BadParams("faces_facing: zero direction".into()));
            }
            let cos = max_angle_deg.to_radians().cos();
            let set = mesh
                .faces
                .keys()
                .copied()
                .filter(|&f| mesh.face_normal(f).map(|n| n.dot(dir) >= cos).unwrap_or(false))
                .collect();
            Selection { object: object.clone(), elems: Elems::Faces(set) }
        }
        SelectQuery::BoundaryLoop { object } => {
            let mesh = &doc.object(object)?.mesh;
            Selection {
                object: object.clone(),
                elems: Elems::Edges(mesh.boundary_edges().into_iter().collect()),
            }
        }
        SelectQuery::IslandOf { object, face } => {
            let mesh = &doc.object(object)?.mesh;
            let start = FaceId(*face);
            mesh.face(start)?;
            let edge_faces = mesh.edge_faces();
            let mut seen: BTreeSet<FaceId> = BTreeSet::from([start]);
            let mut stack = vec![start];
            while let Some(f) = stack.pop() {
                for e in mesh.face(f)?.edges() {
                    if mesh.seams.contains(&e) {
                        continue;
                    }
                    if let Some(neighbours) = edge_faces.get(&e) {
                        for &nf in neighbours {
                            if seen.insert(nf) {
                                stack.push(nf);
                            }
                        }
                    }
                }
            }
            Selection { object: object.clone(), elems: Elems::Faces(seen) }
        }
        SelectQuery::ByMaterial { material } => {
            if !doc.materials.contains_key(material) {
                return Err(OpError::UnknownMaterial(material.clone()));
            }
            let mut bound: Vec<&String> = doc
                .objects
                .iter()
                .filter(|(_, o)| o.material.as_deref() == Some(material))
                .map(|(n, _)| n)
                .collect();
            match (bound.pop(), bound.is_empty()) {
                (Some(object), true) => {
                    let mesh = &doc.object(object)?.mesh;
                    Selection {
                        object: object.clone(),
                        elems: Elems::Faces(mesh.faces.keys().copied().collect()),
                    }
                }
                (Some(_), false) => {
                    return Err(OpError::BadParams(format!(
                        "by_material: material `{material}` is bound to several objects; select per object"
                    )));
                }
                (None, _) => {
                    return Err(OpError::BadParams(format!(
                        "by_material: material `{material}` is bound to no object"
                    )));
                }
            }
        }
        SelectQuery::InBox { object, min, max, kind } => {
            let mesh = &doc.object(object)?.mesh;
            let (lo, hi) = (Vec3::from(*min), Vec3::from(*max));
            let inside = |p: Vec3| p.cmpge(lo).all() && p.cmple(hi).all();
            let verts: BTreeSet<VertId> = mesh
                .verts
                .iter()
                .filter(|&(_, &p)| inside(p))
                .map(|(&v, _)| v)
                .collect();
            let elems = match kind {
                ElemKind::Verts => Elems::Verts(verts),
                ElemKind::Faces => Elems::Faces(
                    mesh.faces
                        .iter()
                        .filter(|(_, f)| f.verts().all(|v| verts.contains(&v)))
                        .map(|(&f, _)| f)
                        .collect(),
                ),
                ElemKind::Edges => Elems::Edges(
                    mesh.edges()
                        .into_iter()
                        .filter(|e| verts.contains(&e.0) && verts.contains(&e.1))
                        .collect(),
                ),
            };
            Selection { object: object.clone(), elems }
        }
    };
    Ok(sel)
}

/// Any selection, as vertices (faces/edges dissolve to their vertices).
pub fn to_verts(mesh: &Mesh, sel: &Selection) -> Result<BTreeSet<VertId>, OpError> {
    Ok(match &sel.elems {
        Elems::Verts(s) => s.clone(),
        Elems::Faces(s) => {
            let mut out = BTreeSet::new();
            for &f in s {
                out.extend(mesh.face(f)?.verts());
            }
            out
        }
        Elems::Edges(s) => s.iter().flat_map(|e| [e.0, e.1]).collect(),
    })
}

/// Faces only; anything else is a kind error naming what it got.
pub fn to_faces(sel: &Selection) -> Result<BTreeSet<FaceId>, OpError> {
    match &sel.elems {
        Elems::Faces(s) => Ok(s.clone()),
        other => Err(OpError::SelectionKind(format!(
            "need faces, got {:?}",
            other.kind()
        ))),
    }
}

/// Edges only.
pub fn to_edges(sel: &Selection) -> Result<BTreeSet<EdgeKey>, OpError> {
    match &sel.elems {
        Elems::Edges(s) => Ok(s.clone()),
        other => Err(OpError::SelectionKind(format!(
            "need edges, got {:?}",
            other.kind()
        ))),
    }
}
