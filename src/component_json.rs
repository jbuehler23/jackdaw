//! Type-aware navigation of reflect-serialized component JSON.
//!
//! The PIE live protocol carries whole components to the running game as
//! canonical reflect JSON (`TypedReflectSerializer` output). These helpers
//! read and write a single field inside such a value by dotted path, using
//! the `TypeRegistry` to resolve named fields to array indices when the JSON
//! value is an array (e.g. `Vec3` serializes as `[x, y, z]` but reflection
//! paths use `translation.x`).

use bevy::reflect::enums::{EnumInfo, VariantInfo};
use bevy::reflect::{NamedField, TypeInfo, TypeRegistration, TypeRegistry, UnnamedField};

/// Read a nested field by dotted path from a standalone component JSON value,
/// resolving named fields to array indices via the type registry. Uses the
/// same path syntax as [`set_field_in_component_json`]: dot-separated
/// segments, bracket notation for list elements (e.g. `faces[0]`), and
/// automatic enum-variant unwrapping. An empty `field_path` returns the whole
/// component value. Returns `None` when `type_path` is not registered or any
/// path segment is absent.
pub fn get_field_in_component_json<'a>(
    component: &'a serde_json::Value,
    type_path: &str,
    field_path: &str,
    registry: &TypeRegistry,
) -> Option<&'a serde_json::Value> {
    let registration = registry.get_with_type_path(type_path)?;
    typed_json_path_get(component, field_path, registration, registry)
}

/// Set a nested field by dotted path inside a standalone component JSON
/// value, resolving named fields to array indices via the type registry.
///
/// Callers hold a component value keyed by type path (the PIE live mirror
/// stores component JSON this way). `component` is mutated in place; a
/// `field_path` of `""` replaces the whole value. A no-op when `type_path`
/// isn't registered.
pub fn set_field_in_component_json(
    component: &mut serde_json::Value,
    type_path: &str,
    field_path: &str,
    value: serde_json::Value,
    registry: &TypeRegistry,
) {
    let Some(registration) = registry.get_with_type_path(type_path) else {
        return;
    };
    typed_json_path_set(component, field_path, value, registration, registry);
}

/// Resolve a field name to an array index using type info.
/// Returns `None` if the type doesn't have named fields or the name isn't found.
fn field_index_from_type_info(type_info: &TypeInfo, field_name: &str) -> Option<usize> {
    match type_info {
        TypeInfo::Struct(s) => s.index_of(field_name),
        TypeInfo::TupleStruct(_) => field_name.parse::<usize>().ok(),
        _ => None,
    }
}

/// Given a JSON value representing an enum in Bevy's reflect serialization
/// format (`{"VariantName": inner}` for struct/tuple, `"VariantName"` for
/// unit), return the variant name and a reference to the inner JSON.
///
/// For unit variants the "inner" is the string itself  -- callers must check
/// the variant kind via `EnumInfo` before descending further.
fn enum_variant_from_json(json: &serde_json::Value) -> Option<(&str, &serde_json::Value)> {
    match json {
        serde_json::Value::Object(map) if map.len() == 1 => {
            let (name, inner) = map.iter().next()?;
            Some((name.as_str(), inner))
        }
        serde_json::Value::String(name) => Some((name.as_str(), json)),
        _ => None,
    }
}

fn enum_variant_from_json_mut(
    json: &mut serde_json::Value,
) -> Option<(String, &mut serde_json::Value)> {
    match json {
        serde_json::Value::Object(map) if map.len() == 1 => {
            let name = map.keys().next().cloned()?;
            let inner = map.get_mut(&name)?;
            Some((name, inner))
        }
        _ => None,
    }
}

/// Find a field on the current variant by name (or index for tuple variants)
/// and return its [`TypeRegistration`]. Used to advance `current_reg` after an
/// enum has been unwrapped during path navigation.
fn variant_field_type_registration<'a>(
    enum_info: &EnumInfo,
    variant_name: &str,
    field_name: &str,
    registry: &'a TypeRegistry,
) -> Option<&'a TypeRegistration> {
    let variant = enum_info.variant(variant_name)?;
    let field_type_id = match variant {
        VariantInfo::Struct(s) => s.field(field_name).map(NamedField::type_id)?,
        VariantInfo::Tuple(t) => {
            let idx: usize = field_name.parse().ok()?;
            t.field_at(idx).map(UnnamedField::type_id)?
        }
        VariantInfo::Unit(_) => return None,
    };
    registry.get(field_type_id)
}

