//! Authorable camera rig for Jackdaw scenes. Add a `CameraRig` component (and `ActiveCameraRig`
//! to enable it) to a camera entity; `JackdawCameraRigPlugin` materializes a `Camera3d` on it
//! and drives it relative to the rig's parent entity. The camera orbits/looks in world space so
//! parent rotation does not bleed into the camera orientation.
use bevy::prelude::*;
use bevy::reflect::std_traits::ReflectDefault;
use jackdaw_jsn::EditorCategory;

/// Which driving mode the rig uses.
#[derive(Reflect, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CameraMode {
    #[default]
    ThirdPerson,
    FirstPerson,
}

/// Camera rig authored on a camera entity. Attach `ActiveCameraRig` to the same entity to enable
/// the runtime driver.
#[derive(Component, Reflect, Clone, Copy, PartialEq, Debug)]
#[reflect(Component, Default, @EditorCategory::new("Camera"))]
pub struct CameraRig {
    pub mode: CameraMode,
    /// Distance from the focus point (third-person), in metres.
    pub distance: f32,
    /// Initial downward pitch angle, in radians (third-person).
    pub pitch: f32,
    /// Eye height above the parent origin (first-person), in metres.
    pub eye_height: f32,
    /// Mouse-look sensitivity (radians per pixel of motion).
    pub sensitivity: f32,
}
impl Default for CameraRig {
    fn default() -> Self {
        Self {
            mode: CameraMode::ThirdPerson,
            distance: 10.0,
            pitch: 0.4,
            eye_height: 1.6,
            sensitivity: 0.005,
        }
    }
}

/// Enables the runtime camera driver on a `CameraRig` entity. Only one should be active at a
/// time. `activate_lone_rig` inserts this automatically when exactly one rig exists.
#[derive(Component, Reflect, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[reflect(Component, Default)]
pub struct ActiveCameraRig;

/// The rig's look orientation. Updated by right-mouse-drag; games may write `yaw` directly to
/// apply keyboard turn and read it to align the player's facing direction.
#[derive(Component, Default)]
pub struct CameraLook {
    /// Yaw (turn) in radians.
    pub yaw: f32,
    /// Pitch (up/down look) in radians.
    pub pitch: f32,
}

/// Registers the camera-rig components for reflection so the inspector and `.jsn`
/// (de)serializer handle them. `JackdawCameraRigPlugin` adds it automatically; the editor adds
/// it directly for authoring without the runtime driver.
pub struct JackdawCameraRigTypesPlugin;
impl Plugin for JackdawCameraRigTypesPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<CameraRig>()
            .register_type::<CameraMode>()
            .register_type::<ActiveCameraRig>();
        app.register_type_data::<CameraRig, ReflectDefault>()
            .register_type_data::<ActiveCameraRig, ReflectDefault>();
    }
}

/// Runtime camera driver. Add this in the game (client) only. Registers the types (if not
/// already) and runs the rig systems each frame.
pub struct JackdawCameraRigPlugin;
impl Plugin for JackdawCameraRigPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<JackdawCameraRigTypesPlugin>() {
            app.add_plugins(JackdawCameraRigTypesPlugin);
        }
        app.init_resource::<CameraRigActivation>();
        // activate_lone_rig and materialize_active run in Update so Camera3d is inserted before
        // PostUpdate rendering systems inspect it.
        // drive_camera_rig runs in PostUpdate after TransformSystems::Propagate so it reads
        // the freshly-propagated parent GlobalTransform, computes world-space camera placement,
        // and writes both the local Transform and GlobalTransform on the rig directly.
        app.add_systems(
            Update,
            (
                activate_lone_rig,
                materialize_active,
                dematerialize_inactive,
            )
                .chain(),
        );
        app.add_systems(
            PostUpdate,
            drive_camera_rig.after(bevy::transform::TransformSystems::Propagate),
        );
    }
}

/// Who decides which rig is active.
///
/// `Automatic` (the default) keeps the single-player convenience: a lone rig
/// activates itself. `Gated` hands ownership to an external system (the
/// multiplayer local-player gate), which both grants AND revokes the marker;
/// the convenience must then stay out of the way, or the two fight in an
/// add/remove loop over any rig the gate refuses (the camera flickers on and
/// off every frame).
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraRigActivation {
    #[default]
    Automatic,
    Gated,
}

