//! Right-click context menus for prefab-instance inspector rows.
//!
//! Component-header menu actions:
//! - `inspector.prefab.revert_component` rewinds the entity's component
//!   value to the inherited prefab value.
//! - `inspector.prefab.apply_to_source` pushes each overridden field on
//!   the component into the prefab source file (which then propagates
//!   to every instance pointing at that file).
//! - `inspector.prefab.bulk_apply` pushes each overridden field into
//!   every other prefab instance in the scene that shares the same
//!   source path.
//!
//! Per-field menu actions:
//! - `inspector.prefab.revert_field` rewinds a single field on the
//!   entity back to the inherited prefab value.
//! - `inspector.prefab.apply_field_to_source` pushes the scene-side
//!   value for a single field into the prefab source file.
//!
//! Routing data (entity AST key, prefab source path, etc) lives in
//! [`PrefabMenuTarget`] because the existing
//! [`ContextMenuAction`] event only carries an action string and an
//! optional entity, not the rich context the prefab operators need.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::SceneJsnAst;
use jackdaw_widgets::context_menu::{ContextMenuAction, ContextMenuState};
use std::path::PathBuf;

use crate::prefab::PrefabAstCache;

pub const REVERT_COMPONENT: &str = "inspector.prefab.revert_component";
pub const APPLY_TO_SOURCE: &str = "inspector.prefab.apply_to_source";
pub const BULK_APPLY: &str = "inspector.prefab.bulk_apply";
pub const REVERT_FIELD: &str = "inspector.prefab.revert_field";
pub const APPLY_FIELD_TO_SOURCE: &str = "inspector.prefab.apply_field_to_source";

/// Holds the prefab-instance data the right-click observer captured at
/// menu-open time. Only one inspector context menu can be open at a
/// time, so a single-slot resource is enough.
#[derive(Resource, Default)]
pub(crate) struct PrefabMenuTarget {
    pub(crate) entity_key: Option<usize>,
    pub(crate) instance_root: Option<usize>,
    pub(crate) prefab_entity_id: Option<u32>,
    pub(crate) prefab_path: Option<PathBuf>,
    pub(crate) type_path: Option<String>,
    pub(crate) field_path: Option<String>,
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<PrefabMenuTarget>()
        .add_observer(on_prefab_menu_action);
}

fn on_prefab_menu_action(
    event: On<ContextMenuAction>,
    mut commands: Commands,
    mut state: ResMut<ContextMenuState>,
    target: Res<PrefabMenuTarget>,
) {
    match event.action.as_str() {
        REVERT_COMPONENT => {
            let (Some(entity_key), Some(type_path)) = (target.entity_key, target.type_path.clone())
            else {
                return;
            };
            commands
                .operator("prefab.revert_component")
                .settings(CallOperatorSettings {
                    creates_history_entry: true,
                    ..default()
                })
                .param("entity_key", entity_key as i64)
                .param("type_path", type_path)
                .call();
            commands.queue(move |world: &mut World| rebuild_inspectors_for_key(world, entity_key));
        }
        APPLY_TO_SOURCE => {
            let Some(instance_root) = target.instance_root else {
                return;
            };
            let Some(prefab_entity_id) = target.prefab_entity_id else {
                return;
            };
            let Some(entity_key) = target.entity_key else {
                return;
            };
            let Some(type_path) = target.type_path.clone() else {
                return;
            };
            commands.queue(move |world: &mut World| {
                apply_component_to_source(
                    world,
                    instance_root,
                    entity_key,
                    prefab_entity_id,
                    &type_path,
                );
                rebuild_inspectors_for_key(world, entity_key);
            });
        }
        BULK_APPLY => {
            let Some(entity_key) = target.entity_key else {
                return;
            };
            let Some(type_path) = target.type_path.clone() else {
                return;
            };
            commands.queue(move |world: &mut World| {
                bulk_apply_component_to_scene(world, entity_key, &type_path);
                rebuild_inspectors_for_key(world, entity_key);
            });
        }
        REVERT_FIELD => {
            let Some(entity_key) = target.entity_key else {
                return;
            };
            let Some(type_path) = target.type_path.clone() else {
                return;
            };
            let Some(field_path) = target.field_path.clone() else {
                return;
            };
            commands
                .operator("prefab.revert_field")
                .settings(CallOperatorSettings {
                    creates_history_entry: true,
                    ..default()
                })
                .param("entity_key", entity_key as i64)
                .param("type_path", type_path)
                .param("field_path", field_path)
                .call();
            commands.queue(move |world: &mut World| rebuild_inspectors_for_key(world, entity_key));
        }
        APPLY_FIELD_TO_SOURCE => {
            let Some(instance_root) = target.instance_root else {
                return;
            };
            let Some(prefab_entity_id) = target.prefab_entity_id else {
                return;
            };
            let Some(entity_key) = target.entity_key else {
                return;
            };
            let Some(type_path) = target.type_path.clone() else {
                return;
            };
            let Some(field_path) = target.field_path.clone() else {
                return;
            };
            commands.queue(move |world: &mut World| {
                let value: Option<serde_json::Value> = {
                    let ast = world.resource::<SceneJsnAst>();
                    ast.get_component_at(entity_key, &type_path)
                        .and_then(|v| walk_dot_path(v, &field_path).cloned())
                };
                let Some(value) = value else { return };
                let value_json = match serde_json::to_string(&value) {
                    Ok(s) => s,
                    Err(err) => {
                        warn!(
                            "inspector.prefab.apply_field_to_source: serialize value failed: {err}"
                        );
                        return;
                    }
                };
                let _ = world
                    .operator("prefab.apply_to_source")
                    .settings(CallOperatorSettings {
                        creates_history_entry: true,
                        ..default()
                    })
                    .param("instance_root", instance_root as i64)
                    .param("entity_id", prefab_entity_id as i64)
                    .param("type_path", type_path.clone())
                    .param("field_path", field_path.clone())
                    .param("value_json", value_json)
                    .call();
                rebuild_inspectors_for_key(world, entity_key);
            });
        }
        _ => return,
    }

    // Close the menu after dispatching.
    if let Some(menu) = state.menu_entity.take()
        && let Ok(mut ec) = commands.get_entity(menu)
    {
        ec.despawn();
    }
    state.target_entity = None;
}

