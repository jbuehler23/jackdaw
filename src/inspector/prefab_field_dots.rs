use crate::prelude::*;
use std::collections::HashSet;

use bevy::ecs::component::ComponentInfo;
use bevy::{
    ecs::{component::ComponentId, reflect::ReflectComponent},
    prelude::*,
};

use super::{InspectorDirty, InspectorFieldRow};
use crate::prefab::PrefabAstCache;
use jackdaw_jsn::SceneJsnAst;

/// Resolved prefab-instance context for a component being inspected. When
/// present, override info comes from the prefab AST + cache and the
/// header's revert / right-click actions route to the new prefab
/// operators rather than the legacy baseline path.
#[derive(Clone)]
pub(crate) struct PrefabInstanceCtx {
    pub(crate) entity_key: usize,
    pub(crate) instance_root: usize,
    /// ECS entity for the prefab-instance root. Prefab operators
    /// resolve their AST keys post-snapshot-install, so dispatch sites
    /// pass this Entity rather than the (stale) `instance_root` key.
    pub(crate) instance_entity: Entity,
    pub(crate) prefab_path: std::path::PathBuf,
    pub(crate) prefab_entity_id: u32,
    pub(crate) has_cached_prefab: bool,
}

/// Marker on the override-status dot rendered next to a prefab-instance
/// field row. Carries the data needed to call `revert_field` on click.
/// Filled = override; hollow = inherited from prefab.
#[derive(Component, Clone)]
pub(crate) struct PrefabFieldOverrideDot {
    /// ECS entity the row belongs to. The dispatcher passes this through
    /// to `prefab.revert_field`, which resolves the AST key inside the
    /// operator (the live AST is rebuilt during the framework's
    /// before-snapshot capture, so any pre-resolved key is stale).
    pub(crate) entity: Entity,
    pub(crate) entity_key: usize,
    pub(crate) type_path: String,
    pub(crate) field_path: String,
}

/// Compute the set of component type paths to treat as "AST-tracked"
/// for inspector filtering. For ECS-only inherited descendants of a
/// prefab instance (entity has `PrefabEntityId` but no AST node), the
/// AST has nothing to anchor on; fall back to the matching entry in the
/// prefab cache so the inspector still has a baseline component set to
/// render against.
pub(crate) fn inspector_type_paths_for(
    ast: &SceneJsnAst,
    prefab_cache: &PrefabAstCache,
    source_entity: Entity,
    entity_ref: bevy::ecs::world::EntityRef,
    child_of_query: &Query<&bevy::ecs::hierarchy::ChildOf>,
    isa_query: &Query<&crate::prefab::IsA>,
) -> HashSet<String> {
    if let Some(node) = ast.node_for_entity(source_entity) {
        return node.components.keys().cloned().collect();
    }
    let Some(peid) = entity_ref.get::<crate::prefab::PrefabEntityId>() else {
        return HashSet::new();
    };
    // Walk up ChildOf to find the nearest ancestor with IsA.
    let mut current = source_entity;
    let isa_source = loop {
        let Ok(child_of) = child_of_query.get(current) else {
            return HashSet::new();
        };
        let parent = child_of.0;
        if let Ok(isa) = isa_query.get(parent) {
            break isa.source.clone();
        }
        current = parent;
    };
    let Some(prefab) = prefab_cache.get(&isa_source) else {
        return HashSet::new();
    };
    let prefab_entity_id_type = "jackdaw::prefab::components::PrefabEntityId";
    for node in &prefab.nodes {
        let matches = node
            .components
            .get(prefab_entity_id_type)
            .and_then(serde_json::Value::as_u64)
            .map(|u| u as u32)
            == Some(peid.0);
        if matches {
            return node.components.keys().cloned().collect();
        }
    }
    HashSet::new()
}

