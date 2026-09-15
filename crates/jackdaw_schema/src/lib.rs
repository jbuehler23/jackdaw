//! Project type schema: the data the editor needs about a project's
//! reflected types, produced by the project itself and consumed by the
//! editor as plain data.
//!
//! Every field defaults on deserialization, so an older dump still loads and
//! the editor's offer is out of date rather than wrong. Nothing here is the
//! authority on what a game accepts; the live type registry is.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The argument that puts a game into schema-reporting mode.
///
/// The editor/build pipeline passes this; `jackdaw_runtime` answers it.
/// One constant so the two halves cannot drift.
pub const SCHEMA_FLAG: &str = "--jackdaw-extract-schema";

/// Parse an extractor's stdout into a [`ProjectSchema`].
///
/// The whole stream is tried first. Games may print during startup, so
/// a stray line before the payload falls back to scanning for the line
/// that parses rather than failing over unrelated output.
pub fn parse_from_stdout(stdout: &[u8]) -> Result<ProjectSchema, String> {
    if let Ok(schema) = serde_json::from_slice::<ProjectSchema>(stdout) {
        return Ok(schema);
    }
    let text = String::from_utf8_lossy(stdout);
    text.lines()
        .rev()
        .find_map(|line| serde_json::from_str::<ProjectSchema>(line.trim()).ok())
        .ok_or_else(|| "extractor produced no parseable schema on stdout".to_string())
}

/// The on-disk schema file for a project, under its `.jackdaw/` dir.
/// This file is the decoupling point between building and pickup:
/// whoever builds the project (the editor's build, or a `jd build` from
/// the terminal) writes it, and the editor watches it to refresh its
/// known component types.
pub fn schema_path(jackdaw_dir: &Path) -> PathBuf {
    jackdaw_dir.join("schema.json")
}

/// Write a freshly extracted schema to `<jackdaw_dir>/schema.json`.
/// Written atomically via a temp file + rename so a watcher never reads
/// a half-written file.
pub fn write_schema(jackdaw_dir: &Path, schema: &ProjectSchema) -> std::io::Result<()> {
    std::fs::create_dir_all(jackdaw_dir)?;
    let path = schema_path(jackdaw_dir);
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(schema)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

/// Read the persisted schema for a project, or `None` when it is absent
/// or unparseable (a stale or partial file is treated as "no schema
/// yet" rather than an error).
pub fn read_schema(jackdaw_dir: &Path) -> Option<ProjectSchema> {
    let path = schema_path(jackdaw_dir);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Everything the editor learns about one project build.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectSchema {
    /// Reflected `Component` types the picker can offer and the
    /// inspector can edit.
    pub components: Vec<TypeSchema>,
    /// Reflected `Resource` types (scene-level data).
    pub resources: Vec<TypeSchema>,
    /// Reflected `Event` types an action binding can fire. Empty in a schema
    /// that does not carry them.
    #[serde(default)]
    pub events: Vec<TypeSchema>,
    /// Functions the game registered for bindings to call.
    #[serde(default)]
    pub functions: Vec<FunctionSchema>,
    /// The types the game registers as reflected assets, plus every project
    /// type their fields reach. Empty in a schema that does not carry them.
    #[serde(default)]
    pub assets: Vec<TypeSchema>,
}

/// The shape and editor metadata of one reflected type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeSchema {
    /// Fully-qualified reflect type path, e.g. `my_game::SpinningCube`.
    pub type_path: String,
    /// Last path segment, for display.
    pub short_name: String,
    /// Module path, for grouping.
    pub module_path: String,
    /// `@EditorCategory`, or empty.
    pub category: String,
    /// Reflected rustdoc, or empty.
    pub description: String,
    /// `@EditorDescription`, or empty.
    #[serde(default)]
    pub editor_description: String,
    /// `@EditorHidden`: skip in the picker.
    pub hidden: bool,
    /// Whether the game registers this type as a reflected asset.
    #[serde(default)]
    pub asset: bool,
    /// `@EditorPreview` glTF path under `assets/`, or empty.
    #[serde(default)]
    pub preview: String,
    /// Whether a default value could be constructed (picker requires it).
    pub default_constructible: bool,
    /// The type's fields (empty for unit/opaque/enum kinds).
    pub fields: Vec<FieldSchema>,
    /// The type's kind, so the inspector chooses a layout.
    pub kind: TypeKind,
    /// A default value, serialized via `ReflectSerializer` as JSON, for
    /// "add component". `None` when not default-constructible.
    pub default: Option<serde_json::Value>,
    /// One entry per variant for enums; empty for every other kind.
    #[serde(default)]
    pub variants: Vec<VariantSchema>,
    /// Every field the type declares as an `Entity`, by name. None can be
    /// mapped by a binding; the dispatcher fills one named `entity` with the
    /// widget's subject and refuses the rest.
    #[serde(default)]
    pub entity_fields: Vec<String>,
    /// Whether reflection can build a value with fields left unset
    /// (`Default` or `FromWorld`). An action binding on a type without
    /// it has to map every declared field.
    #[serde(default)]
    pub fills_gaps: bool,
}

/// One variant of a reflected enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantSchema {
    /// Variant name, e.g. `Walk`.
    pub name: String,
    /// The variant's fields; for tuple variants the name is the index
    /// as a string, matching how tuple structs are reported.
    pub fields: Vec<FieldSchema>,
}

