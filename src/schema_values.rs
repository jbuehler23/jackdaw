//! Values of a type the editor knows only as the project's schema.
//!
//! Such a value lives as a BSN patch: the fields it authors, and nothing else.
//! The inspector rows and the operators speak JSON, so this module is the pair
//! of conversions between the two, guided by the type's reported shape, plus
//! the field-path walk both sides use to reach into a list or a nested struct.

use bevy::prelude::*;
use bevy::reflect::PartialReflect;
use jackdaw_bsn::{BsnAssetContext, BsnField, BsnStructData, BsnStructFields, BsnValue};
use jackdaw_schema::{FieldSchema, TypeKind, TypeSchema};
use serde_json::Value;

use crate::project_types::ProjectTypes;

/// One step of a field path: `loot[0].item` is a field, an index and a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Field(String),
    Index(usize),
}

/// Split a dotted, indexed field path into the steps that walk it.
pub fn parse_path(path: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    for segment in path.split('.').filter(|segment| !segment.is_empty()) {
        let (name, rest) = match segment.split_once('[') {
            Some((name, rest)) => (name, Some(rest)),
            None => (segment, None),
        };
        if !name.is_empty() {
            steps.push(Step::Field(name.to_string()));
        }
        let Some(rest) = rest else { continue };
        for index in rest.trim_end_matches(']').split("][") {
            match index.parse::<usize>() {
                Ok(index) => steps.push(Step::Index(index)),
                Err(_) => return Vec::new(),
            }
        }
    }
    steps
}

/// The value `steps` reaches inside `value`.
pub fn json_at<'a>(value: &'a Value, steps: &[Step]) -> Option<&'a Value> {
    let mut current = value;
    for step in steps {
        current = match step {
            Step::Field(name) => current.get(name)?,
            Step::Index(index) => current.get(*index)?,
        };
    }
    Some(current)
}

/// Write `new` where `steps` reaches inside `value`, reporting whether the
/// path was there to write to.
pub fn json_set(value: &mut Value, steps: &[Step], new: Value) -> bool {
    let Some((last, leading)) = steps.split_last() else {
        *value = new;
        return true;
    };
    let mut current = value;
    for step in leading {
        current = match step {
            Step::Field(name) => match current.get_mut(name) {
                Some(next) => next,
                None => return false,
            },
            Step::Index(index) => match current.get_mut(*index) {
                Some(next) => next,
                None => return false,
            },
        };
    }
    match last {
        Step::Field(name) => match current.as_object_mut() {
            Some(object) => {
                object.insert(name.clone(), new);
                true
            }
            None => false,
        },
        Step::Index(index) => match current.as_array_mut() {
            Some(array) if *index < array.len() => {
                array[*index] = new;
                true
            }
            _ => false,
        },
    }
}

/// The type's default value as JSON. The extractor stores it in
/// `ReflectSerializer` form, `{ "full::Type": value }`, so unwrap the one key.
pub fn type_default_json(schema: &TypeSchema) -> Option<Value> {
    let default = schema.default.as_ref()?;
    let object = default.as_object()?;
    Some(
        object
            .get(&schema.type_path)
            .cloned()
            .unwrap_or_else(|| default.clone()),
    )
}

/// One field's default, from the type's whole-value default.
pub fn default_field_json(schema: &TypeSchema, field: &str) -> Option<Value> {
    type_default_json(schema)?.get(field).cloned()
}

/// The element type of a list field, from the schema where it is reported and
/// from the field's own type path otherwise.
pub fn item_type_path(field: &FieldSchema) -> Option<&str> {
    if !field.item_type_path.is_empty() {
        return Some(&field.item_type_path);
    }
    list_item_type_path(&field.type_path)
}

/// The element type spelled inside a `Vec<T>` type path.
pub fn list_item_type_path(type_path: &str) -> Option<&str> {
    type_path
        .strip_prefix("alloc::vec::Vec<")
        .and_then(|inner| inner.strip_suffix('>'))
}

/// The type of the value a field path reaches inside a value of `root`.
pub fn field_type_path(types: &ProjectTypes, root: &str, steps: &[Step]) -> Option<String> {
    let mut current = root.to_string();
    for step in steps {
        current = match step {
            Step::Field(name) => {
                let schema = types.type_schema(&current)?;
                let field = schema.fields.iter().find(|field| &field.name == name)?;
                field.type_path.clone()
            }
            Step::Index(_) => list_item_type_path(&current)?.to_string(),
        };
    }
    Some(current)
}