/// Whether `segment` addresses field 0 of a single-field (newtype) tuple
/// variant. Bevy serializes such a variant with its one field flattened
/// (`{"Mirror": {..}}`, not `{"Mirror": [{..}]}`), so the path navigator must
/// treat index 0 as the inner value itself rather than looking for an array
/// element or an object key named "0".
fn is_newtype_index0(enum_info: &EnumInfo, variant_name: &str, segment: &str) -> bool {
    segment == "0"
        && matches!(
            enum_info.variant(variant_name),
            Some(VariantInfo::Tuple(t)) if t.field_len() == 1
        )
}

/// Get the [`TypeRegistration`] for a field by name, advancing through the type tree.
fn field_type_registration<'a>(
    type_info: &TypeInfo,
    field_name: &str,
    registry: &'a TypeRegistry,
) -> Option<&'a TypeRegistration> {
    let field_type_id = match type_info {
        TypeInfo::Struct(s) => s.field(field_name).map(NamedField::type_id),
        TypeInfo::TupleStruct(ts) => {
            let idx = field_name.parse::<usize>().ok()?;
            ts.field_at(idx).map(UnnamedField::type_id)
        }
        TypeInfo::List(l) => Some(l.item_ty().id()),
        _ => None,
    }?;
    registry.get(field_type_id)
}

/// Navigate into a JSON value using a dotted field path and type info.
fn typed_json_path_get<'a>(
    root: &'a serde_json::Value,
    path: &str,
    registration: &TypeRegistration,
    registry: &TypeRegistry,
) -> Option<&'a serde_json::Value> {
    if path.is_empty() {
        return Some(root);
    }

    let mut current = root;
    let mut current_reg = registration;

    for segment in path.split('.').filter(|s| !s.is_empty()) {
        let type_info = current_reg.type_info();

        // Auto-unwrap enums: Bevy's reflect-path for `ColliderConstructor::Sphere`
        // treats `"radius"` as a field of the *current variant*, not a sibling of
        // the variant tag. Mirror that by descending into the variant's inner
        // JSON object before consuming the segment.
        if let TypeInfo::Enum(enum_info) = type_info {
            let (variant_name, inner) = enum_variant_from_json(current)?;
            let next_reg =
                variant_field_type_registration(enum_info, variant_name, segment, registry)?;
            let next_val = if is_newtype_index0(enum_info, variant_name, segment) {
                // A single-field (newtype) tuple variant is serialized with the
                // inner value flattened, not wrapped in a one-element array, so
                // index 0 is the inner value itself.
                inner
            } else {
                match inner {
                    serde_json::Value::Object(_) => inner.get(segment)?,
                    serde_json::Value::Array(_) => {
                        let idx: usize = segment.parse().ok()?;
                        inner.get(idx)?
                    }
                    _ => return None,
                }
            };
            current = next_val;
            current_reg = next_reg;
            continue;
        }

        // Handle bracket indexing (e.g., "faces[0]")
        if let Some(bracket_pos) = segment.find('[') {
            let key = &segment[..bracket_pos];
            let idx_str = &segment[bracket_pos + 1..segment.len() - 1];
            let idx: usize = idx_str.parse().ok()?;

            // Navigate to the key first
            current = navigate_json_field(current, key, type_info)?;
            // Then look up the list element type
            if let Some(key_reg) = field_type_registration(type_info, key, registry) {
                current_reg = key_reg;
            }
            // Navigate into the array
            current = current.get(idx)?;
            // Advance type info to list element
            let list_info = current_reg.type_info();
            if let TypeInfo::List(l) = list_info
                && let Some(elem_reg) = registry.get(l.item_ty().id())
            {
                current_reg = elem_reg;
            }
        } else {
            // Simple field navigation
            current = navigate_json_field(current, segment, type_info)?;
            // Advance type info
            if let Some(next_reg) = field_type_registration(type_info, segment, registry) {
                current_reg = next_reg;
            }
        }
    }

    Some(current)
}

