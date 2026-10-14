//! Avian physics for jackdaw games.
//!
//! A scene authors colliders as [`AvianCollider`] and bodies as avian's own
//! [`RigidBody`]. [`AvianColliderBridgePlugin`] builds the [`Collider`] each
//! authored collider stands for, from a brush's faces, a placed model's
//! meshes or a primitive shape, and with the `terrain` feature a heightfield
//! under every loaded terrain. Add avian's `PhysicsPlugins` beside it to
//! simulate.

use avian3d::prelude::*;
use bevy::prelude::*;
use jackdaw_geometry::{ModifierStack, is_convex_topology, triangulate_polygons};
use jackdaw_scene_types::{Brush, evaluate_brush_geometry};

mod model_colliders;
mod static_bodies;
#[cfg(feature = "terrain")]
mod terrain_ground;

pub use model_colliders::ModelCollidersPlugin;
pub use static_bodies::{ImpliedStaticBody, StaticCollidersPlugin};

/// Editor-facing collider shape selector. Wraps avian's [`ColliderConstructor`]
/// as a newtype so it lives outside avian's auto-processing pipeline (which
/// consumes and removes `ColliderConstructor` after building `Collider`).
///
/// When this component is added or changed, the editor's sync system builds
/// a `Collider` from the inner constructor and inserts it directly. Avian's
/// `init_collider_constructors` never fires because `ColliderConstructor`
/// is never placed on the entity.
///
/// No `#[require(RigidBody)]`: avian supports collider-on-child patterns
/// where the rigid body lives on a parent entity, and forcing both onto
/// the same entity would disable that. A collider with no body on it or
/// above it gets a static one from [`StaticCollidersPlugin`].
#[derive(Component, Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component, Default)]
pub struct AvianCollider(pub ColliderConstructor);

/// Builds an avian [`Collider`] from every authored [`AvianCollider`], so
/// brushes and models saved with a collider collide in the game and in the
/// editor alike. Add it beside `JackdawPlugin`.
///
/// The collider is rebuilt from the saved `Brush` geometry, so it also works
/// in a headless server with no rendering. Registering the avian types here
/// also lets the scene loader read `AvianCollider` in the first place.
pub struct AvianColliderBridgePlugin;

impl Plugin for AvianColliderBridgePlugin {
    fn build(&self, app: &mut App) {
        register_avian_types(app);
        app.add_plugins((ModelCollidersPlugin, StaticCollidersPlugin))
            .add_systems(PreUpdate, build_brush_colliders)
            .add_observer(remove_collider_with_avian_collider);
        #[cfg(feature = "terrain")]
        app.add_plugins(terrain_ground::plugin);
    }
}

/// Build a `Collider` for every brush whose `AvianCollider`, geometry, or
/// modifier stack changed. Spawning a scene sets these components via
/// reflection, which marks them changed, so colliders appear on load and track
/// any later geometry change.
fn build_brush_colliders(
    mut commands: Commands,
    changed: Query<
        (Entity, &AvianCollider, &Brush, Option<&ModifierStack>),
        Or<(
            Changed<AvianCollider>,
            Changed<Brush>,
            Changed<ModifierStack>,
        )>,
    >,
) {
    for (entity, collider_cfg, brush, stack) in &changed {
        if let Some(collider) = brush_collider(brush, stack, collider_cfg) {
            commands.entity(entity).insert(collider);
        }
    }
}

/// Remove the built `Collider` when its source `AvianCollider` is removed, so a
/// physics-off toggle or undo does not leave a stale collider behind.
fn remove_collider_with_avian_collider(trigger: On<Remove<AvianCollider>>, mut commands: Commands) {
    let entity = trigger.event_target();
    if let Ok(mut ec) = commands.get_entity(entity) {
        ec.try_remove::<Collider>();
    }
}

