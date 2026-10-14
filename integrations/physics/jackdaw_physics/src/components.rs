//! Turning physics on and off for an entity: the inspector's Rigid Body and
//! Collider toggles, and the `physics.enable` and `physics.disable`
//! operators that set or clear both at once.

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_api::document::SceneDocument;
use jackdaw_api::op::with_history_span;
use jackdaw_api::prelude::*;
use jackdaw_avian_integration::{AvianCollider, ImpliedStaticBody};

/// The inspector category the physics toggles sit in.
pub const PHYSICS_CATEGORY: &str = "physics";

pub(crate) fn register(ctx: &mut ExtensionRegistrar) {
    ctx.register_operator::<PhysicsEnableOp>()
        .register_operator::<PhysicsDisableOp>()
        .register_inspector_chip(PHYSICS_CATEGORY, "Rigid Body", RigidBody::type_path())
        .register_inspector_chip(PHYSICS_CATEGORY, "Collider", AvianCollider::type_path())
        .register_brush_default(AvianCollider::default())
        .register_brush_default(RigidBody::Static);
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<PendingPhysicsEdits>().add_systems(
        Update,
        apply_physics_edits.run_if(in_state(AppState::Editor)),
    );
}

/// What `physics.enable` and `physics.disable` asked for, applied on the next
/// frame because applying them calls the editor's component operators.
#[derive(Resource, Default)]
struct PendingPhysicsEdits(Vec<(Entity, PhysicsEdit)>);

#[derive(Clone, Copy)]
enum PhysicsEdit {
    Enable,
    Disable,
}

fn apply_physics_edits(world: &mut World) {
    let edits = std::mem::take(&mut world.resource_mut::<PendingPhysicsEdits>().0);
    for (entity, edit) in edits {
        match edit {
            PhysicsEdit::Enable => enable_physics(world, entity),
            PhysicsEdit::Disable => disable_physics(world, entity),
        }
    }
}

fn has_primary_selection(selection: Res<Selection>) -> bool {
    selection.primary().is_some()
}

/// Give the entity a static rigid body and a collider.
#[operator(
    id = "physics.enable",
    label = "Enable Physics",
    description = "Give the selected entity a static rigid body and a collider, as one undo step.",
    allows_undo = false,
    is_available = has_primary_selection,
    params(entity(Entity, doc = "Entity to make physical. Defaults to the selection.")),
)]
pub fn physics_enable(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut pending: ResMut<PendingPhysicsEdits>,
) -> OperatorResult {
    let Some(entity) = params.as_entity("entity").or(selection.primary()) else {
        return OperatorResult::Cancelled;
    };
    pending.0.push((entity, PhysicsEdit::Enable));
    OperatorResult::Finished
}

/// Take every physics component the entity authored off it.
#[operator(
    id = "physics.disable",
    label = "Disable Physics",
    description = "Remove the rigid body, collider and every other physics component the \
                   selected entity authored, as one undo step.",
    allows_undo = false,
    is_available = has_primary_selection,
    params(entity(Entity, doc = "Entity to make non-physical. Defaults to the selection.")),
)]
pub fn physics_disable(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut pending: ResMut<PendingPhysicsEdits>,
) -> OperatorResult {
    let Some(entity) = params.as_entity("entity").or(selection.primary()) else {
        return OperatorResult::Cancelled;
    };
    pending.0.push((entity, PhysicsEdit::Disable));
    OperatorResult::Finished
}

/// Whether a component type path is one `physics.disable` takes off: the
/// collider, or anything of avian's.
pub fn is_physics_type_path(type_path: &str) -> bool {
    type_path == AvianCollider::type_path() || type_path.starts_with("avian3d::")
}

fn enable_physics(world: &mut World, entity: Entity) {
    let Ok(entity_ref) = world.get_entity(entity) else {
        return;
    };
    let has_collider = entity_ref.contains::<AvianCollider>();
    let has_body = entity_ref.contains::<RigidBody>();
    let implied_body = entity_ref.contains::<ImpliedStaticBody>();
    with_history_span(world, PhysicsEnableOp::LABEL, |world| {
        if !has_collider {
            call(
                world,
                "component.add",
                entity,
                AvianCollider::type_path(),
                None,
            );
        }
        if !has_body {
            call(world, "component.add", entity, RigidBody::type_path(), None);
        }
        if !has_body || implied_body {
            call(
                world,
                "component.set",
                entity,
                RigidBody::type_path(),
                Some("\"Static\""),
            );
        }
    });
}

fn disable_physics(world: &mut World, entity: Entity) {
    let authored: Vec<String> = SceneDocument::authored_components(world, entity)
        .into_iter()
        .filter(|type_path| is_physics_type_path(type_path))
        .collect();
    with_history_span(world, PhysicsDisableOp::LABEL, |world| {
        for type_path in &authored {
            call(world, "component.remove", entity, type_path, None);
        }
    });
}

/// Call one of the editor's component operators on `entity`, setting the
/// whole component to `value` when one is given.
fn call(world: &mut World, id: &str, entity: Entity, type_path: &str, value: Option<&str>) {
    let mut call = world
        .operator(id.to_string())
        .param("entity", entity)
        .param("type_path", type_path.to_string());
    if let Some(value) = value {
        call = call
            .param("field", String::new())
            .param("value", value.to_string());
    }
    if let Err(error) = call.call() {
        warn!("{id} {type_path} on {entity}: {error}");
    }
}
