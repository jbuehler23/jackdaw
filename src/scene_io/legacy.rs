//! Legacy JSN-era read machinery kept for the on-disk conversion path.
//!
//! The legacy converter (`crate::jsn_to_bsn`) still deserializes old `.jsn`
//! scenes through these reflect processors before writing the `.bsn`
//! sibling. They need the editor `World` / `AssetServer`, so they live here
//! in the editor crate rather than in `jackdaw_jsn` (which must not depend
//! on the editor).

use std::any::TypeId;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Formatter};
use std::path::{Path, PathBuf};
use std::result::Result;

use bevy::asset::{ReflectAsset, ReflectHandle, UntypedAssetId};
use bevy::image::ImageLoaderSettings;
use bevy::reflect::serde::{ReflectDeserializerProcessor, ReflectSerializerProcessor};
use bevy::reflect::{TypeInfo, TypeRegistration, TypeRegistry};
use bevy::{
    asset::AssetPath,
    ecs::reflect::AppTypeRegistry,
    prelude::*,
    reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer},
    transform::components::TransformTreeChanged,
};
use jackdaw_jsn::format::{JsnAssets, JsnEntity};
use serde::de::{DeserializeSeed, Visitor};
use serde::{Deserializer, Serializer};

struct JsnSerializerProcessor<'a> {
    parent_path: Cow<'a, Path>,
    /// Maps runtime asset IDs (no path) to inline `#Name` references.
    inline_assets: &'a HashMap<UntypedAssetId, String>,
    /// Maps scene entities to their index in the entity array.
    entity_to_index: &'a HashMap<Entity, usize>,
}

impl<'a> ReflectSerializerProcessor for JsnSerializerProcessor<'a> {
    fn try_serialize<S>(
        &self,
        value: &dyn PartialReflect,
        registry: &TypeRegistry,
        serializer: S,
    ) -> Result<Result<S::Ok, S>, S::Error>
    where
        S: Serializer,
    {
        let Some(value) = value.try_as_reflect() else {
            return Ok(Err(serializer));
        };
        let type_id = value.reflect_type_info().type_id();

        // Non-finite floats: JSON has no infinity/NaN, serialize as descriptive strings
        if type_id == TypeId::of::<f32>()
            && let Some(&v) = value.as_any().downcast_ref::<f32>()
            && !v.is_finite()
        {
            let s = if v == f32::INFINITY {
                "inf"
            } else if v == f32::NEG_INFINITY {
                "-inf"
            } else {
                "NaN"
            };
            return Ok(Ok(serializer.serialize_str(s)?));
        }
        if type_id == TypeId::of::<f64>()
            && let Some(&v) = value.as_any().downcast_ref::<f64>()
            && !v.is_finite()
        {
            let s = if v == f64::INFINITY {
                "inf"
            } else if v == f64::NEG_INFINITY {
                "-inf"
            } else {
                "NaN"
            };
            return Ok(Ok(serializer.serialize_str(s)?));
        }

        // Handle<T> -> path string or inline #Name
        if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(type_id) {
            let untyped_handle = reflect_handle
                .downcast_handle_untyped(value.as_any())
                .expect("This must have been a handle");

            // Check collected asset references first (both inline and external)
            if let Some(inline_name) = self.inline_assets.get(&untyped_handle.id()) {
                return Ok(Ok(serializer.serialize_str(inline_name)?));
            }

            if let Some(path) = untyped_handle.path() {
                // Uncollected external asset  -- serialize as relative path (backward compat)
                let rel = pathdiff::diff_paths(path.path(), &self.parent_path)
                    .unwrap_or_else(|| path.path().to_owned());
                let mut path_str = rel.to_string_lossy().into_owned();
                if let Some(label) = path.label() {
                    path_str.push('#');
                    path_str.push_str(label);
                }
                return Ok(Ok(serializer.serialize_str(&path_str)?));
            }

            // Unknown handle (no path, not inline)  -- serialize as null
            return Ok(Ok(serializer.serialize_unit()?));
        }

        // Entity -> scene-local index
        if type_id == TypeId::of::<Entity>() {
            if let Some(entity) = value.as_any().downcast_ref::<Entity>()
                && let Some(&idx) = self.entity_to_index.get(entity)
            {
                return Ok(Ok(serializer.serialize_u64(idx as u64)?));
            }
            return Ok(Ok(serializer.serialize_unit()?));
        }

        Ok(Err(serializer))
    }
}

