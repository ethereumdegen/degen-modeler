//! The closed op vocabulary. JSON wire: `{"op": "<snake_name>", ...params}`.
//! Unknown fields are rejected so agents get loud errors, not silent drift.

use std::collections::BTreeMap;

use dgm_atlas::AssetClass;
use dgm_mesh::{Finding, MeshDelta};
use serde::{Deserialize, Serialize};

use crate::doc::{AlphaMode, TextureRef};
use crate::select::{SelRef, SelectQuery};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn unit(self) -> glam::Vec3 {
        match self {
            Axis::X => glam::Vec3::X,
            Axis::Y => glam::Vec3::Y,
            Axis::Z => glam::Vec3::Z,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectKind {
    Planar { axis: Axis },
    Box,
    Cylindrical { axis: Axis },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaintMode {
    Add,
    #[default]
    Replace,
    Smooth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoneFit {
    pub head: [f32; 3],
    pub tail: [f32; 3],
}

fn dtrue() -> bool { true }
fn done() -> u32 { 1 }
fn dspeed() -> f32 { 1.0 }
fn dmargin() -> u32 { 4 }
fn dmerge() -> f32 { 1e-4 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    // ---- primitives ----
    PrimBox { object: String, size: [f32; 3] },
    PrimPlane { object: String, size: [f32; 2] },
    PrimCylinder { object: String, radius: f32, height: f32, segments: u32, #[serde(default = "dtrue")] caps: bool },
    /// Revolve a (radius, y) profile around +Y.
    PrimLathe { object: String, profile: Vec<[f32; 2]>, segments: u32, #[serde(default = "dtrue")] caps: bool },
    PrimNgonPrism { object: String, sides: u32, radius: f32, height: f32 },

    // ---- topology ----
    Extrude { sel: SelRef, offset: f32 },
    Inset { sel: SelRef, thickness: f32, #[serde(default)] depth: f32 },
    Bevel { sel: SelRef, width: f32, #[serde(default = "done")] segments: u32 },
    LoopCut { object: String, edge: [u32; 2], #[serde(default = "done")] cuts: u32 },
    Bridge { sel_a: SelRef, sel_b: SelRef },
    MergeVerts { sel: SelRef, distance: f32 },
    Dissolve { sel: SelRef },
    Mirror { object: String, axis: Axis, #[serde(default = "dmerge")] merge_distance: f32 },
    FlipNormals { sel: SelRef },
    /// Edge-collapse decimation to a triangle target (also used for LODs).
    DecimateToBudget { object: String, target_tris: u32 },

    // ---- deform ----
    Translate { sel: SelRef, delta: [f32; 3], #[serde(default)] proportional: Option<f32> },
    Rotate { sel: SelRef, axis: Axis, degrees: f32, #[serde(default)] origin: Option<[f32; 3]>, #[serde(default)] proportional: Option<f32> },
    Scale { sel: SelRef, factors: [f32; 3], #[serde(default)] origin: Option<[f32; 3]>, #[serde(default)] proportional: Option<f32> },
    Lattice { object: String, dims: [u32; 3], displacements: Vec<[f32; 3]> },
    SnapToGrid { sel: SelRef, step: f32 },

    // ---- selection ----
    SelectSave { name: String, query: SelectQuery },

    // ---- uv ----
    MarkSeams { sel: SelRef },
    ClearSeams { sel: SelRef },
    UvUnwrap { object: String },
    UvProject { sel: SelRef, #[serde(flatten)] kind: ProjectKind },
    /// Map the selection's UV islands into a named trim region.
    UvAssignTrim { sel: SelRef, sheet: String, region: String },
    UvDeclareMirror { name: String, sel: SelRef },
    UvSetTexelDensity { sel: SelRef, texels_per_meter: f32 },
    UvPack { object: String, #[serde(default = "dmargin")] margin_px: u32 },

    // ---- material ----
    MaterialNew { name: String, texture: TextureRef, #[serde(default)] alpha: AlphaMode, #[serde(default)] double_sided: bool },
    ObjectMaterial { object: String, material: String },

    // ---- rig (preset-first) ----
    RigApply { object: String, preset: String, #[serde(default)] fit: BTreeMap<String, BoneFit> },
    RigAutoWeights { object: String },
    RigPaintWeights { sel: SelRef, bone: String, value: f32, #[serde(default)] falloff: f32, #[serde(default)] mode: PaintMode },
    /// Extend a preset chain (tails, banners); parent must exist.
    RigAddBone { object: String, name: String, parent: String, head: [f32; 3], tail: [f32; 3] },

    // ---- anim (preset-first) ----
    ClipApply { name: String, object: String, template: String, #[serde(default = "dspeed")] speed: f32 },
    ClipKey { clip: String, bone: String, time: f32, #[serde(default)] rotation_euler_deg: Option<[f32; 3]>, #[serde(default)] translation: Option<[f32; 3]>, #[serde(default)] scale: Option<[f32; 3]> },
    ClipSetLoop { clip: String, looped: bool },
    ClipDelete { clip: String },

    // ---- scene ----
    ObjectDelete { object: String },
    ObjectRename { from: String, to: String },
    ObjectInstance { src: String, dst: String, #[serde(default)] translate: [f32; 3] },
    /// Generate `<object>_lod1..n` at the given triangle ratios.
    LodGenerate { object: String, ratios: Vec<f32> },
    SetGoal { goal: String },
    SetClass { class: AssetClass },
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::PrimBox { .. } => "prim_box",
            Op::PrimPlane { .. } => "prim_plane",
            Op::PrimCylinder { .. } => "prim_cylinder",
            Op::PrimLathe { .. } => "prim_lathe",
            Op::PrimNgonPrism { .. } => "prim_ngon_prism",
            Op::Extrude { .. } => "extrude",
            Op::Inset { .. } => "inset",
            Op::Bevel { .. } => "bevel",
            Op::LoopCut { .. } => "loop_cut",
            Op::Bridge { .. } => "bridge",
            Op::MergeVerts { .. } => "merge_verts",
            Op::Dissolve { .. } => "dissolve",
            Op::Mirror { .. } => "mirror",
            Op::FlipNormals { .. } => "flip_normals",
            Op::DecimateToBudget { .. } => "decimate_to_budget",
            Op::Translate { .. } => "translate",
            Op::Rotate { .. } => "rotate",
            Op::Scale { .. } => "scale",
            Op::Lattice { .. } => "lattice",
            Op::SnapToGrid { .. } => "snap_to_grid",
            Op::SelectSave { .. } => "select_save",
            Op::MarkSeams { .. } => "mark_seams",
            Op::ClearSeams { .. } => "clear_seams",
            Op::UvUnwrap { .. } => "uv_unwrap",
            Op::UvProject { .. } => "uv_project",
            Op::UvAssignTrim { .. } => "uv_assign_trim",
            Op::UvDeclareMirror { .. } => "uv_declare_mirror",
            Op::UvSetTexelDensity { .. } => "uv_set_texel_density",
            Op::UvPack { .. } => "uv_pack",
            Op::MaterialNew { .. } => "material_new",
            Op::ObjectMaterial { .. } => "object_material",
            Op::RigApply { .. } => "rig_apply",
            Op::RigAutoWeights { .. } => "rig_auto_weights",
            Op::RigPaintWeights { .. } => "rig_paint_weights",
            Op::RigAddBone { .. } => "rig_add_bone",
            Op::ClipApply { .. } => "clip_apply",
            Op::ClipKey { .. } => "clip_key",
            Op::ClipSetLoop { .. } => "clip_set_loop",
            Op::ClipDelete { .. } => "clip_delete",
            Op::ObjectDelete { .. } => "object_delete",
            Op::ObjectRename { .. } => "object_rename",
            Op::ObjectInstance { .. } => "object_instance",
            Op::LodGenerate { .. } => "lod_generate",
            Op::SetGoal { .. } => "set_goal",
            Op::SetClass { .. } => "set_class",
        }
    }
}

const DIFF_ELEM_CAP: usize = 64;

/// What an op did, in display element ids, capped for the wire.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Diff {
    pub summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modified: Vec<String>,
}

impl Diff {
    pub fn summary(text: impl Into<String>) -> Self {
        Self { summary: text.into(), ..Self::default() }
    }

    pub fn from_delta(object: &str, delta: &MeshDelta) -> Self {
        let cap = |v: Vec<String>| -> Vec<String> {
            if v.len() > DIFF_ELEM_CAP {
                let extra = v.len() - DIFF_ELEM_CAP;
                let mut v: Vec<String> = v.into_iter().take(DIFF_ELEM_CAP).collect();
                v.push(format!("(+{extra} more)"));
                v
            } else {
                v
            }
        };
        let created: Vec<String> = delta
            .created_verts
            .iter()
            .map(|v| v.to_string())
            .chain(delta.created_faces.iter().map(|f| f.to_string()))
            .collect();
        let removed: Vec<String> = delta
            .removed_verts
            .iter()
            .map(|v| v.to_string())
            .chain(delta.removed_faces.iter().map(|f| f.to_string()))
            .collect();
        let modified: Vec<String> = delta
            .moved_verts
            .iter()
            .map(|v| v.to_string())
            .chain(delta.uv_faces.iter().map(|f| f.to_string()))
            .collect();
        Self {
            summary: format!(
                "{object}: +{}v/+{}f, -{}v/-{}f, ~{} elems",
                delta.created_verts.len(),
                delta.created_faces.len(),
                delta.removed_verts.len(),
                delta.removed_faces.len(),
                delta.moved_verts.len() + delta.uv_faces.len(),
            ),
            created: cap(created),
            removed: cap(removed),
            modified: cap(modified),
        }
    }
}

/// The op result on the wire: new revision, what changed, advisory findings.
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub revision: u64,
    pub diff: Diff,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
}
