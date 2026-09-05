//! Operators a caller with no pointer needs: the parametric form of gestures
//! that otherwise only a mouse can perform. Each pushes the same command the
//! gesture pushes, so it undoes the same way.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_commands::{CommandGroup, EditorCommand};
use jackdaw_feathers::dialog::{DialogChoices, EditorDialog, answer_dialog, resolve_dialog_choice};

use crate::commands::{CommandHistory, ReparentEntity, SetBsnField, SetTransform, SpawnEntity};
use crate::entity_ops::{EntityTemplate, spawn_template_in_document};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<EntityAddGroupOp>()
        .register_operator::<EntitySetTransformOp>()
        .register_operator::<ComponentSetOp>()
        .register_operator::<DialogAnswerOp>();
}

/// Press a button on the dialog that is up. `jackdaw/status` reports the
/// pending dialog and its choices.
#[operator(
    id = "dialog.answer",
    label = "Answer Dialog",
    description = "Press a button on the dialog that is up, by label or index.",
    allows_undo = false,
    is_available = a_dialog_is_up,
    params(choice(
        String,
        doc = "Button label, or its index counting from the primary action. \
               Matched exactly, then as an index, then as an unambiguous \
               case-insensitive prefix. Defaults to the primary action."
    ))
)]
pub(crate) fn dialog_answer(
    params: In<OperatorParameters>,
    dialogs: Query<(Entity, &DialogChoices), With<EditorDialog>>,
    mut commands: Commands,
) -> OperatorResult {
    // Entity ids rise with each spawn, so the greatest is the dialog on top.
    let Some((dialog, choices)) = dialogs.iter().max_by_key(|(entity, _)| *entity) else {
        commands.queue(|world: &mut World| warn_caller(world, "dialog.answer: no dialog is up"));
        return OperatorResult::Cancelled;
    };
    let wanted = params.as_str("choice").unwrap_or("0");
    let choice = match resolve_dialog_choice(choices, wanted) {
        Ok(choice) => choice,
        Err(err) => {
            let message = format!(
                "dialog.answer: {wanted:?} was not pressed: {err}; it offers {:?}",
                choices.labels()
            );
            commands.queue(move |world: &mut World| warn_caller(world, message));
            return OperatorResult::Cancelled;
        }
    };
    answer_dialog(&mut commands, dialog, choice);
    OperatorResult::Finished
}

fn a_dialog_is_up(dialogs: Query<(), With<EditorDialog>>) -> bool {
    !dialogs.is_empty()
}

/// Add an empty node, named, optionally under another node, as one undo entry.
#[operator(
    id = "entity.add.group",
    label = "Add Group",
    description = "Add an empty node, named, optionally under another node.",
    allows_undo = false,
    params(
        name(String, doc = "Name for the new node. Defaults to \"Group\"."),
        parent(Entity, doc = "Node that adopts it. Top level when omitted."),
    )
)]
pub(crate) fn entity_add_group(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let name = params
        .as_str("name")
        .filter(|name| !name.is_empty())
        .unwrap_or("Group")
        .to_string();
    let parent = params.as_entity("parent");
    commands.queue(move |world: &mut World| {
        // Spawn through the same command `entity.add.empty` pushes, so undo
        // despawns the node rather than only taking back its rename.
        let mut spawn = SpawnEntity {
            spawned: None,
            spawn_fn: Box::new(|world: &mut World| {
                spawn_template_in_document(world, EntityTemplate::Empty)
            }),
            label: "Add Group".to_string(),
        };
        spawn.execute(world);
        let Some(entity) = spawn
            .spawned
            .filter(|entity| *entity != Entity::PLACEHOLDER)
        else {
            warn_caller(world, "entity.add.group: could not spawn the node");
            return;
        };

        let old_name = world
            .get::<Name>(entity)
            .map(|name| jackdaw_bsn::BsnValue::String(name.as_str().to_string()));
        let mut group: Vec<Box<dyn EditorCommand>> = vec![
            Box::new(spawn),
            Box::new(SetBsnField {
                entity,
                type_path: crate::commands::NAME_TYPE_PATH.to_string(),
                field_path: String::new(),
                old_value: old_name,
                new_value: jackdaw_bsn::BsnValue::String(name.clone()),
                was_derived: false,
            }),
        ];
        if let Some(parent) = parent
            && world.get_entity(parent).is_ok()
        {
            group.push(Box::new(ReparentEntity {
                entity,
                old_parent: None,
                new_parent: Some(parent),
            }));
        }

        // The spawn already ran; the rest runs here and the group goes on the
        // history as one already-executed entry.
        for command in group.iter_mut().skip(1) {
            command.execute(world);
        }
        world
            .resource_mut::<CommandHistory>()
            .push_executed(Box::new(CommandGroup {
                label: format!("Add Group {name}"),
                commands: group,
            }));
    });
    OperatorResult::Finished
}