pub(crate) struct JsnDeserializerProcessor<'a> {
    pub(crate) asset_server: &'a AssetServer,
    pub(crate) parent_path: &'a Path,
    /// Maps inline `#Name` references to loaded handles.
    pub(crate) local_assets: &'a HashMap<String, UntypedHandle>,
    /// Maps catalog `@Name` references to loaded handles.
    pub(crate) catalog_assets: &'a HashMap<String, UntypedHandle>,
    /// Maps scene-local indices to spawned entities.
    pub(crate) entity_map: &'a [Entity],
}

impl<'a> ReflectDeserializerProcessor for JsnDeserializerProcessor<'a> {
    fn try_deserialize<'de, D>(
        &mut self,
        registration: &TypeRegistration,
        _registry: &TypeRegistry,
        deserializer: D,
    ) -> Result<Result<Box<dyn PartialReflect>, D>, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Non-finite floats: deserialize from string ("inf", "-inf", "NaN") or number
        if registration.type_id() == TypeId::of::<f32>() {
            let val = deserializer
                .deserialize_any(F32Visitor)
                .map_err(<D::Error as serde::de::Error>::custom)?;
            return Ok(Ok(Box::new(val).into_partial_reflect()));
        }
        if registration.type_id() == TypeId::of::<f64>() {
            let val = deserializer
                .deserialize_any(F64Visitor)
                .map_err(<D::Error as serde::de::Error>::custom)?;
            return Ok(Ok(Box::new(val).into_partial_reflect()));
        }

        // Handle<T>  -- deserialize from path string or #Name
        if registration.data::<ReflectHandle>().is_some() {
            let type_info = registration.type_info();

            let relative_path = match deserializer.deserialize_any(&*self) {
                Ok(path) => path,
                Err(error) => {
                    error!(
                        "Failed to deserialize `{}`: {:?}",
                        type_info.type_path(),
                        error
                    );
                    return Err(error);
                }
            };

            // Null sentinel (from old files with "material": null) -> default handle
            if relative_path.is_empty()
                && let Some(reflect_default) = registration.data::<ReflectDefault>()
            {
                return Ok(Ok(reflect_default.default().into_partial_reflect()));
            }

            // Check for catalog asset reference (@Name)
            if relative_path.starts_with('@') {
                if let Some(handle) = self.catalog_assets.get(&relative_path) {
                    return Ok(Ok(Box::new(handle.clone()).into_partial_reflect()));
                }
                warn!(
                    "Catalog asset '{}' not found  -- using default",
                    relative_path
                );
                if let Some(reflect_default) = registration.data::<ReflectDefault>() {
                    return Ok(Ok(reflect_default.default().into_partial_reflect()));
                }
            }

            // Check for inline asset reference (#Name)
            if let Some(handle) = self.local_assets.get(&relative_path) {
                return Ok(Ok(Box::new(handle.clone()).into_partial_reflect()));
            }

            // External asset path. Resolve to a filesystem path
            // first (in case it was scene-relative), then strip
            // the assets-dir prefix so AssetServer treats it as
            // an approved path.
            let stem_pos = relative_path.find('#').unwrap_or(relative_path.len());
            let stem = self.relative_path_to_asset_path(&relative_path[0..stem_pos]);
            let stem_fs = stem.to_string_lossy().into_owned();
            let mut asset_path = crate::entity_ops::to_asset_path(&stem_fs);
            asset_path.push_str(&relative_path[stem_pos..]);

            let handle = self.asset_server.load_builder().load_untyped(asset_path);
            return Ok(Ok(Box::new(handle).into_partial_reflect()));
        }

        // Entity  -- deserialize from scene-local index
        if registration.type_id() == TypeId::of::<Entity>() {
            let Ok(idx_str) = deserializer.deserialize_u64(&*self) else {
                // Not a valid index, return placeholder
                return Ok(Ok(Box::new(Entity::PLACEHOLDER).into_partial_reflect()));
            };
            let idx: usize = idx_str.parse().unwrap_or(usize::MAX);
            let entity = self
                .entity_map
                .get(idx)
                .copied()
                .unwrap_or(Entity::PLACEHOLDER);
            return Ok(Ok(Box::new(entity).into_partial_reflect()));
        }

        Ok(Err(deserializer))
    }
}

impl<'a> Visitor<'_> for &'a JsnDeserializerProcessor<'a> {
    type Value = String;

    fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
        write!(formatter, "a string, integer, or null")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(String::new())
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.to_owned())
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.to_string())
    }
}

struct F32Visitor;

impl Visitor<'_> for F32Visitor {
    type Value = f32;

    fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
        write!(formatter, "a number or float string (inf, -inf, NaN)")
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
        Ok(v as f32)
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(v as f32)
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(v as f32)
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        match v {
            "inf" | "Infinity" => Ok(f32::INFINITY),
            "-inf" | "-Infinity" => Ok(f32::NEG_INFINITY),
            "NaN" | "nan" => Ok(f32::NAN),
            _ => Err(E::custom(format!("unexpected float string: {v}"))),
        }
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(0.0) // backward compat: old files with null
    }
}

