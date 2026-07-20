//! Project type schema: the data the editor needs about a project's
//! reflected types, extracted out-of-process so the editor never maps
//! project code.
//!
//! A loaded dylib can never be unmapped (bevy pins component
//! descriptors; live code cannot be unloaded), so loading project code
//! into the editor leaks on every refresh. Instead a throwaway process
//! (the game runner in `--extract-schema` mode) dlopens the freshly
//! built dylib, drains its reflected types, serializes this schema to
//! stdout, and exits. The editor reads the schema and represents
//! project components as dynamic data backed by the scene document;
//! their real types live only in the game runner at Play time.
//!
//! These types are the wire format shared by the extractor (which
//! writes them) and the editor (which reads them). They are plain
//! serde so no reflection is needed to consume them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The on-disk schema file for a project, under its `.jackdaw/` dir.
/// This file is the decoupling point between building and pickup:
/// whoever builds the project (the editor's in-process build, or a
/// `jackdaw build` from the terminal) writes it, and the editor watches
/// it to refresh its known component types. One artifact, one consumer,
/// regardless of who triggered the build.
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

/// Run the schema extractor (`jackdaw-runner --extract-schema`) on a
/// built project dylib and parse its output. This is how the editor
/// learns a project's types without mapping project code: the
/// subprocess dlopens the dylib and dies, taking the mapping with it.
/// A missing runner or a nonzero exit yields an error the caller can
/// treat as "no schema this build" rather than a hard failure.
pub fn run_extractor(runner: &Path, dylib: &Path) -> Result<ProjectSchema, String> {
    if !runner.is_file() {
        return Err(format!("runner not found at {}", runner.display()));
    }
    let output = std::process::Command::new(runner)
        .arg("--extract-schema")
        .arg(dylib)
        .output()
        .map_err(|e| format!("spawn extractor: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "extractor failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("parse extractor output: {e}"))
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

    /// Build the schema for every reflected `Component` and `Resource`
    /// in `registry`. The caller drains the project dylib's types into
    /// `registry` first (via `register_derived_types`). Everything is
    /// dumped; the editor filters to types it does not already know.
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
pub use extract::extract_from_registry;
