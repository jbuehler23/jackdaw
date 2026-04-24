//! Avian physics integration for the jackdaw editor.
//!
//! Provides collider wireframe visualization, hierarchy arrows, type
//! registration for avian3d physics components, and an interactive
//! simulation workflow (see [`simulation`]).

use std::f32::consts::FRAC_PI_2;
use std::marker::PhantomData;

use avian3d::parry::math::{Point, Real};
use avian3d::parry::shape::{SharedShape, TypedShape};
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

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct ColliderGizmoGroup;

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
            .init_gizmo_group::<ColliderGizmoGroup>()
            .add_systems(
                PostUpdate,
                // TODO: Use `JackdawDrawSystems` here
                (draw_collider_gizmos::<S>, draw_hierarchy_arrows::<S>)
                    .after(bevy::transform::TransformSystems::Propagate),
            );

        let mut store = app.world_mut().resource_mut::<GizmoConfigStore>();
        let (config, _) = store.config_mut::<ColliderGizmoGroup>();
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

/// Convert a parry3d nalgebra Point to bevy Vec3.
fn parry_point(p: &Point<Real>) -> Vec3 {
    Vec3::new(p.x, p.y, p.z)
}

fn draw_collider_gizmos<S: Component>(
    mut gizmos: Gizmos<ColliderGizmoGroup>,
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

    // Collect highlighted colliders (belonging to a selected rigid body)
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

        let transform = tf.compute_transform();
        draw_parry_shape(
            &mut gizmos,
            collider.shape(),
            transform.translation,
            transform.rotation,
            color,
        );
    }
}

