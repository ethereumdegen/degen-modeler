//! `dgm-view <file.glb|.gltf>` — the simple glTF viewer.
//!
//! Opens one window on one asset through Bevy's own glTF loader: what a
//! game engine will actually show, nothing else. Drag = orbit, scroll =
//! zoom, shift-drag = pan, space = toggle turntable spin (on by default).

use std::path::PathBuf;
use std::process::ExitCode;

use bevy::asset::AssetPlugin;
use bevy::camera::visibility::VisibilityRange;
use bevy::gltf::GltfAssetLabel;
use bevy::image::ImagePlugin;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::camera::primitives::Aabb;
use bevy::window::WindowPlugin;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: dgm-view <file.glb|.gltf>");
        return ExitCode::from(2);
    };
    let Ok(path) = path.canonicalize() else {
        eprintln!("dgm-view: no such file: {}", path.display());
        return ExitCode::from(2);
    };
    let (Some(dir), Some(name)) = (
        path.parent().map(|p| p.to_string_lossy().into_owned()),
        path.file_name().map(|n| n.to_string_lossy().into_owned()),
    ) else {
        eprintln!("dgm-view: not a file path: {}", path.display());
        return ExitCode::from(2);
    };

    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: format!("dgm-view — {name}"),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .set(AssetPlugin { file_path: dir, ..Default::default() })
                .set(ImagePlugin::default_nearest()),
        )
        .insert_resource(ClearColor(Color::srgb(0.11, 0.12, 0.15)))
        .insert_resource(bevy::light::GlobalAmbientLight {
            brightness: 400.0,
            ..Default::default()
        })
        .insert_resource(Orbit::default())
        .insert_resource(AssetName(name))
        .add_systems(Startup, setup)
        .add_systems(Update, (frame_when_loaded, orbit_input, apply_camera).chain())
        .run();
    ExitCode::SUCCESS
}

#[derive(Resource)]
struct AssetName(String);

#[derive(Resource)]
struct Orbit {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    spin: bool,
    framed: bool,
}

impl Default for Orbit {
    fn default() -> Self {
        Self { target: Vec3::ZERO, yaw: 0.5, pitch: -0.35, distance: 6.0, spin: true, framed: false }
    }
}

#[derive(Component)]
struct Viewed;

fn setup(mut commands: Commands, asset_server: Res<AssetServer>, name: Res<AssetName>) {
    let scene = asset_server.load(GltfAssetLabel::Scene(0).from_asset(name.0.clone()));
    commands.spawn((bevy::world_serialization::WorldAssetRoot(scene), Viewed));
    commands.spawn(Camera3d::default());
    // Only matters for lit fallback materials; our exports are unlit.
    commands.spawn((
        DirectionalLight { illuminance: 6000.0, ..Default::default() },
        Transform::from_xyz(3.0, 6.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Once meshes exist, frame the union of their AABBs — then leave the user's
/// camera alone.
fn frame_when_loaded(
    mut orbit: ResMut<Orbit>,
    meshes: Query<(&GlobalTransform, &Aabb), (Without<Camera3d>, Without<VisibilityRange>)>,
) {
    if orbit.framed {
        return;
    }
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for (gt, aabb) in &meshes {
        for corner in [-1.0f32, 1.0] {
            for cy in [-1.0f32, 1.0] {
                for cz in [-1.0f32, 1.0] {
                    let local = Vec3::from(aabb.center)
                        + Vec3::from(aabb.half_extents) * Vec3::new(corner, cy, cz);
                    let p = gt.transform_point(local);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
        }
    }
    if lo.x.is_finite() {
        orbit.target = (lo + hi) * 0.5;
        orbit.distance = (hi - lo).length().max(0.5) * 1.4;
        orbit.framed = true;
    }
}

fn orbit_input(
    mut orbit: ResMut<Orbit>,
    time: Res<Time>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
) {
    if keys.just_pressed(KeyCode::Space) {
        orbit.spin = !orbit.spin;
    }
    if buttons.pressed(MouseButton::Left) {
        let d = motion.delta;
        if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            let pan = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0)
                * Vec3::new(-d.x, d.y, 0.0)
                * (orbit.distance * 0.0015);
            orbit.target += pan;
        } else {
            orbit.yaw -= d.x * 0.008;
            orbit.pitch = (orbit.pitch - d.y * 0.008).clamp(-1.5, 1.5);
            orbit.spin = false;
        }
    }
    orbit.distance = (orbit.distance * (1.0 - scroll.delta.y * 0.1)).clamp(0.2, 200.0);
    if orbit.spin {
        orbit.yaw += time.delta_secs() * 0.5;
    }
}

fn apply_camera(orbit: Res<Orbit>, mut cams: Query<&mut Transform, With<Camera3d>>) {
    let rot = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
    for mut t in &mut cams {
        *t = Transform::from_translation(orbit.target + rot * (Vec3::Z * orbit.distance))
            .looking_at(orbit.target, Vec3::Y);
    }
}
