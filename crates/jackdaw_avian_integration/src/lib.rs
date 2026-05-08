//! Avian physics integration for the jackdaw editor.
//!
//! Provides collider wireframe visualization, hierarchy arrows, type
//! registration for avian3d physics components, and an interactive
//! simulation workflow (see [`simulation`]).

use std::marker::PhantomData;

use avian3d::debug_render::{PhysicsGizmoExt, PhysicsGizmos};
use avian3d::prelude::*;
use bevy::prelude::*;

pub mod simulation;

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
/// the same entity would disable that.
#[derive(Component, Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component, Default)]
pub struct AvianCollider(pub ColliderConstructor);

pub mod physics_colors {
    use bevy::prelude::Color;

    pub const COLLIDER_WIREFRAME: Color = Color::srgba(0.0, 1.0, 0.5, 0.7);
    pub const SENSOR_WIREFRAME: Color = Color::srgba(0.0, 0.8, 1.0, 0.5);
    pub const COLLIDER_SELECTED: Color = Color::srgba(0.0, 1.0, 0.5, 1.0);
    pub const SENSOR_SELECTED: Color = Color::srgba(0.0, 0.8, 1.0, 0.85);
    pub const COLLIDER_HIERARCHY_ARROW: Color = Color::srgba(0.4, 0.7, 1.0, 0.6);
}

#[derive(Resource, Clone, PartialEq)]
pub struct PhysicsOverlayConfig {
    pub show_colliders: bool,
    pub show_hierarchy_arrows: bool,
}

impl Default for PhysicsOverlayConfig {
    fn default() -> Self {
        Self {
            show_colliders: true,
            show_hierarchy_arrows: false,
        }
    }
}

/// Plugin that renders collider wireframes and hierarchy arrows.
///
/// Generic over a `SelectionMarker` component type so callers can wire in
/// their own selection system. Systems run unconditionally; wrap the plugin
/// in your own run condition if you need editor-only behavior.
pub struct PhysicsOverlaysPlugin<S: Component> {
    _marker: PhantomData<S>,
}

impl<S: Component> Default for PhysicsOverlaysPlugin<S> {
    fn default() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<S: Component> PhysicsOverlaysPlugin<S> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<S: Component> Plugin for PhysicsOverlaysPlugin<S> {
    fn build(&self, app: &mut App) {
        register_avian_types(app);

        app.init_resource::<PhysicsOverlayConfig>()
            .init_gizmo_group::<PhysicsGizmos>()
            .add_systems(
                PostUpdate,
                // TODO: Use `JackdawDrawSystems` here
                (draw_collider_gizmos::<S>, draw_hierarchy_arrows::<S>)
                    .after(bevy::transform::TransformSystems::Propagate),
            );

        let mut store = app.world_mut().resource_mut::<GizmoConfigStore>();
        let (config, _) = store.config_mut::<PhysicsGizmos>();
        config.depth_bias = -0.5;
        config.line.width = 1.5;
    }
}

/// Register avian3d types that have both `reflect(Component)` and `reflect(Default)`,
/// so they appear in the editor's component picker and can be edited via the JSN AST.
///
/// TODO: Remove once jackdaw moves to Bevy 0.19+, which has `reflect_auto_register`
/// that automatically registers all types with `#[derive(Reflect)]` via the `inventory`
/// crate at app startup.
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

fn draw_collider_gizmos<S: Component>(
    mut gizmos: Gizmos<PhysicsGizmos>,
    config: Res<PhysicsOverlayConfig>,
    colliders: Query<(
        Entity,
        &Collider,
        &GlobalTransform,
        &InheritedVisibility,
        Option<&Sensor>,
    )>,
    selected_bodies: Query<Entity, (With<RigidBody>, With<S>)>,
    children_query: Query<&Children>,
    collider_check: Query<(), With<Collider>>,
) {
    if !config.show_colliders {
        return;
    }

    let mut highlighted = bevy::ecs::entity::EntityHashSet::default();
    for body_entity in &selected_bodies {
        collect_descendant_colliders(
            body_entity,
            &children_query,
            &collider_check,
            &mut highlighted,
        );
        if collider_check.contains(body_entity) {
            highlighted.insert(body_entity);
        }
    }

    for (entity, collider, tf, vis, sensor) in &colliders {
        if !vis.get() {
            continue;
        }

        let is_highlighted = highlighted.contains(&entity);
        let color = match (sensor.is_some(), is_highlighted) {
            (false, false) => physics_colors::COLLIDER_WIREFRAME,
            (false, true) => physics_colors::COLLIDER_SELECTED,
            (true, false) => physics_colors::SENSOR_WIREFRAME,
            (true, true) => physics_colors::SENSOR_SELECTED,
        };

        let position = Position::from(tf);
        let rotation = Rotation::from(tf);
        gizmos.draw_collider(collider, position, rotation, color);
    }
}

fn draw_hierarchy_arrows<S: Component>(
    mut gizmos: Gizmos<PhysicsGizmos>,
    config: Res<PhysicsOverlayConfig>,
    selected_bodies: Query<(Entity, &GlobalTransform), (With<RigidBody>, With<S>)>,
    children_query: Query<&Children>,
    collider_transforms: Query<&GlobalTransform, With<Collider>>,
    collider_check: Query<(), With<Collider>>,
) {
    if !config.show_hierarchy_arrows {
        return;
    }

    for (body_entity, body_tf) in &selected_bodies {
        let body_pos = body_tf.translation();
        let mut descendants = bevy::ecs::entity::EntityHashSet::default();
        collect_descendant_colliders(
            body_entity,
            &children_query,
            &collider_check,
            &mut descendants,
        );

        for collider_entity in &descendants {
            if *collider_entity == body_entity {
                continue;
            }
            if let Ok(collider_tf) = collider_transforms.get(*collider_entity) {
                gizmos.arrow(
                    body_pos,
                    collider_tf.translation(),
                    physics_colors::COLLIDER_HIERARCHY_ARROW,
                );
            }
        }
    }
}

fn collect_descendant_colliders(
    entity: Entity,
    children_query: &Query<&Children>,
    collider_check: &Query<(), With<Collider>>,
    out: &mut bevy::ecs::entity::EntityHashSet,
) {
    if let Ok(children) = children_query.get(entity) {
        for child in children.iter() {
            if collider_check.contains(child) {
                out.insert(child);
            }
            collect_descendant_colliders(child, children_query, collider_check, out);
        }
    }
}
