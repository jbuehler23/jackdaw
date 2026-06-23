//! Avian physics integration for the jackdaw editor.
//!
//! Provides collider wireframe visualization, hierarchy arrows, type
//! registration for avian3d physics components, and an interactive
//! simulation workflow (see [`simulation`]).

#[cfg(feature = "overlays")]
use std::marker::PhantomData;

#[cfg(feature = "overlays")]
use avian3d::debug_render::{PhysicsGizmoExt, PhysicsGizmos};
use avian3d::prelude::*;
use bevy::prelude::*;
use jackdaw_geometry::{ModifierStack, is_convex_topology};
use jackdaw_jsn::{Brush, evaluate_brush_geometry};

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

/// Runtime bridge that builds an avian [`Collider`] from the editor-authored
/// [`AvianCollider`] wrapper, so brushes and meshes saved with a collider
/// actually collide at runtime, not only in the editor preview. Wired into
/// `jackdaw_runtime`'s `JackdawPlugin` behind its `physics` feature.
///
/// The editor uses its own bridge that reads the live edit caches; this one
/// rebuilds the collider from the serialized `Brush` geometry, so it also works
/// in a headless server with no rendering. Registering the avian types here also
/// lets the scene loader deserialize `AvianCollider` in the first place.
pub struct AvianColliderBridgePlugin;

impl Plugin for AvianColliderBridgePlugin {
    fn build(&self, app: &mut App) {
        register_avian_types(app);
        app.add_systems(PreUpdate, build_brush_colliders)
            .add_observer(remove_collider_with_avian_collider);
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
fn remove_collider_with_avian_collider(trigger: On<Remove, AvianCollider>, mut commands: Commands) {
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

    let (vertices, face_polygons, _) = evaluate_brush_geometry(brush, stack);
    if vertices.is_empty() {
        return None;
    }

    if !force_trimesh && matches!(cfg.0, ColliderConstructor::ConvexHullFromMesh) {
        return Collider::convex_hull(vertices);
    }

    let mut indices: Vec<[u32; 3]> = Vec::new();
    for polygon in &face_polygons {
        for i in 1..polygon.len().saturating_sub(1) {
            indices.push([polygon[0] as u32, polygon[i] as u32, polygon[i + 1] as u32]);
        }
    }
    if indices.is_empty() {
        return None;
    }
    Some(Collider::trimesh(vertices, indices))
}

pub mod physics_colors {
    use bevy::prelude::Color;

    pub const COLLIDER_WIREFRAME: Color = Color::srgba(0.0, 1.0, 0.5, 0.7);
    pub const SENSOR_WIREFRAME: Color = Color::srgba(0.0, 0.8, 1.0, 0.5);
    pub const COLLIDER_SELECTED: Color = Color::srgba(0.0, 1.0, 0.5, 1.0);
    pub const SENSOR_SELECTED: Color = Color::srgba(0.0, 0.8, 1.0, 0.85);
    pub const COLLIDER_HIERARCHY_ARROW: Color = Color::srgba(0.4, 0.7, 1.0, 0.6);
}

#[cfg(feature = "overlays")]
#[derive(Resource, Clone, PartialEq)]
pub struct PhysicsOverlayConfig {
    pub show_colliders: bool,
    pub show_hierarchy_arrows: bool,
}

#[cfg(feature = "overlays")]
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
#[cfg(feature = "overlays")]
pub struct PhysicsOverlaysPlugin<S: Component> {
    _marker: PhantomData<S>,
}

#[cfg(feature = "overlays")]
impl<S: Component> Default for PhysicsOverlaysPlugin<S> {
    fn default() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

#[cfg(feature = "overlays")]
impl<S: Component> PhysicsOverlaysPlugin<S> {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(feature = "overlays")]
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

#[cfg(feature = "overlays")]
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

#[cfg(feature = "overlays")]
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

#[cfg(feature = "overlays")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::reflect::AppTypeRegistry;
    use jackdaw_jsn::Brush;

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
