//! Degen Modeler viewer: `dgm-ui <project-dir> [--screenshot out.png --frames N]`.
//!
//! Loads the project directly (same crate APIs as the CLI), shows the doc as
//! unlit Bevy meshes with bevy_egui panels, polls `ops.jsonl` every second so
//! an agent run shows live, and can smoke-prove itself with `--screenshot`.

mod anim;
mod camera;
mod feed;
mod panels;
mod playback;
mod preview;
mod shot;
mod state;
mod sync;
mod viewport;

use std::path::PathBuf;
use std::process::ExitCode;

use bevy::asset::AssetPlugin;
use bevy::image::ImagePlugin;
use bevy::prelude::*;
use bevy::window::WindowPlugin;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use dgm_scene::project::Project;

struct Args {
    root: PathBuf,
    screenshot: Option<PathBuf>,
    frames: u32,
    preview: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut root = None;
    let mut screenshot = None;
    let mut frames = 30u32;
    let mut preview = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--screenshot" => {
                let path = it.next().ok_or("--screenshot needs a path")?;
                screenshot = Some(PathBuf::from(path));
            }
            "--frames" => {
                let n = it.next().ok_or("--frames needs a number")?;
                frames = n.parse().map_err(|_| format!("bad --frames value `{n}`"))?;
            }
            "--preview" => preview = true,
            "--help" | "-h" => {
                return Err(
                    "usage: dgm-ui <project-dir> [--screenshot out.png --frames N] [--preview]"
                        .into(),
                );
            }
            other if root.is_none() && !other.starts_with('-') => {
                root = Some(PathBuf::from(other));
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(Args {
        root: root
            .ok_or("usage: dgm-ui <project-dir> [--screenshot out.png --frames N] [--preview]")?,
        screenshot,
        frames,
        preview,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let project = match Project::load(&args.root) {
        Ok(project) => project,
        Err(e) => {
            eprintln!("dgm-ui: {e}");
            return ExitCode::FAILURE;
        }
    };
    // A stale screenshot must never pass the smoke check.
    if let Some(path) = &args.screenshot {
        let _ = std::fs::remove_file(path);
    }

    let title = format!("degen modeler — {}", project.meta.name);
    // Asset root = project root: pack textures and exports/preview.glb load
    // by relative path, and AssetServer::reload works after re-export.
    let asset_root = project.root.to_string_lossy().into_owned();
    let orbit = camera::OrbitCamera::framing(viewport::doc_bounds(&project.doc));

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window { title, ..Default::default() }),
                ..Default::default()
            })
            .set(AssetPlugin { file_path: asset_root, ..Default::default() })
            .set(ImagePlugin::default_nearest()),
    )
    .add_plugins(EguiPlugin::default())
    .insert_resource(ClearColor(Color::srgb(0.13, 0.14, 0.16)))
    .insert_resource(state::UiProject::new(project))
    .insert_resource(orbit)
    .init_resource::<state::Views>()
    .init_resource::<state::DigestCache>()
    .init_resource::<state::Playback>()
    .insert_resource(state::Preview { requested: args.preview, ..Default::default() })
    .init_resource::<state::Console>()
    .init_resource::<state::Ticker>()
    .init_resource::<state::Review>()
    .init_resource::<state::PickState>()
    .add_systems(Startup, camera::spawn_camera)
    .add_systems(
        Update,
        (
            (sync::poll_ledger, sync::rebuild_meshes).chain(),
            camera::orbit_camera,
            camera::pick_element,
            playback::advance_playback,
            preview::handle_preview,
        ),
    )
    .add_systems(EguiPrimaryContextPass, panels::panels);

    if let Some(path) = args.screenshot {
        app.insert_resource(shot::ShotConfig {
            path,
            frames: args.frames,
            triggered_at: None,
            target: None,
        })
        .add_systems(Startup, shot::setup_shot_target)
        .add_systems(Update, (shot::follow_orbit, shot::screenshot_and_exit));
    }

    match app.run() {
        AppExit::Success => ExitCode::SUCCESS,
        AppExit::Error(code) => ExitCode::from(code.get()),
    }
}