/// Push every overridden field on `type_path` into the prefab source.
/// "Overridden" means the entity's component value differs from the
/// cached prefab value at that field path. The flattened delta object
/// is walked recursively so nested struct fields (`translation.x`)
/// land as dotted paths. Each leaf dispatches `prefab.apply_to_source`
/// so the operator framework owns history / telemetry.
fn apply_component_to_source(
    world: &mut World,
    instance_root: usize,
    entity_key: usize,
    prefab_entity_id: u32,
    type_path: &str,
) {
    let deltas: Vec<(String, serde_json::Value)> = {
        let ast = world.resource::<SceneJsnAst>();
        let Some(scene_value) = ast.get_component_at(entity_key, type_path) else {
            return;
        };
        let cache = world.resource::<PrefabAstCache>();
        let prefab_value = resolve_prefab_value(ast, cache, entity_key, type_path);
        collect_overridden_paths(scene_value, prefab_value.as_ref())
    };

    for (field_path, value) in deltas {
        let value_json = match serde_json::to_string(&value) {
            Ok(s) => s,
            Err(err) => {
                warn!("apply_component_to_source: serialize value failed: {err}");
                continue;
            }
        };
        let _ = world
            .operator("prefab.apply_to_source")
            .param("instance_root", instance_root as i64)
            .param("entity_id", prefab_entity_id as i64)
            .param("type_path", type_path.to_string())
            .param("field_path", field_path)
            .param("value_json", value_json)
            .call();
    }
}

/// For every overridden leaf on `type_path` of the entity at
/// `entity_key`, dispatch `prefab.bulk_apply_in_scene` so all other
/// instances in the same scene receive the same delta.
fn bulk_apply_component_to_scene(world: &mut World, entity_key: usize, type_path: &str) {
    let (deltas, source_path): (Vec<(String, serde_json::Value)>, PathBuf) = {
        let ast = world.resource::<SceneJsnAst>();
        let Some(scene_value) = ast.get_component_at(entity_key, type_path) else {
            return;
        };
        let Some((path, _)) = crate::prefab::overrides::resolve_inheritance(ast, entity_key) else {
            return;
        };
        let cache = world.resource::<PrefabAstCache>();
        let prefab_value = resolve_prefab_value(ast, cache, entity_key, type_path);
        (
            collect_overridden_paths(scene_value, prefab_value.as_ref()),
            path,
        )
    };

    let source_str = source_path.to_string_lossy().into_owned();
    for (field_path, value) in deltas {
        let value_json = match serde_json::to_string(&value) {
            Ok(s) => s,
            Err(err) => {
                warn!("bulk_apply_component_to_scene: serialize value failed: {err}");
                continue;
            }
        };
        let _ = world
            .operator("prefab.bulk_apply_in_scene")
            .param("source_path", source_str.clone())
            .param("type_path", type_path.to_string())
            .param("field_path", field_path)
            .param("value_json", value_json)
            .call();
    }
}