/// Place an entity: position, rotation in degrees, scale. Every field is
/// optional and an omitted one is left alone.
#[operator(
    id = "entity.set_transform",
    label = "Set Transform",
    description = "Set an entity's position, rotation and scale.",
    allows_undo = false,
    params(
        entity(Entity, doc = "Entity to place. Defaults to the selection."),
        x(f64, doc = "Position X. Unchanged when omitted."),
        y(f64, doc = "Position Y. Unchanged when omitted."),
        z(f64, doc = "Position Z. Unchanged when omitted."),
        yaw(f64, doc = "Rotation about Y in degrees. Unchanged when omitted."),
        pitch(f64, doc = "Rotation about X in degrees. Unchanged when omitted."),
        roll(f64, doc = "Rotation about Z in degrees. Unchanged when omitted."),
        sx(f64, doc = "Scale X. Unchanged when omitted."),
        sy(f64, doc = "Scale Y. Unchanged when omitted."),
        sz(f64, doc = "Scale Z. Unchanged when omitted."),
        world(
            bool,
            doc = "Read the values as world space rather than as the entity's own \
                   local transform. Defaults to false."
        ),
    )
)]
pub(crate) fn entity_set_transform(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let entity = params.as_entity("entity")?;
    let params = params.0;
    commands.queue(move |world: &mut World| {
        let Some(old) = world.get::<Transform>(entity).copied() else {
            warn_caller(
                world,
                format!("entity.set_transform: {entity} has no Transform"),
            );
            return;
        };
        let in_world = params.as_bool("world").unwrap_or(false);

        // In world space the values are read from and written back through the
        // parent's frame.
        let parent = world
            .get::<ChildOf>(entity)
            .map(ChildOf::parent)
            .and_then(|parent| world.get::<GlobalTransform>(parent).copied())
            .unwrap_or_default();
        let basis = if in_world {
            world
                .get::<GlobalTransform>(entity)
                .copied()
                .unwrap_or_default()
                .compute_transform()
        } else {
            old
        };

        let mut wanted = basis;
        let axis = |key: &str, current: f32| params.as_float(key).map_or(current, |v| v as f32);
        wanted.translation = Vec3::new(
            axis("x", wanted.translation.x),
            axis("y", wanted.translation.y),
            axis("z", wanted.translation.z),
        );
        wanted.scale = Vec3::new(
            axis("sx", wanted.scale.x),
            axis("sy", wanted.scale.y),
            axis("sz", wanted.scale.z),
        );
        // Only recomposed when a rotation was asked for: several euler triples
        // name the same quaternion, so a needless round-trip rewrites it.
        let angles = ["yaw", "pitch", "roll"].map(|key| params.as_float(key));
        if angles.iter().any(Option::is_some) {
            let (current_yaw, current_pitch, current_roll) =
                wanted.rotation.to_euler(EulerRot::YXZ);
            let [yaw, pitch, roll] = angles;
            wanted.rotation = Quat::from_euler(
                EulerRot::YXZ,
                yaw.map_or(current_yaw, |v| (v as f32).to_radians()),
                pitch.map_or(current_pitch, |v| (v as f32).to_radians()),
                roll.map_or(current_roll, |v| (v as f32).to_radians()),
            );
        }

        let new_transform = if in_world {
            Transform::from_matrix(parent.to_matrix().inverse() * wanted.to_matrix())
        } else {
            wanted
        };

        if new_transform == old {
            return;
        }
        let mut cmd: Box<dyn EditorCommand> = Box::new(SetTransform {
            entity,
            old_transform: old,
            new_transform,
        });
        cmd.execute(world);
        world.resource_mut::<CommandHistory>().push_executed(cmd);
    });
    OperatorResult::Finished
}

/// Set one field on one entity's component, by reflection path. Unlike
/// `field.set`, this writes to the named entity and leaves the selection alone.
#[operator(
    id = "component.set",
    label = "Set Component Field",
    description = "Set one field on one entity's component by reflection path.",
    allows_undo = false,
    params(
        entity(Entity, doc = "Entity whose component is edited."),
        type_path(
            String,
            doc = "Fully-qualified Bevy reflected type path of the component."
        ),
        field(
            String,
            doc = "Dotted reflection path within the component (e.g. \"translation.x\"). \
                   Empty sets the whole component."
        ),
        value(
            String,
            doc = "New value as JSON: 12, true, \"text\", or {\"Px\": 12}."
        ),
    )
)]
pub(crate) fn component_set(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let entity = params.as_entity("entity")?;
    let type_path = params.as_str("type_path").map(str::to_string)?;
    let field_path = params.as_str("field").unwrap_or_default().to_string();
    let value = param_json(&params, "value")?;
    commands.queue(move |world: &mut World| {
        if !crate::commands::field_edit_commit_on(world, entity, &type_path, &field_path, &value) {
            warn_caller(
                world,
                format!("component.set: {value} is not a value for {type_path}.{field_path}"),
            );
        }
    });
    OperatorResult::Finished
}

/// A parameter as JSON. A string parses as JSON when it can and stands for
/// itself when it cannot.
fn param_json(params: &OperatorParameters, key: &str) -> Option<serde_json::Value> {
    use jackdaw_scene_types::PropertyValue;
    match params.get(key)? {
        PropertyValue::Bool(value) => Some(serde_json::Value::Bool(*value)),
        PropertyValue::Int(value) => Some(serde_json::json!(value)),
        PropertyValue::Float(value) => Some(serde_json::json!(value)),
        PropertyValue::String(value) => Some(
            serde_json::from_str(value)
                .unwrap_or_else(|_| serde_json::Value::String(value.to_string())),
        ),
        _ => None,
    }
}