/// One function the game registered for bindings to call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSchema {
    /// Registered name, usually the full path (`my_game::ui::ratio`).
    pub name: String,
    /// Argument type paths, in order, from the function's first
    /// signature. Overloads beyond the first are not reported.
    pub arg_type_paths: Vec<String>,
    /// How each argument is taken, positionally matching `arg_type_paths`.
    /// Empty in a schema that does not report ownership.
    #[serde(default)]
    pub arg_ownerships: Vec<ArgOwnership>,
    /// The return type's path.
    pub return_type_path: String,
    /// How the return value is handed back.
    #[serde(default)]
    pub return_ownership: ArgOwnership,
    /// Doc comment, when the registry carries one.
    #[serde(default)]
    pub docs: Option<String>,
}

impl FunctionSchema {
    /// Whether a binding can call this function: the evaluator passes owned
    /// arguments and accepts only an owned return. A schema carrying no
    /// ownership answers `true`, leaving a bad pick to fail at call time.
    pub fn callable_by_value(&self) -> bool {
        self.return_ownership == ArgOwnership::Owned
            && self
                .arg_ownerships
                .iter()
                .all(|ownership| *ownership == ArgOwnership::Owned)
    }
}

/// How a function takes an argument or hands back its result. Mirrors
/// bevy's reflected `Ownership`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArgOwnership {
    /// `&T`.
    Ref,
    /// `&mut T`.
    Mut,
    /// `T`. The only kind a binding can supply or consume.
    #[default]
    Owned,
}

/// One field of a struct or tuple-struct type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSchema {
    /// Field name; for tuple structs this is the index as a string.
    pub name: String,
    /// The field's reflect type path.
    pub type_path: String,
    /// For a list or array field, the element type's reflect type path;
    /// empty for every other field.
    #[serde(default)]
    pub item_type_path: String,
    /// `@AssetRef`: the reflect type path of the asset this field names by
    /// path, or whose elements it names. Empty for a field without one.
    #[serde(default)]
    pub asset_type_path: String,
}

/// The reflect kind of a schema'd type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeKind {
    Struct,
    TupleStruct,
    Enum,
    /// Unit struct, opaque, or anything else the inspector renders as a
    /// marker with no fields.
    Marker,
}

#[cfg(feature = "reflect")]
mod extract {
    use super::*;
    use std::collections::{HashSet, VecDeque};

    use bevy::asset::ReflectAsset;
    use bevy::ecs::entity::Entity;
    use bevy::ecs::reflect::{ReflectComponent, ReflectEvent, ReflectFromWorld, ReflectResource};
    use bevy::reflect::enums::{EnumInfo, VariantInfo};
    use bevy::reflect::func::FunctionRegistry;
    use bevy::reflect::func::args::Ownership;
    use bevy::reflect::serde::ReflectSerializer;
    use bevy::reflect::{NamedField, TypeInfo, TypeRegistration, TypeRegistry, UnnamedField};
    use jackdaw_scene_types::{
        AssetRef, EditorCategory, EditorDescription, EditorHidden, EditorPreview,
    };

    /// Build the schema for this process's reflected types.
    ///
    /// Reads the link-time auto-registration inventory rather than a
    /// running `App`, so it does not matter whether (or in what order)
    /// the game's plugins have been added. That is what lets a game
    /// answer the schema flag before it builds anything.
    pub fn extract_derived_schema() -> ProjectSchema {
        let mut registry = TypeRegistry::default();
        registry.register_derived_types();
        extract_from_registry(&registry)
    }

    /// Builds the schema for every reflected `Component`, `Resource` and
    /// `Event` in `registry`.
    ///
    /// A type can land in more than one bucket, so no picker loses it: every
    /// resource lands in two, since `ReflectResource` registers
    /// `ReflectComponent` beside itself.
    pub fn extract_from_registry(registry: &TypeRegistry) -> ProjectSchema {
        let mut schema = ProjectSchema::default();
        for registration in registry.iter() {
            let is_component = registration.data::<ReflectComponent>().is_some();
            let is_resource = registration.data::<ReflectResource>().is_some();
            let is_event = registration.data::<ReflectEvent>().is_some();
            if !is_component && !is_resource && !is_event {
                continue;
            }
            let type_schema = type_schema_for(registration, registry);
            if is_event {
                schema.events.push(type_schema.clone());
            }
            if is_resource {
                schema.resources.push(type_schema.clone());
            }
            if is_component {
                schema.components.push(type_schema);
            }
        }
        schema
    }