/// When activation is `Automatic`, no rig is active, and exactly one rig
/// exists in the world, marks it active. Does nothing if a second rig is
/// present (the game must choose explicitly) or when an external gate owns
/// activation.
fn activate_lone_rig(
    mut commands: Commands,
    activation: Option<Res<CameraRigActivation>>,
    active: Query<(), With<ActiveCameraRig>>,
    rigs: Query<Entity, With<CameraRig>>,
) {
    if activation.as_deref().copied().unwrap_or_default() == CameraRigActivation::Gated {
        return;
    }
    if !active.is_empty() {
        return;
    }
    let mut iter = rigs.iter();
    if let (Some(only), None) = (iter.next(), iter.next()) {
        // try_insert: gameplay can despawn the rig in the same frame.
        commands.entity(only).try_insert(ActiveCameraRig);
    }
}

/// Inserts `Camera3d` on any active rig that does not yet have one, plus a
/// fresh `CameraLook` when the rig has never had one. A rig that is
/// re-activated keeps its existing look, so a transient control loss does not
/// snap the orbit back to its defaults.
fn materialize_active(
    mut commands: Commands,
    rigs: Query<(Entity, &CameraRig, Has<CameraLook>), (With<ActiveCameraRig>, Without<Camera3d>)>,
) {
    for (e, rig, has_look) in &rigs {
        let mut entity = commands.entity(e);
        entity.try_insert(Camera3d::default());
        if !has_look {
            entity.try_insert(CameraLook {
                yaw: 0.0,
                pitch: if rig.mode == CameraMode::ThirdPerson {
                    rig.pitch
                } else {
                    0.0
                },
            });
        }
    }
}

/// Removes the camera from a rig that is no longer active. Paired with `materialize_active`,
/// this keeps `Camera3d` on exactly the rig holding `ActiveCameraRig`: when the active marker
/// moves to a different rig, the old rig's camera is torn down instead of left rendering.
fn dematerialize_inactive(
    mut commands: Commands,
    rigs: Query<Entity, (With<CameraRig>, With<Camera3d>, Without<ActiveCameraRig>)>,
) {
    for rig in &rigs {
        commands.entity(rig).remove::<Camera3d>();
    }
}

/// Vertical offset from parent origin to the third-person focus point.
const THIRD_PERSON_FOCUS_LIFT: f32 = 1.2;

