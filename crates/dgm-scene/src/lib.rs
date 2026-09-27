//! The scene document, the op vocabulary, and the append-only ledger.
//!
//! Everything an agent or the UI does is an [`op::Op`] applied through
//! [`project::Project::apply`]: validated, budget-gated, appended to
//! `ops.jsonl`, revision bumped. Replaying the ledger reproduces the doc
//! exactly (and therefore a byte-identical export).

pub mod anim;
pub mod anim_ops;
pub mod bake_ops;
pub mod digest;
pub mod dispatch;
pub mod doc;
pub mod error;
pub mod gate;
pub mod ledger;
pub mod op;
pub mod project;
pub mod rig;
pub mod rig_ops;
pub mod select;

pub use anim::{Channel, Clip};
pub use doc::{AlphaMode, Doc, Material, MirrorSet, Object, TextureRef};
pub use error::OpError;
pub use gate::{GateReport, budget_findings};
pub use op::{Axis, Diff, Op, Outcome, Projection};
pub use rig::{Bone, Rig};
pub use select::{ElemKind, Elems, SelRef, SelectQuery, Selection};