    /// Describes every function registered for bindings to call. Only the first
    /// signature of an overloaded function is reported.
    pub fn extract_functions(registry: &FunctionRegistry) -> Vec<FunctionSchema> {
        registry
            .iter()
            .filter_map(|function| {
                let info = function.info();
                let name = info.name()?.to_string();
                let signature = info.signatures().first()?;
                Some(FunctionSchema {
                    name,
                    arg_type_paths: signature
                        .args()
                        .iter()
                        .map(|arg| arg.type_path().to_string())
                        .collect(),
                    arg_ownerships: signature
                        .args()
                        .iter()
                        .map(|arg| ownership_of(arg.ownership()))
                        .collect(),
                    return_type_path: signature.return_info().type_path().to_string(),
                    return_ownership: ownership_of(signature.return_info().ownership()),
                    // The function registry carries no doc comments.
                    docs: None,
                })
            })
            .collect()
    }

    fn ownership_of(ownership: Ownership) -> ArgOwnership {
        match ownership {
            Ownership::Ref => ArgOwnership::Ref,
            Ownership::Mut => ArgOwnership::Mut,
            Ownership::Owned => ArgOwnership::Owned,
        }
    }

    fn type_schema_for(registration: &TypeRegistration, registry: &TypeRegistry) -> TypeSchema {
        let info = registration.type_info();
        let table = info.type_path_table();
        let attrs = custom_attributes(info);

        let category = attrs
            .and_then(|a| a.get::<EditorCategory>())
            .map(|c| c.0.to_string())
            .unwrap_or_default();
        let description = info
            .docs()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        let editor_description = attrs
            .and_then(|a| a.get::<EditorDescription>())
            .map(|d| d.0.to_string())
            .unwrap_or_default();
        let hidden = attrs.is_some_and(|a| a.get::<EditorHidden>().is_some());
        let preview = attrs
            .and_then(|a| a.get::<EditorPreview>())
            .map(|p| p.0.to_string())
            .unwrap_or_default();

        let (kind, fields, variants) = shape_of(info, registry);
        let entity_fields = info
            .as_struct()
            .map(|s| {
                s.iter()
                    .filter(|f| NamedField::is::<Entity>(f))
                    .map(|f| f.name().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let fills_gaps = registration
            .data::<bevy::reflect::prelude::ReflectDefault>()
            .is_some()
            || registration.data::<ReflectFromWorld>().is_some();

        // A default value drives "add component" on the editor side.
        // Serialize it with the same registry so nested project types
        // resolve; `None` when the type is not default-constructible.
        let default = registration
            .data::<bevy::reflect::prelude::ReflectDefault>()
            .map(bevy::reflect::prelude::ReflectDefault::default)
            .and_then(|value| {
                serde_json::to_value(ReflectSerializer::new(value.as_partial_reflect(), registry))
                    .ok()
            });

        TypeSchema {
            type_path: table.path().to_string(),
            short_name: table.short_path().to_string(),
            module_path: table.module_path().unwrap_or("").to_string(),
            category,
            description,
            editor_description,
            hidden,
            asset: false,
            preview,
            default_constructible: default.is_some(),
            fields,
            kind,
            default,
            variants,
            entity_fields,
            fills_gaps,
        }
    }

    /// A type's kind, its own fields, and (for enums) its variants.
    fn shape_of(
        info: &TypeInfo,
        registry: &TypeRegistry,
    ) -> (TypeKind, Vec<FieldSchema>, Vec<VariantSchema>) {
        match info {
            TypeInfo::Struct(s) => (
                TypeKind::Struct,
                s.iter()
                    .map(|field| named_field_schema(field, registry))
                    .collect(),
                Vec::new(),
            ),
            TypeInfo::TupleStruct(s) => (
                TypeKind::TupleStruct,
                s.iter()
                    .enumerate()
                    .map(|(i, field)| unnamed_field_schema(i, field, registry))
                    .collect(),
                Vec::new(),
            ),
            TypeInfo::Enum(e) => (
                TypeKind::Enum,
                Vec::new(),
                e.iter().map(|v| variant_of(v, registry)).collect(),
            ),
            _ => (TypeKind::Marker, Vec::new(), Vec::new()),
        }
    }

    fn variant_of(variant: &VariantInfo, registry: &TypeRegistry) -> VariantSchema {
        let fields = match variant {
            VariantInfo::Struct(s) => s
                .iter()
                .map(|field| named_field_schema(field, registry))
                .collect(),
            VariantInfo::Tuple(t) => t
                .iter()
                .enumerate()
                .map(|(i, field)| unnamed_field_schema(i, field, registry))
                .collect(),
            VariantInfo::Unit(_) => Vec::new(),
        };
        VariantSchema {
            name: variant.name().to_string(),
            fields,
        }
    }

    fn named_field_schema(field: &NamedField, registry: &TypeRegistry) -> FieldSchema {
        FieldSchema {
            name: field.name().to_string(),
            type_path: field.type_path().to_string(),
            item_type_path: item_type_path(field.type_info(), field.type_id(), registry),
            asset_type_path: asset_ref_of(field.custom_attributes()),
        }
    }

    fn unnamed_field_schema(
        index: usize,
        field: &UnnamedField,
        registry: &TypeRegistry,
    ) -> FieldSchema {
        FieldSchema {
            name: index.to_string(),
            type_path: field.type_path().to_string(),
            item_type_path: item_type_path(field.type_info(), field.type_id(), registry),
            asset_type_path: asset_ref_of(field.custom_attributes()),
        }
    }

    /// The asset type a field's `@AssetRef` names, empty without one.
    fn asset_ref_of(attributes: &bevy::reflect::attributes::CustomAttributes) -> String {
        attributes
            .get::<AssetRef>()
            .map(|reference| reference.0.to_string())
            .unwrap_or_default()
    }

    /// The element type of a list or array field, empty for anything else.
    /// An `Option` is an enum here, not a list, and reports nothing.
    fn item_type_path(
        declared: Option<&'static TypeInfo>,
        type_id: std::any::TypeId,
        registry: &TypeRegistry,
    ) -> String {
        match registry.get_type_info(type_id).or(declared) {
            Some(TypeInfo::List(list)) => list.item_ty().path().to_string(),
            Some(TypeInfo::Array(array)) => array.item_ty().path().to_string(),
            _ => String::new(),
        }
    }

    /// Describes every type the game registers as a reflected asset, plus the
    /// project types their fields reach.
    ///
    /// "Project" means the crate segment of a discovered root: a field whose
    /// type comes from one of those crates is described too, recursively,
    /// while an engine type is named but not walked into.
    pub fn extract_asset_types(registry: &TypeRegistry) -> Vec<TypeSchema> {
        let resolved: Vec<&str> = registry
            .iter()
            .filter(|registration| registration.data::<ReflectAsset>().is_some())
            .map(|registration| registration.type_info().type_path())
            .filter(|path| is_project_crate(crate_segment(path)))
            .collect();
        let crates: HashSet<&str> = resolved.iter().map(|path| crate_segment(path)).collect();
        let root_paths: HashSet<&str> = resolved.iter().copied().collect();

        let mut queue: VecDeque<String> = resolved.iter().map(|p| (*p).to_string()).collect();
        let mut visited: HashSet<String> = HashSet::new();
        let mut described: Vec<TypeSchema> = Vec::new();
        while let Some(path) = queue.pop_front() {
            if !visited.insert(path.clone()) {
                continue;
            }
            let Some(registration) = registry.get_with_type_path(&path) else {
                continue;
            };
            let info = registration.type_info();
            let describable = matches!(
                info,
                TypeInfo::Struct(_) | TypeInfo::TupleStruct(_) | TypeInfo::Enum(_)
            );
            let belongs =
                root_paths.contains(path.as_str()) || crates.contains(crate_segment(path.as_str()));
            if !describable || !belongs {
                continue;
            }
            let mut described_type = type_schema_for(registration, registry);
            described_type.asset = root_paths.contains(path.as_str());
            described.push(described_type);
            for reached in types_reached_by(info) {
                queue.push_back(reached);
            }
        }
        described.sort_by(|a, b| a.type_path.cmp(&b.type_path));
        described
    }

    /// The part of a reflect type path before its first `::`.
    fn crate_segment(type_path: &str) -> &str {
        type_path.split("::").next().unwrap_or(type_path)
    }

    /// Whether a crate segment names the game's own code rather than the
    /// engine, the editor, or the standard library. Only crate names the
    /// engine itself ships are excluded, so a game crate that merely reads
    /// like one keeps its types.
    fn is_project_crate(segment: &str) -> bool {
        !matches!(segment, "bevy" | "jackdaw" | "std" | "core" | "alloc")
            && !segment.starts_with("bevy_")
            && !segment.starts_with("jackdaw_")
    }

    /// Type paths a type's own fields name, with containers flattened to
    /// what they hold.
    fn types_reached_by(info: &TypeInfo) -> Vec<String> {
        let mut reached = Vec::new();
        match info {
            TypeInfo::Struct(s) => {
                for field in s.iter() {
                    reach_into(field.type_info(), field.type_path(), &mut reached);
                }
            }
            TypeInfo::TupleStruct(s) => {
                for field in s.iter() {
                    reach_into(field.type_info(), field.type_path(), &mut reached);
                }
            }
            TypeInfo::Enum(e) => {
                for variant in e.iter() {
                    reach_into_variant(variant, &mut reached);
                }
            }
            _ => {}
        }
        reached
    }

    fn reach_into_variant(variant: &VariantInfo, reached: &mut Vec<String>) {
        match variant {
            VariantInfo::Struct(s) => {
                for field in s.iter() {
                    reach_into(field.type_info(), field.type_path(), reached);
                }
            }
            VariantInfo::Tuple(t) => {
                for field in t.iter() {
                    reach_into(field.type_info(), field.type_path(), reached);
                }
            }
            VariantInfo::Unit(_) => {}
        }
    }

    /// Records the type at `path`, stepping through a list, array, set, map,
    /// tuple or `Option` to what it holds.
    fn reach_into(info: Option<&'static TypeInfo>, path: &str, reached: &mut Vec<String>) {
        let Some(info) = info else {
            reached.push(path.to_string());
            return;
        };
        match info {
            TypeInfo::List(list) => {
                reach_into(list.item_info(), list.item_ty().path(), reached);
            }
            TypeInfo::Array(array) => {
                reach_into(array.item_info(), array.item_ty().path(), reached);
            }
            TypeInfo::Set(set) => {
                reach_into(None, set.value_ty().path(), reached);
            }
            TypeInfo::Map(map) => {
                reach_into(map.key_info(), map.key_ty().path(), reached);
                reach_into(map.value_info(), map.value_ty().path(), reached);
            }
            TypeInfo::Tuple(tuple) => {
                for field in tuple.iter() {
                    reach_into(field.type_info(), field.type_path(), reached);
                }
            }
            TypeInfo::Enum(e) if is_option(e) => {
                for variant in e.iter() {
                    reach_into_variant(variant, reached);
                }
            }
            _ => reached.push(info.type_path().to_string()),
        }
    }

    fn is_option(info: &EnumInfo) -> bool {
        info.type_path_table().crate_name() == Some("core")
            && info.type_path_table().ident() == Some("Option")
    }

    fn custom_attributes(info: &TypeInfo) -> Option<&bevy::reflect::attributes::CustomAttributes> {
        match info {
            TypeInfo::Struct(s) => Some(s.custom_attributes()),
            TypeInfo::TupleStruct(s) => Some(s.custom_attributes()),
            TypeInfo::Enum(e) => Some(e.custom_attributes()),
            _ => None,
        }
    }
}

#[cfg(feature = "reflect")]
pub use extract::{
    extract_asset_types, extract_derived_schema, extract_from_registry, extract_functions,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_stdout_parses() {
        let json = br#"{"components":[],"resources":[]}"#;
        assert!(parse_from_stdout(json).is_ok());
    }

    #[test]
    fn a_leading_log_line_does_not_defeat_parsing() {
        let noisy = b"starting up\n{\"components\":[],\"resources\":[]}\n";
        assert!(parse_from_stdout(noisy).is_ok());
    }

    #[test]
    fn output_without_a_schema_is_an_error() {
        assert!(parse_from_stdout(b"no schema here\n").is_err());
    }

    /// A dump carrying none of the optional keys still loads.
    #[test]
    fn a_schema_without_the_new_keys_still_parses() {
        let old = br#"{
            "components": [{
                "type_path": "my_game::Spin",
                "short_name": "Spin",
                "module_path": "my_game",
                "category": "",
                "description": "",
                "hidden": false,
                "default_constructible": false,
                "fields": [],
                "kind": "Marker",
                "default": null
            }],
            "resources": []
        }"#;
        let schema = parse_from_stdout(old).expect("old schema parses");
        assert_eq!(schema.components.len(), 1);
        assert!(schema.events.is_empty());
        assert!(schema.functions.is_empty());
        assert!(schema.assets.is_empty());
        assert!(!schema.components[0].asset);
        assert!(schema.components[0].variants.is_empty());
        assert!(schema.components[0].entity_fields.is_empty());
        assert!(!schema.components[0].fills_gaps);
    }

    fn function(name: &str) -> FunctionSchema {
        FunctionSchema {
            name: name.to_string(),
            arg_type_paths: vec!["f32".to_string()],
            arg_ownerships: vec![ArgOwnership::Owned],
            return_type_path: "f32".to_string(),
            return_ownership: ArgOwnership::Owned,
            docs: None,
        }
    }

    #[test]
    fn functions_survive_a_json_round_trip() {
        let schema = ProjectSchema {
            functions: vec![function("my_game::double")],
            ..ProjectSchema::default()
        };
        let json = serde_json::to_vec(&schema).expect("serialize");
        let back = parse_from_stdout(&json).expect("parse");
        assert_eq!(back.functions.len(), 1);
        assert_eq!(back.functions[0].name, "my_game::double");
        assert_eq!(back.functions[0].arg_type_paths, ["f32"]);
        assert_eq!(back.functions[0].arg_ownerships, [ArgOwnership::Owned]);
        assert_eq!(back.functions[0].return_type_path, "f32");
    }

    /// The evaluator passes owned arguments and accepts only an owned return,
    /// which is what the picker filters registered functions on.
    #[test]
    fn a_function_that_borrows_is_not_callable_by_value() {
        assert!(function("ok").callable_by_value());

        let mut borrows_an_arg = function("borrows");
        borrows_an_arg.arg_ownerships = vec![ArgOwnership::Ref];
        assert!(!borrows_an_arg.callable_by_value());

        let mut returns_a_reference = function("lends");
        returns_a_reference.return_ownership = ArgOwnership::Mut;
        assert!(!returns_a_reference.callable_by_value());

        // A schema carrying no ownership is offered and fails at call time.
        let mut older = function("unknown");
        older.arg_ownerships = Vec::new();
        assert!(older.callable_by_value());
    }

    /// The panic-hook fallback prints the inventory-only schema and the runner
    /// prints the full one later, so the last parseable line wins.
    #[test]
    fn the_last_schema_line_on_stdout_wins() {
        let fallback = ProjectSchema::default();
        let full = ProjectSchema {
            functions: vec![function("my_game::double")],
            ..ProjectSchema::default()
        };
        let mut stdout = serde_json::to_vec(&fallback).expect("serialize");
        stdout.push(b'\n');
        stdout.extend(serde_json::to_vec(&full).expect("serialize"));
        stdout.push(b'\n');

        let parsed = parse_from_stdout(&stdout).expect("parse");
        assert_eq!(parsed.functions.len(), 1, "the later dump must win");
    }
}

#[cfg(all(test, feature = "reflect"))]
mod extract_tests {
    use super::*;
    use bevy::asset::{Asset, ReflectAsset};
    use bevy::prelude::*;
    use bevy::reflect::{FromReflect, GetTypeRegistration, TypePath, TypeRegistry};

    /// An event whose fields a binding can fill, and which reflection can
    /// build when a binding leaves one unmapped.
    #[derive(Event, Reflect)]
    #[reflect(Event, Default)]
    struct Fired {
        entity: Entity,
        amount: f32,
    }

    impl Default for Fired {
        fn default() -> Self {
            Self {
                entity: Entity::PLACEHOLDER,
                amount: 0.0,
            }
        }
    }

    /// An event with neither `Default` nor `FromWorld`: every declared field
    /// has to be mapped or the dispatch fails.
    #[derive(Event, Reflect)]
    #[reflect(Event)]
    struct Bare {
        label: String,
    }

    /// `ReflectResource` registers `ReflectComponent` with it, so this lands in
    /// both buckets.
    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource, Default)]
    struct Score {
        points: u32,
    }

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    enum Mode {
        #[default]
        Idle,
        Walk(f32),
        Run {
            speed: f32,
        },
    }

