use bevy::{
    input::mouse::{MouseMotion, MouseScrollUnit, MouseWheel},
    prelude::*,
};
use jackdaw_commands::keybinds::{EditorAction, KeybindRegistry};

pub struct JackdawCameraPlugin;

impl Plugin for JackdawCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, camera_system);
    }
}

/// Settings component placed on the camera entity to enable fly-camera controls.
///
/// Controls:
/// - Right-click + drag: look around (yaw/pitch)
/// - WASD: move forward/back/left/right (view-relative)
/// - Q / E: move up / down (world-space Y)
/// - Scroll wheel: move forward/back along view direction
/// - Right-click + scroll: adjust camera speed
/// - Shift (held): run speed multiplier
#[derive(Component)]
pub struct JackdawCameraSettings {
    /// Mouse look sensitivity (radians per pixel).
    pub sensitivity: f32,
    /// Base movement speed (units per second).
    pub speed: f32,
    /// Speed multiplier when Shift is held.
    pub run_multiplier: f32,
    /// Whether camera controls are enabled. Set to false during UI focus, etc.
    pub enabled: bool,
    /// Scroll movement speed (units per scroll line).
    pub scroll_speed: f32,
}

impl Default for JackdawCameraSettings {
    fn default() -> Self {
        Self {
            sensitivity: 0.003,
            speed: 5.0,
            run_multiplier: 2.0,
            enabled: true,
            scroll_speed: 1.0,
        }
    }
}

fn camera_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    keybinds: Res<KeybindRegistry>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mut scroll_events: MessageReader<MouseWheel>,
    time: Res<Time>,
    mut camera_query: Query<(
        &mut JackdawCameraSettings,
        &mut Transform,
        Option<&mut Projection>,
    )>,
) {
    // Drain motion / scroll events once up-front. Multi-viewport: this
    // system iterates every camera, so reading inside the loop would
    // let the first-iterated camera consume the events (whether it was
    // enabled or not) and starve later cameras. We collect once and
    // then distribute to whichever cameras are currently enabled, of
    // which there is normally exactly one (the hovered viewport's).
    let mut mouse_delta = Vec2::ZERO;
    for motion in mouse_motion.read() {
        mouse_delta += motion.delta;
    }
    let scroll_deltas: Vec<f32> = scroll_events
        .read()
        .map(|event| match event.unit {
            MouseScrollUnit::Line => event.y,
            MouseScrollUnit::Pixel => event.y * 0.01,
        })
        .collect();

    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let right_held = mouse.pressed(MouseButton::Right);
    let dt = time.delta_secs();

    for (mut settings, mut transform, projection) in &mut camera_query {
        if !settings.enabled {
            continue;
        }

        let is_ortho = projection
            .as_ref()
            .is_some_and(|p| matches!(p.as_ref(), Projection::Orthographic(_)));

        // Mouse look (only while right-click held; disabled in ortho
        // so axis-locked views stay aligned).
        if right_held && !is_ortho && mouse_delta != Vec2::ZERO {
            let (mut yaw, mut pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
            yaw -= mouse_delta.x * settings.sensitivity;
            pitch -= mouse_delta.y * settings.sensitivity;
            pitch = pitch.clamp(
                -std::f32::consts::FRAC_PI_2 + 0.01,
                std::f32::consts::FRAC_PI_2 - 0.01,
            );
            transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        }

        // Scroll wheel: skip when Ctrl+Alt held (grid size shortcut) or Shift held (brush/grid resize)
        if (!ctrl || !alt) && !shift {
            let mut projection = projection;
            for &delta in &scroll_deltas {
                if right_held {
                    // Right-click + scroll: adjust speed
                    settings.speed = (settings.speed * (1.0 + delta * 0.1)).clamp(0.5, 100.0);
                } else if let Some(proj) = projection.as_deref_mut()
                    && let Projection::Orthographic(ortho) = proj
                {
                    // Ortho zoom: smaller scale = closer view.
                    ortho.scale = (ortho.scale * (1.0 - delta * 0.1)).clamp(0.05, 1000.0);
                } else {
                    // Perspective: dolly along view direction.
                    let forward = transform.forward().as_vec3();
                    transform.translation += forward * delta * settings.scroll_speed;
                }
            }
        }

        // WASD + QE movement (independent of right-click, but skip when Ctrl/Alt held for shortcuts)
        let mut movement = Vec3::ZERO;

        if !ctrl && !alt && keybinds.key_pressed(EditorAction::CameraForward, &keyboard) {
            movement += transform.forward().as_vec3();
        }
        if !ctrl && !alt && keybinds.key_pressed(EditorAction::CameraBackward, &keyboard) {
            movement -= transform.forward().as_vec3();
        }
        if !ctrl && !alt && keybinds.key_pressed(EditorAction::CameraLeft, &keyboard) {
            movement -= transform.right().as_vec3();
        }
        if !ctrl && !alt && keybinds.key_pressed(EditorAction::CameraRight, &keyboard) {
            movement += transform.right().as_vec3();
        }
        if !ctrl && !alt && keybinds.key_pressed(EditorAction::CameraUp, &keyboard) {
            movement += Vec3::Y;
        }
        if !ctrl && !alt && keybinds.key_pressed(EditorAction::CameraDown, &keyboard) {
            movement -= Vec3::Y;
        }

        if movement != Vec3::ZERO {
            let speed_mult = if shift { settings.run_multiplier } else { 1.0 };
            transform.translation += movement.normalize() * settings.speed * speed_mult * dt;
        }
    }
}
