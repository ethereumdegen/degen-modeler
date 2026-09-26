//! UV toolkit: projections, unwrap, trim binding, texel density, packing,
//! and the `uv.*` gate rules. Owned by the UvAtlas slice; see CONTRACT.md.
//!
//! Conventions
//! - UV origin is top-left (image space, v down), matching glTF.
//! - [`project`] writes world-scaled UVs: 1 UV unit == 1 meter, until
//!   [`set_texel_density`] or [`pack_islands`] rescales them.
//! - A UV island is a set of faces connected across shared non-seam edges —
//!   the same flood fill as `dgm-scene`'s `IslandOf` selection query.
//! - Everything iterates `BTreeMap`/`BTreeSet`; all output order is
//!   deterministic.
//!
//! Dependency direction: `dgm-scene` depends on this crate, never the other
//! way around. Two consequences for integrators:
//! - [`ProjectKind`]/[`Axis`] mirror `dgm_scene::{ProjectKind, Axis}`
//!   field-for-field with the identical serde shape; dispatch converts (or
//!   round-trips through serde) trivially.
//! - [`findings::uv_findings`] takes plain per-object inputs instead of a
//!   `Doc`. Build a [`TexInfo`] per object from its material —
//!   `Trim { sheet }` -> `TexInfo { px: max sheet edge, trim: true }`,
//!   `File { .. }` -> `TexInfo { px: image edge, trim: false }`,
//!   `Color { .. }` or no material -> `None` — and pass the union of the
//!   object's declared `MirrorSet` faces as `mirror_faces[object]`.
//!   (`dgm_atlas::usage::region_usage` follows the same pattern.)

pub mod density;
pub mod error;
pub mod findings;
pub mod islands;
pub mod packing;
pub mod project;
pub mod trim;
pub mod unwrap;

mod geom;

pub use density::set_texel_density;
pub use error::UvError;
pub use findings::{TexInfo, uv_findings};
pub use islands::{island_of, islands};
pub use packing::pack_islands;
pub use project::{Axis, ProjectKind, project};
pub use trim::assign_trim;
pub use unwrap::unwrap;
