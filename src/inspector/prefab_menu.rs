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
//! Routing data (row entity, prefab source path, etc) lives in
//! [`PrefabMenuTarget`] because the existing
//! [`ContextMenuAction`] event only carries an action string and an
//! optional entity, not the rich context the prefab operators need.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_bsn::{BsnValue, SceneBsnAst, get_bsn_field};
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
    /// ECS entity the row belongs to. Used to dispatch prefab operators
    /// that resolve their document nodes post-snapshot-install.
    pub(crate) entity: Option<Entity>,
    /// ECS entity of the prefab instance root the row's entity sits
    /// inside. Used for the same reason as `entity`.
    pub(crate) instance_entity: Option<Entity>,
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
            let (Some(entity), Some(type_path)) = (target.entity, target.type_path.clone()) else {
                return;
            };
            commands
                .operator("prefab.revert_component")
                .settings(CallOperatorSettings {
                    creates_history_entry: true,
                    ..default()
                })
                .param("entity", entity)
                .param("type_path", type_path)
                .call();
            commands.queue(move |world: &mut World| rebuild_inspectors_for_entity(world, entity));
        }
        APPLY_TO_SOURCE => {
            let Some(entity) = target.entity else {
                return;
            };
            let Some(instance_entity) = target.instance_entity else {
                return;
            };
            let Some(prefab_entity_id) = target.prefab_entity_id else {
                return;
            };
            let Some(type_path) = target.type_path.clone() else {
                return;
            };
            commands.queue(move |world: &mut World| {
                apply_component_to_source(
                    world,
                    instance_entity,
                    entity,
                    prefab_entity_id,
                    &type_path,
                );
                rebuild_inspectors_for_entity(world, entity);
            });
        }
        BULK_APPLY => {
            let Some(entity) = target.entity else {
                return;
            };
            let Some(type_path) = target.type_path.clone() else {
                return;
            };
            commands.queue(move |world: &mut World| {
                bulk_apply_component_to_scene(world, entity, &type_path);
                rebuild_inspectors_for_entity(world, entity);
            });
        }
        REVERT_FIELD => {
            let Some(entity) = target.entity else {
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
                .param("entity", entity)
                .param("type_path", type_path)
                .param("field_path", field_path)
                .call();
            commands.queue(move |world: &mut World| rebuild_inspectors_for_entity(world, entity));
        }
        APPLY_FIELD_TO_SOURCE => {
            let Some(entity) = target.entity else {
                return;
            };
            let Some(instance_entity) = target.instance_entity else {
                return;
            };
            let Some(prefab_entity_id) = target.prefab_entity_id else {
                return;
            };
            let Some(type_path) = target.type_path.clone() else {
                return;
            };
            let Some(field_path) = target.field_path.clone() else {
                return;
            };
            commands.queue(move |world: &mut World| {
                let value: Option<BsnValue> = {
                    let ast = world.resource::<SceneBsnAst>();
                    ast.ast_for(entity)
                        .and_then(|node| get_bsn_field(ast, node, &type_path, &field_path))
                };
                let Some(value) = value else { return };
                let value_json = match serde_json::to_string(&bsn_leaf_to_json(&value)) {
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
                    .param("instance_entity", instance_entity)
                    .param("entity_id", prefab_entity_id as i64)
                    .param("type_path", type_path.clone())
                    .param("field_path", field_path.clone())
                    .param("value_json", value_json)
                    .call();
                rebuild_inspectors_for_entity(world, entity);
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
/// cached prefab value at that field path. The flattened delta is walked
/// recursively so nested struct fields (`translation.x`) land as dotted
/// paths. Each leaf dispatches `prefab.apply_to_source` so the operator
/// framework owns history / telemetry.
fn apply_component_to_source(
    world: &mut World,
    instance_entity: Entity,
    entity: Entity,
    prefab_entity_id: u32,
    type_path: &str,
) {
    let deltas: Vec<(String, BsnValue)> = {
        let ast = world.resource::<SceneBsnAst>();
        let Some(node) = ast.ast_for(entity) else {
            return;
        };
        let Some(scene_value) = get_bsn_field(ast, node, type_path, "") else {
            return;
        };
        let cache = world.resource::<PrefabAstCache>();
        let prefab_value = resolve_prefab_value(ast, cache, node, type_path);
        crate::prefab::overrides_bsn::collect_overridden_paths(&scene_value, prefab_value.as_ref())
    };

    for (field_path, value) in deltas {
        let value_json = match serde_json::to_string(&bsn_leaf_to_json(&value)) {
            Ok(s) => s,
            Err(err) => {
                warn!("apply_component_to_source: serialize value failed: {err}");
                continue;
            }
        };
        let _ = world
            .operator("prefab.apply_to_source")
            .param("instance_entity", instance_entity)
            .param("entity_id", prefab_entity_id as i64)
            .param("type_path", type_path.to_string())
            .param("field_path", field_path)
            .param("value_json", value_json)
            .call();
    }
}

/// For every overridden leaf on `type_path` of `entity`, dispatch
/// `prefab.bulk_apply_in_scene` so all other instances in the same scene
/// receive the same delta.
fn bulk_apply_component_to_scene(world: &mut World, entity: Entity, type_path: &str) {
    let (deltas, source_path): (Vec<(String, BsnValue)>, PathBuf) = {
        let ast = world.resource::<SceneBsnAst>();
        let Some(node) = ast.ast_for(entity) else {
            return;
        };
        let Some(scene_value) = get_bsn_field(ast, node, type_path, "") else {
            return;
        };
        let Some((path, _)) = crate::prefab::overrides_bsn::resolve_inheritance(ast, node) else {
            return;
        };
        let cache = world.resource::<PrefabAstCache>();
        let prefab_value = resolve_prefab_value(ast, cache, node, type_path);
        (
            crate::prefab::overrides_bsn::collect_overridden_paths(
                &scene_value,
                prefab_value.as_ref(),
            ),
            path,
        )
    };

    let source_str = source_path.to_string_lossy().into_owned();
    for (field_path, value) in deltas {
        let value_json = match serde_json::to_string(&bsn_leaf_to_json(&value)) {
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

/// The cached prefab's baseline value for `type_path` on the prefab entry
/// that `node` inherits from.
fn resolve_prefab_value(
    ast: &SceneBsnAst,
    cache: &PrefabAstCache,
    node: Entity,
    type_path: &str,
) -> Option<BsnValue> {
    let (path, prefab_entity_id) = crate::prefab::overrides_bsn::resolve_inheritance(ast, node)?;
    let prefab = cache.get(&path)?;
    let prefab_node = prefab.find_node_by_component_int(
        "jackdaw::prefab::components::PrefabEntityId",
        u64::from(prefab_entity_id),
    )?;
    get_bsn_field(prefab, prefab_node, type_path, "")
}

/// Structural JSON form of an override leaf for the `value_json` operator
/// params. Scalars map cleanly; lists, maps, and tuple structs map to
/// arrays / objects (the operators apply them structurally). Struct leaves
/// do not occur: `collect_overridden_paths` recurses into structs.
fn bsn_leaf_to_json(value: &BsnValue) -> serde_json::Value {
    match value {
        BsnValue::Float(f) => serde_json::json!(f),
        BsnValue::Int(i) => serde_json::json!(*i as i64),
        BsnValue::Bool(b) => serde_json::json!(b),
        BsnValue::String(s) | BsnValue::Type(s) => serde_json::json!(s),
        BsnValue::List(items) => {
            serde_json::Value::Array(items.iter().map(bsn_leaf_to_json).collect())
        }
        BsnValue::TupleStruct(data) => {
            serde_json::Value::Array(data.values.iter().map(bsn_leaf_to_json).collect())
        }
        BsnValue::Map(entries) => {
            let mut map = serde_json::Map::new();
            for (k, v) in entries {
                let key = match k {
                    BsnValue::String(s) => s.clone(),
                    BsnValue::Int(i) => i.to_string(),
                    BsnValue::Float(fl) => fl.to_string(),
                    BsnValue::Bool(b) => b.to_string(),
                    _ => continue,
                };
                map.insert(key, bsn_leaf_to_json(v));
            }
            serde_json::Value::Object(map)
        }
        BsnValue::Struct(data) => {
            let mut map = serde_json::Map::new();
            for field in &data.fields.0 {
                map.insert(field.name.clone(), bsn_leaf_to_json(&field.value));
            }
            serde_json::Value::Object(map)
        }
    }
}

fn rebuild_inspectors_for_entity(world: &mut World, entity: Entity) {
    if let Ok(mut ec) = world.get_entity_mut(entity) {
        ec.insert(super::InspectorDirty);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefab::overrides_bsn::collect_overridden_paths;
    use jackdaw_bsn::{BsnField, BsnStructData, BsnStructFields, bsn_value_eq};

    fn f(name: &str, value: BsnValue) -> BsnField {
        BsnField {
            name: name.to_string(),
            value,
        }
    }

    fn strukt(type_path: &str, fields: Vec<BsnField>) -> BsnValue {
        BsnValue::Struct(BsnStructData {
            type_path: type_path.to_string(),
            fields: BsnStructFields(fields),
        })
    }

    fn vec3(x: f64, y: f64, z: f64) -> BsnValue {
        strukt(
            "glam::Vec3",
            vec![
                f("x", BsnValue::Float(x)),
                f("y", BsnValue::Float(y)),
                f("z", BsnValue::Float(z)),
            ],
        )
    }

    #[test]
    fn flat_leaf_difference_emits_dotted_path() {
        let scene = strukt("Transform", vec![f("translation", vec3(1.0, 0.0, 0.0))]);
        let prefab = strukt("Transform", vec![f("translation", vec3(0.0, 0.0, 0.0))]);
        let out = collect_overridden_paths(&scene, Some(&prefab));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "translation.x");
        assert!(bsn_value_eq(&out[0].1, &BsnValue::Float(1.0)));
    }

    #[test]
    fn equal_values_emit_nothing() {
        let scene = strukt("Transform", vec![f("translation", vec3(0.0, 0.0, 0.0))]);
        let prefab = strukt("Transform", vec![f("translation", vec3(0.0, 0.0, 0.0))]);
        let out = collect_overridden_paths(&scene, Some(&prefab));
        assert!(out.is_empty());
    }

    #[test]
    fn missing_prefab_treats_every_leaf_as_override() {
        let scene = strukt(
            "Marker",
            vec![
                f("a", BsnValue::Int(1)),
                f("b", strukt("Inner", vec![f("c", BsnValue::Int(2))])),
            ],
        );
        let out = collect_overridden_paths(&scene, None);
        let names: Vec<&str> = out.iter().map(|(p, _)| p.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b.c"));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn leaf_json_form_covers_scalars_and_containers() {
        assert_eq!(
            bsn_leaf_to_json(&BsnValue::Float(1.5)),
            serde_json::json!(1.5)
        );
        assert_eq!(bsn_leaf_to_json(&BsnValue::Int(7)), serde_json::json!(7));
        assert_eq!(
            bsn_leaf_to_json(&BsnValue::Bool(true)),
            serde_json::json!(true)
        );
        assert_eq!(
            bsn_leaf_to_json(&BsnValue::String("hi".into())),
            serde_json::json!("hi")
        );
        assert_eq!(
            bsn_leaf_to_json(&BsnValue::List(vec![BsnValue::Int(1), BsnValue::Int(2)])),
            serde_json::json!([1, 2])
        );
    }
}