fn walk_dot_path<'a>(
    value: &'a serde_json::Value,
    dot_path: &str,
) -> Option<&'a serde_json::Value> {
    let mut cur = value;
    for part in dot_path.split('.') {
        cur = cur.as_object()?.get(part)?;
    }
    Some(cur)
}

fn resolve_prefab_value(
    ast: &SceneJsnAst,
    cache: &PrefabAstCache,
    entity_key: usize,
    type_path: &str,
) -> Option<serde_json::Value> {
    let (path, prefab_entity_id) = crate::prefab::overrides::resolve_inheritance(ast, entity_key)?;
    let prefab_ast = cache.get(&path)?;
    let prefab_key = prefab_ast.nodes.iter().enumerate().find_map(|(i, node)| {
        let id = node
            .components
            .get("jackdaw::prefab::components::PrefabEntityId")
            .and_then(serde_json::Value::as_u64)?;
        if id as u32 == prefab_entity_id {
            Some(i)
        } else {
            None
        }
    })?;
    prefab_ast.get_component_at(prefab_key, type_path).cloned()
}

/// Walk `scene_value` and emit `(dot_path, leaf)` pairs for every
/// scalar / non-object leaf that differs from `prefab_value`'s value at
/// the same path. Object branches recurse; scalar branches compare
/// directly. When `prefab_value` is None (the component itself was
/// added on the instance), every leaf is reported.
fn collect_overridden_paths(
    scene_value: &serde_json::Value,
    prefab_value: Option<&serde_json::Value>,
) -> Vec<(String, serde_json::Value)> {
    let mut out: Vec<(String, serde_json::Value)> = Vec::new();
    walk(scene_value, prefab_value, String::new(), &mut out);
    out
}

fn walk(
    scene: &serde_json::Value,
    prefab: Option<&serde_json::Value>,
    path: String,
    out: &mut Vec<(String, serde_json::Value)>,
) {
    match scene {
        serde_json::Value::Object(scene_map) => {
            // Recurse field-by-field so a single Vec3 axis difference
            // produces `translation.x` rather than a full Vec3 blob.
            let prefab_map = prefab.and_then(serde_json::Value::as_object);
            for (key, scene_child) in scene_map {
                let next_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let prefab_child = prefab_map.and_then(|m| m.get(key));
                walk(scene_child, prefab_child, next_path, out);
            }
        }
        leaf => {
            let prefab_leaf = prefab;
            let differs = match prefab_leaf {
                Some(p) => p != leaf,
                None => true,
            };
            if differs && !path.is_empty() {
                out.push((path, leaf.clone()));
            }
        }
    }
}

fn rebuild_inspectors_for_key(world: &mut World, entity_key: usize) {
    let ast = world.resource::<SceneJsnAst>();
    let entity = ast.nodes.get(entity_key).and_then(|node| node.ecs_entity);
    if let Some(entity) = entity
        && let Ok(mut ec) = world.get_entity_mut(entity)
    {
        ec.insert(super::InspectorDirty);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flat_leaf_difference_emits_dotted_path() {
        let scene = json!({ "translation": { "x": 1.0, "y": 0.0, "z": 0.0 } });
        let prefab = json!({ "translation": { "x": 0.0, "y": 0.0, "z": 0.0 } });
        let out = collect_overridden_paths(&scene, Some(&prefab));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "translation.x");
        assert_eq!(out[0].1, json!(1.0));
    }

    #[test]
    fn equal_values_emit_nothing() {
        let scene = json!({ "translation": { "x": 0.0 } });
        let prefab = json!({ "translation": { "x": 0.0 } });
        let out = collect_overridden_paths(&scene, Some(&prefab));
        assert!(out.is_empty());
    }

    #[test]
    fn missing_prefab_treats_every_leaf_as_override() {
        let scene = json!({ "a": 1, "b": { "c": 2 } });
        let out = collect_overridden_paths(&scene, None);
        let names: Vec<&str> = out.iter().map(|(p, _)| p.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b.c"));
        assert_eq!(out.len(), 2);
    }
}
