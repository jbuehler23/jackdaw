//! Collider wireframes and the arrows from a selected body to its colliders,
//! drawn over the viewport, with the operators and menu rows that show and
//! hide them.

use avian3d::debug_render::{PhysicsGizmoExt, PhysicsGizmos};
use avian3d::prelude::*;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api::viewport::ViewportOverlay;

/// The overlay colours.
pub mod physics_colors {
    use bevy::prelude::Color;

    pub const COLLIDER_WIREFRAME: Color = Color::srgba(0.0, 1.0, 0.5, 0.7);
    pub const SENSOR_WIREFRAME: Color = Color::srgba(0.0, 0.8, 1.0, 0.5);
    pub const COLLIDER_SELECTED: Color = Color::srgba(0.0, 1.0, 0.5, 1.0);
    pub const SENSOR_SELECTED: Color = Color::srgba(0.0, 0.8, 1.0, 0.85);
    pub const COLLIDER_HIERARCHY_ARROW: Color = Color::srgba(0.4, 0.7, 1.0, 0.6);
}

/// Which physics overlays the viewport draws.
#[derive(Resource, Clone, PartialEq, Default)]
pub struct PhysicsOverlayConfig {
    pub show_colliders: bool,
    pub show_hierarchy_arrows: bool,
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<PhysicsOverlayConfig>()
        .init_gizmo_group::<PhysicsGizmos>()
        .add_systems(
            PostUpdate,
            (draw_collider_gizmos, draw_hierarchy_arrows)
                .after(bevy::transform::TransformSystems::Propagate)
                .run_if(in_state(AppState::Editor)),
        );

    let mut store = app.world_mut().resource_mut::<GizmoConfigStore>();
    let (config, _) = store.config_mut::<PhysicsGizmos>();
    config.depth_bias = -0.5;
    config.line.width = 1.5;
}

pub(crate) fn register(ctx: &mut ExtensionRegistrar) {
    ctx.register_operator::<ViewToggleColliderGizmosOp>()
        .register_operator::<ViewToggleHierarchyArrowsOp>()
        .register_menu_entry::<ViewToggleColliderGizmosOp>(TopLevelMenu::View)
        .register_menu_entry::<ViewToggleHierarchyArrowsOp>(TopLevelMenu::View)
        .register_viewport_overlay::<ViewToggleColliderGizmosOp>("Colliders", false);
}

/// Show or hide collider wireframes.
#[operator(
    id = "view.toggle_collider_gizmos",
    label = "Toggle Collider Gizmos",
    allows_undo = false
)]
pub fn view_toggle_collider_gizmos(
    _: In<OperatorParameters>,
    mut config: ResMut<PhysicsOverlayConfig>,
    mut overlays: Query<&mut ViewportOverlay>,
) -> OperatorResult {
    config.show_colliders = !config.show_colliders;
    for mut overlay in &mut overlays {
        if overlay.operator_id == ViewToggleColliderGizmosOp::ID {
            overlay.shown = config.show_colliders;
        }
    }
    OperatorResult::Finished
}

/// Show or hide the arrows from a selected body to its colliders.
#[operator(
    id = "view.toggle_hierarchy_arrows",
    label = "Toggle Hierarchy Arrows",
    allows_undo = false
)]
pub fn view_toggle_hierarchy_arrows(
    _: In<OperatorParameters>,
    mut config: ResMut<PhysicsOverlayConfig>,
) -> OperatorResult {
    config.show_hierarchy_arrows = !config.show_hierarchy_arrows;
    OperatorResult::Finished
}

fn draw_collider_gizmos(
    mut gizmos: Gizmos<PhysicsGizmos>,
    config: Res<PhysicsOverlayConfig>,
    colliders: Query<(
        Entity,
        &Collider,
        &GlobalTransform,
        &InheritedVisibility,
        Option<&Sensor>,
    )>,
    selected_bodies: Query<Entity, (With<RigidBody>, With<Selected>)>,
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

        let position = Position::from(tf).0;
        let rotation = Rotation::from(tf);
        gizmos.draw_collider(collider, position, rotation, color);
    }
}

fn draw_hierarchy_arrows(
    mut gizmos: Gizmos<PhysicsGizmos>,
    config: Res<PhysicsOverlayConfig>,
    selected_bodies: Query<(Entity, &GlobalTransform), (With<RigidBody>, With<Selected>)>,
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