/// Whether a value of this type decides how many rows the inspector shows for
/// it: a list, whose length is the rows, or an enum, whose variant is.
pub fn shapes_its_own_rows(types: &ProjectTypes, type_path: &str) -> bool {
    list_item_type_path(type_path).is_some()
        || types
            .type_schema(type_path)
            .is_some_and(|schema| schema.kind == TypeKind::Enum)
}

/// Every field of a patched value as JSON: what the patch authors, and the
/// type's default for the rest.
pub fn value_json(
    world: &World,
    types: &ProjectTypes,
    schema: &TypeSchema,
    data: &BsnStructData,
) -> Value {
    let mut object = serde_json::Map::new();
    for field in &schema.fields {
        if let Some(value) = field_json(world, types, schema, data, field) {
            object.insert(field.name.clone(), value);
        }
    }
    Value::Object(object)
}

/// One field of a patched value, authored or defaulted.
pub fn field_json(
    world: &World,
    types: &ProjectTypes,
    schema: &TypeSchema,
    data: &BsnStructData,
    field: &FieldSchema,
) -> Option<Value> {
    match authored(data, &field.name) {
        Some(value) => json_for_bsn(world, types, &field.type_path, value),
        None => default_field_json(schema, &field.name),
    }
}

fn authored<'a>(data: &'a BsnStructData, name: &str) -> Option<&'a BsnValue> {
    data.fields
        .0
        .iter()
        .find(|field| field.name == name)
        .map(|field| &field.value)
}

/// Write one field into a patch, or drop it when it holds what the type
/// defaults to.
///
/// A field already spelled keeps its place and a new one takes the place the
/// type declares it in, so one edit rewrites one line.
pub fn set_authored(
    data: &mut BsnStructData,
    schema: &TypeSchema,
    name: &str,
    value: Option<BsnValue>,
) {
    let Some(value) = value else {
        data.fields.0.retain(|field| field.name != name);
        return;
    };
    if let Some(field) = data.fields.0.iter_mut().find(|field| field.name == name) {
        field.value = value;
        return;
    }
    let at = declared_position(data, schema, name);
    data.fields.0.insert(
        at,
        BsnField {
            name: name.to_string(),
            value,
        },
    );
}

/// Where a field the patch does not spell yet belongs: before the first field
/// the type declares after it, and at the end when the type declares neither.
fn declared_position(data: &BsnStructData, schema: &TypeSchema, name: &str) -> usize {
    let Some(declared) = schema.fields.iter().position(|field| field.name == name) else {
        return data.fields.0.len();
    };
    data.fields
        .0
        .iter()
        .position(|field| {
            schema
                .fields
                .iter()
                .position(|known| known.name == field.name)
                .is_some_and(|known| known > declared)
        })
        .unwrap_or(data.fields.0.len())
}

/// The JSON a BSN value stands for in a field of `type_path`.
pub fn json_for_bsn(
    world: &World,
    types: &ProjectTypes,
    type_path: &str,
    value: &BsnValue,
) -> Option<Value> {
    if let Some(item_type) = list_item_type_path(type_path)
        && let BsnValue::List(items) = value
    {
        return Some(Value::Array(
            items
                .iter()
                .filter_map(|item| json_for_bsn(world, types, item_type, item))
                .collect(),
        ));
    }
    if let Some(schema) = types.type_schema(type_path) {
        return project_json_for_bsn(world, types, schema, value);
    }
    if let Some(json) = native_json_for_bsn(world, type_path, value) {
        return Some(json);
    }
    scalar_json(value)
}

fn project_json_for_bsn(
    world: &World,
    types: &ProjectTypes,
    schema: &TypeSchema,
    value: &BsnValue,
) -> Option<Value> {
    match schema.kind {
        TypeKind::Enum => enum_json_for_bsn(world, types, schema, value),
        TypeKind::Struct => {
            let BsnValue::Struct(data) = value else {
                return scalar_json(value);
            };
            Some(value_json(world, types, schema, data))
        }
        TypeKind::TupleStruct => {
            let BsnValue::TupleStruct(data) = value else {
                return scalar_json(value);
            };
            Some(Value::Array(
                data.values
                    .iter()
                    .zip(&schema.fields)
                    .filter_map(|(value, field)| {
                        json_for_bsn(world, types, &field.type_path, value)
                    })
                    .collect(),
            ))
        }
        TypeKind::Marker => scalar_json(value),
    }
}

