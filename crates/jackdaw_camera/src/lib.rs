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
/// - Right-click + WASD: move forward/back/left/right (view-relative)
/// - Right-click + Q / E: move up / down (world-space Y)
/// - Scroll wheel: move forward/back along view direction
/// - Right-click + scroll: adjust camera speed
/// - Shift (held): run speed multiplier while flying
/// - Middle-click + drag: orbit around a pivot point
/// - Shift + Middle-click + drag: pan the camera
/// - Alt + Left-click + drag: orbit (alternative binding)
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
    /// Distance from camera to orbit pivot point (units). Updated by
    /// scroll-wheel dolly so the orbit centre tracks the user's
    /// implicit focus distance.
    pub orbit_distance: f32,
    /// Pan sensitivity multiplier (units per pixel).
    pub pan_sensitivity: f32,
}

impl Default for JackdawCameraSettings {
    fn default() -> Self {
        Self {
            sensitivity: 0.003,
            speed: 5.0,
            run_multiplier: 2.0,
            enabled: true,
            scroll_speed: 1.0,
            orbit_distance: 10.0,
            pan_sensitivity: 0.01,
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
    let middle_held = mouse.pressed(MouseButton::Middle);
    let left_held = mouse.pressed(MouseButton::Left);
    let dt = time.delta_secs();

    for (mut settings, mut transform, projection) in &mut camera_query {
        if !settings.enabled {
            continue;
        }

        let is_ortho = projection
            .as_ref()
            .is_some_and(|p| matches!(p.as_ref(), Projection::Orthographic(_)));

        // --- Orbit (MMB drag, or Alt+LMB drag) -----------------------
        // Rotates the camera around a pivot point `orbit_distance`
        // units ahead along the current view direction.  Disabled in
        // ortho mode so axis-locked views stay aligned.
        let orbiting = middle_held && !shift && !right_held;
        let alt_orbiting = alt && left_held && !right_held && !middle_held;
        if (orbiting || alt_orbiting) && !is_ortho && mouse_delta != Vec2::ZERO {
            let pivot =
                transform.translation + transform.forward().as_vec3() * settings.orbit_distance;

            let (mut yaw, mut pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
            yaw -= mouse_delta.x * settings.sensitivity;
            pitch -= mouse_delta.y * settings.sensitivity;
            pitch = pitch.clamp(
                -std::f32::consts::FRAC_PI_2 + 0.01,
                std::f32::consts::FRAC_PI_2 - 0.01,
            );
            transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);

            // Reposition so the camera stays orbit_distance away from
            // the pivot while facing the new direction.
            transform.translation = pivot - transform.forward().as_vec3() * settings.orbit_distance;
        }

        // --- Pan (Shift+MMB drag) ------------------------------------
        // Translates the camera along its local right and up axes.
        // Works in both perspective and ortho.
        let panning = middle_held && shift && !right_held;
        if panning && mouse_delta != Vec2::ZERO {
            let pan_scale = if is_ortho {
                // In ortho the view extent doesn't change with
                // translation, so scale pan speed by the ortho scale
                // so movement feels proportional to the visible area.
                projection
                    .as_ref()
                    .and_then(|p| match p.as_ref() {
                        Projection::Orthographic(o) => Some(o.scale),
                        _ => None,
                    })
                    .unwrap_or(1.0)
            } else {
                // In perspective, scale pan speed by orbit_distance so
                // closer objects feel snappier and far ones don't
                // under-pan.
                settings.orbit_distance
            };
            let right = transform.right().as_vec3();
            let up = transform.up().as_vec3();
            let offset = (-right * mouse_delta.x + up * mouse_delta.y)
                * settings.pan_sensitivity
                * pan_scale;
            transform.translation += offset;
        }

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
                    // Keep orbit_distance in sync with scroll dolly so
                    // the orbit pivot tracks the user's implicit focus.
                    settings.orbit_distance =
                        (settings.orbit_distance - delta * settings.scroll_speed).clamp(0.5, 500.0);
                }
            }
        }

        // Fly only while RMB is held. The camera-movement bindings default
        // to RMB chords, but holding RMB is enforced here at the system level
        // so a stale or hand-edited keybind file that maps plain WASDQE can
        // never let the camera fly without RMB, which would clash with the
        // Q/W/E/R tool shortcuts.
        let mut movement = Vec3::ZERO;

        if right_held {
            if keybinds.key_chord_pressed(EditorAction::CameraForward, &keyboard, &mouse) {
                movement += transform.forward().as_vec3();
            }
            if keybinds.key_chord_pressed(EditorAction::CameraBackward, &keyboard, &mouse) {
                movement -= transform.forward().as_vec3();
            }
            if keybinds.key_chord_pressed(EditorAction::CameraLeft, &keyboard, &mouse) {
                movement -= transform.right().as_vec3();
            }
            if keybinds.key_chord_pressed(EditorAction::CameraRight, &keyboard, &mouse) {
                movement += transform.right().as_vec3();
            }
            if keybinds.key_chord_pressed(EditorAction::CameraUp, &keyboard, &mouse) {
                movement += Vec3::Y;
            }
            if keybinds.key_chord_pressed(EditorAction::CameraDown, &keyboard, &mouse) {
                movement -= Vec3::Y;
            }
        }

        if movement != Vec3::ZERO {
            let speed_mult = if shift { settings.run_multiplier } else { 1.0 };
            transform.translation += movement.normalize() * settings.speed * speed_mult * dt;
        }
    }
}

#[cfg(test)]
mod fly_chord_tests {
    use super::*;
    use bevy::input::ButtonInput;
    use jackdaw_commands::keybinds::{EditorAction, KeybindRegistry};

    fn keyboard_with(keys: &[KeyCode]) -> ButtonInput<KeyCode> {
        let mut k = ButtonInput::<KeyCode>::default();
        for &c in keys {
            k.press(c);
        }
        k
    }
    fn mouse_with(btns: &[MouseButton]) -> ButtonInput<MouseButton> {
        let mut m = ButtonInput::<MouseButton>::default();
        for &b in btns {
            m.press(b);
        }
        m
    }

    #[test]
    fn forward_chord_requires_rmb() {
        let kb = KeybindRegistry::default();
        let keyboard = keyboard_with(&[KeyCode::KeyW]);
        let no_mouse = mouse_with(&[]);
        let rmb = mouse_with(&[MouseButton::Right]);

        assert!(!kb.key_chord_pressed(EditorAction::CameraForward, &keyboard, &no_mouse));
        assert!(kb.key_chord_pressed(EditorAction::CameraForward, &keyboard, &rmb));
    }

    #[test]
    fn forward_chord_composes_with_shift_for_run_speed() {
        let kb = KeybindRegistry::default();
        let keyboard = keyboard_with(&[KeyCode::KeyW, KeyCode::ShiftLeft]);
        let rmb = mouse_with(&[MouseButton::Right]);
        assert!(kb.key_chord_pressed(EditorAction::CameraForward, &keyboard, &rmb));
    }
}
