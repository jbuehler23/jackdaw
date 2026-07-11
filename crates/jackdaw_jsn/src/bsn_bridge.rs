//! Worldless structural bridge from parsed `.bsn` scene text to a
//! [`JsnScene`](crate::format::JsnScene).
//!
//! Document values map onto the reflect-JSON forms
//! `TypedReflectDeserializer` expects (newtype tuple structs flatten to
//! their inner value, unit enum variants become bare strings, data variants
//! become single-key objects, glam math types become arrays). Consumers with
//! no `World` access (the prefab cache, the headless runtime loader) read
//! `.bsn` files through this.

/// Convert parsed `.bsn` scene text into a `JsnScene`, without a `World`.
///
/// The bridge is structural: document values map onto the reflect-JSON forms
/// `TypedReflectDeserializer` expects (newtype tuple structs flatten to their
/// inner value, unit enum variants become bare strings, data variants become
/// single-key objects). The prefab cache uses this to read `.bsn` prefab
/// baselines in contexts that have no world access.
pub fn bsn_scene_to_jsn(text: &str) -> Result<crate::format::JsnScene, String> {
    bsn_scene_to_jsn_with_registry(text, None)
}

/// [`bsn_scene_to_jsn`] with asset-entry routing: when a registry is
/// available, document roots whose type resolves to a registered `Asset`
/// become entries in the scene's asset table (keyed `#Name`) instead of
/// entities, matching how the editor embeds scene-inline assets.
pub fn bsn_scene_to_jsn_with_registry(
    text: &str,
    registry: Option<&bevy::reflect::TypeRegistry>,
) -> Result<crate::format::JsnScene, String> {
    let ast =
        jackdaw_bsn::parse_bsn_text(text).map_err(|e| format!("could not parse .bsn: {e}"))?;

    let mut assets = crate::format::JsnAssets::default();
    let mut entities = Vec::new();
    let mut queue: Vec<(bevy::ecs::entity::Entity, Option<usize>)> =
        ast.roots.iter().map(|&r| (r, None)).collect();
    queue.reverse();

    while let Some((node, parent)) = queue.pop() {
        if parent.is_none()
            && let Some(registry) = registry
            && let Some((type_path, name, value)) = asset_entry_from_root(&ast, node, registry)
        {
            assets.0.entry(type_path).or_default().insert(name, value);
            continue;
        }
        let mut entity = crate::format::JsnEntity {
            id: None,
            parent,
            components: std::collections::HashMap::new(),
        };
        let index = entities.len();

        let Some(patches) = ast.get_patches(node) else {
            entities.push(entity);
            continue;
        };
        let mut children = Vec::new();
        for &pe in &patches.0 {
            match ast.get_patch(pe) {
                Some(jackdaw_bsn::BsnPatch::Name(name)) => {
                    entity.components.insert(
                        "bevy_ecs::name::Name".to_string(),
                        serde_json::Value::String(name.clone()),
                    );
                }
                Some(jackdaw_bsn::BsnPatch::Type(type_path)) => {
                    let (component_path, value) = type_patch_to_json(type_path);
                    entity.components.insert(component_path, value);
                }
                Some(jackdaw_bsn::BsnPatch::Struct(data)) => {
                    if data.type_path == jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH {
                        continue;
                    }
                    entity.components.insert(
                        base_type_path(&data.type_path),
                        struct_fields_to_json(&data.type_path, &data.fields),
                    );
                }
                Some(jackdaw_bsn::BsnPatch::TupleStruct(data)) => {
                    if data.type_path == jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH {
                        if let Some(jackdaw_bsn::BsnValue::Int(id)) = data.values.first() {
                            entity.id = u64::try_from(*id).ok();
                        }
                        continue;
                    }
                    entity.components.insert(
                        base_type_path(&data.type_path),
                        tuple_values_to_json(&data.type_path, &data.values),
                    );
                }
                Some(jackdaw_bsn::BsnPatch::Children(child_nodes)) => {
                    children.extend(child_nodes.iter().copied());
                }
                _ => {}
            }
        }
        entities.push(entity);
        for &child in children.iter().rev() {
            queue.push((child, Some(index)));
        }
    }

    Ok(crate::format::JsnScene {
        jsn: crate::format::JsnHeader::default(),
        metadata: crate::format::JsnMetadata::default(),
        assets,
        editor: None,
        scene: entities,
    })
}