/// An enum's JSON: a bare name for a unit variant, `{"Variant": ..}` for one
/// carrying fields, matching what a reflect deserializer takes.
fn enum_json_for_bsn(
    world: &World,
    types: &ProjectTypes,
    schema: &TypeSchema,
    value: &BsnValue,
) -> Option<Value> {
    let variant_of = |spelled: &str| -> String {
        spelled
            .strip_prefix(&format!("{}::", schema.type_path))
            .unwrap_or(spelled)
            .to_string()
    };
    match value {
        BsnValue::Type(spelled) => Some(Value::String(variant_of(spelled))),
        BsnValue::String(name) => Some(Value::String(name.clone())),
        BsnValue::Struct(data) => {
            let name = variant_of(&data.type_path);
            let variant = schema.variants.iter().find(|known| known.name == name)?;
            let mut object = serde_json::Map::new();
            for field in &variant.fields {
                let Some(authored) = authored(data, &field.name) else {
                    continue;
                };
                if let Some(json) = json_for_bsn(world, types, &field.type_path, authored) {
                    object.insert(field.name.clone(), json);
                }
            }
            Some(serde_json::json!({ name: Value::Object(object) }))
        }
        BsnValue::TupleStruct(data) => {
            let name = variant_of(&data.type_path);
            let variant = schema.variants.iter().find(|known| known.name == name)?;
            let items: Vec<Value> = data
                .values
                .iter()
                .zip(&variant.fields)
                .filter_map(|(value, field)| json_for_bsn(world, types, &field.type_path, value))
                .collect();
            Some(serde_json::json!({ name: Value::Array(items) }))
        }
        _ => scalar_json(value),
    }
}

/// A value of a type the editor has a registration for, through reflection.
fn native_json_for_bsn(world: &World, type_path: &str, value: &BsnValue) -> Option<Value> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let type_id = registry.get_with_type_path(type_path)?.type_id();
    if crate::typed_values::takes_asset_path(&registry, type_id) {
        return match value {
            BsnValue::String(path) => Some(Value::String(path.clone())),
            _ => Some(Value::String(String::new())),
        };
    }
    let server = world.get_resource::<AssetServer>();
    let assets = server.map(|server| jackdaw_bsn::BsnApplyAssets {
        server,
        local: None,
    });
    let reflected = jackdaw_bsn::bsn_value_to_reflect(value, type_id, &registry, assets.as_ref())?;
    crate::inspector::reflect_fields::reflect_to_json(reflected.as_ref(), &registry)
}

fn scalar_json(value: &BsnValue) -> Option<Value> {
    match value {
        BsnValue::Float(number) => Some(serde_json::json!(number)),
        BsnValue::Int(number) => i64::try_from(*number).ok().map(|n| serde_json::json!(n)),
        BsnValue::Bool(flag) => Some(Value::Bool(*flag)),
        BsnValue::String(text) => Some(Value::String(text.clone())),
        BsnValue::Type(path) => Some(Value::String(
            path.rsplit("::").next().unwrap_or(path).to_string(),
        )),
        _ => None,
    }
}

/// The BSN value a JSON value spells in a field of `type_path`.
pub fn bsn_for_json(
    world: &World,
    types: &ProjectTypes,
    type_path: &str,
    json: &Value,
) -> Option<BsnValue> {
    if let Some(item_type) = list_item_type_path(type_path)
        && let Value::Array(items) = json
    {
        return Some(BsnValue::List(
            items
                .iter()
                .filter_map(|item| bsn_for_json(world, types, item_type, item))
                .collect(),
        ));
    }
    if let Some(schema) = types.type_schema(type_path) {
        return project_bsn_for_json(world, types, schema, json);
    }
    if let Some(value) = native_bsn_for_json(world, type_path, json) {
        return Some(value);
    }
    scalar_bsn(json)
}

