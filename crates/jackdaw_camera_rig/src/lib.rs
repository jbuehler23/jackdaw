//! Authorable camera rig for Jackdaw scenes. Add a mode component
//! (`ThirdPersonCamera` or `FirstPersonCamera`) to a camera prefab entity in the
//! editor; at runtime `JackdawCameraRigPlugin` materializes a `Camera3d` on it and
//! drives it toward the single `CameraTarget`-tagged entity (e.g. the local player).
//! The mode is denoted by WHICH component is present. `CameraTarget` is added by the
//! game at runtime, not authored.
use bevy::prelude::*;
use bevy::reflect::std_traits::ReflectDefault;
use jackdaw_jsn::EditorCategory;

/// Third-person mode: orbit behind/above the target. Authored on the camera prefab.
#[derive(Component, Reflect, Clone, Copy, PartialEq, Debug)]
#[reflect(Component, Default, @EditorCategory::new("Camera"))]
pub struct ThirdPersonCamera {
    /// Distance from the focus point, in metres.
    pub distance: f32,
    /// Initial downward pitch, in radians (look-down angle).
    pub pitch: f32,
    /// Mouse-look sensitivity (radians per pixel of motion).
    pub sensitivity: f32,
}
impl Default for ThirdPersonCamera {
    fn default() -> Self {
        Self {
            distance: 10.0,
            pitch: 0.4,
            sensitivity: 0.005,
        }
    }
}

/// First-person mode: sit at the target's eye, look with the mouse. Authored on the camera prefab.
#[derive(Component, Reflect, Clone, Copy, PartialEq, Debug)]
#[reflect(Component, Default, @EditorCategory::new("Camera"))]
pub struct FirstPersonCamera {
    /// Eye height above the target origin, in metres.
    pub eye_height: f32,
    /// Mouse-look sensitivity (radians per pixel of motion).
    pub sensitivity: f32,
}
impl Default for FirstPersonCamera {
    fn default() -> Self {
        Self {
            eye_height: 1.6,
            sensitivity: 0.005,
        }
    }
}

/// Marker for the entity the active camera follows (the game adds this to the local player).
#[derive(Component, Reflect, Clone, Copy, PartialEq, Debug, Default)]
#[reflect(Component, Default)]
pub struct CameraTarget;

/// The camera rig's look orientation. The camera is positioned from this each frame.
/// The rig updates it from right-mouse-drag; games may also write `yaw` to add their own
/// turn controls (e.g. keyboard turn) and read it to align the player's facing. Public so
/// consuming games can drive/read the camera direction.
#[derive(Component, Default)]
pub struct CameraLook {
    /// Yaw (turn) in radians.
    pub yaw: f32,
    /// Pitch (up/down look) in radians.
    pub pitch: f32,
}

/// Registers the camera-rig components for reflection so the inspector + `.jsn`
/// (de)serializer handle them. `JackdawCameraRigPlugin` adds it for you; the editor
/// adds it directly for authoring.
pub struct JackdawCameraRigTypesPlugin;
impl Plugin for JackdawCameraRigTypesPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ThirdPersonCamera>()
            .register_type::<FirstPersonCamera>()
            .register_type::<CameraTarget>();
        app.register_type_data::<ThirdPersonCamera, ReflectDefault>()
            .register_type_data::<FirstPersonCamera, ReflectDefault>()
            .register_type_data::<CameraTarget, ReflectDefault>();
    }
}

/// Runtime camera driver. Add this in the GAME (client) only. Registers the types
/// (if not already) and runs the rig systems.
pub struct JackdawCameraRigPlugin;
impl Plugin for JackdawCameraRigPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<JackdawCameraRigTypesPlugin>() {
            app.add_plugins(JackdawCameraRigTypesPlugin);
        }
        app.add_systems(
            Update,
            (materialize_cameras, drive_third_person, drive_first_person),
        );
    }
}

