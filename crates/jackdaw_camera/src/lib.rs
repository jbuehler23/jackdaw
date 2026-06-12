use bevy::prelude::*;

pub struct JackdawCameraPlugin;

impl Plugin for JackdawCameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraNavInput>()
            .add_systems(Update, camera_system);
    }
}

/// Per-frame navigation input, populated by the host application.
///
/// The camera controller reads only this resource, so the editor can drive it
/// from rebindable input actions while other hosts feed it directly.
///
/// The editor populates this in `PreUpdate` (after `EnhancedInputSystems::Update`)
/// via `populate_camera_nav_input` in `src/input_contexts.rs`. The camera
/// system runs in `Update`, so it always sees the freshly written frame value.
#[derive(Resource, Default, Clone)]
pub struct CameraNavInput {
    /// Fly mode engaged (the editor binds this to RMB hold via a code-level
    /// `Down` condition; see `src/input_contexts.rs`).
    pub fly_active: bool,
    /// Pointer delta this frame while navigating.
    pub look_delta: Vec2,
    /// Wheel movement this frame (positive = zoom in / forward).
    pub zoom_ticks: f32,
    /// Normalized movement intent in camera-local axes while flying
    /// (x = right, y = up, z = forward). The host resolves WASD/QE from raw
    /// keyboard because those keys are not yet BEI-managed this pass.
    pub move_axes: Vec3,
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
    // Shift raw read is intentionally kept: it is a speed modifier that
    // compounds with `move_axes` but is not a navigation trigger, so it
    // does not need to be preset-bindable this pass.
    keyboard: Res<ButtonInput<KeyCode>>,
    nav: Res<CameraNavInput>,
    time: Res<Time>,
    mut camera_query: Query<(
        &mut JackdawCameraSettings,
        &mut Transform,
        Option<&mut Projection>,
    )>,
) {
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let dt = time.delta_secs();

    for (mut settings, mut transform, projection) in &mut camera_query {
        if !settings.enabled {
            continue;
        }

        let is_ortho = projection
            .as_ref()
            .is_some_and(|p| matches!(p.as_ref(), Projection::Orthographic(_)));

        // Mouse look (only while fly mode is active; disabled in ortho
        // so axis-locked views stay aligned).
        if nav.fly_active && !is_ortho && nav.look_delta != Vec2::ZERO {
            let (mut yaw, mut pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
            yaw -= nav.look_delta.x * settings.sensitivity;
            pitch -= nav.look_delta.y * settings.sensitivity;
            pitch = pitch.clamp(
                -std::f32::consts::FRAC_PI_2 + 0.01,
                std::f32::consts::FRAC_PI_2 - 0.01,
            );
            transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        }

        // Scroll zoom. Ctrl+Alt (grid size shortcut) and Shift (brush/grid
        // resize) skip zoom so those chords are not stolen.
        let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
        let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
        if (!ctrl || !alt) && !shift && nav.zoom_ticks != 0.0 {
            let mut projection = projection;
            if nav.fly_active {
                // Right-click + scroll: adjust speed.
                settings.speed = (settings.speed * (1.0 + nav.zoom_ticks * 0.1)).clamp(0.5, 100.0);
            } else if let Some(proj) = projection.as_deref_mut()
                && let Projection::Orthographic(ortho) = proj
            {
                // Ortho zoom: smaller scale = closer view.
                ortho.scale = (ortho.scale * (1.0 - nav.zoom_ticks * 0.1)).clamp(0.05, 1000.0);
            } else {
                // Perspective: dolly along view direction.
                let forward = transform.forward().as_vec3();
                transform.translation += forward * nav.zoom_ticks * settings.scroll_speed;
            }
        }

        // Fly movement. `move_axes` is in camera-local space (x=right, y=up,
        // z=forward); the host resolves WASD/QE from raw keyboard this pass.
        if nav.move_axes != Vec3::ZERO {
            let movement = nav.move_axes.x * transform.right().as_vec3()
                + nav.move_axes.y * Vec3::Y
                + nav.move_axes.z * transform.forward().as_vec3();
            if movement != Vec3::ZERO {
                let speed_mult = if shift { settings.run_multiplier } else { 1.0 };
                transform.translation += movement.normalize() * settings.speed * speed_mult * dt;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_nav_input_default_is_inactive() {
        let nav = CameraNavInput::default();
        assert!(!nav.fly_active);
        assert_eq!(nav.look_delta, Vec2::ZERO);
        assert_eq!(nav.zoom_ticks, 0.0);
        assert_eq!(nav.move_axes, Vec3::ZERO);
    }
}
