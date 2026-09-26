//! Orbit camera (drag rotate, scroll zoom, shift-drag pan) + click picking.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::input::EguiWantsInput;

use crate::state::{PickState, UiProject};
use crate::viewport;

#[derive(Resource)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl OrbitCamera {
    /// Frame the given doc bounds with a 3/4 view.
    pub fn framing(bounds: Option<(glam::Vec3, glam::Vec3)>) -> Self {
        let (target, distance) = match bounds {
            Some((lo, hi)) => {
                let center = (lo + hi) * 0.5;
                let extent = (hi - lo).length().max(0.5);
                (Vec3::from_array(center.to_array()), extent * 1.8)
            }
            None => (Vec3::ZERO, 4.0),
        };
        Self { target, yaw: 0.6, pitch: -0.5, distance }
    }

    fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }

    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.target + self.rotation() * (Vec3::Z * self.distance))
            .looking_at(self.target, Vec3::Y)
    }
}

pub fn spawn_camera(mut commands: Commands, orbit: Res<OrbitCamera>) {
    commands.spawn((Camera3d::default(), orbit.transform()));
    // Doc materials are unlit; this only lights the engine-preview glTF's
    // lit-fallback materials so the second scene reads.
    commands.spawn((
        DirectionalLight { illuminance: 6_000.0, ..Default::default() },
        Transform::from_xyz(3.0, 6.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

pub fn orbit_camera(
    mut orbit: ResMut<OrbitCamera>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    egui_wants: Res<EguiWantsInput>,
    mut camera: Single<&mut Transform, With<Camera3d>>,
) {
    if !egui_wants.wants_any_input() {
        let dragging =
            buttons.pressed(MouseButton::Left) || buttons.pressed(MouseButton::Right);
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let delta = motion.delta;
        if dragging && delta != Vec2::ZERO {
            if shift {
                let rot = orbit.rotation();
                let pan = (rot * Vec3::X * -delta.x + rot * Vec3::Y * delta.y)
                    * (orbit.distance * 0.0015);
                orbit.target += pan;
            } else {
                orbit.yaw -= delta.x * 0.008;
                orbit.pitch = (orbit.pitch - delta.y * 0.008).clamp(-1.54, 1.54);
            }
        }
        let steps = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y,
            MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
        };
        if steps != 0.0 {
            orbit.distance = (orbit.distance * (1.0 - steps * 0.1).clamp(0.5, 1.5))
                .clamp(0.05, 500.0);
        }
    }
    **camera = orbit.transform();
}

/// Click (press+release with <4px travel) -> CPU raycast -> element readout.
pub fn pick_element(
    buttons: Res<ButtonInput<MouseButton>>,
    egui_wants: Res<EguiWantsInput>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    up: Res<UiProject>,
    mut pick: ResMut<PickState>,
) {
    if buttons.just_pressed(MouseButton::Left) && !egui_wants.is_pointer_over_area() {
        pick.press = window.cursor_position();
    }
    if !buttons.just_released(MouseButton::Left) {
        return;
    }
    let Some(start) = pick.press.take() else { return };
    if egui_wants.is_pointer_over_area() {
        return;
    }
    let Some(cursor) = window.cursor_position() else { return };
    if (cursor - start).length() > 4.0 {
        return; // that was an orbit drag
    }
    let (camera, camera_transform) = *camera;
    let Ok(ray) = camera.viewport_to_world(camera_transform, cursor) else { return };
    let origin = glam::Vec3::from_array(ray.origin.to_array());
    let dir = glam::Vec3::from_array(ray.direction.to_array());
    pick.text = Some(match viewport::pick(&up.project.doc, origin, dir) {
        Some(hit) => format!("{} {} {}", hit.object, hit.face, hit.vert),
        None => "nothing".into(),
    });
}
