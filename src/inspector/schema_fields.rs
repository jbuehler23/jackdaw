//! Field rows for a value whose type the editor knows only as the project's
//! schema.
//!
//! The rows are the ones a compiled type gets: a scalar goes through the
//! reflected field row, an enum through the same variant menu, and a list
//! through the same add, move and remove controls. Only the shape comes from
//! somewhere else, since there is no registration to walk.

use bevy::asset::AssetServer;
use bevy::feathers::controls::{FeathersListRow, FeathersListView, FeathersListViewProps};
use bevy::prelude::*;
use jackdaw_schema::{FieldSchema, TypeKind, TypeSchema};
use serde_json::Value;

use crate::project_types::ProjectTypes;
use crate::schema_values::{item_type_path, native_value_for_json};

use super::reflect_fields::{
    spawn_enum_menu, spawn_field_row, spawn_list_add_button, spawn_list_row_controls,
    spawn_text_row,
};

/// Everything the rows need that does not change as the walk descends.
pub(crate) struct SchemaFieldContext<'a> {
    pub(crate) types: &'a ProjectTypes,
    pub(crate) source: Entity,
    pub(crate) type_path: &'a str,
    pub(crate) names: &'a Query<'a, 'a, &'static Name>,
    pub(crate) registry: &'a AppTypeRegistry,
    pub(crate) server: Option<&'a AssetServer>,
    pub(crate) editor_font: &'a Handle<Font>,
    pub(crate) icon_font: &'a Handle<Font>,
}

/// Spawn a row for every field the schema declares, reading each from `value`.
pub(crate) fn spawn_schema_fields(
    commands: &mut Commands,
    parent: Entity,
    ctx: &SchemaFieldContext,
    schema: &TypeSchema,
    value: &Value,
    prefix: &str,
    depth: usize,
) {
    if depth >= super::MAX_REFLECT_DEPTH {
        return;
    }
    for field in &schema.fields {
        let path = format!("{prefix}{}", field.name);
        let held = value.get(&field.name).cloned().unwrap_or(Value::Null);
        spawn_one_field(commands, parent, ctx, field, &held, &path, depth);
    }
}

fn spawn_one_field(
    commands: &mut Commands,
    parent: Entity,
    ctx: &SchemaFieldContext,
    field: &FieldSchema,
    held: &Value,
    path: &str,
    depth: usize,
) {
    if let Some(item_type) = item_type_path(field) {
        spawn_list_field(
            commands,
            parent,
            ctx,
            &field.name,
            item_type,
            held,
            path,
            depth,
        );
        return;
    }
    if let Some(schema) = ctx.types.type_schema(&field.type_path) {
        match schema.kind {
            TypeKind::Enum if all_unit_variants(schema) => {
                spawn_enum_field(
                    commands,
                    parent,
                    ctx,
                    &field.name,
                    schema,
                    held,
                    path,
                    depth,
                );
            }
            TypeKind::Struct => {
                spawn_text_row(commands, parent, &format!("{}:", field.name), depth);
                spawn_schema_fields(
                    commands,
                    parent,
                    ctx,
                    schema,
                    held,
                    &format!("{path}."),
                    depth + 1,
                );
            }
            _ => spawn_read_only(commands, parent, &field.name, held, depth),
        }
        return;
    }
    let value = {
        let registry = ctx.registry.read();
        native_value_for_json(&registry, ctx.server, None, &field.type_path, held)
    };
    let Some(value) = value else {
        spawn_read_only(commands, parent, &field.name, held, depth);
        return;
    };
    spawn_field_row(
        commands,
        parent,
        &field.name,
        value.as_ref(),
        depth,
        path.to_string(),
        ctx.source,
        ctx.type_path,
        ctx.names,
        ctx.registry,
        ctx.editor_font,
        ctx.icon_font,
    );
}

