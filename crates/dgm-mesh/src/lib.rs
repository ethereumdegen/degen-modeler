//! Low-poly indexed mesh kernel for Degen Modeler.
//!
//! Invariants the whole workspace leans on:
//! - element ids are stable for the life of a mesh and never reused;
//! - every collection iterates deterministically (`BTreeMap`/`BTreeSet`);
//! - all math is `f32`; no randomness, no time.

pub mod ids;
pub mod mesh;
pub mod ops;
pub mod primitives;
pub mod validate;

pub use ids::{EdgeKey, FaceId, VertId};
pub use mesh::{Corner, Face, Mesh, MeshDelta, MeshError, Tri};
pub use validate::{Finding, Severity, validate_mesh};
