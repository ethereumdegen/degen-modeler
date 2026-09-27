use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    #[default]
    Prop,
    Weapon,
    Building,
    Character,
    /// Walkable spaces: caves, rooms, ruins — one connected surface.
    Environment,
}

impl fmt::Display for AssetClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            AssetClass::Prop => "prop",
            AssetClass::Weapon => "weapon",
            AssetClass::Building => "building",
            AssetClass::Character => "character",
            AssetClass::Environment => "environment",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Budget {
    pub max_tris: u32,
}

/// Texel-density band in texels per meter.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Band {
    pub min: f32,
    pub max: f32,
}

/// Pixel rectangle inside a trim sheet.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrimSheet {
    /// PNG path relative to the pack dir.
    pub file: String,
    pub size: [u32; 2],
    pub regions: BTreeMap<String, Rect>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetBone {
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
    /// Normalized space: the rigged mesh is fitted into a unit box, feet at
    /// y=0, height 1; `rig_apply` scales these into the mesh bounds.
    pub head: [f32; 3],
    pub tail: [f32; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RigPreset {
    pub bones: Vec<PresetBone>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateChannel {
    pub times: Vec<f32>,
    #[serde(default)]
    pub rotations_euler_deg: Option<Vec<[f32; 3]>>,
    #[serde(default)]
    pub translations: Option<Vec<[f32; 3]>>,
    #[serde(default)]
    pub scales: Option<Vec<[f32; 3]>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipTemplate {
    /// Which rig preset this template targets.
    pub rig: String,
    pub looped: bool,
    pub duration: f32,
    /// Bone name -> normalized curves. Translations are in fractions of the
    /// fitted rig height; `clip_apply` scales them.
    pub channels: BTreeMap<String, TemplateChannel>,
}

fn default_hard_edge() -> f32 {
    40.0
}
fn default_uv_waste_max() -> f32 {
    0.15
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackManifest {
    pub name: String,
    pub version: String,
    pub budgets: BTreeMap<AssetClass, Budget>,
    pub texel_density: Band,
    /// Allowed texture edge sizes (e.g. [256, 512]).
    pub texture_sizes: Vec<u32>,
    /// Hex colours ("#a86f3d") the style leans on; palette-distance metric.
    pub palette: Vec<String>,
    #[serde(default = "default_hard_edge")]
    pub hard_edge_angle_deg: f32,
    /// Max wasted fraction of an owned atlas (gate `uv.waste`).
    #[serde(default = "default_uv_waste_max")]
    pub uv_waste_max: f32,
    #[serde(default)]
    pub trims: BTreeMap<String, TrimSheet>,
    #[serde(default)]
    pub rigs: BTreeMap<String, RigPreset>,
    #[serde(default)]
    pub clips: BTreeMap<String, ClipTemplate>,
    /// Reference board image (style head grounding), relative path.
    #[serde(default)]
    pub reference_board: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("pack manifest {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("pack manifest {0}: {1}")]
    Json(PathBuf, serde_json::Error),
    #[error("{0}")]
    Trim(String),
}

/// Install `src` (a PNG) into the pack at `pack_dir` as the trim sheet `name`:
/// copied to `trims/<name>.png` and registered in `pack.json` with one
/// full-sheet region `region`. This is how an externally made texture — a
/// degen-paint reference-generated tile — becomes a *tiling* material
/// (trims wrap and stack by design; owned File textures must stay in
/// [0,1]). The manifest is edited as JSON so fields this build does not
/// know survive. An existing sheet is refused unless `force`.
/// Returns the sheet's pixel size.
pub fn install_trim(
    pack_dir: &Path,
    name: &str,
    region: &str,
    src: &Path,
    force: bool,
) -> Result<[u32; 2], PackError> {
    let valid = |s: &str| {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    if !valid(name) || !valid(region) {
        return Err(PackError::Trim(format!(
            "trim and region names must be [A-Za-z0-9_-]+, got `{name}` / `{region}`"
        )));
    }
    let bytes = std::fs::read(src).map_err(|e| PackError::Io(src.to_path_buf(), e))?;
    let size = png_size(&bytes)
        .ok_or_else(|| PackError::Trim(format!("{} is not a PNG", src.display())))?;

    let manifest_path = pack_dir.join("pack.json");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| PackError::Io(manifest_path.clone(), e))?;
    let mut manifest: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| PackError::Json(manifest_path.clone(), e))?;
    let trims = manifest
        .as_object_mut()
        .ok_or_else(|| PackError::Trim("pack.json is not an object".into()))?
        .entry("trims")
        .or_insert_with(|| serde_json::json!({}));
    let trims = trims
        .as_object_mut()
        .ok_or_else(|| PackError::Trim("pack.json `trims` is not an object".into()))?;
    if trims.contains_key(name) && !force {
        return Err(PackError::Trim(format!(
            "trim `{name}` already exists in {}; pass --force to replace it",
            manifest_path.display()
        )));
    }

    let rel = format!("trims/{name}.png");
    let dst = pack_dir.join(&rel);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| PackError::Io(parent.to_path_buf(), e))?;
    }
    std::fs::write(&dst, &bytes).map_err(|e| PackError::Io(dst.clone(), e))?;
    trims.insert(
        name.to_owned(),
        serde_json::json!({
            "file": rel,
            "size": size,
            "regions": { region: { "x": 0, "y": 0, "w": size[0], "h": size[1] } },
        }),
    );
    let out = serde_json::to_string_pretty(&manifest).expect("json value serializes");
    std::fs::write(&manifest_path, out).map_err(|e| PackError::Io(manifest_path.clone(), e))?;
    // Prove the result still loads as a pack before reporting success.
    Pack::load(pack_dir)?;
    Ok(size)
}

/// Width/height from a PNG's IHDR chunk.
fn png_size(bytes: &[u8]) -> Option<[u32; 2]> {
    if bytes.len() < 24 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some([w, h])
}

#[derive(Debug, Clone)]
pub struct Pack {
    pub root: PathBuf,
    pub manifest: PackManifest,
}

impl Pack {
    pub fn load(root: &Path) -> Result<Self, PackError> {
        let path = root.join("pack.json");
        let text = std::fs::read_to_string(&path).map_err(|e| PackError::Io(path.clone(), e))?;
        let manifest: PackManifest =
            serde_json::from_str(&text).map_err(|e| PackError::Json(path, e))?;
        Ok(Self { root: root.to_path_buf(), manifest })
    }

    pub fn budget(&self, class: AssetClass) -> Option<&Budget> {
        self.manifest.budgets.get(&class)
    }

    pub fn texture_path(&self, sheet: &str) -> Option<PathBuf> {
        self.manifest.trims.get(sheet).map(|t| self.root.join(&t.file))
    }

    /// Region rect in UV space of its sheet: (u0, v0, u1, v1), v down as in
    /// image space; the glTF exporter flips nothing (glTF UV origin is
    /// top-left too).
    pub fn region_uv(&self, sheet: &str, region: &str) -> Option<[f32; 4]> {
        let t = self.manifest.trims.get(sheet)?;
        let r = t.regions.get(region)?;
        let (w, h) = (t.size[0] as f32, t.size[1] as f32);
        Some([r.x as f32 / w, r.y as f32 / h, (r.x + r.w) as f32 / w, (r.y + r.h) as f32 / h])
    }

    pub fn palette_rgb(&self) -> Vec<[u8; 3]> {
        self.manifest
            .palette
            .iter()
            .filter_map(|hex| {
                let hex = hex.strip_prefix('#')?;
                if hex.len() != 6 {
                    return None;
                }
                let n = u32::from_str_radix(hex, 16).ok()?;
                Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
            })
            .collect()
    }
}
