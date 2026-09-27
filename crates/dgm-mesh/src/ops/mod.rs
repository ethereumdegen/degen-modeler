//! Topology and deform operations over [`crate::Mesh`].
//!
//! Contract: every op takes `&mut Mesh` plus semantic targets, returns
//! `Result<MeshDelta, MeshError>`, never leaves the mesh in a state that
//! `add_face` would have refused, and is deterministic.

pub mod deform;
pub mod organic;
pub mod topo;
