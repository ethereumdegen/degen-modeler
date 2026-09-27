use std::collections::{BTreeMap, BTreeSet};

use dgm_atlas::AssetClass;
use dgm_mesh::{FaceId, Mesh};
use serde::{Deserialize, Serialize};

use crate::anim::Clip;
use crate::error::OpError;
use crate::rig::Rig;
use crate::select::Selection;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TextureRef {
    /// A pack trim sheet; UVs land on its regions.
    Trim { sheet: String },
    /// A project-local texture file (e.g. imported from DMS/degen-paint).
    File { path: String },
    /// Flat colour, no texture (blockouts).
    Color { rgba: [u8; 4] },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaMode {
    #[default]
    Opaque,
    /// Alpha-tested cutout (foliage, rope).
    Mask,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Material {
    pub texture: TextureRef,
    #[serde(default)]
    pub alpha: AlphaMode,
    #[serde(default)]
    pub double_sided: bool,
    /// Linear RGB emission (crystals, lamps); exported as emissiveFactor +
    /// KHR_materials_emissive_strength.
    #[serde(default)]
    pub emissive: Option<[f32; 3]>,
    #[serde(default = "one")]
    pub emissive_strength: f32,
}

fn one() -> f32 {
    1.0
}

/// Faces whose UV islands intentionally overlap (mirroring); exempt from the
/// `uv.overlap` gate rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorSet {
    pub object: String,
    pub faces: BTreeSet<FaceId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Object {
    pub mesh: Mesh,
    #[serde(default)]
    pub material: Option<String>,
    #[serde(default)]
    pub rig: Option<Rig>,
    /// Set on generated LODs: the source object.
    #[serde(default)]
    pub lod_of: Option<String>,
    #[serde(default)]
    pub lod_level: u8,
}

impl Object {
    pub fn new(mesh: Mesh) -> Self {
        Self { mesh, material: None, rig: None, lod_of: None, lod_level: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Doc {
    pub revision: u64,
    #[serde(default)]
    pub goal: Option<String>,
    pub asset_class: AssetClass,
    pub objects: BTreeMap<String, Object>,
    pub materials: BTreeMap<String, Material>,
    pub selections: BTreeMap<String, Selection>,
    pub mirror_sets: BTreeMap<String, MirrorSet>,
    pub clips: BTreeMap<String, Clip>,
    /// Project-relative reference images (style targets); fed to
    /// critique/review and generation prompts.
    #[serde(default)]
    pub references: Vec<String>,
    /// Objects declared as intentionally touching/intersecting others
    /// (`contact`), or intentionally open shells (`open`).
    #[serde(default)]
    pub tags: BTreeMap<String, BTreeSet<String>>,
}

impl Doc {
    pub fn new(asset_class: AssetClass) -> Self {
        Self {
            revision: 0,
            goal: None,
            asset_class,
            objects: BTreeMap::new(),
            materials: BTreeMap::new(),
            selections: BTreeMap::new(),
            mirror_sets: BTreeMap::new(),
            clips: BTreeMap::new(),
            references: Vec::new(),
            tags: BTreeMap::new(),
        }
    }

    pub fn object(&self, name: &str) -> Result<&Object, OpError> {
        self.objects.get(name).ok_or_else(|| OpError::UnknownObject(name.into()))
    }

    pub fn object_mut(&mut self, name: &str) -> Result<&mut Object, OpError> {
        self.objects.get_mut(name).ok_or_else(|| OpError::UnknownObject(name.into()))
    }
}