/// Navigate one level into a JSON value using a field name and type info.
/// Handles both Object (named key) and Array (field index from type info).
fn navigate_json_field<'a>(
    json: &'a serde_json::Value,
    field_name: &str,
    type_info: &TypeInfo,
) -> Option<&'a serde_json::Value> {
    match json {
        serde_json::Value::Object(_) => json.get(field_name),
        serde_json::Value::Array(_) => {
            // Resolve named field to array index via type info
            let idx = if let Ok(i) = field_name.parse::<usize>() {
                i
            } else {
                field_index_from_type_info(type_info, field_name)?
            };
            json.get(idx)
        }
        _ => None,
    }
}

/// Set a value at a dotted field path within a JSON value, using type info.
fn typed_json_path_set(
    root: &mut serde_json::Value,
    path: &str,
    value: serde_json::Value,
    registration: &TypeRegistration,
    registry: &TypeRegistry,
) {
    if path.is_empty() {
        *root = value;
        return;
    }

    let segments: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    let mut current = root;
    let mut current_reg = registration;

    for (i, segment) in segments.iter().enumerate() {
        let is_last = i == segments.len() - 1;
        let type_info = current_reg.type_info();

        // Auto-unwrap enums: descend into the variant's inner value so paths
        // like `"radius"` address the field on the current variant rather
        // than inserting a sibling of the variant tag.
        if let TypeInfo::Enum(enum_info) = type_info {
            let Some((variant_name, inner)) = enum_variant_from_json_mut(current) else {
                return;
            };
            let Some(next_reg) =
                variant_field_type_registration(enum_info, &variant_name, segment, registry)
            else {
                return;
            };
            let newtype_index0 = is_newtype_index0(enum_info, &variant_name, segment);
            if is_last {
                if newtype_index0 {
                    // Setting index 0 of a newtype variant replaces the inner
                    // value directly (it is flattened, not array-wrapped).
                    *inner = value;
                    return;
                }
                // Set the field inside the variant's inner value.
                match inner {
                    serde_json::Value::Object(map) => {
                        map.insert(segment.to_string(), value);
                    }
                    serde_json::Value::Array(arr) => {
                        if let Ok(idx) = segment.parse::<usize>()
                            && idx < arr.len()
                        {
                            arr[idx] = value;
                        }
                    }
                    _ => {}
                }
                return;
            }
            // Descend into the variant's field and continue. A newtype variant
            // flattens its single field, so index 0 is the inner value itself.
            let field_val = if newtype_index0 {
                Some(inner)
            } else {
                match inner {
                    serde_json::Value::Object(map) => map.get_mut(*segment),
                    serde_json::Value::Array(arr) => match segment.parse::<usize>() {
                        Ok(idx) => arr.get_mut(idx),
                        Err(_) => return,
                    },
                    _ => None,
                }
            };
            let Some(next) = field_val else { return };
            current = next;
            current_reg = next_reg;
            continue;
        }

        if let Some(bracket_pos) = segment.find('[') {
            let key = &segment[..bracket_pos];
            let idx_str = &segment[bracket_pos + 1..segment.len() - 1];
            let idx: usize = match idx_str.parse() {
                Ok(i) => i,
                Err(_) => return,
            };
            // Navigate to key
            let next = navigate_json_field_mut(current, key, type_info);
            let Some(arr_val) = next else { return };
            if let Some(key_reg) = field_type_registration(type_info, key, registry) {
                current_reg = key_reg;
            }
            if is_last {
                if let Some(arr) = arr_val.as_array_mut()
                    && idx < arr.len()
                {
                    arr[idx] = value;
                }
                return;
            }
            current = match arr_val.get_mut(idx) {
                Some(v) => v,
                None => return,
            };
            let list_info = current_reg.type_info();
            if let TypeInfo::List(l) = list_info
                && let Some(elem_reg) = registry.get(l.item_ty().id())
            {
                current_reg = elem_reg;
            }
        } else {
            if is_last {
                set_json_field(current, segment, value, type_info);
                return;
            }
            let next_reg = field_type_registration(type_info, segment, registry);
            let Some(next) = navigate_json_field_mut(current, segment, type_info) else {
                return;
            };
            current = next;
            if let Some(nr) = next_reg {
                current_reg = nr;
            }
        }
    }
}

/// Navigate one level into a mutable JSON value.
fn navigate_json_field_mut<'a>(
    json: &'a mut serde_json::Value,
    field_name: &str,
    type_info: &TypeInfo,
) -> Option<&'a mut serde_json::Value> {
    match json {
        serde_json::Value::Object(_) => json.get_mut(field_name),
        serde_json::Value::Array(_) => {
            let idx = if let Ok(i) = field_name.parse::<usize>() {
                i
            } else {
                field_index_from_type_info(type_info, field_name)?
            };
            json.get_mut(idx)
        }
        _ => None,
    }
}