/// Draw a wireframe for any parry shape using `TypedShape` pattern matching.
fn draw_parry_shape(
    gizmos: &mut Gizmos<ColliderGizmoGroup>,
    shape: &SharedShape,
    pos: Vec3,
    rot: Quat,
    color: Color,
) {
    match shape.as_typed_shape() {
        TypedShape::Ball(ball) => {
            let r = if ball.radius > 0.0 { ball.radius } else { 0.5 };
            gizmos.circle(
                Isometry3d::new(pos, rot * Quat::from_rotation_x(FRAC_PI_2)),
                r,
                color,
            );
            gizmos.circle(Isometry3d::new(pos, rot), r, color);
            gizmos.circle(
                Isometry3d::new(pos, rot * Quat::from_rotation_y(FRAC_PI_2)),
                r,
                color,
            );
        }
        TypedShape::Cuboid(cuboid) => {
            let he = &cuboid.half_extents;
            let half = Vec3::new(he.x, he.y, he.z);
            let half = if half.length_squared() < 0.0001 {
                Vec3::splat(0.5)
            } else {
                half
            };
            draw_box_wireframe(gizmos, pos, rot, half, color);
        }
        TypedShape::RoundCuboid(rc) => {
            let he = &rc.inner_shape.half_extents;
            let half = Vec3::new(he.x, he.y, he.z);
            let half = if half.length_squared() < 0.0001 {
                Vec3::splat(0.5)
            } else {
                half
            };
            draw_box_wireframe(gizmos, pos, rot, half, color);
        }
        TypedShape::Cylinder(cyl) => {
            let r = cyl.radius;
            let half_h = cyl.half_height;
            let up = rot * Vec3::Y;
            gizmos.circle(Isometry3d::new(pos + up * half_h, rot), r, color);
            gizmos.circle(Isometry3d::new(pos - up * half_h, rot), r, color);
            let right = rot * Vec3::X;
            let fwd = rot * Vec3::Z;
            for dir in [right, -right, fwd, -fwd] {
                gizmos.line(
                    pos + dir * r + up * half_h,
                    pos + dir * r - up * half_h,
                    color,
                );
            }
        }
        TypedShape::Cone(cone) => {
            let r = cone.radius;
            let half_h = cone.half_height;
            let up = rot * Vec3::Y;
            let apex = pos + up * half_h;
            gizmos.circle(Isometry3d::new(pos - up * half_h, rot), r, color);
            let right = rot * Vec3::X;
            let fwd = rot * Vec3::Z;
            for dir in [right, -right, fwd, -fwd] {
                gizmos.line(pos + dir * r - up * half_h, apex, color);
            }
        }
        TypedShape::Capsule(cap) => {
            let r = cap.radius;
            let a = parry_point(&cap.segment.a);
            let b = parry_point(&cap.segment.b);
            let half_h = (b - a).length() * 0.5;
            let up = rot * Vec3::Y;
            let right = rot * Vec3::X;
            let fwd = rot * Vec3::Z;
            gizmos.circle(Isometry3d::new(pos + up * half_h, rot), r, color);
            gizmos.circle(Isometry3d::new(pos - up * half_h, rot), r, color);
            gizmos.arc_3d(
                std::f32::consts::PI,
                r,
                Isometry3d::new(pos + up * half_h, rot * Quat::from_rotation_z(FRAC_PI_2)),
                color,
            );
            gizmos.arc_3d(
                std::f32::consts::PI,
                r,
                Isometry3d::new(pos - up * half_h, rot * Quat::from_rotation_z(-FRAC_PI_2)),
                color,
            );
            gizmos.arc_3d(
                std::f32::consts::PI,
                r,
                Isometry3d::new(
                    pos + up * half_h,
                    rot * Quat::from_rotation_z(FRAC_PI_2) * Quat::from_rotation_y(FRAC_PI_2),
                ),
                color,
            );
            gizmos.arc_3d(
                std::f32::consts::PI,
                r,
                Isometry3d::new(
                    pos - up * half_h,
                    rot * Quat::from_rotation_z(-FRAC_PI_2) * Quat::from_rotation_y(FRAC_PI_2),
                ),
                color,
            );
            for dir in [right, -right, fwd, -fwd] {
                gizmos.line(
                    pos + dir * r + up * half_h,
                    pos + dir * r - up * half_h,
                    color,
                );
            }
        }
        TypedShape::TriMesh(trimesh) => {
            let vertices = trimesh.vertices();
            let indices = trimesh.indices();
            for tri in indices {
                let a = pos + rot * parry_point(&vertices[tri[0] as usize]);
                let b = pos + rot * parry_point(&vertices[tri[1] as usize]);
                let c = pos + rot * parry_point(&vertices[tri[2] as usize]);
                gizmos.line(a, b, color);
                gizmos.line(b, c, color);
                gizmos.line(c, a, color);
            }
        }
        TypedShape::ConvexPolyhedron(poly) => {
            let points = poly.points();
            for edge in poly.edges() {
                let a = pos + rot * parry_point(&points[edge.vertices[0] as usize]);
                let b = pos + rot * parry_point(&points[edge.vertices[1] as usize]);
                gizmos.line(a, b, color);
            }
        }
        TypedShape::Compound(compound) => {
            for (iso, sub_shape) in compound.shapes() {
                let sub_pos = pos
                    + rot
                        * Vec3::new(
                            iso.translation.vector.x,
                            iso.translation.vector.y,
                            iso.translation.vector.z,
                        );
                // Approximate sub-rotation: compose with iso rotation is a TODO
                let sub_rot = rot;
                draw_parry_shape(gizmos, sub_shape, sub_pos, sub_rot, color);
            }
        }
        TypedShape::HalfSpace(_) => {
            let right = rot * Vec3::X * 5.0;
            let fwd = rot * Vec3::Z * 5.0;
            gizmos.line(pos - right - fwd, pos + right - fwd, color);
            gizmos.line(pos + right - fwd, pos + right + fwd, color);
            gizmos.line(pos + right + fwd, pos - right + fwd, color);
            gizmos.line(pos - right + fwd, pos - right - fwd, color);
            gizmos.arrow(pos, pos + rot * Vec3::Y * 2.0, color);
        }
        TypedShape::Segment(seg) => {
            let a = pos + rot * parry_point(&seg.a);
            let b = pos + rot * parry_point(&seg.b);
            gizmos.line(a, b, color);
        }
        TypedShape::Triangle(tri) => {
            let a = pos + rot * parry_point(&tri.a);
            let b = pos + rot * parry_point(&tri.b);
            let c = pos + rot * parry_point(&tri.c);
            gizmos.line(a, b, color);
            gizmos.line(b, c, color);
            gizmos.line(c, a, color);
        }
        _ => {
            gizmos.sphere(Isometry3d::new(pos, rot), 0.1, color);
        }
    }
}

fn draw_box_wireframe(
    gizmos: &mut Gizmos<ColliderGizmoGroup>,
    pos: Vec3,
    rot: Quat,
    half: Vec3,
    color: Color,
) {
    let corners = [
        Vec3::new(-half.x, -half.y, -half.z),
        Vec3::new(half.x, -half.y, -half.z),
        Vec3::new(half.x, half.y, -half.z),
        Vec3::new(-half.x, half.y, -half.z),
        Vec3::new(-half.x, -half.y, half.z),
        Vec3::new(half.x, -half.y, half.z),
        Vec3::new(half.x, half.y, half.z),
        Vec3::new(-half.x, half.y, half.z),
    ];
    let edges = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    for (a, b) in edges {
        gizmos.line(pos + rot * corners[a], pos + rot * corners[b], color);
    }
}

fn draw_hierarchy_arrows<S: Component>(
    mut gizmos: Gizmos<ColliderGizmoGroup>,
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
