//! Inspector rendering and edit conversions for project (schema-reported)
//! components.
//!
//! These types are never real ECS components in the editor: mapping
//! project code would leak on every refresh, so the editor keeps only the schema
//! extracted out-of-process ([`crate::project_types::ProjectTypes`]) and
//! stores authored values in the scene document. The inspector renders a
//! project component by assembling a `DynamicStruct` from the schema field
//! defaults with any authored document overrides applied on top, then feeding
//! it to the generic reflected-field renderer. Field edits round-trip back to
//! the document through the same widgets, converted here without a concrete
//! registration.

use bevy::prelude::*;
use bevy::reflect::PartialReflect;

use jackdaw_bsn::{BsnValue, SceneBsnAst};

use jackdaw_schema::TypeSchema;

use super::reflect_fields;

/// Render every field of a project component onto `body_entity`. Values come
/// from the authored document patch where present, otherwise from the schema
/// default. Each scalar field is spawned through the same
/// [`reflect_fields::spawn_field_row`] widget the reflected inspector uses, so
/// edits round-trip back to the document through the shared `ValueChange`
/// observers. Fields whose type is not a supported inspector scalar are shown
/// as a read-only row so the component's shape stays visible.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors reflect_fields spawner arity"
)]
pub(crate) fn spawn_project_component_fields(
    commands: &mut Commands,
    body_entity: Entity,
    schema: &TypeSchema,
    ast: &SceneBsnAst,
    node: Entity,
    source_entity: Entity,
    type_registry: &bevy::ecs::reflect::AppTypeRegistry,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
    names: &Query<&Name>,
) {
    for field in &schema.fields {
        let json = jackdaw_bsn::get_bsn_field(ast, node, &schema.type_path, &field.name)
            .and_then(|v| bsn_value_to_json(&v))
            .or_else(|| component_default_field_json(schema, &field.name));

        match json
            .as_ref()
            .and_then(|j| json_to_scalar_reflect(&field.type_path, j))
        {
            Some(boxed) => reflect_fields::spawn_field_row(
                commands,
                body_entity,
                &field.name,
                boxed.as_ref(),
                0,
                field.name.clone(),
                source_entity,
                &schema.type_path,
                names,
                type_registry,
                editor_font,
                icon_font,
            ),
            None => {
                let shown = json.map(|j| j.to_string()).unwrap_or_default();
                commands.spawn((
                    Text::new(format!("{}: {shown}", field.name)),
                    TextFont {
                        font_size: jackdaw_feathers::tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(jackdaw_feathers::tokens::TEXT_SECONDARY),
                    ChildOf(body_entity),
                ));
            }
        }
    }
}

/// The default JSON value for one field, pulled from the schema's whole-type
/// default. The extractor stores the default in `ReflectSerializer` form,
/// `{ "full::Type": { field: value, .. } }`, so unwrap the single type key.
fn component_default_field_json(
    schema: &TypeSchema,
    field_name: &str,
) -> Option<serde_json::Value> {
    let default = schema.default.as_ref()?;
    let obj = default.as_object()?;
    let fields = obj
        .get(&schema.type_path)
        .and_then(|v| v.as_object())
        .unwrap_or(obj);
    fields.get(field_name).cloned()
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

/// Build a concrete scalar reflect value the inspector's field renderer can
/// downcast, guided by the field's reflected type path. Returns `None` for
/// types the inspector does not render as an editable scalar (compound types,
/// enums, unknown opaques).
fn json_to_scalar_reflect(
    field_type_path: &str,
    json: &serde_json::Value,
) -> Option<Box<dyn PartialReflect>> {
    let short = field_type_path
        .rsplit("::")
        .next()
        .unwrap_or(field_type_path);
    let boxed: Box<dyn PartialReflect> = match short {
        "f32" => Box::new(json.as_f64()? as f32),
        "f64" => Box::new(json.as_f64()?),
        "bool" => Box::new(json.as_bool()?),
        "String" => Box::new(json.as_str()?.to_string()),
        "i8" => Box::new(json.as_i64()? as i8),
        "i16" => Box::new(json.as_i64()? as i16),
        "i32" => Box::new(json.as_i64()? as i32),
        "i64" => Box::new(json.as_i64()?),
        "isize" => Box::new(json.as_i64()? as isize),
        "u8" => Box::new(json.as_u64()? as u8),
        "u16" => Box::new(json.as_u64()? as u16),
        "u32" => Box::new(json.as_u64()? as u32),
        "u64" => Box::new(json.as_u64()?),
        "usize" => Box::new(json.as_u64()? as usize),
        _ => return None,
    };
    Some(boxed)
}

/// Convert one reflect-format JSON field edit into the [`BsnValue`] to author,
/// using the field's reflected type path to pick the scalar variant. The
/// registration-free counterpart to [`crate::commands::json_field_edit_to_bsn_value`]
/// for project components.
pub(crate) fn json_to_bsn_value_typed(field_type_path: &str, json: &serde_json::Value) -> BsnValue {
    let short = field_type_path
        .rsplit("::")
        .next()
        .unwrap_or(field_type_path);
    match short {
        "f32" | "f64" => BsnValue::Float(json.as_f64().unwrap_or(0.0)),
        "bool" => BsnValue::Bool(json.as_bool().unwrap_or(false)),
        "String" => BsnValue::String(json.as_str().unwrap_or_default().to_string()),
        "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => {
            BsnValue::Int(i128::from(json.as_i64().unwrap_or(0)))
        }
        _ => infer_bsn_value(json),
    }
}

/// Best-effort scalar `BsnValue` from raw JSON when the field type is unknown.
fn infer_bsn_value(json: &serde_json::Value) -> BsnValue {
    if let Some(b) = json.as_bool() {
        BsnValue::Bool(b)
    } else if let Some(i) = json.as_i64() {
        BsnValue::Int(i128::from(i))
    } else if let Some(f) = json.as_f64() {
        BsnValue::Float(f)
    } else if let Some(s) = json.as_str() {
        BsnValue::String(s.to_string())
    } else {
        BsnValue::String(json.to_string())
    }
}
