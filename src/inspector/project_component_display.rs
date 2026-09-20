//! Inspector rendering and edit conversions for project (schema-reported)
//! components.
//!
//! These types are never real ECS components in the editor: mapping
//! project code would leak on every refresh, so the editor keeps only the schema
//! extracted out-of-process ([`crate::project_types::ProjectTypes`]) and
//! stores authored values in the scene document. The inspector renders a
//! project component through the same schema field rows a definition card
//! uses, so scalars, enums and lists edit the same way. Field edits
//! round-trip back to the document without a concrete registration.

use bevy::ecs::system::SystemState;
use bevy::prelude::*;

use jackdaw_bsn::{BsnPatch, BsnValue, SceneBsnAst};
use jackdaw_feathers::icons::{EditorFont, IconFont};
use jackdaw_schema::TypeSchema;
use serde_json::Value;

use crate::project_types::ProjectTypes;
use crate::schema_values;

use super::schema_fields::{SchemaFieldContext, spawn_schema_fields};

/// Render every field of a project component onto `body`. Values come from
/// the authored document patch where present, otherwise from the schema
/// default. Rows are the same schema field widgets a definition card uses.
pub(crate) fn fill_project_component_fields(
    world: &mut World,
    body: Entity,
    source: Entity,
    node: Entity,
    type_path: &str,
) {
    let Some(schema) = world
        .get_resource::<ProjectTypes>()
        .and_then(|types| types.component(type_path).cloned())
    else {
        return;
    };
    let value = project_component_json(world, node, &schema);
    let registry = world.resource::<AppTypeRegistry>().clone();
    let server = world.get_resource::<AssetServer>().cloned();
    let editor_font = world.resource::<EditorFont>().0.clone();
    let icon_font = world.resource::<IconFont>().0.clone();
    let asset_types = super::schema_fields::asset_path_types(world);
    world.resource_scope(|world, types: Mut<ProjectTypes>| {
        let mut state: SystemState<(Commands, Query<&Name>)> = SystemState::new(world);
        let Ok((mut commands, names)) = state.get_mut(world) else {
            return;
        };
        let ctx = SchemaFieldContext {
            types: &types,
            source,
            type_path,
            names: &names,
            registry: &registry,
            server: server.as_ref(),
            asset_types: &asset_types,
            editor_font: &editor_font,
            icon_font: &icon_font,
        };
        spawn_schema_fields(&mut commands, body, &ctx, &schema, &value, "", 0);
        state.apply(world);
    });
}

/// The component's current value as JSON: what the document authors, and the
/// type's default for the rest.
fn project_component_json(world: &World, node: Entity, schema: &TypeSchema) -> Value {
    let Some(types) = world.get_resource::<ProjectTypes>() else {
        return schema_values::type_default_json(schema)
            .unwrap_or(Value::Object(Default::default()));
    };
    let authored = world.get_resource::<SceneBsnAst>().and_then(|ast| {
        let patch = ast.find_patch_by_type_path(node, &schema.type_path)?;
        match ast.get_patch(patch)? {
            BsnPatch::Struct(data) => Some(data.clone()),
            _ => None,
        }
    });
    match authored {
        Some(data) => schema_values::value_json(world, types, schema, &data),
        None => {
            schema_values::type_default_json(schema).unwrap_or(Value::Object(Default::default()))
        }
    }
}

/// Render a component the document holds that the editor cannot edit: no
/// registration answers to it, and the schema reports the type it belongs to
/// at most. Every authored field is shown as the document spells it.
pub(crate) fn spawn_document_component_fields(
    commands: &mut Commands,
    body_entity: Entity,
    ast: &SceneBsnAst,
    node: Entity,
    type_path: &str,
    variant_of_a_reported_type: bool,
) {
    let note = match variant_of_a_reported_type {
        true => "This variant is shown as the document spells it, read-only.",
        false => "The project schema does not describe this type, so its fields are read-only.",
    };
    commands.spawn((
        Text::new(note),
        TextFont {
            font_size: jackdaw_feathers::tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(jackdaw_feathers::tokens::TEXT_SECONDARY),
        ChildOf(body_entity),
    ));
    for (name, value) in authored_fields(ast, node, type_path) {
        commands.spawn((
            Text::new(format!("{name}: {value}")),
            TextFont {
                font_size: jackdaw_feathers::tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(jackdaw_feathers::tokens::TEXT_SECONDARY),
            ChildOf(body_entity),
        ));
    }
}

/// The fields a document patch spells, as the text each one reads.
fn authored_fields(ast: &SceneBsnAst, node: Entity, type_path: &str) -> Vec<(String, String)> {
    let Some(patch) = ast
        .find_patch_by_type_path(node, type_path)
        .and_then(|patch| ast.get_patch(patch))
    else {
        return Vec::new();
    };
    match patch {
        jackdaw_bsn::BsnPatch::Struct(data) => data
            .fields
            .0
            .iter()
            .map(|field| (field.name.clone(), shown_value(&field.value)))
            .collect(),
        jackdaw_bsn::BsnPatch::TupleStruct(data) => data
            .values
            .iter()
            .enumerate()
            .map(|(at, value)| (at.to_string(), shown_value(value)))
            .collect(),
        _ => Vec::new(),
    }
}

/// One document value as a line a person reads.
fn shown_value(value: &BsnValue) -> String {
    match bsn_value_to_json(value) {
        Some(json) => json.to_string(),
        None => format!("{value:?}"),
    }
}

/// Convert a document field value to JSON for display. Only the scalar variants
/// the inspector edits are handled; richer values fall through to the read-only
/// row.
fn bsn_value_to_json(value: &BsnValue) -> Option<serde_json::Value> {
    match value {
        BsnValue::Float(f) => Some(serde_json::json!(f)),
        BsnValue::Int(i) => i64::try_from(*i).ok().map(|v| serde_json::json!(v)),
        BsnValue::Bool(b) => Some(serde_json::json!(b)),
        BsnValue::String(s) => Some(serde_json::json!(s)),
        _ => None,
    }
}
