//! `--screenshot <path> --frames N`: render N frames, capture the viewport
//! and save it, exit 0 once the file is on disk (1 on timeout) — smoke proof.
//!
//! The capture reads a dedicated offscreen render target, not the window
//! surface: on macOS/Metal the swapchain texture is not COPY_SRC, so
//! window screenshots read back black. The shot camera mirrors the orbit
//! camera every frame, so the file shows exactly what the viewport shows
//! (egui panels excluded — they live on the window surface).

use std::path::PathBuf;

use bevy::asset::RenderAssetUsages;
use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use bevy::camera::RenderTarget;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureUsages,
};
use bevy::render::view::window::screenshot::{Screenshot, save_to_disk};

use crate::camera::OrbitCamera;

/// Frames to wait for the async capture before giving up.
const CAPTURE_TIMEOUT_FRAMES: u32 = 600;
const SHOT_SIZE: (u32, u32) = (1600, 900);

#[derive(Resource)]
pub struct ShotConfig {
    pub path: PathBuf,
    pub frames: u32,
    pub triggered_at: Option<u32>,
    pub target: Option<Handle<Image>>,
}

#[derive(Component)]
pub struct ShotCamera;

/// Startup (screenshot mode only): offscreen target + a camera into it.
pub fn setup_shot_target(
    mut cfg: ResMut<ShotConfig>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
    orbit: Res<OrbitCamera>,
) {
    let size = Extent3d { width: SHOT_SIZE.0, height: SHOT_SIZE.1, depth_or_array_layers: 1 };
    let mut image = Image::new_fill(
        size,
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
        | TextureUsages::COPY_DST
        | TextureUsages::COPY_SRC
        | TextureUsages::RENDER_ATTACHMENT;
    let handle = images.add(image);
    commands.spawn((
        Camera3d::default(),
        RenderTarget::Image(handle.clone().into()),
        orbit.transform(),
        ShotCamera,
    ));
    cfg.target = Some(handle);
}

/// Keep the shot camera glued to the orbit camera.
pub fn follow_orbit(
    orbit: Res<OrbitCamera>,
    mut cams: Query<&mut Transform, With<ShotCamera>>,
) {
    for mut t in &mut cams {
        *t = orbit.transform();
    }
}

pub fn screenshot_and_exit(
    mut cfg: ResMut<ShotConfig>,
    frames: Res<FrameCount>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    match cfg.triggered_at {
        None => {
            if frames.0 >= cfg.frames
                && let Some(target) = cfg.target.clone()
            {
                commands
                    .spawn(Screenshot::image(target))
                    .observe(save_to_disk(cfg.path.clone()));
                cfg.triggered_at = Some(frames.0);
            }
        }
        Some(at) => {
            // save_to_disk runs in an observer once the GPU readback lands;
            // poll for the file so we never exit before it is written.
            let written = std::fs::metadata(&cfg.path).map(|m| m.len() > 0).unwrap_or(false);
            if written {
                exit.write(AppExit::Success);
            } else if frames.0.saturating_sub(at) > CAPTURE_TIMEOUT_FRAMES {
                eprintln!(
                    "dgm-ui: screenshot was not written to {} within {} frames",
                    cfg.path.display(),
                    CAPTURE_TIMEOUT_FRAMES
                );
                exit.write(AppExit::error());
            }
        }
    }
}