/// Revert a single component on a prefab instance back to its baseline value.
pub(crate) fn revert_component_to_baseline(
    In((entity, component_id)): In<(Entity, ComponentId)>,
    world: &mut World,
) {
    use bevy::ecs::reflect::AppTypeRegistry;
    use bevy::reflect::serde::TypedReflectDeserializer;
    use serde::de::DeserializeSeed;

    let Some(baseline) = world.get::<jackdaw_jsn::JsnPrefabBaseline>(entity).cloned() else {
        return;
    };

    let Some(type_id) = world
        .components()
        .get_info(component_id)
        .and_then(ComponentInfo::type_id)
    else {
        return;
    };

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();

    let Some(registration) = registry.get(type_id) else {
        return;
    };
    let type_path = registration.type_info().type_path_table().path();

    let Some(baseline_val) = baseline.components.get(type_path) else {
        return;
    };

    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        return;
    };

    let deserializer = TypedReflectDeserializer::new(registration, &registry);
    let Ok(reflected) = deserializer.deserialize(baseline_val) else {
        warn!("Failed to deserialize baseline for '{type_path}'");
        return;
    };

    reflect_component.apply(world.entity_mut(entity), reflected.as_ref());

    drop(registry);

    // Trigger inspector rebuild
    world.entity_mut(entity).insert(InspectorDirty);
}

/// Filled / hollow palette for the per-field override dot. Filled uses
/// the same amber as `tokens::CATEGORY_PREFAB` so the dot reads in the
/// same visual register as other prefab-related affordances.
fn override_dot_color(overridden: bool) -> Color {
    if overridden {
        jackdaw_feathers::tokens::CATEGORY_PREFAB
    } else {
        Color::srgba(0.55, 0.55, 0.55, 0.45)
    }
}

