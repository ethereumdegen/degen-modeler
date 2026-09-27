//! Style packs. A pack is data: budgets, palette, trim sheets with named
//! regions, rig presets and clip templates. `Pack::load` reads
//! `<dir>/pack.json`; file references inside are relative to the pack dir.

pub mod pack;
pub mod usage;

pub use pack::{
    AssetClass, Band, Budget, ClipTemplate, Pack, PackError, PackManifest, PresetBone, Rect, install_trim,
    RigPreset, TemplateChannel, TrimSheet,
};