fn project_bsn_for_json(
    world: &World,
    types: &ProjectTypes,
    schema: &TypeSchema,
    json: &Value,
) -> Option<BsnValue> {
    match schema.kind {
        TypeKind::Enum => enum_bsn_for_json(world, types, schema, json),
        TypeKind::Struct => {
            let object = json.as_object()?;
            let mut fields = Vec::new();
            for field in &schema.fields {
                let Some(value) = object.get(&field.name) else {
                    continue;
                };
                if let Some(value) = bsn_for_json(world, types, &field.type_path, value) {
                    fields.push(BsnField {
                        name: field.name.clone(),
                        value,
                    });
                }
            }
            Some(BsnValue::Struct(BsnStructData {
                type_path: schema.type_path.clone(),
                fields: BsnStructFields(fields),
            }))
        }
        TypeKind::TupleStruct => {
            let items = json.as_array()?;
            Some(BsnValue::TupleStruct(jackdaw_bsn::BsnTupleStructData {
                type_path: schema.type_path.clone(),
                values: items
                    .iter()
                    .zip(&schema.fields)
                    .filter_map(|(item, field)| bsn_for_json(world, types, &field.type_path, item))
                    .collect(),
            }))
        }
        TypeKind::Marker => scalar_bsn(json),
    }
}

fn enum_bsn_for_json(
    world: &World,
    types: &ProjectTypes,
    schema: &TypeSchema,
    json: &Value,
) -> Option<BsnValue> {
    if let Some(name) = json.as_str() {
        let known = schema.variants.iter().any(|variant| variant.name == name);
        if !known && !schema.variants.is_empty() {
            return None;
        }
        return Some(BsnValue::Type(format!("{}::{name}", schema.type_path)));
    }
    let (name, body) = json.as_object()?.iter().next()?;
    let variant = schema.variants.iter().find(|known| &known.name == name)?;
    let type_path = format!("{}::{name}", schema.type_path);
    if let Some(items) = body.as_array() {
        return Some(BsnValue::TupleStruct(jackdaw_bsn::BsnTupleStructData {
            type_path,
            values: items
                .iter()
                .zip(&variant.fields)
                .filter_map(|(item, field)| bsn_for_json(world, types, &field.type_path, item))
                .collect(),
        }));
    }
    let object = body.as_object()?;
    let mut fields = Vec::new();
    for field in &variant.fields {
        let Some(value) = object.get(&field.name) else {
            continue;
        };
        if let Some(value) = bsn_for_json(world, types, &field.type_path, value) {
            fields.push(BsnField {
                name: field.name.clone(),
                value,
            });
        }
    }
    Some(BsnValue::Struct(BsnStructData {
        type_path,
        fields: BsnStructFields(fields),
    }))
}

/// A value of a type the editor has a registration for: text takes the
/// field's own spelling (an asset path, a colour), everything else goes
/// through the reflect deserializer.
pub fn native_value_for_json(
    registry: &bevy::reflect::TypeRegistry,
    server: Option<&AssetServer>,
    references: Option<&crate::typed_values::References>,
    type_path: &str,
    json: &Value,
) -> Option<Box<dyn PartialReflect>> {
    let registration = registry.get_with_type_path(type_path)?;
    if let Some(text) = json.as_str()
        && let Some(value) = crate::typed_values::text_value_for_field(
            registry,
            server,
            references,
            registration.type_id(),
            text,
        )
    {
        return Some(value);
    }
    deserialize_typed(registration, registry, json)
}

fn native_bsn_for_json(world: &World, type_path: &str, json: &Value) -> Option<BsnValue> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let type_id = registry.get_with_type_path(type_path)?.type_id();
    if crate::typed_values::takes_asset_path(&registry, type_id) {
        return Some(BsnValue::String(
            json.as_str().unwrap_or_default().to_string(),
        ));
    }
    let server = world.get_resource::<AssetServer>();
    let references = jackdaw_bsn::apply_reference_map(world);
    let reflected = native_value_for_json(&registry, server, Some(&references), type_path, json)?;
    Some(match server {
        Some(server) => BsnValue::from_reflect_with_assets(
            reflected.as_ref(),
            &registry,
            &BsnAssetContext {
                asset_server: server,
                parent_path: std::path::Path::new(""),
                asset_names: None,
            },
        ),
        None => BsnValue::from_reflect(reflected.as_ref(), &registry),
    })
}

fn deserialize_typed(
    registration: &bevy::reflect::TypeRegistration,
    registry: &bevy::reflect::TypeRegistry,
    json: &Value,
) -> Option<Box<dyn PartialReflect>> {
    use serde::de::DeserializeSeed;
    bevy::reflect::serde::TypedReflectDeserializer::new(registration, registry)
        .deserialize(json)
        .ok()
}