/// When a root is a named asset entry (its non-name patch resolves to a
/// registered `Asset` type), produce `(type_path, "#Name", reflect JSON)`.
fn asset_entry_from_root(
    ast: &jackdaw_bsn::SceneBsnAst,
    root: bevy::ecs::entity::Entity,
    registry: &bevy::reflect::TypeRegistry,
) -> Option<(String, String, serde_json::Value)> {
    use bevy::asset::ReflectAsset;

    let patches = ast.get_patches(root)?;
    let name = ast.get_name(root)?.to_string();
    for &pe in &patches.0 {
        let (type_path, value) = match ast.get_patch(pe)? {
            jackdaw_bsn::BsnPatch::Struct(data) => (
                data.type_path.clone(),
                struct_fields_to_json(&data.type_path, &data.fields),
            ),
            jackdaw_bsn::BsnPatch::TupleStruct(data) => (
                data.type_path.clone(),
                tuple_values_to_json(&data.type_path, &data.values),
            ),
            jackdaw_bsn::BsnPatch::Type(type_path) => {
                (type_path.clone(), type_patch_to_json(type_path).1)
            }
            _ => continue,
        };
        let is_asset = registry
            .get_with_type_path(&type_path)
            .is_some_and(|registration| registration.data::<ReflectAsset>().is_some());
        if is_asset {
            return Some((type_path, format!("#{name}"), value));
        }
        return None;
    }
    None
}

/// Strip a variant segment from `Enum::Variant` paths; plain paths pass
/// through. Mirrors the document's variant-qualified type paths.
fn base_type_path(type_path: &str) -> String {
    match type_path.rsplit_once("::") {
        Some((base, variant))
            if variant.chars().next().is_some_and(char::is_uppercase)
                && base
                    .rsplit_once("::")
                    .is_some_and(|(_, ty)| ty.chars().next().is_some_and(char::is_uppercase)) =>
        {
            base.to_string()
        }
        _ => type_path.to_string(),
    }
}

fn type_patch_to_json(type_path: &str) -> (String, serde_json::Value) {
    let base = base_type_path(type_path);
    if base != type_path {
        // Unit enum variant: reflect JSON is the bare variant name.
        let variant = &type_path[base.len() + 2..];
        (base, serde_json::Value::String(variant.to_string()))
    } else {
        // Unit struct: reflect JSON is an empty object.
        (base, serde_json::json!({}))
    }
}

fn struct_fields_to_json(
    type_path: &str,
    fields: &jackdaw_bsn::BsnStructFields,
) -> serde_json::Value {
    // glam math types serialize as bare arrays of their fields, not objects.
    if type_path.starts_with("glam::") {
        return serde_json::Value::Array(
            fields
                .0
                .iter()
                .map(|f| bsn_value_to_json(&f.value))
                .collect(),
        );
    }
    let object: serde_json::Map<String, serde_json::Value> = fields
        .0
        .iter()
        .map(|f| (f.name.clone(), bsn_value_to_json(&f.value)))
        .collect();
    let base = base_type_path(type_path);
    if base != type_path {
        // Struct enum variant: {"Variant": {fields}}.
        let variant = &type_path[base.len() + 2..];
        serde_json::json!({ variant: object })
    } else {
        serde_json::Value::Object(object)
    }
}

fn tuple_values_to_json(type_path: &str, values: &[jackdaw_bsn::BsnValue]) -> serde_json::Value {
    let inner = if values.len() == 1 {
        bsn_value_to_json(&values[0])
    } else {
        serde_json::Value::Array(values.iter().map(bsn_value_to_json).collect())
    };
    let base = base_type_path(type_path);
    if base != type_path {
        let variant = &type_path[base.len() + 2..];
        serde_json::json!({ variant: inner })
    } else {
        inner
    }
}

fn bsn_value_to_json(value: &jackdaw_bsn::BsnValue) -> serde_json::Value {
    use jackdaw_bsn::BsnValue;
    match value {
        BsnValue::Float(f) => serde_json::json!(f),
        BsnValue::Int(i) => serde_json::json!(*i as i64),
        BsnValue::Bool(b) => serde_json::json!(b),
        BsnValue::String(s) => serde_json::Value::String(s.clone()),
        BsnValue::Type(type_path) => type_patch_to_json(type_path).1,
        BsnValue::Struct(data) => struct_fields_to_json(&data.type_path, &data.fields),
        BsnValue::TupleStruct(data) => tuple_values_to_json(&data.type_path, &data.values),
        BsnValue::List(items) => {
            serde_json::Value::Array(items.iter().map(bsn_value_to_json).collect())
        }
        BsnValue::Map(entries) => {
            let object: serde_json::Map<String, serde_json::Value> = entries
                .iter()
                .filter_map(|(k, v)| match k {
                    BsnValue::String(key) => Some((key.clone(), bsn_value_to_json(v))),
                    _ => None,
                })
                .collect();
            serde_json::Value::Object(object)
        }
    }
}