/// Drives the active camera rig each frame. Reads the parent's world transform and writes the
/// rig's LOCAL transform so that the rig's position/orientation is expressed in world space
/// regardless of how the parent entity is rotated.
fn drive_camera_rig(
    mouse_buttons: Option<Res<ButtonInput<MouseButton>>>,
    motion: Option<Res<bevy::input::mouse::AccumulatedMouseMotion>>,
    mut queries: ParamSet<(
        // p0: read GlobalTransform from any entity (parents)
        Query<&GlobalTransform>,
        // p1: read rig config + look state
        Query<(Entity, &CameraRig, &ChildOf, &mut CameraLook), With<ActiveCameraRig>>,
        // p2: write Transform + GlobalTransform on rig entities
        Query<(&mut Transform, &mut GlobalTransform), With<ActiveCameraRig>>,
    )>,
) {
    let rmb_held = mouse_buttons
        .as_deref()
        .is_some_and(|b| b.pressed(MouseButton::Right));
    let delta = if rmb_held {
        motion.as_deref().map(|m| m.delta).unwrap_or_default()
    } else {
        Vec2::ZERO
    };

    // Collect (rig_entity, parent_entity) from p1 first, then look up parent globals from p0.
    // These two borrows cannot overlap, so we collect before releasing p1.
    let rig_parents: Vec<(Entity, Entity)> = queries
        .p1()
        .iter()
        .map(|(e, _, child_of, _)| (e, child_of.0))
        .collect();

    let parent_globals: Vec<(Entity, bevy::math::Affine3A)> = {
        let globals = queries.p0();
        rig_parents
            .iter()
            .filter_map(|(rig_e, parent_e)| {
                globals.get(*parent_e).ok().map(|pg| (*rig_e, pg.affine()))
            })
            .collect()
    };

    // Second pass: update look state and compute desired world/local affines.
    let mut writes: Vec<(Entity, bevy::math::Affine3A, bevy::math::Affine3A)> = Vec::new();
    {
        let mut cam = queries.p1();
        for (entity, parent_affine) in &parent_globals {
            let Ok((_, rig, _, mut look)) = cam.get_mut(*entity) else {
                continue;
            };

            if rmb_held {
                look.yaw -= delta.x * rig.sensitivity;
                match rig.mode {
                    CameraMode::ThirdPerson => {
                        look.pitch = (look.pitch + delta.y * rig.sensitivity).clamp(0.05, 1.4);
                    }
                    CameraMode::FirstPerson => {
                        look.pitch = (look.pitch - delta.y * rig.sensitivity).clamp(-1.4, 1.4);
                    }
                }
            }

            let parent_translation = bevy::math::Vec3::from(parent_affine.translation);

            // Desired world-space transform for the camera.
            let desired_world = match rig.mode {
                CameraMode::ThirdPerson => {
                    let focus = parent_translation + Vec3::Y * THIRD_PERSON_FOCUS_LIFT;
                    let offset = Quat::from_euler(EulerRot::YXZ, look.yaw, -look.pitch, 0.0)
                        * (Vec3::Z * rig.distance);
                    Transform::from_translation(focus + offset).looking_at(focus, Vec3::Y)
                }
                CameraMode::FirstPerson => {
                    let eye = parent_translation + Vec3::Y * rig.eye_height;
                    let dir = Quat::from_euler(EulerRot::YXZ, look.yaw, look.pitch, 0.0) * -Vec3::Z;
                    Transform::from_translation(eye).looking_to(dir, Vec3::Y)
                }
            };

            let world_affine = desired_world.compute_affine();
            let local_affine = parent_affine.inverse() * world_affine;
            writes.push((*entity, world_affine, local_affine));
        }
    }

    // Second pass: write Transform (local) and GlobalTransform (world) on rig entities.
    // GlobalTransform is written directly because this system runs after
    // TransformSystems::Propagate and propagation will not re-run this frame.
    let mut rigs = queries.p2();
    for (entity, world_affine, local_affine) in writes {
        let Ok((mut tf, mut global_tf)) = rigs.get_mut(entity) else {
            continue;
        };
        *global_tf = GlobalTransform::from(world_affine);
        *tf = Transform::from_matrix(Mat4::from(local_affine));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::std_traits::ReflectDefault;

    #[test]
    fn camera_rig_registers_with_reflect_component_and_default() {
        let mut app = App::new();
        app.add_plugins(JackdawCameraRigTypesPlugin);
        let registry = app.world().resource::<AppTypeRegistry>().read();
        let reg = registry
            .get_with_type_path(std::any::type_name::<CameraRig>())
            .expect("CameraRig registered");
        assert!(reg.data::<bevy::ecs::reflect::ReflectComponent>().is_some());
        assert!(reg.data::<ReflectDefault>().is_some());
    }

    #[test]
    fn only_active_rig_materializes_camera() {
        let mut app = App::new();
        app.add_plugins((bevy::transform::TransformPlugin, JackdawCameraRigPlugin));
        let parent = app.world_mut().spawn(Transform::default()).id();
        let dormant = app
            .world_mut()
            .spawn((CameraRig::default(), ChildOf(parent)))
            .id();
        let active = app
            .world_mut()
            .spawn((CameraRig::default(), ActiveCameraRig, ChildOf(parent)))
            .id();
        app.update();
        assert!(!app.world().entity(dormant).contains::<Camera3d>());
        assert!(app.world().entity(active).contains::<Camera3d>());
    }

    #[test]
    fn deactivating_a_rig_removes_its_camera() {
        let mut app = App::new();
        app.add_plugins((bevy::transform::TransformPlugin, JackdawCameraRigPlugin));
        let parent = app.world_mut().spawn(Transform::default()).id();
        // Two rigs so the lone-rig convenience does not re-activate the one we deactivate.
        let a = app
            .world_mut()
            .spawn((CameraRig::default(), ActiveCameraRig, ChildOf(parent)))
            .id();
        let b = app
            .world_mut()
            .spawn((CameraRig::default(), ChildOf(parent)))
            .id();
        app.update();
        assert!(app.world().entity(a).contains::<Camera3d>());
        assert!(!app.world().entity(b).contains::<Camera3d>());
        // Hand the active marker from a to b, as the multiplayer gate does when control moves.
        app.world_mut().entity_mut(a).remove::<ActiveCameraRig>();
        app.world_mut().entity_mut(b).insert(ActiveCameraRig);
        app.update();
        assert!(!app.world().entity(a).contains::<Camera3d>());
        assert!(app.world().entity(b).contains::<Camera3d>());
    }

    #[test]
    fn third_person_orbit_is_decoupled_from_parent_rotation() {
        use bevy::math::Vec3;
        let mut app = App::new();
        app.add_plugins((bevy::transform::TransformPlugin, JackdawCameraRigPlugin));
        let parent = app.world_mut().spawn(Transform::default()).id();
        let rig = app
            .world_mut()
            .spawn((
                CameraRig {
                    mode: CameraMode::ThirdPerson,
                    distance: 10.0,
                    pitch: 0.0,
                    eye_height: 1.6,
                    sensitivity: 0.005,
                },
                ActiveCameraRig,
                ChildOf(parent),
            ))
            .id();
        app.update();
        let world_a = app
            .world()
            .entity(rig)
            .get::<GlobalTransform>()
            .unwrap()
            .translation();
        app.world_mut()
            .entity_mut(parent)
            .insert(Transform::from_rotation(Quat::from_rotation_y(
                std::f32::consts::FRAC_PI_2,
            )));
        app.update();
        let world_b = app
            .world()
            .entity(rig)
            .get::<GlobalTransform>()
            .unwrap()
            .translation();
        assert!(
            (world_a - world_b).length() < 1e-3,
            "camera world position must not follow parent rotation"
        );
        assert!(
            (world_a - Vec3::new(0.0, 1.2, 10.0)).length() < 1e-2,
            "got {world_a:?}"
        );
    }

    #[test]
    fn first_person_sits_at_eye_height() {
        use bevy::math::Vec3;
        let mut app = App::new();
        app.add_plugins((bevy::transform::TransformPlugin, JackdawCameraRigPlugin));
        let parent = app.world_mut().spawn(Transform::default()).id();
        let rig = app
            .world_mut()
            .spawn((
                CameraRig {
                    mode: CameraMode::FirstPerson,
                    distance: 10.0,
                    pitch: 0.0,
                    eye_height: 1.6,
                    sensitivity: 0.005,
                },
                ActiveCameraRig,
                ChildOf(parent),
            ))
            .id();
        app.update();
        let pos = app
            .world()
            .entity(rig)
            .get::<GlobalTransform>()
            .unwrap()
            .translation();
        assert!(
            (pos - Vec3::new(0.0, 1.6, 0.0)).length() < 1e-2,
            "got {pos:?}"
        );
    }

    #[test]
    fn convenience_activates_lone_rig_only() {
        let mut app = App::new();
        app.add_plugins((bevy::transform::TransformPlugin, JackdawCameraRigPlugin));
        let p = app.world_mut().spawn(Transform::default()).id();
        let only = app
            .world_mut()
            .spawn((CameraRig::default(), ChildOf(p)))
            .id();
        app.update();
        assert!(app.world().entity(only).contains::<ActiveCameraRig>());
        let second = app
            .world_mut()
            .spawn((CameraRig::default(), ChildOf(p)))
            .id();
        app.update();
        assert!(!app.world().entity(second).contains::<ActiveCameraRig>());
    }

    #[test]
    fn gated_activation_leaves_a_lone_rig_dormant() {
        let mut app = App::new();
        app.add_plugins((bevy::transform::TransformPlugin, JackdawCameraRigPlugin));
        app.insert_resource(CameraRigActivation::Gated);
        let p = app.world_mut().spawn(Transform::default()).id();
        let only = app
            .world_mut()
            .spawn((CameraRig::default(), ChildOf(p)))
            .id();
        app.update();
        app.update();
        assert!(
            !app.world().entity(only).contains::<ActiveCameraRig>(),
            "a gate owns activation; the lone-rig convenience must not fight it"
        );
        assert!(!app.world().entity(only).contains::<Camera3d>());
    }
}
