//! `--screenshot <path> --frames N`: render N frames, save the primary
//! window, exit 0 once the file is on disk (1 on timeout) — smoke proof.

use std::path::PathBuf;

use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use bevy::render::view::window::screenshot::{Screenshot, save_to_disk};

/// Frames to wait for the async capture before giving up.
const CAPTURE_TIMEOUT_FRAMES: u32 = 600;

#[derive(Resource)]
pub struct ShotConfig {
    pub path: PathBuf,
    pub frames: u32,
    pub triggered_at: Option<u32>,
}

pub fn screenshot_and_exit(
    mut cfg: ResMut<ShotConfig>,
    frames: Res<FrameCount>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    match cfg.triggered_at {
        None => {
            if frames.0 >= cfg.frames {
                commands
                    .spawn(Screenshot::primary_window())
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
