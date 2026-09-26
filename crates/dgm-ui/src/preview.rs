//! Engine preview: export the doc through dgm-gltf, drop the GLB in
//! `exports/preview.glb`, and spawn it through Bevy's own glTF loader
//! beside the live meshes — what the engine will actually show.

use bevy::gltf::GltfAssetLabel;
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;
use dgm_gltf::{ExportOptions, export_glb};

use crate::state::{Preview, UiProject};
use crate::viewport::doc_bounds;

pub const PREVIEW_ASSET_PATH: &str = "exports/preview.glb";

pub fn handle_preview(
    mut preview: ResMut<Preview>,
    up: Res<UiProject>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    if !preview.requested {
        return;
    }
    preview.requested = false;
    let project = &up.project;
    let opts = ExportOptions { base_dir: project.root.clone(), embed_report: None };
    let bytes = match export_glb(&project.doc, &project.pack, &opts) {
        Ok(bytes) => bytes,
        Err(e) => {
            preview.status = Some(format!("export failed: {e}"));
            return;
        }
    };
    let dir = project.exports_dir();
    if let Err(e) =
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join("preview.glb"), &bytes))
    {
        preview.status = Some(format!("write failed: {e}"));
        return;
    }

    // Offset the preview beside the live meshes by the scene width.
    let dx = doc_bounds(&project.doc)
        .map(|(lo, hi)| (hi.x - lo.x).max(0.5))
        .unwrap_or(1.0)
        * 1.1;

    // The asset path is stable, so force a reload to pick up the new bytes.
    asset_server.reload(PREVIEW_ASSET_PATH);
    let scene = asset_server.load(GltfAssetLabel::Scene(0).from_asset(PREVIEW_ASSET_PATH));
    if let Some(entity) = preview.entity.take() {
        commands.entity(entity).despawn();
    }
    preview.entity =
        Some(commands.spawn((WorldAssetRoot(scene), Transform::from_xyz(dx, 0.0, 0.0))).id());
    preview.status = Some(format!("preview.glb · {} bytes", bytes.len()));
}