struct F64Visitor;

impl Visitor<'_> for F64Visitor {
    type Value = f64;

    fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
        write!(formatter, "a number or float string (inf, -inf, NaN)")
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
        Ok(v)
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(v as f64)
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(v as f64)
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        match v {
            "inf" | "Infinity" => Ok(f64::INFINITY),
            "-inf" | "-Infinity" => Ok(f64::NEG_INFINITY),
            "NaN" | "nan" => Ok(f64::NAN),
            _ => Err(E::custom(format!("unexpected float string: {v}"))),
        }
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(0.0) // backward compat: old files with null
    }
}

impl<'a> JsnDeserializerProcessor<'a> {
    fn relative_path_to_asset_path(&self, asset_path: &str) -> PathBuf {
        let mut asset_path = Path::new(asset_path).to_owned();
        if asset_path.is_relative() {
            asset_path = self.parent_path.join(asset_path);
        }
        asset_path
    }
}

/// Recursively walk a reflected value looking for `Handle<T>` fields that are runtime-created.
fn collect_handles_from_reflect(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    world: &World,
    parent_path: &Path,
    id_to_name: &mut HashMap<UntypedAssetId, String>,
    asset_data: &mut HashMap<String, HashMap<String, serde_json::Value>>,
    counters: &mut HashMap<String, usize>,
    catalog_id_to_name: &HashMap<UntypedAssetId, String>,
) {
    let Some(value) = value.try_as_reflect() else {
        return;
    };
    let type_id = value.reflect_type_info().type_id();

    // Check if this is a Handle<T>
    if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(type_id) {
        let untyped_handle = reflect_handle
            .downcast_handle_untyped(value.as_any())
            .expect("This must have been a handle");

        // Already collected  -- skip
        if id_to_name.contains_key(&untyped_handle.id()) {
            return;
        }

        // Check catalog first  -- if this handle is a catalog asset with an @Name,
        // emit @Name and don't inline it into the scene's asset table.
        // Skip #-prefixed entries (internal catalog references like #Image8)
        // because those are only meaningful inside the catalog, not in scenes.
        if let Some(catalog_name) = catalog_id_to_name.get(&untyped_handle.id())
            && catalog_name.starts_with('@')
        {
            id_to_name.insert(untyped_handle.id(), catalog_name.clone());
            return;
        }

        // External file-backed resource  -- store as a path string entry
        if let Some(asset_path) = untyped_handle.path() {
            let asset_type_id = reflect_handle.asset_type_id();
            let Some(asset_registration) = registry.get(asset_type_id) else {
                return;
            };
            let asset_type_path = asset_registration
                .type_info()
                .type_path_table()
                .path()
                .to_string();

            let counter = counters.entry(asset_type_path.clone()).or_insert(0);
            let short_name = asset_type_path
                .rsplit("::")
                .next()
                .unwrap_or(&asset_type_path);
            let inline_name = format!("#{short_name}{counter}");
            *counter += 1;

            let rel = pathdiff::diff_paths(asset_path.path(), parent_path)
                .unwrap_or_else(|| asset_path.path().to_owned());
            let mut path_str = rel.to_string_lossy().into_owned();
            if let Some(label) = asset_path.label() {
                path_str.push('#');
                path_str.push_str(label);
            }

            id_to_name.insert(untyped_handle.id(), inline_name.clone());
            asset_data
                .entry(asset_type_path)
                .or_default()
                .insert(inline_name, serde_json::Value::String(path_str));
            return;
        }

        // Skip default/UUID handles (not backed by a live asset)
        if matches!(untyped_handle, UntypedHandle::Uuid { .. }) {
            return;
        }

        let asset_type_id = reflect_handle.asset_type_id();
        let Some(asset_registration) = registry.get(asset_type_id) else {
            return;
        };
        let Some(reflect_asset) = asset_registration.data::<ReflectAsset>() else {
            return;
        };

        let asset_type_path = asset_registration
            .type_info()
            .type_path_table()
            .path()
            .to_string();

        // Get the asset data and serialize it
        let Some(asset_reflect) = reflect_asset.get(world, untyped_handle.id()) else {
            return;
        };

        // Recurse into the asset to collect nested handles (e.g. textures inside materials)
        // before serializing, so they get #Name entries and the serializer emits refs not paths.
        collect_handles_from_reflect(
            asset_reflect.as_partial_reflect(),
            registry,
            world,
            parent_path,
            id_to_name,
            asset_data,
            counters,
            catalog_id_to_name,
        );

        // Generate a name like "Material0", "Material1"
        let counter = counters.entry(asset_type_path.clone()).or_insert(0);
        let short_name = asset_type_path
            .rsplit("::")
            .next()
            .unwrap_or(&asset_type_path);
        let inline_name = format!("#{short_name}{counter}");
        *counter += 1;

        // Serialize the asset using the processor (for nested handles like textures inside materials)
        let ser_processor = JsnSerializerProcessor {
            parent_path: Cow::Borrowed(parent_path),
            inline_assets: id_to_name, // partial map, but handles already collected will be there
            entity_to_index: &HashMap::new(),
        };
        let serializer =
            TypedReflectSerializer::with_processor(asset_reflect, registry, &ser_processor);
        if let Ok(json_value) = serde_json::to_value(&serializer) {
            id_to_name.insert(untyped_handle.id(), inline_name.clone());
            asset_data
                .entry(asset_type_path)
                .or_default()
                .insert(inline_name, json_value);
        }

        return;
    }

    // Recurse into struct/tuple/list/map fields
    match value.reflect_ref() {
        bevy::reflect::ReflectRef::Struct(s) => {
            for i in 0..s.field_len() {
                if let Some(field) = s.field_at(i) {
                    collect_handles_from_reflect(
                        field,
                        registry,
                        world,
                        parent_path,
                        id_to_name,
                        asset_data,
                        counters,
                        catalog_id_to_name,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::TupleStruct(ts) => {
            for i in 0..ts.field_len() {
                if let Some(field) = ts.field(i) {
                    collect_handles_from_reflect(
                        field,
                        registry,
                        world,
                        parent_path,
                        id_to_name,
                        asset_data,
                        counters,
                        catalog_id_to_name,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::Tuple(t) => {
            for i in 0..t.field_len() {
                if let Some(field) = t.field(i) {
                    collect_handles_from_reflect(
                        field,
                        registry,
                        world,
                        parent_path,
                        id_to_name,
                        asset_data,
                        counters,
                        catalog_id_to_name,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::List(l) => {
            for i in 0..l.len() {
                if let Some(item) = l.get(i) {
                    collect_handles_from_reflect(
                        item,
                        registry,
                        world,
                        parent_path,
                        id_to_name,
                        asset_data,
                        counters,
                        catalog_id_to_name,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::Array(a) => {
            for i in 0..a.len() {
                if let Some(item) = a.get(i) {
                    collect_handles_from_reflect(
                        item,
                        registry,
                        world,
                        parent_path,
                        id_to_name,
                        asset_data,
                        counters,
                        catalog_id_to_name,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::Map(m) => {
            for (_k, v) in m.iter() {
                collect_handles_from_reflect(
                    v,
                    registry,
                    world,
                    parent_path,
                    id_to_name,
                    asset_data,
                    counters,
                    catalog_id_to_name,
                );
            }
        }
        bevy::reflect::ReflectRef::Set(s) => {
            for item in s.iter() {
                collect_handles_from_reflect(
                    item,
                    registry,
                    world,
                    parent_path,
                    id_to_name,
                    asset_data,
                    counters,
                    catalog_id_to_name,
                );
            }
        }
        bevy::reflect::ReflectRef::Enum(e) => {
            for i in 0..e.field_len() {
                if let Some(field) = e.field_at(i) {
                    collect_handles_from_reflect(
                        field,
                        registry,
                        world,
                        parent_path,
                        id_to_name,
                        asset_data,
                        counters,
                        catalog_id_to_name,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::Opaque(_) => {}
    }
}

/// Serialize a single runtime asset (and its nested handles like textures)
/// into `JsnAssets` format. `parent_path` is used to compute relative file paths
/// (should be the assets directory so texture paths resolve correctly on reload).
pub fn serialize_asset_into(
    world: &World,
    handle: UntypedHandle,
    name: &str,
    parent_path: &Path,
    assets: &mut JsnAssets,
) {
    let registry = world.resource::<AppTypeRegistry>().read();

    // UntypedHandle::type_id() returns the *asset* type ID directly (e.g. StandardMaterial)
    let asset_type_id = handle.type_id();
    let Some(asset_registration) = registry.get(asset_type_id) else {
        return;
    };
    let Some(reflect_asset) = asset_registration.data::<ReflectAsset>() else {
        return;
    };
    let asset_type_path = asset_registration
        .type_info()
        .type_path_table()
        .path()
        .to_string();

    let Some(asset_reflect) = reflect_asset.get(world, handle.id()) else {
        return;
    };

    // Collect nested handles (e.g. textures inside a StandardMaterial)
    let empty_catalog = HashMap::new();
    let mut id_to_name: HashMap<UntypedAssetId, String> = HashMap::new();
    let mut nested_assets: HashMap<String, HashMap<String, serde_json::Value>> = HashMap::new();

    // Seed counters from existing entries so subsequent calls don't reuse names
    let mut counters: HashMap<String, usize> = HashMap::new();
    for (type_path, entries) in &assets.0 {
        counters.insert(type_path.clone(), entries.len());
    }

    collect_handles_from_reflect(
        asset_reflect.as_partial_reflect(),
        &registry,
        world,
        parent_path,
        &mut id_to_name,
        &mut nested_assets,
        &mut counters,
        &empty_catalog,
    );

    // Merge nested asset entries (images etc.) into the output JsnAssets
    for (type_path, entries) in nested_assets {
        let target = assets.0.entry(type_path).or_default();
        for (entry_name, value) in entries {
            target.insert(entry_name, value);
        }
    }

    // Serialize the root asset itself
    let ser_processor = JsnSerializerProcessor {
        parent_path: Cow::Borrowed(parent_path),
        inline_assets: &id_to_name,
        entity_to_index: &HashMap::new(),
    };
    let serializer =
        TypedReflectSerializer::with_processor(asset_reflect, &registry, &ser_processor);
    if let Ok(json_value) = serde_json::to_value(&serializer) {
        assets
            .0
            .entry(asset_type_path)
            .or_default()
            .insert(name.to_string(), json_value);
    }
}

/// Deserialize inline assets from the generic assets table.
/// Returns a map of `#Name` / `@Name` -> `UntypedHandle` for the deserializer processor.
/// Scan material definitions in `JsnAssets` to find image names used in non-color slots.
/// These images must be loaded with `is_srgb = false` to avoid gamma decoding artifacts.
fn collect_linear_image_names(assets: &JsnAssets) -> HashSet<String> {
    const LINEAR_SLOTS: &[&str] = &[
        "normal_map_texture",
        "metallic_roughness_texture",
        "occlusion_texture",
        "depth_map",
    ];
    let mut linear_names = HashSet::new();
    let mat_type = "bevy_pbr::pbr_material::StandardMaterial";
    if let Some(materials) = assets.0.get(mat_type) {
        for json_value in materials.values() {
            if let serde_json::Value::Object(obj) = json_value {
                for slot in LINEAR_SLOTS {
                    if let Some(serde_json::Value::String(img_name)) = obj.get(*slot) {
                        linear_names.insert(img_name.clone());
                    }
                }
            }
        }
    }
    linear_names
}

/// Serialize a type's reflect `Default` to a JSON value. Returns
/// `None` when the type has no `ReflectDefault` or the value cannot
/// be serialized.
fn serialize_reflect_default(
    registration: &TypeRegistration,
    registry: &TypeRegistry,
) -> Option<serde_json::Value> {
    let default = registration.data::<ReflectDefault>()?.default();
    let serializer = TypedReflectSerializer::new(default.as_partial_reflect(), registry);
    serde_json::to_value(&serializer).ok()
}

/// Produce a complete JSON object for `registration` by filling
/// on-disk values into the type's serialized default. Fields present in
/// `on_disk` are kept; missing fields take the default; keys not in the
/// struct are dropped.
///
/// Only top-level struct fields are filled. Enums, tuple structs,
/// tuples, collections, and opaque leaves are taken wholly from
/// `on_disk` so variant tags and element shapes stay intact. Handle
/// refs (`@Name`/`#Name` strings, `null`) ride through unchanged and
/// resolve in the deserializer processor; default handle fields
/// serialize to `null`, which the processor turns into a default handle.
fn fill_missing_with_defaults(
    on_disk: &serde_json::Value,
    registration: &TypeRegistration,
    registry: &TypeRegistry,
) -> serde_json::Value {
    let TypeInfo::Struct(struct_info) = registration.type_info() else {
        return on_disk.clone();
    };
    let serde_json::Value::Object(disk_map) = on_disk else {
        return on_disk.clone();
    };

    let mut out = match serialize_reflect_default(registration, registry) {
        Some(serde_json::Value::Object(default_map)) => default_map,
        // No default to fill from: pass the on-disk fields through.
        _ => disk_map.clone(),
    };

    for field in struct_info.iter() {
        let Some(disk_value) = disk_map.get(field.name()) else {
            continue;
        };
        let filled = match registry.get(field.type_id()) {
            Some(field_reg) => fill_missing_with_defaults(disk_value, field_reg, registry),
            None => disk_value.clone(),
        };
        out.insert(field.name().to_string(), filled);
    }

    serde_json::Value::Object(out)
}

pub fn load_inline_assets(
    world: &mut World,
    assets: &JsnAssets,
    parent_path: &Path,
) -> HashMap<String, UntypedHandle> {
    let mut local_assets: HashMap<String, UntypedHandle> = HashMap::new();

    // Pre-populate with catalog assets so @Name references in string values resolve
    let catalog_handles = world
        .get_resource::<crate::asset_catalog::AssetCatalog>()
        .map(|c| c.handles.clone())
        .unwrap_or_default();

    let linear_image_names = collect_linear_image_names(assets);

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry_guard = registry.read();
    let asset_server = world.resource::<AssetServer>().clone();

    // First pass: load all string-value entries (external file refs like textures).
    // These must be loaded before inline assets that may reference them.
    for (type_path, named_entries) in &assets.0 {
        for (name, json_value) in named_entries {
            let serde_json::Value::String(rel_path) = json_value else {
                continue;
            };

            // @Name reference -> resolve from catalog
            if rel_path.starts_with('@') {
                if let Some(handle) = catalog_handles.get(rel_path.as_str()) {
                    local_assets.insert(name.clone(), handle.clone());
                } else {
                    warn!("Catalog asset '{rel_path}' referenced by '{name}' not found");
                }
                continue;
            }

            let abs_path = if Path::new(rel_path).is_relative() {
                parent_path.join(rel_path)
            } else {
                PathBuf::from(rel_path)
            };
            let path_str = abs_path.to_string_lossy().into_owned();
            // AssetServer is rooted at the project's `assets/`;
            // strip the prefix so the load stays inside Bevy's
            // approved-path set (no `UnapprovedPathMode::Allow`).
            let asset_path = crate::entity_ops::to_asset_path(&path_str);

            let handle = if type_path == "bevy_image::image::Image" {
                if linear_image_names.contains(name) {
                    asset_server
                        .load_builder()
                        .with_settings(|s: &mut ImageLoaderSettings| s.is_srgb = false)
                        .load::<Image>(&asset_path)
                        .untyped()
                } else {
                    asset_server.load::<Image>(&asset_path).untyped()
                }
            } else {
                warn!(
                    "External asset entry '{name}' has unknown type '{type_path}'  -- loading untyped"
                );
                asset_server
                    .load::<bevy::asset::LoadedUntypedAsset>(&asset_path)
                    .untyped()
            };
            local_assets.insert(name.clone(), handle);
        }
    }

    // Second pass: deserialize all object-value entries (inline assets like materials)
    for (type_path, named_entries) in &assets.0 {
        let Some(registration) = registry_guard.get_with_type_path(type_path) else {
            warn!("Unknown asset type '{type_path}' in inline assets  -- skipping");
            continue;
        };
        let Some(reflect_asset) = registration.data::<ReflectAsset>() else {
            warn!("Type '{type_path}' has no ReflectAsset  -- skipping");
            continue;
        };

        for (name, json_value) in named_entries {
            // String entries already handled in first pass
            if json_value.is_string() {
                continue;
            }

            // Deserialize with processor to resolve nested handles (e.g. textures in materials)
            let mut deser_processor = JsnDeserializerProcessor {
                asset_server: &asset_server,
                parent_path,
                local_assets: &local_assets,
                catalog_assets: &catalog_handles,
                entity_map: &[],
            };

            // Fill fields the on-disk value omits using the type's
            // default so a strict deserializer sees a complete object.
            let filled = fill_missing_with_defaults(json_value, registration, &registry_guard);
            let deserializer = TypedReflectDeserializer::with_processor(
                registration,
                &registry_guard,
                &mut deser_processor,
            );
            let Ok(reflected) = deserializer.deserialize(&filled) else {
                warn!("Failed to deserialize inline asset '{name}' of type '{type_path}'");
                continue;
            };

            // Add into the asset store and get a handle
            let handle = reflect_asset.add(world, reflected.as_ref());
            local_assets.insert(name.clone(), handle);
        }
    }

    local_assets
}

/// Spawn entities from a `Vec<JsnEntity>` into the world using reflection.
/// Returns the spawned entity list (index-matched to input).
pub fn load_scene_from_jsn(
    world: &mut World,
    entities: &[JsnEntity],
    parent_path: &Path,
    local_assets: &HashMap<String, UntypedHandle>,
) -> Vec<Entity> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let asset_server = world.resource::<AssetServer>().clone();
    let catalog_handles = world
        .get_resource::<crate::asset_catalog::AssetCatalog>()
        .map(|c| c.handles.clone())
        .unwrap_or_default();

    // First pass: spawn empty entities (Name/Transform/Visibility come from components)
    let mut spawned: Vec<Entity> = Vec::new();
    for _jsn in entities.iter() {
        let entity = world.spawn_empty();
        spawned.push(entity.id());
    }

    // Second pass: deserialize extensible components via reflection with processor.
    //
    // `ChildOf` is inserted last, after components + require-chain
    // backfill. Bevy's `validate_parent_has_component` on `on_insert`
    // for `InheritedVisibility` / `GlobalTransform` would otherwise
    // log spurious B0004 warnings when children get their derived
    // components before parents do.
    let registry_guard = registry.read();
    for (i, jsn) in entities.iter().enumerate() {
        for (type_path, value) in &jsn.components {
            let Some(registration) = registry_guard.get_with_type_path(type_path) else {
                warn!("Unknown type '{type_path}'  -- skipping");
                continue;
            };
            if registration.data::<ReflectComponent>().is_none() {
                warn!("Type '{type_path}' has no ReflectComponent  -- skipping");
                continue;
            }

            // A marker or otherwise-empty component serializes to
            // `null`. Insert the type's default so the marker survives
            // the round-trip; a strict deserializer rejects `null`.
            if value.is_null()
                && let Some(reflect_default) = registration.data::<ReflectDefault>()
            {
                world
                    .entity_mut(spawned[i])
                    .insert_reflect(reflect_default.default().into_partial_reflect());
                continue;
            }

            let mut deser_processor = JsnDeserializerProcessor {
                asset_server: &asset_server,
                parent_path,
                local_assets,
                catalog_assets: &catalog_handles,
                entity_map: &spawned,
            };
            // Fill fields the on-disk value omits using the type's
            // default so a strict deserializer sees a complete object.
            let filled = fill_missing_with_defaults(value, registration, &registry_guard);
            let deserializer = TypedReflectDeserializer::with_processor(
                registration,
                &registry_guard,
                &mut deser_processor,
            );
            let Ok(reflected) = deserializer.deserialize(&filled) else {
                warn!("Failed to deserialize '{type_path}'  -- skipping");
                continue;
            };

            world.entity_mut(spawned[i]).insert_reflect(reflected);
        }
    }
    drop(registry_guard);

    // `insert_reflect` doesn't fire `#[require(...)]`. Backfill the
    // hierarchy-propagation chain so Bevy doesn't B0004-warn and
    // children render at correct world positions.
    for &entity in &spawned {
        let mut ent = world.entity_mut(entity);
        if ent.contains::<Transform>() {
            if !ent.contains::<GlobalTransform>() {
                ent.insert(GlobalTransform::default());
            }
            if !ent.contains::<TransformTreeChanged>() {
                ent.insert(TransformTreeChanged);
            }
        }
        if ent.contains::<Visibility>() {
            if !ent.contains::<InheritedVisibility>() {
                ent.insert(InheritedVisibility::default());
            }
            if !ent.contains::<ViewVisibility>() {
                ent.insert(ViewVisibility::default());
            }
        }
    }

    // Attach the stable node id so the live preview entity can be mapped
    // back to its authored node (PIE "save runtime values" relies on this).
    // The structural `id` is canonical; mint a fresh one only when the
    // source entry predates node ids.
    for (i, jsn) in entities.iter().enumerate() {
        let node_id = jsn
            .id
            .map(jackdaw_scene_types::SceneNodeId)
            .unwrap_or_else(jackdaw_scene_types::SceneNodeId::next);
        world.entity_mut(spawned[i]).insert(node_id);
    }

    // Wire ChildOf relationships now that every entity has its full
    // component set (see the ChildOf-last comment above).
    for (i, jsn) in entities.iter().enumerate() {
        if let Some(parent_idx) = jsn.parent
            && let Some(&parent_entity) = spawned.get(parent_idx)
        {
            world.entity_mut(spawned[i]).insert(ChildOf(parent_entity));
        }
    }

    // Post-load: re-trigger GLTF loading for GltfSource entities
    let gltf_entities: Vec<(Entity, String, usize)> = spawned
        .iter()
        .filter_map(|&e| {
            world
                .get::<jackdaw_scene_types::GltfSource>(e)
                .map(|gs| (e, gs.path.clone(), gs.scene_index))
        })
        .collect();
    for (entity, gltf_path, scene_index) in gltf_entities {
        let asset_server = world.resource::<AssetServer>();
        let asset_path: AssetPath<'static> = crate::entity_ops::to_asset_path(&gltf_path).into();
        let scene = asset_server.load(GltfAssetLabel::Scene(scene_index).from_asset(asset_path));
        world.entity_mut(entity).insert(WorldAssetRoot(scene));
    }

    spawned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_io::register_entities_in_ast;

    /// A struct whose on-disk form lacks `added`, standing in for data
    /// that predates a field being added to the type.
    #[derive(Reflect, Default)]
    #[reflect(Default)]
    struct OldNew {
        kept: f32,
        added: u32,
    }

    /// Fills missing struct fields from the type default, keeps present
    /// fields, drops unknown keys, and leaves a strict deserializer able
    /// to build the value.
    #[test]
    fn fill_missing_with_defaults_completes_partial_struct() {
        let mut registry = TypeRegistry::new();
        registry.register::<OldNew>();
        registry.register::<f32>();
        registry.register::<u32>();
        let registration = registry.get(TypeId::of::<OldNew>()).unwrap();

        // Old data: `kept` only, plus a field that no longer exists.
        let on_disk = serde_json::json!({ "kept": 2.5, "gone": 9 });
        let filled = fill_missing_with_defaults(&on_disk, registration, &registry);

        let obj = filled.as_object().expect("filled value is an object");
        assert_eq!(
            obj.get("kept").and_then(serde_json::Value::as_f64),
            Some(2.5)
        );
        assert_eq!(
            obj.get("added").and_then(serde_json::Value::as_u64),
            Some(0)
        );
        assert!(!obj.contains_key("gone"), "unknown key must be dropped");

        let deserializer = TypedReflectDeserializer::new(registration, &registry);
        let reflected = deserializer
            .deserialize(&filled)
            .expect("strict deserialize succeeds on filled object");
        let value = OldNew::from_reflect(reflected.as_ref()).expect("materialize OldNew");
        assert_eq!(value.kept, 2.5);
        assert_eq!(value.added, 0);
    }

    /// A complete object round-trips unchanged: every present value is
    /// preserved, so behavior is a no-op for current data.
    #[test]
    fn fill_missing_with_defaults_is_noop_for_complete_object() {
        let mut registry = TypeRegistry::new();
        registry.register::<OldNew>();
        registry.register::<f32>();
        registry.register::<u32>();
        let registration = registry.get(TypeId::of::<OldNew>()).unwrap();

        let on_disk = serde_json::json!({ "kept": 1.0, "added": 7 });
        let filled = fill_missing_with_defaults(&on_disk, registration, &registry);

        let obj = filled.as_object().expect("filled value is an object");
        assert_eq!(
            obj.get("kept").and_then(serde_json::Value::as_f64),
            Some(1.0)
        );
        assert_eq!(
            obj.get("added").and_then(serde_json::Value::as_u64),
            Some(7)
        );
    }

    /// Every entity spawned from a scene carries a `SceneNodeId`, and the id
    /// survives a save (`build_scene_snapshot`) then load round-trip so the
    /// running game can map a live entity back to its authored node.
    #[test]
    fn spawned_entities_carry_node_id_and_round_trip() {
        use jackdaw_jsn::format::JsnEntity;
        use jackdaw_scene_types::SceneNodeId;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default());
        app.register_type::<SceneNodeId>();
        app.register_type::<Name>();

        // Two entities with explicit on-disk ids (parent + child).
        let entities = vec![
            JsnEntity {
                id: Some(42),
                parent: None,
                components: HashMap::new(),
            },
            JsnEntity {
                id: Some(99),
                parent: Some(0),
                components: HashMap::new(),
            },
        ];

        let spawned =
            load_scene_from_jsn(app.world_mut(), &entities, Path::new("."), &HashMap::new());
        assert_eq!(spawned.len(), 2);

        let id0 = app
            .world()
            .get::<SceneNodeId>(spawned[0])
            .expect("spawned entity should carry SceneNodeId");
        let id1 = app
            .world()
            .get::<SceneNodeId>(spawned[1])
            .expect("spawned child should carry SceneNodeId");
        assert_eq!(*id0, SceneNodeId(42));
        assert_eq!(*id1, SceneNodeId(99));

        // Register the spawned entities in the live scene document and
        // confirm the ids ride along as the nodes' stable ids.
        app.world_mut()
            .insert_resource(jackdaw_bsn::SceneBsnAst::default());
        register_entities_in_ast(app.world_mut(), &spawned);
        let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
        let node0 = ast.ast_for(spawned[0]).expect("node 42 registered");
        let node1 = ast.ast_for(spawned[1]).expect("node 99 registered");
        assert_eq!(ast.stable_id_of(node0), Some(42));
        assert_eq!(ast.stable_id_of(node1), Some(99));
    }
}