/// Build a collider from a brush's evaluated geometry. Non-convex brushes are
/// forced to a trimesh (a convex hull or primitive would mis-simulate); convex
/// brushes honor a requested primitive or convex hull and otherwise fall back to
/// a trimesh.
pub fn brush_collider(
    brush: &Brush,
    stack: Option<&ModifierStack>,
    cfg: &AvianCollider,
) -> Option<Collider> {
    let force_trimesh = !is_convex_topology(&brush.topology);

    // A primitive shape (cuboid, sphere, ...) needs no geometry.
    if !force_trimesh && !cfg.0.requires_mesh() {
        return Collider::try_from_constructor(cfg.0.clone(), None);
    }

    let (vertices, face_polygons, faces) = evaluate_brush_geometry(brush, stack);
    if vertices.is_empty() {
        return None;
    }

    if !force_trimesh && matches!(cfg.0, ColliderConstructor::ConvexHullFromMesh) {
        return Collider::convex_hull(&vertices);
    }

    let indices = triangulate_polygons(
        &vertices,
        &face_polygons,
        faces.iter().map(|f| f.plane.normal),
    );
    if indices.is_empty() {
        return None;
    }
    Some(Collider::trimesh(vertices, indices))
}

/// Register avian3d types that have both `reflect(Component)` and `reflect(Default)`,
/// so they appear in the editor's component picker, edit through the JSN AST, and
/// deserialize at runtime.
///
/// These are external (avian) types, so Bevy's `reflect_auto_register` cannot
/// register them: the `inventory` shims it relies on are dropped by the linker
/// for a dependency crate that nothing references. Registering them explicitly is
/// the reliable path and stays correct regardless of Bevy version.
pub fn register_avian_types(app: &mut App) {
    app
        // Core
        .register_type::<RigidBody>()
        // ColliderConstructor is NOT registered  -- avian consumes and removes
        // it. Users add AvianCollider instead (clean wrapper).
        .register_type::<Sensor>()
        .register_type::<AvianCollider>()
        // Velocity
        .register_type::<LinearVelocity>()
        .register_type::<AngularVelocity>()
        .register_type::<MaxLinearSpeed>()
        .register_type::<MaxAngularSpeed>()
        // Damping/gravity
        .register_type::<GravityScale>()
        .register_type::<LinearDamping>()
        .register_type::<AngularDamping>()
        .register_type::<LockedAxes>()
        // Forces
        .register_type::<ConstantForce>()
        .register_type::<ConstantTorque>()
        .register_type::<ConstantLocalForce>()
        // State
        .register_type::<RigidBodyDisabled>()
        .register_type::<Sleeping>()
        .register_type::<SleepingDisabled>()
        // Internal avian components  -- registered so the inspector can display
        // them when added via `#[require]`. Not all have ReflectDefault, so
        // they won't appear in the component picker, only in the inspector.
        .register_type::<Position>()
        .register_type::<Rotation>()
        .register_type::<CollisionLayers>()
        .register_type::<ColliderDensity>()
        .register_type::<SleepThreshold>()
        .register_type::<SleepTimer>();
    // NOTE: Many more avian internal types (ColliderAabb, ComputedMass,
    // ColliderMassProperties, etc.) also exist but may not be publicly
    // exported from avian3d::prelude. Register more as needed.
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::reflect::AppTypeRegistry;
    use jackdaw_scene_types::Brush;

    #[test]
    fn cuboid_brush_builds_a_trimesh_collider() {
        let brush = Brush::cuboid(1.0, 1.0, 1.0);
        let cfg = AvianCollider(ColliderConstructor::TrimeshFromMesh);
        assert!(
            brush_collider(&brush, None, &cfg).is_some(),
            "a convex cuboid with a mesh constructor yields a trimesh collider"
        );
    }

    #[test]
    fn convex_brush_honors_convex_hull_request() {
        let brush = Brush::cuboid(0.5, 0.5, 0.5);
        let cfg = AvianCollider(ColliderConstructor::ConvexHullFromMesh);
        assert!(
            brush_collider(&brush, None, &cfg).is_some(),
            "a convex-hull request on a cuboid yields a collider"
        );
    }

    #[test]
    fn bridge_inserts_collider_on_spawn() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<AppTypeRegistry>();
        app.add_plugins(AvianColliderBridgePlugin);

        let entity = app
            .world_mut()
            .spawn((
                Brush::cuboid(1.0, 1.0, 1.0),
                AvianCollider(ColliderConstructor::TrimeshFromMesh),
            ))
            .id();

        app.update();

        assert!(
            app.world().get::<Collider>(entity).is_some(),
            "spawning a brush with an AvianCollider builds a Collider"
        );
    }
}