fn scalar_bsn(json: &Value) -> Option<BsnValue> {
    match json {
        Value::Bool(flag) => Some(BsnValue::Bool(*flag)),
        Value::String(text) => Some(BsnValue::String(text.clone())),
        Value::Number(number) => number
            .as_i64()
            .map(|int| BsnValue::Int(i128::from(int)))
            .or_else(|| number.as_f64().map(BsnValue::Float)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_splits_into_fields_and_indices() {
        assert_eq!(
            parse_path("stack_size"),
            vec![Step::Field("stack_size".into())]
        );
        assert_eq!(
            parse_path("loot[0].item"),
            vec![
                Step::Field("loot".into()),
                Step::Index(0),
                Step::Field("item".into())
            ]
        );
        assert!(parse_path("").is_empty());
    }

    #[test]
    fn a_write_reaches_through_a_list_row() {
        let mut value = serde_json::json!({ "loot": [{ "item": "coin" }] });
        assert!(json_set(
            &mut value,
            &parse_path("loot[0].item"),
            Value::String("gem".into())
        ));
        assert_eq!(value["loot"][0]["item"], "gem");
    }

    #[test]
    fn a_write_to_a_row_that_is_not_there_is_refused() {
        let mut value = serde_json::json!({ "loot": [] });
        assert!(!json_set(
            &mut value,
            &parse_path("loot[2].item"),
            Value::String("gem".into())
        ));
    }

    fn item_schema() -> TypeSchema {
        serde_json::from_value(serde_json::json!({
            "type_path": "my_game::content::ItemDef",
            "short_name": "ItemDef",
            "module_path": "my_game::content",
            "category": "",
            "description": "",
            "hidden": false,
            "default_constructible": true,
            "kind": "Struct",
            "default": null,
            "fields": [
                { "name": "stack_size", "type_path": "u32" },
                { "name": "rarity", "type_path": "my_game::content::Rarity" },
                { "name": "weight", "type_path": "f32" }
            ]
        }))
        .expect("the schema reads")
    }

    fn patch_of(names: &[&str]) -> BsnStructData {
        BsnStructData {
            type_path: "my_game::content::ItemDef".to_string(),
            fields: BsnStructFields(
                names
                    .iter()
                    .map(|name| BsnField {
                        name: (*name).to_string(),
                        value: BsnValue::Int(1),
                    })
                    .collect(),
            ),
        }
    }

    fn field_names(data: &BsnStructData) -> Vec<&str> {
        data.fields
            .0
            .iter()
            .map(|field| field.name.as_str())
            .collect()
    }

    #[test]
    fn a_field_the_patch_already_spells_keeps_the_place_it_has() {
        let mut data = patch_of(&["weight", "stack_size"]);
        set_authored(&mut data, &item_schema(), "weight", Some(BsnValue::Int(4)));

        assert_eq!(field_names(&data), ["weight", "stack_size"]);
        assert_eq!(
            data.fields.0[0].value,
            BsnValue::Int(4),
            "and holds what was written to it"
        );
    }

    #[test]
    fn a_field_arriving_takes_the_place_its_type_declares_it_in() {
        let mut data = patch_of(&["stack_size", "weight"]);
        set_authored(&mut data, &item_schema(), "rarity", Some(BsnValue::Int(1)));

        assert_eq!(field_names(&data), ["stack_size", "rarity", "weight"]);
    }

    #[test]
    fn a_field_the_type_does_not_declare_arrives_at_the_end() {
        let mut data = patch_of(&["stack_size"]);
        set_authored(&mut data, &item_schema(), "charges", Some(BsnValue::Int(1)));

        assert_eq!(field_names(&data), ["stack_size", "charges"]);
    }

    #[test]
    fn a_field_holding_what_its_type_defaults_to_stops_being_authored() {
        let mut data = patch_of(&["stack_size", "weight"]);
        set_authored(&mut data, &item_schema(), "stack_size", None);

        assert_eq!(field_names(&data), ["weight"]);
    }

    #[test]
    fn a_vec_field_reports_the_type_it_holds() {
        assert_eq!(
            list_item_type_path("alloc::vec::Vec<my_game::content::LootRoll>"),
            Some("my_game::content::LootRoll")
        );
        assert_eq!(list_item_type_path("u32"), None);
    }
}
