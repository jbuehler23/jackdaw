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
    /// What each file the project holds says it is, for a field spelling a
    /// reference as the path itself.
    pub(crate) asset_types: &'a AssetPathTypes,
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
    let declared = declared_asset_type(field);
    if let Some(item_type) = item_type_path(field) {
        spawn_list_field(
            commands,
            parent,
            ctx,
            ListField {
                name: &field.name,
                item_type,
                element_asset: declared,
            },
            held,
            path,
            depth,
        );
        return;
    }
    if let Some(asset_type_path) =
        declared.or_else(|| asset_type_of_path(ctx, &field.type_path, held))
    {
        spawn_path_row(
            commands,
            parent,
            ctx,
            &field.name,
            asset_type_path,
            path,
            depth,
        );
        return;
    }
    if let Some(schema) = ctx.types.type_schema(&field.type_path) {
        match schema.kind {
            TypeKind::Enum => {
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

/// The list a row stands for: what it is called, what it holds, and the asset
/// kind its elements name when the type declares one.
struct ListField<'a> {
    name: &'a str,
    item_type: &'a str,
    element_asset: Option<String>,
}

fn spawn_list_field(
    commands: &mut Commands,
    parent: Entity,
    ctx: &SchemaFieldContext,
    field: ListField,
    held: &Value,
    path: &str,
    depth: usize,
) {
    let ListField {
        name,
        item_type,
        element_asset,
    } = field;
    let items = held.as_array().cloned().unwrap_or_default();
    let element_asset = element_asset.or_else(|| {
        items
            .iter()
            .find_map(|item| asset_type_of_path(ctx, item_type, item))
    });
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
                Some(schema) if schema.kind == TypeKind::Enum => {
                    spawn_enum_field(commands, row, ctx, "", schema, item, &item_path, depth + 1);
                }
                _ => match element_asset.clone() {
                    Some(asset_type_path) => spawn_path_row(
                        commands,
                        row,
                        ctx,
                        "",
                        asset_type_path,
                        &item_path,
                        depth + 1,
                    ),
                    None => {
                        spawn_item_value(
                            commands,
                            row,
                            ctx,
                            item_type,
                            item,
                            &item_path,
                            depth + 1,
                        );
                    }
                },
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

/// The asset kind a field's own type declares its strings name, through
/// `@AssetRef`.
fn declared_asset_type(field: &FieldSchema) -> Option<String> {
    use bevy::reflect::TypePath as _;

    if field.asset_type_path.is_empty() {
        return None;
    }
    let names = item_type_path(field).unwrap_or(&field.type_path);
    (names == String::type_path()).then(|| field.asset_type_path.clone())
}

/// The asset a string field names, for a schema that spells a reference as the
/// path itself rather than as a handle. `None` when the type is not a string,
/// or the string names no file this project holds.
fn asset_type_of_path(ctx: &SchemaFieldContext, type_path: &str, held: &Value) -> Option<String> {
    use bevy::reflect::TypePath as _;

    if type_path != String::type_path() {
        return None;
    }
    let named = held.as_str().filter(|path| !path.is_empty())?;
    ctx.asset_types.get(named).cloned()
}

/// The type each file the project holds is, keyed by the path naming it.
pub(crate) type AssetPathTypes = bevy::platform::collections::HashMap<String, String>;

/// Read what every indexed file says it is, so the walk can tell a path from
/// any other string without the index in hand.
pub(crate) fn asset_path_types(world: &World) -> AssetPathTypes {
    use path_slash::PathExt as _;

    let Some(index) = world.get_resource::<crate::asset_index::AssetIndex>() else {
        return AssetPathTypes::default();
    };
    index
        .iter()
        .map(|entry| {
            (
                entry.path.to_slash_lossy().into_owned(),
                entry.type_path.clone(),
            )
        })
        .collect()
}

/// A row for a field naming a file, whether the schema spells it as a handle
/// or as the path itself.
fn spawn_path_row(
    commands: &mut Commands,
    parent: Entity,
    ctx: &SchemaFieldContext,
    label: &str,
    asset_type_path: String,
    path: &str,
    depth: usize,
) {
    super::asset_row::spawn_asset_row(
        commands,
        parent,
        super::asset_row::AssetRowProps {
            target: super::asset_row::AssetFieldTarget::Inspected {
                source: ctx.source,
                type_path: ctx.type_path.to_string(),
            },
            field_path: path.to_string(),
            asset_type_path,
            label: label.to_string(),
            indent: depth.min(u8::MAX as usize) as u8,
        },
        ctx.icon_font,
    );
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
    if variants.is_empty() {
        spawn_read_only(commands, parent, name, held, depth);
        return;
    }
    let column = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(jackdaw_feathers::tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();
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
            ChildOf(column),
        ))
        .id();
    if !name.is_empty() {
        commands.spawn((
            Text::new(format!("{name}:")),
            TextFont {
                font_size: jackdaw_feathers::tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(jackdaw_feathers::tokens::TEXT_SECONDARY),
            ChildOf(row),
        ));
    }
    let chosen = current_variant(held);
    spawn_enum_menu(
        commands,
        row,
        &variants,
        &chosen,
        path,
        ctx.source,
        ctx.type_path,
    );
    spawn_variant_fields(commands, column, ctx, schema, &chosen, held, path, depth);
}

/// The rows for the fields the chosen variant carries, under the menu that
/// chose it. A variant carrying nothing adds none.
#[expect(
    clippy::too_many_arguments,
    reason = "a variant's rows are field rows: where they go, what they hold, and the context every control needs"
)]
fn spawn_variant_fields(
    commands: &mut Commands,
    parent: Entity,
    ctx: &SchemaFieldContext,
    schema: &TypeSchema,
    chosen: &str,
    held: &Value,
    path: &str,
    depth: usize,
) {
    if depth + 1 >= super::MAX_REFLECT_DEPTH {
        return;
    }
    let Some(variant) = schema
        .variants
        .iter()
        .find(|variant| variant.name == chosen)
    else {
        return;
    };
    let body = held.get(chosen);
    for (index, field) in variant.fields.iter().enumerate() {
        let (held_field, field_path) = if field.name.parse::<usize>().is_ok() {
            (
                body.and_then(|body| body.get(index)),
                format!("{path}.{chosen}[{index}]"),
            )
        } else {
            (
                body.and_then(|body| body.get(&field.name)),
                format!("{path}.{chosen}.{}", field.name),
            )
        };
        spawn_one_field(
            commands,
            parent,
            ctx,
            field,
            held_field.unwrap_or(&Value::Null),
            &field_path,
            depth + 1,
        );
    }
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

#[cfg(test)]
mod tests {
    use super::declared_asset_type;
    use jackdaw_schema::FieldSchema;

    fn marked(type_path: &str, item_type_path: &str) -> FieldSchema {
        FieldSchema {
            name: "reward".to_string(),
            type_path: type_path.to_string(),
            item_type_path: item_type_path.to_string(),
            asset_type_path: "my_game::content::ItemDef".to_string(),
        }
    }

    #[test]
    fn a_string_a_type_marks_as_a_reference_names_the_kind_it_declares() {
        assert_eq!(
            declared_asset_type(&marked("alloc::string::String", "")).as_deref(),
            Some("my_game::content::ItemDef"),
        );
        assert_eq!(
            declared_asset_type(&marked(
                "alloc::vec::Vec<alloc::string::String>",
                "alloc::string::String",
            ))
            .as_deref(),
            Some("my_game::content::ItemDef"),
            "a list of strings names the kind its elements hold",
        );
    }

    #[test]
    fn a_field_holding_no_string_names_no_kind_however_it_is_marked() {
        for (type_path, item_type_path) in [
            ("u32", ""),
            ("core::option::Option<alloc::string::String>", ""),
            ("alloc::vec::Vec<u32>", "u32"),
            ("my_game::content::ItemDef", ""),
        ] {
            assert_eq!(
                declared_asset_type(&marked(type_path, item_type_path)),
                None,
                "{type_path} holds no path to name a file with",
            );
        }
    }
}