/// For every newly-spawned `InspectorFieldRow` whose source entity lives
/// inside a prefab instance subtree, attach a small dot showing whether
/// the field is overridden on this instance. Clicking the dot reverts
/// the field via `revert_field`. Rows whose entity is not part of a
/// prefab instance get no dot.
pub(crate) fn decorate_prefab_field_rows(
    new_rows: Query<(Entity, &InspectorFieldRow), Added<InspectorFieldRow>>,
    ast: Res<SceneJsnAst>,
    prefab_cache: Res<PrefabAstCache>,
    mut commands: Commands,
) {
    for (row_entity, row) in &new_rows {
        let Some(key) = ast.key_for_entity(row.source_entity) else {
            continue;
        };
        if !crate::prefab::overrides::is_inside_prefab_instance(&ast, key) {
            continue;
        }
        let overridden = crate::prefab::overrides::field_is_overridden(
            &ast,
            &prefab_cache,
            key,
            &row.type_path,
            Some(&row.field_path),
        );
        let inheritance = crate::prefab::overrides::resolve_inheritance(&ast, key);
        let instance_root_key =
            ast.ancestor_with_component(key, "jackdaw::prefab::components::IsA");
        let instance_entity = instance_root_key
            .and_then(|k| ast.nodes.get(k))
            .and_then(|n| n.ecs_entity);
        if let (
            Some((prefab_path, prefab_entity_id)),
            Some(instance_root_key),
            Some(instance_entity),
        ) = (inheritance, instance_root_key, instance_entity)
        {
            let row_entity_param = row.source_entity;
            let row_type_path = row.type_path.clone();
            let row_field_path = row.field_path.clone();
            commands.entity(row_entity).observe(
                move |click: On<Pointer<Click>>,
                      mut commands: Commands,
                      windows: Query<&Window>,
                      mut state: ResMut<jackdaw_widgets::context_menu::ContextMenuState>,
                      mut target: ResMut<super::prefab_menu::PrefabMenuTarget>| {
                    if click.event().button != PointerButton::Secondary {
                        return;
                    }
                    let cursor_pos = windows
                        .single()
                        .ok()
                        .and_then(bevy::prelude::Window::cursor_position)
                        .unwrap_or_default();
                    if let Some(existing) = state.menu_entity.take()
                        && let Ok(mut ec) = commands.get_entity(existing)
                    {
                        ec.despawn();
                    }
                    target.entity = Some(row_entity_param);
                    target.instance_entity = Some(instance_entity);
                    target.entity_key = Some(key);
                    target.instance_root = Some(instance_root_key);
                    target.prefab_entity_id = Some(prefab_entity_id);
                    target.prefab_path = Some(prefab_path.clone());
                    target.type_path = Some(row_type_path.clone());
                    target.field_path = Some(row_field_path.clone());
                    let items: [(&str, &str); 2] = [
                        (super::prefab_menu::REVERT_FIELD, "Revert Field"),
                        (
                            super::prefab_menu::APPLY_FIELD_TO_SOURCE,
                            "Apply Field to Prefab Source",
                        ),
                    ];
                    let menu = jackdaw_feathers::context_menu::spawn_context_menu(
                        &mut commands,
                        cursor_pos,
                        None,
                        &items,
                    );
                    state.menu_entity = Some(menu);
                },
            );
        }

        // Absolutely-positioned wrapper so the dot anchors to the row's
        // right edge without disturbing the row's flex layout. Same
        // approach `anim_diamond::decorate_animatable_fields` uses for
        // its corner button. The dot itself is the wrapper's only
        // child; sharing the wrapper keeps a single entity-level click
        // observer driving the revert.
        let wrapper = commands
            .spawn(Node {
                position_type: PositionType::Absolute,
                top: Val::Px(2.0),
                right: Val::Px(20.0),
                ..default()
            })
            .id();

        let dot = commands
            .spawn((
                PrefabFieldOverrideDot {
                    entity: row.source_entity,
                    entity_key: key,
                    type_path: row.type_path.clone(),
                    field_path: row.field_path.clone(),
                },
                Node {
                    width: Val::Px(8.0),
                    height: Val::Px(8.0),
                    border_radius: BorderRadius::all(Val::Px(4.0)),
                    ..default()
                },
                BackgroundColor(override_dot_color(overridden)),
            ))
            .id();

        commands.entity(dot).observe(
            move |click: On<Pointer<Click>>,
                  dots: Query<&PrefabFieldOverrideDot>,
                  mut commands: Commands| {
                if click.event().button != PointerButton::Primary {
                    return;
                }
                let Ok(dot_data) = dots.get(click.event_target()) else {
                    return;
                };
                let entity = dot_data.entity;
                let type_path = dot_data.type_path.clone();
                let field_path = dot_data.field_path.clone();
                // `revert_field` is a no-op when the current value
                // already matches the prefab, so a click on a hollow
                // dot is harmless. The visual short-circuit still lives
                // in `refresh_prefab_field_dots` (which paints the
                // color); the operator is the source of truth for
                // whether anything actually changes.
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
                commands.queue(move |world: &mut World| {
                    if let Ok(mut ec) = world.get_entity_mut(entity) {
                        ec.insert(InspectorDirty);
                    }
                });
            },
        );

        jackdaw_feathers::utils::attach_or_despawn(&mut commands, wrapper, dot);
        jackdaw_feathers::utils::attach_or_despawn(&mut commands, row_entity, wrapper);
    }
}

/// Repaint every existing override dot whenever the scene AST changes.
/// Runs only on `ast.is_changed()` ticks so the per-frame cost is one
/// resource-changed check when nothing is editing.
pub(crate) fn refresh_prefab_field_dots(
    ast: Res<SceneJsnAst>,
    prefab_cache: Res<PrefabAstCache>,
    mut dots: Query<(&PrefabFieldOverrideDot, &mut BackgroundColor)>,
) {
    if !ast.is_changed() && !prefab_cache.is_changed() {
        return;
    }
    for (dot, mut bg) in &mut dots {
        let overridden = crate::prefab::overrides::field_is_overridden(
            &ast,
            &prefab_cache,
            dot.entity_key,
            &dot.type_path,
            Some(&dot.field_path),
        );
        bg.0 = override_dot_color(overridden);
    }
}
