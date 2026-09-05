//! Project type schema: the data the editor needs about a project's
//! reflected types, produced by the project itself and consumed by the
//! editor as plain data.

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
    /// `@EditorCategory`, or a fallback, or empty.
    pub category: String,
    /// `@EditorDescription` or the reflected doc comment.
    pub description: String,
    /// `@EditorHidden`: skip in the picker.
    pub hidden: bool,
    /// Whether a default value could be constructed (picker requires it).
    pub default_constructible: bool,
    /// The type's fields (empty for unit/opaque/enum kinds).
    pub fields: Vec<FieldSchema>,
    /// The type's kind, so the inspector chooses a layout.
    pub kind: TypeKind,
    /// A default value, serialized via `ReflectSerializer` as JSON, for
    /// "add component". `None` when not default-constructible.
    pub default: Option<serde_json::Value>,
}

/// One field of a struct or tuple-struct type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSchema {
    /// Field name; for tuple structs this is the index as a string.
    pub name: String,
    /// The field's reflect type path.
    pub type_path: String,
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
    use bevy::ecs::reflect::{ReflectComponent, ReflectResource};
    use bevy::reflect::serde::ReflectSerializer;
    use bevy::reflect::{TypeInfo, TypeRegistration, TypeRegistry};
    use jackdaw_scene_types::{EditorCategory, EditorDescription, EditorHidden};

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

    /// Build the schema for every reflected `Component` and `Resource`
    /// in `registry`. Everything is dumped; the editor filters to types
    /// it does not already know.
    pub fn extract_from_registry(registry: &TypeRegistry) -> ProjectSchema {
        let mut schema = ProjectSchema::default();
        for registration in registry.iter() {
            let is_component = registration.data::<ReflectComponent>().is_some();
            let is_resource = registration.data::<ReflectResource>().is_some();
            if !is_component && !is_resource {
                continue;
            }
            let type_schema = type_schema_for(registration, registry);
            if is_component {
                schema.components.push(type_schema);
            } else {
                schema.resources.push(type_schema);
            }
        }
        schema
    }

    fn type_schema_for(registration: &TypeRegistration, registry: &TypeRegistry) -> TypeSchema {
        let info = registration.type_info();
        let table = info.type_path_table();
        let attrs = custom_attributes(info);

        let category = attrs
            .and_then(|a| a.get::<EditorCategory>())
            .map(|c| c.0.to_string())
            .unwrap_or_default();
        let description = attrs
            .and_then(|a| a.get::<EditorDescription>())
            .map(|d| d.0.to_string())
            .or_else(|| {
                info.docs()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_default();
        let hidden = attrs.is_some_and(|a| a.get::<EditorHidden>().is_some());

        let (kind, fields) = kind_and_fields(info);

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
            hidden,
            default_constructible: default.is_some(),
            fields,
            kind,
            default,
        }
    }

    fn kind_and_fields(info: &TypeInfo) -> (TypeKind, Vec<FieldSchema>) {
        match info {
            TypeInfo::Struct(s) => (
                TypeKind::Struct,
                s.iter()
                    .map(|field| FieldSchema {
                        name: field.name().to_string(),
                        type_path: field.type_path().to_string(),
                    })
                    .collect(),
            ),
            TypeInfo::TupleStruct(s) => (
                TypeKind::TupleStruct,
                s.iter()
                    .enumerate()
                    .map(|(i, field)| FieldSchema {
                        name: i.to_string(),
                        type_path: field.type_path().to_string(),
                    })
                    .collect(),
            ),
            TypeInfo::Enum(_) => (TypeKind::Enum, Vec::new()),
            _ => (TypeKind::Marker, Vec::new()),
        }
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
pub use extract::{extract_derived_schema, extract_from_registry};

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
}