/// Set a field value in a JSON value (handles both Object and Array).
fn set_json_field(
    json: &mut serde_json::Value,
    field_name: &str,
    value: serde_json::Value,
    type_info: &TypeInfo,
) {
    match json {
        serde_json::Value::Object(map) => {
            map.insert(field_name.to_string(), value);
        }
        serde_json::Value::Array(arr) => {
            let idx = if let Ok(i) = field_name.parse::<usize>() {
                i
            } else if let Some(i) = field_index_from_type_info(type_info, field_name) {
                i
            } else {
                return;
            };
            if idx < arr.len() {
                arr[idx] = value;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;
    use bevy::reflect::serde::TypedReflectSerializer;

    /// Build the canonical reflect JSON for a value, the same form the PIE
    /// live mirror stores and the game's `TypedReflectDeserializer` expects.
    fn to_canonical_json<T: bevy::reflect::PartialReflect>(
        value: &T,
        registry: &TypeRegistry,
    ) -> serde_json::Value {
        let serializer = TypedReflectSerializer::new(value, registry);
        serde_json::to_value(&serializer).expect("serialize reflected value")
    }

    #[test]
    fn set_field_in_component_json_sets_nested_array_field() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();

        let type_path = "bevy_transform::components::transform::Transform";
        let mut component = to_canonical_json(&Transform::from_xyz(1.0, 2.0, 3.0), &registry);

        // `Vec3` serializes as `[x, y, z]`, so `translation.x` must resolve
        // the named axis to array index 0 via the registry.
        set_field_in_component_json(
            &mut component,
            type_path,
            "translation.x",
            serde_json::json!(9.5),
            &registry,
        );

        // The edited axis changed; the siblings and other fields did not,
        // so the merged value is still a full, deserializable component.
        let translation = &component["translation"];
        assert_eq!(translation[0], 9.5);
        assert_eq!(translation[1], 2.0);
        assert_eq!(translation[2], 3.0);
        assert!(component.get("rotation").is_some());
        assert!(component.get("scale").is_some());
    }

    #[test]
    fn set_field_in_component_json_empty_path_replaces_value() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();

        let type_path = "bevy_transform::components::transform::Transform";
        let mut component = to_canonical_json(&Transform::IDENTITY, &registry);
        let replacement = to_canonical_json(&Transform::from_xyz(4.0, 5.0, 6.0), &registry);

        set_field_in_component_json(
            &mut component,
            type_path,
            "",
            replacement.clone(),
            &registry,
        );

        assert_eq!(component, replacement);
    }

    #[test]
    fn set_field_in_component_json_unregistered_type_is_noop() {
        let registry = TypeRegistry::new();
        let mut component = serde_json::json!({ "translation": [0.0, 0.0, 0.0] });
        let before = component.clone();

        set_field_in_component_json(
            &mut component,
            "not::A::RegisteredType",
            "translation.x",
            serde_json::json!(1.0),
            &registry,
        );

        assert_eq!(component, before);
    }

    #[test]
    fn get_field_reads_named_struct_field() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();

        let type_path = "bevy_transform::components::transform::Transform";
        let component = to_canonical_json(&Transform::from_xyz(1.0, 2.0, 3.0), &registry);

        // `Vec3` serializes as `[x, y, z]`; the named segment resolves via the registry.
        let x = get_field_in_component_json(&component, type_path, "translation.x", &registry);
        assert_eq!(x, Some(&serde_json::json!(1.0)));

        let translation =
            get_field_in_component_json(&component, type_path, "translation", &registry);
        assert_eq!(translation, Some(&serde_json::json!([1.0, 2.0, 3.0])));
    }

    #[test]
    fn get_field_empty_path_returns_whole_component() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();

        let type_path = "bevy_transform::components::transform::Transform";
        let component = to_canonical_json(&Transform::IDENTITY, &registry);

        let result = get_field_in_component_json(&component, type_path, "", &registry);
        assert_eq!(result, Some(&component));
    }

    #[test]
    fn get_field_missing_path_returns_none() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();

        let type_path = "bevy_transform::components::transform::Transform";
        let component = to_canonical_json(&Transform::IDENTITY, &registry);

        let result =
            get_field_in_component_json(&component, type_path, "does_not_exist", &registry);
        assert!(result.is_none());
    }

    #[test]
    fn get_field_unregistered_type_returns_none() {
        let registry = TypeRegistry::new();
        let component = serde_json::json!({ "translation": [0.0, 0.0, 0.0] });

        let result = get_field_in_component_json(
            &component,
            "not::A::RegisteredType",
            "translation",
            &registry,
        );
        assert!(result.is_none());
    }

    #[test]
    fn get_field_round_trips_with_set_field() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();

        let type_path = "bevy_transform::components::transform::Transform";
        let mut component = to_canonical_json(&Transform::IDENTITY, &registry);

        set_field_in_component_json(
            &mut component,
            type_path,
            "translation.y",
            serde_json::json!(7.0),
            &registry,
        );

        let result = get_field_in_component_json(&component, type_path, "translation.y", &registry);
        assert_eq!(result, Some(&serde_json::json!(7.0)));
    }

    // Local enum for the enum-unwrap test. Mirrors the `ColliderConstructor`
    // pattern referenced in `typed_json_path_get`.
    #[derive(Reflect, Clone)]
    enum TestShape {
        Sphere { radius: f32 },
        Box { half_x: f32, half_y: f32 },
    }

    // Local struct with a list field for the bracket-index test.
    #[derive(Reflect, Clone)]
    struct TestList {
        items: Vec<f32>,
    }

    #[test]
    fn get_field_unwraps_enum_variant_and_reads_field() {
        let mut registry = TypeRegistry::new();
        registry.register::<TestShape>();
        registry.register::<f32>();

        let type_path = "jackdaw::component_json::tests::TestShape";
        // Bevy's reflect serializer emits struct-variants as `{"Sphere": {"radius": 1.0}}`.
        let component = serde_json::json!({ "Sphere": { "radius": 1.0_f32 } });

        let result = get_field_in_component_json(&component, type_path, "radius", &registry);
        assert_eq!(result, Some(&serde_json::json!(1.0_f32)));
    }

    // Newtype tuple variant wrapping a struct, mirroring the
    // `Modifier::Mirror(MeshMirror)` shape the modifier stack uses. Bevy
    // serializes it flattened: `{"Wrap": {"flag": false, ...}}`.
    #[derive(Reflect, Clone)]
    enum TestModifier {
        Wrap(TestInner),
    }

    #[derive(Reflect, Clone)]
    struct TestInner {
        flag: bool,
        amount: f32,
    }

    #[test]
    fn newtype_variant_field_round_trips_through_index_zero() {
        let mut registry = TypeRegistry::new();
        registry.register::<TestModifier>();
        registry.register::<TestInner>();
        registry.register::<bool>();
        registry.register::<f32>();

        let type_path = "jackdaw::component_json::tests::TestModifier";
        let mut component = to_canonical_json(
            &TestModifier::Wrap(TestInner {
                flag: false,
                amount: 1.5,
            }),
            &registry,
        );

        // Reading the inner field through `.0` resolves the flattened newtype.
        let read = get_field_in_component_json(&component, type_path, "0.flag", &registry);
        assert_eq!(read, Some(&serde_json::json!(false)));

        // Writing through `.0` sets the inner field and reads back the new
        // value (the bug: this used to be a silent no-op so the edit reverted).
        set_field_in_component_json(
            &mut component,
            type_path,
            "0.flag",
            serde_json::json!(true),
            &registry,
        );
        let read_back = get_field_in_component_json(&component, type_path, "0.flag", &registry);
        assert_eq!(read_back, Some(&serde_json::json!(true)));

        // The sibling field is untouched.
        let amount = get_field_in_component_json(&component, type_path, "0.amount", &registry);
        assert_eq!(amount, Some(&serde_json::json!(1.5_f32)));
    }

    #[test]
    fn get_field_bracket_index_reads_list_element() {
        let mut registry = TypeRegistry::new();
        registry.register::<TestList>();
        registry.register::<Vec<f32>>();
        registry.register::<f32>();

        let type_path = "jackdaw::component_json::tests::TestList";
        let component = to_canonical_json(
            &TestList {
                items: vec![10.0, 20.0, 30.0],
            },
            &registry,
        );

        let result = get_field_in_component_json(&component, type_path, "items[1]", &registry);
        assert_eq!(result, Some(&serde_json::json!(20.0_f32)));
    }
}