    fn schema_of<T: GetTypeRegistration>() -> ProjectSchema {
        let mut registry = TypeRegistry::default();
        registry.register::<T>();
        extract_from_registry(&registry)
    }

    fn find<'a>(types: &'a [TypeSchema], short_name: &str) -> &'a TypeSchema {
        types
            .iter()
            .find(|t| t.short_name == short_name)
            .unwrap_or_else(|| panic!("no {short_name} in schema"))
    }

    #[test]
    fn an_event_reports_its_fields_and_dispatch_traits() {
        let schema = schema_of::<Fired>();
        assert!(schema.components.is_empty());
        let event = find(&schema.events, "Fired");
        assert_eq!(event.kind, TypeKind::Struct);
        let fields: Vec<(&str, &str)> = event
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.type_path.as_str()))
            .collect();
        assert!(fields.contains(&("amount", "f32")), "got {fields:?}");
        assert!(
            fields.iter().any(|(name, _)| *name == "entity"),
            "got {fields:?}"
        );
        assert_eq!(event.entity_fields, vec!["entity".to_string()]);
        assert!(event.fills_gaps);
    }

    #[test]
    fn an_event_that_cannot_fill_gaps_says_so() {
        let schema = schema_of::<Bare>();
        let event = find(&schema.events, "Bare");
        assert!(event.entity_fields.is_empty());
        assert!(!event.fills_gaps);
    }

    #[test]
    fn an_enum_reports_its_variants_and_their_fields() {
        let schema = schema_of::<Mode>();
        let mode = find(&schema.components, "Mode");
        assert_eq!(mode.kind, TypeKind::Enum);
        let names: Vec<&str> = mode.variants.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["Idle", "Walk", "Run"]);
        assert!(mode.variants[0].fields.is_empty());
        assert_eq!(mode.variants[1].fields[0].name, "0");
        assert_eq!(mode.variants[1].fields[0].type_path, "f32");
        assert_eq!(mode.variants[2].fields[0].name, "speed");
        assert_eq!(mode.variants[2].fields[0].type_path, "f32");
    }

    #[test]
    fn a_resource_reaches_the_resources_bucket() {
        let schema = schema_of::<Score>();
        let resource = find(&schema.resources, "Score");
        assert_eq!(resource.fields[0].name, "points");
    }

    /// The picker reads the resources bucket and the inspector the components
    /// one, so a resource belongs in both.
    #[test]
    fn a_resource_is_reported_as_a_component_too() {
        let schema = schema_of::<Score>();
        find(&schema.components, "Score");
    }

    #[test]
    fn a_plain_component_is_not_reported_as_a_resource() {
        let schema = schema_of::<Mode>();
        find(&schema.components, "Mode");
        assert!(schema.resources.is_empty(), "{:?}", schema.resources);
    }

    #[test]
    fn registered_functions_report_their_signature() {
        fn double(value: f32) -> f32 {
            value * 2.0
        }
        let mut registry = bevy::reflect::func::FunctionRegistry::default();
        registry
            .register_with_name("my_game::double", double)
            .expect("register");
        let functions = extract_functions(&registry);
        let found = functions
            .iter()
            .find(|f| f.name == "my_game::double")
            .expect("double is registered");
        assert_eq!(found.arg_type_paths, ["f32"]);
        assert_eq!(found.arg_ownerships, [ArgOwnership::Owned]);
        assert_eq!(found.return_type_path, "f32");
        assert!(found.callable_by_value());
    }

    /// A function taking `&T` is registrable but not bindable, and the dump
    /// reports the ownership the picker filters on.
    #[test]
    fn a_borrowing_function_is_reported_as_borrowing() {
        fn doubled(value: &f32) -> f32 {
            value * 2.0
        }
        let mut registry = bevy::reflect::func::FunctionRegistry::default();
        registry
            .register_with_name("my_game::doubled", doubled)
            .expect("register");
        let functions = extract_functions(&registry);
        let found = functions
            .iter()
            .find(|f| f.name == "my_game::doubled")
            .expect("doubled is registered");
        assert_eq!(found.arg_ownerships, [ArgOwnership::Ref]);
        assert!(!found.callable_by_value());
    }

    /// A nested type an asset's list field holds.
    #[derive(Reflect, Default)]
    #[type_path = "my_game::content"]
    #[reflect(Default)]
    struct Effect {
        magnitude: f32,
    }

    #[derive(Reflect, Default)]
    #[type_path = "my_game::content"]
    #[reflect(Default)]
    enum Rarity {
        #[default]
        Common,
        Rare,
    }

    /// An asset the game registers: a scalar, an enum, a list of a nested
    /// type, and an engine type the closure must not walk into.
    #[derive(Asset, Reflect, Default)]
    #[type_path = "my_game::content"]
    #[reflect(Default)]
    struct ItemDef {
        display_name: String,
        rarity: Rarity,
        effects: Vec<Effect>,
        tag: Name,
    }

    /// An asset the engine registers, which the game did not write.
    #[derive(Asset, Reflect, Default)]
    #[type_path = "bevy_image::image"]
    #[reflect(Default)]
    struct Image {
        width: u32,
    }

    fn register_asset<T: GetTypeRegistration + FromReflect + Asset>(registry: &mut TypeRegistry) {
        registry.register::<T>();
        registry.register_type_data::<T, ReflectAsset>();
    }

    fn assets_of<T: GetTypeRegistration + FromReflect + Asset>() -> Vec<TypeSchema> {
        let mut registry = TypeRegistry::default();
        register_asset::<T>(&mut registry);
        extract_asset_types(&registry)
    }

    #[test]
    fn a_registered_asset_type_reports_its_fields() {
        let assets = assets_of::<ItemDef>();
        let item = find(&assets, "ItemDef");
        assert_eq!(item.kind, TypeKind::Struct);
        assert!(item.asset);

        let field = |name: &str| {
            item.fields
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("no {name} field"))
        };
        assert_eq!(field("display_name").type_path, "alloc::string::String");
        assert!(field("display_name").item_type_path.is_empty());
        assert_eq!(field("rarity").type_path, Rarity::type_path());
        assert!(field("rarity").item_type_path.is_empty());
        assert_eq!(field("effects").item_type_path, Effect::type_path());
    }

    #[test]
    fn the_asset_closure_reaches_project_types_but_not_engine_types() {
        let assets = assets_of::<ItemDef>();
        let paths: Vec<&str> = assets.iter().map(|d| d.type_path.as_str()).collect();
        assert!(paths.contains(&Effect::type_path()), "got {paths:?}");
        assert!(paths.contains(&Rarity::type_path()), "got {paths:?}");
        assert!(!paths.contains(&Name::type_path()), "got {paths:?}");
        assert!(paths.windows(2).all(|pair| pair[0] < pair[1]), "{paths:?}");
    }

    #[test]
    fn a_game_s_asset_types_are_reported_and_the_engine_s_are_not() {
        let mut registry = TypeRegistry::default();
        register_asset::<ItemDef>(&mut registry);
        register_asset::<Image>(&mut registry);
        let assets = extract_asset_types(&registry);

        let paths: Vec<&str> = assets.iter().map(|d| d.type_path.as_str()).collect();
        assert!(paths.contains(&ItemDef::type_path()), "got {paths:?}");
        assert!(!paths.contains(&Image::type_path()), "got {paths:?}");
    }

    /// An asset in a game crate whose name only reads like the engine's.
    #[derive(Asset, Reflect, Default)]
    #[type_path = "bevygone::content"]
    #[reflect(Default)]
    struct SpellDef {
        cost: u32,
    }

    #[test]
    fn a_game_crate_whose_name_reads_like_the_engine_s_keeps_its_asset_types() {
        let assets = assets_of::<SpellDef>();
        let paths: Vec<&str> = assets.iter().map(|d| d.type_path.as_str()).collect();
        assert!(paths.contains(&SpellDef::type_path()), "got {paths:?}");
    }

    #[test]
    fn a_type_reached_only_through_a_field_carries_no_asset_mark() {
        let assets = assets_of::<ItemDef>();
        assert!(find(&assets, "ItemDef").asset);
        assert!(!find(&assets, "Effect").asset);
        assert!(!find(&assets, "Rarity").asset);
    }

    #[test]
    fn a_registry_with_no_assets_reports_none() {
        let mut registry = TypeRegistry::default();
        registry.register::<ItemDef>();
        assert!(extract_asset_types(&registry).is_empty());
    }

    /// A type holding more of itself, which the closure must walk once.
    #[derive(Asset, Reflect, Default)]
    #[type_path = "my_game::content"]
    #[reflect(Default)]
    struct SkillNode {
        cost: u32,
        unlocks: Vec<SkillNode>,
    }

    #[test]
    fn a_type_holding_more_of_itself_is_described_once() {
        let assets = assets_of::<SkillNode>();
        let described: Vec<&str> = assets
            .iter()
            .map(|schema| schema.type_path.as_str())
            .filter(|path| *path == SkillNode::type_path())
            .collect();
        assert_eq!(described.len(), 1, "got {assets:?}");
    }

    /// A type in the same crate as an asset root, which nothing the root
    /// holds names.
    #[derive(Reflect, Default)]
    #[type_path = "my_game::content"]
    #[reflect(Default)]
    struct Unreachable {
        idle: bool,
    }

    /// An objective that is one of several shapes, which is what a quest's
    /// steps look like.
    #[derive(Reflect, Default)]
    #[type_path = "my_game::content"]
    #[reflect(Default)]
    enum Objective {
        #[default]
        Explore,
        Kill {
            mob: String,
            count: u32,
        },
        Reach(String),
    }

    /// A quest naming the item it pays out, as the path of that item's file.
    #[derive(Asset, Reflect, Default)]
    #[type_path = "my_game::content"]
    #[reflect(Default)]
    struct QuestDef {
        #[reflect(@jackdaw_scene_types::AssetRef("my_game::content::ItemDef"))]
        reward: String,
        #[reflect(@jackdaw_scene_types::AssetRef("my_game::content::ItemDef"))]
        extras: Vec<String>,
        objectives: Vec<Objective>,
    }

    #[test]
    fn a_field_marked_as_a_reference_reports_the_type_it_names() {
        let assets = assets_of::<QuestDef>();
        let quest = find(&assets, "QuestDef");
        let field = |name: &str| {
            quest
                .fields
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("no {name} field"))
        };

        assert_eq!(field("reward").asset_type_path, "my_game::content::ItemDef");
        assert_eq!(
            field("extras").asset_type_path,
            "my_game::content::ItemDef",
            "a list carries the kind its elements name",
        );
        assert!(
            field("objectives").asset_type_path.is_empty(),
            "a field without the attribute names no kind",
        );
    }

    #[test]
    fn a_variant_carrying_fields_reports_them() {
        let assets = assets_of::<QuestDef>();
        let objective = find(&assets, "Objective");
        assert_eq!(objective.kind, TypeKind::Enum);
        let variant = |name: &str| {
            objective
                .variants
                .iter()
                .find(|v| v.name == name)
                .unwrap_or_else(|| panic!("no {name} variant"))
        };

        assert!(variant("Explore").fields.is_empty());
        let kill: Vec<&str> = variant("Kill")
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        assert_eq!(kill, ["mob", "count"]);
        let reach: Vec<&str> = variant("Reach")
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        assert_eq!(reach, ["0"], "a tuple variant names its field by index");
    }

    #[test]
    fn a_type_the_roots_never_reach_is_left_out() {
        let mut registry = TypeRegistry::default();
        register_asset::<ItemDef>(&mut registry);
        registry.register::<Unreachable>();
        let assets = extract_asset_types(&registry);

        let paths: Vec<&str> = assets
            .iter()
            .map(|schema| schema.type_path.as_str())
            .collect();
        assert!(paths.contains(&ItemDef::type_path()), "got {paths:?}");
        assert!(
            !paths.contains(&Unreachable::type_path()),
            "sharing a crate with an asset root is not enough, got {paths:?}"
        );
    }
}
