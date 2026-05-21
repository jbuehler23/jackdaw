use bevy_ecs::prelude::*;
use bevy_log::prelude::*;
use bevy_reflect::TypeRegistry;
use bevy_remote::BrpResult;
use serde_json::Value;
use std::path::Path;

use crate::{JackdawAppInfo, schema::*};

/// Handler for the `jackdaw/app_info` BRP method.
/// Returns basic app metadata for the editor's connection status display.
pub fn jackdaw_app_info_handler(
    In(_params): In<Option<Value>>,
    app_info: Res<JackdawAppInfo>,
) -> BrpResult {
    Ok(serde_json::to_value(&*app_info).unwrap())
}

/// Prefixes for types that should be skipped during component definition generation.
/// These are Bevy internals that aren't useful in the editor.
const SKIP_PREFIXES: &[&str] = &[
    "bevy_render::",
    "bevy_ecs::",
    "bevy_core_pipeline::",
    "bevy_pbr::",
    "bevy_sprite::",
    "bevy_text::",
    "bevy_ui::",
    "bevy_picking::",
    "bevy_gizmos::",
    "bevy_animation::",
    "bevy_audio::",
    "bevy_winit::",
    "bevy_window::",
    "bevy_input::",
    "bevy_a11y::",
    "bevy_gilrs::",
    "bevy_remote::",
];

/// Startup system that auto-generates `.jsn/components.jsn` from the type registry.
///
/// Reads existing definitions from disk, merges new types from the registry,
/// preserves hand-edits, and writes the updated file.
pub fn generate_component_definitions(type_registry: Res<AppTypeRegistry>) {
    let registry = type_registry.read();

    // Determine output directory. Use current working directory.
    let jsn_dir = Path::new(".jsn");
    if std::fs::create_dir_all(jsn_dir).is_err() {
        warn!("JackdawRemote: Could not create .jsn directory");
        return;
    }

    let components_path = jsn_dir.join("components.jsn");

    // Load existing definitions if present
    let mut existing = if components_path.is_file() {
        match std::fs::read_to_string(&components_path) {
            Ok(data) => serde_json::from_str::<JsnComponentsFile>(&data).unwrap_or_else(|e| {
                warn!("JackdawRemote: Failed to parse existing components.jsn: {e}");
                JsnComponentsFile::default()
            }),
            Err(_) => JsnComponentsFile::default(),
        }
    } else {
        JsnComponentsFile::default()
    };

    // Iterate registered types and build definitions for game components
    let mut added = 0usize;
    for registration in registry.iter() {
        let type_info = registration.type_info();
        let type_path = type_info.type_path();

        // Skip Bevy internals
        if SKIP_PREFIXES
            .iter()
            .any(|prefix| type_path.starts_with(prefix))
        {
            continue;
        }

        // Only include types that have ReflectComponent
        if registration.data::<ReflectComponent>().is_none() {
            continue;
        }

        // Skip if already defined (preserve hand-edits)
        if existing.components.contains_key(type_path) {
            // Merge new fields into existing definition
            merge_fields_from_registry(
                &registry,
                type_info,
                existing.components.get_mut(type_path).unwrap(),
            );
            continue;
        }

        // Build a new definition from the type info
        let def = build_component_def(&registry, type_info);
        existing.components.insert(type_path.to_string(), def);
        added += 1;
    }

    // Write updated file
    match serde_json::to_string_pretty(&existing) {
        Ok(data) => {
            if let Err(e) = std::fs::write(&components_path, data) {
                warn!("JackdawRemote: Failed to write components.jsn: {e}");
            } else if added > 0 {
                info!(
                    "JackdawRemote: Generated {added} new component definitions in .jsn/components.jsn"
                );
            }
        }
        Err(e) => {
            warn!("JackdawRemote: Failed to serialize components.jsn: {e}");
        }
    }
}

/// Build a `JsnComponentDef` from a Bevy `TypeInfo`.
fn build_component_def(
    registry: &TypeRegistry,
    type_info: &bevy_reflect::TypeInfo,
) -> JsnComponentDef {
    let mut fields = std::collections::HashMap::new();

    if let bevy_reflect::TypeInfo::Struct(struct_info) = type_info {
        for i in 0..struct_info.field_len() {
            let field = struct_info.field_at(i).unwrap();
            let field_name = field.name().to_string();
            let field_type_path = field.type_info().map_or_else(
                || resolve_type_path(registry, field.type_id()),
                |info| info.type_path().to_string(),
            );
            fields.insert(
                field_name,
                JsnFieldDef {
                    type_path: field_type_path,
                    ..Default::default()
                },
            );
        }
    }

    JsnComponentDef {
        fields,
        ..Default::default()
    }
}

/// Merge newly discovered fields into an existing component definition,
/// preserving hand-edits on existing fields.
fn merge_fields_from_registry(
    registry: &TypeRegistry,
    type_info: &bevy_reflect::TypeInfo,
    existing: &mut JsnComponentDef,
) {
    if let bevy_reflect::TypeInfo::Struct(struct_info) = type_info {
        for i in 0..struct_info.field_len() {
            let field = struct_info.field_at(i).unwrap();
            let field_name = field.name();

            if existing.fields.contains_key(field_name) {
                continue; // Preserve hand-edits
            }

            let field_type_path = field.type_info().map_or_else(
                || resolve_type_path(registry, field.type_id()),
                |info| info.type_path().to_string(),
            );
            existing.fields.insert(
                field_name.to_string(),
                JsnFieldDef {
                    type_path: field_type_path,
                    ..Default::default()
                },
            );
        }
    }
}

/// Try to resolve a type path from the registry by `TypeId`.
fn resolve_type_path(registry: &TypeRegistry, type_id: std::any::TypeId) -> String {
    registry
        .get(type_id)
        .map(|r| r.type_info().type_path().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