#[expect(
    clippy::too_many_arguments,
    reason = "a list row is a field row: where it goes, what it holds, and the context every control needs"
)]
fn spawn_list_field(
    commands: &mut Commands,
    parent: Entity,
    ctx: &SchemaFieldContext,
    name: &str,
    item_type: &str,
    held: &Value,
    path: &str,
    depth: usize,
) {
    let items = held.as_array().cloned().unwrap_or_default();
    spawn_text_row(
        commands,
        parent,
        &format!("{name}: [{} items]", items.len()),
        depth,
    );
    if !items.is_empty() {
        let list = commands
            .spawn_scene(FeathersListView::scene(FeathersListViewProps::default()))
            .insert(ChildOf(parent))
            .id();
        for (index, item) in items.iter().enumerate() {
            let row = commands
                .spawn_scene(FeathersListRow::scene())
                .insert(ChildOf(list))
                .id();
            let item_path = format!("{path}[{index}]");
            match ctx.types.type_schema(item_type) {
                Some(schema) if schema.kind == TypeKind::Struct => {
                    spawn_schema_fields(
                        commands,
                        row,
                        ctx,
                        schema,
                        item,
                        &format!("{item_path}."),
                        depth + 1,
                    );
                }
                _ => spawn_item_value(commands, row, ctx, item_type, item, &item_path, depth + 1),
            }
            spawn_list_row_controls(
                commands,
                row,
                ctx.source,
                ctx.type_path,
                path,
                index,
                items.len(),
            );
        }
    }
    spawn_list_add_button(commands, parent, ctx.source, ctx.type_path, path);
}

fn spawn_item_value(
    commands: &mut Commands,
    parent: Entity,
    ctx: &SchemaFieldContext,
    item_type: &str,
    item: &Value,
    path: &str,
    depth: usize,
) {
    let value = {
        let registry = ctx.registry.read();
        native_value_for_json(&registry, ctx.server, None, item_type, item)
    };
    let Some(value) = value else {
        spawn_read_only(commands, parent, "", item, depth);
        return;
    };
    spawn_field_row(
        commands,
        parent,
        "",
        value.as_ref(),
        depth,
        path.to_string(),
        ctx.source,
        ctx.type_path,
        ctx.names,
        ctx.registry,
        ctx.editor_font,
        ctx.icon_font,
    );
}

#[expect(
    clippy::too_many_arguments,
    reason = "an enum row is a field row: where it goes, what it holds, and the context every control needs"
)]
fn spawn_enum_field(
    commands: &mut Commands,
    parent: Entity,
    ctx: &SchemaFieldContext,
    name: &str,
    schema: &TypeSchema,
    held: &Value,
    path: &str,
    depth: usize,
) {
    let variants: Vec<String> = schema
        .variants
        .iter()
        .map(|variant| variant.name.clone())
        .collect();
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(jackdaw_feathers::tokens::SPACING_XS),
                padding: UiRect::left(Val::Px(depth as f32 * jackdaw_feathers::tokens::SPACING_MD)),
                width: Val::Percent(100.0),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();
    commands.spawn((
        Text::new(format!("{name}:")),
        TextFont {
            font_size: jackdaw_feathers::tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(jackdaw_feathers::tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));
    spawn_enum_menu(
        commands,
        row,
        &variants,
        &current_variant(held),
        path,
        ctx.source,
        ctx.type_path,
    );
}

/// The variant a value names: a bare name, or the one key of a variant
/// carrying fields.
fn current_variant(held: &Value) -> String {
    if let Some(name) = held.as_str() {
        return name.to_string();
    }
    held.as_object()
        .and_then(|object| object.keys().next().cloned())
        .unwrap_or_default()
}

fn all_unit_variants(schema: &TypeSchema) -> bool {
    !schema.variants.is_empty()
        && schema
            .variants
            .iter()
            .all(|variant| variant.fields.is_empty())
}

fn spawn_read_only(
    commands: &mut Commands,
    parent: Entity,
    name: &str,
    held: &Value,
    depth: usize,
) {
    let shown = match held {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let label = if name.is_empty() {
        shown
    } else {
        format!("{name}: {shown}")
    };
    spawn_text_row(commands, parent, &label, depth);
}