/// A camera-rig entity (has a mode component) gains a real `Camera3d` + look state once.
/// Seeds `CameraLook.pitch` from `ThirdPersonCamera.pitch` so the authored look-down applies.
fn materialize_cameras(
    mut commands: Commands,
    tp: Query<(Entity, &ThirdPersonCamera), Without<Camera3d>>,
    fp: Query<
        Entity,
        (
            With<FirstPersonCamera>,
            Without<Camera3d>,
            Without<ThirdPersonCamera>,
        ),
    >,
) {
    for (e, cfg) in &tp {
        commands.entity(e).insert((
            Camera3d::default(),
            CameraLook {
                yaw: 0.0,
                pitch: cfg.pitch,
            },
        ));
    }
    for e in &fp {
        commands
            .entity(e)
            .insert((Camera3d::default(), CameraLook::default()));
    }
}

fn drive_third_person(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<bevy::input::mouse::AccumulatedMouseMotion>,
    target: Query<&GlobalTransform, With<CameraTarget>>,
    mut cam: Query<(&ThirdPersonCamera, &mut CameraLook, &mut Transform)>,
) {
    let Ok(target_tf) = target.single() else {
        return;
    };
    let focus = target_tf.translation() + Vec3::Y * 1.2;
    for (cfg, mut look, mut tf) in &mut cam {
        // The mouse rotates the camera only while the right button is held (WoW-style);
        // otherwise the camera follows whatever `yaw` the game set (e.g. keyboard turn).
        if mouse_buttons.pressed(MouseButton::Right) {
            look.yaw -= motion.delta.x * cfg.sensitivity;
            look.pitch = (look.pitch - motion.delta.y * cfg.sensitivity).clamp(0.05, 1.4);
        }
        let offset =
            Quat::from_euler(EulerRot::YXZ, look.yaw, -look.pitch, 0.0) * (Vec3::Z * cfg.distance);
        *tf = Transform::from_translation(focus + offset).looking_at(focus, Vec3::Y);
    }
}

fn drive_first_person(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<bevy::input::mouse::AccumulatedMouseMotion>,
    target: Query<&GlobalTransform, With<CameraTarget>>,
    mut cam: Query<(&FirstPersonCamera, &mut CameraLook, &mut Transform)>,
) {
    let Ok(target_tf) = target.single() else {
        return;
    };
    for (cfg, mut look, mut tf) in &mut cam {
        // Mouse look only while the right button is held; keyboard turn (game-set `yaw`)
        // applies always.
        if mouse_buttons.pressed(MouseButton::Right) {
            look.yaw -= motion.delta.x * cfg.sensitivity;
            look.pitch = (look.pitch - motion.delta.y * cfg.sensitivity).clamp(-1.4, 1.4);
        }
        let eye = target_tf.translation() + Vec3::Y * cfg.eye_height;
        let dir = Quat::from_euler(EulerRot::YXZ, look.yaw, look.pitch, 0.0) * -Vec3::Z;
        *tf = Transform::from_translation(eye).looking_to(dir, Vec3::Y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::std_traits::ReflectDefault;

    #[test]
    fn rig_components_insert_without_panic_and_register() {
        let mut app = App::new();
        app.add_plugins(JackdawCameraRigTypesPlugin);

        let e = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(e)
            .insert(ThirdPersonCamera::default());
        app.world_mut()
            .entity_mut(e)
            .insert(FirstPersonCamera::default());
        app.update();

        assert!(app.world().entity(e).contains::<ThirdPersonCamera>());
        assert!(app.world().entity(e).contains::<FirstPersonCamera>());

        let registry = app.world().resource::<AppTypeRegistry>().read();
        for tn in [
            std::any::type_name::<ThirdPersonCamera>(),
            std::any::type_name::<FirstPersonCamera>(),
            std::any::type_name::<CameraTarget>(),
        ] {
            let reg = registry
                .get_with_type_path(tn)
                .unwrap_or_else(|| panic!("{tn} not registered"));
            assert!(
                reg.data::<bevy::ecs::reflect::ReflectComponent>().is_some(),
                "{tn} missing ReflectComponent"
            );
            assert!(
                reg.data::<ReflectDefault>().is_some(),
                "{tn} missing ReflectDefault"
            );
        }
    }
}
