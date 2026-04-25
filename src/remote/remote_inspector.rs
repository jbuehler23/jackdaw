use bevy::{
    ecs::{
        archetype::Archetype,
        component::{ComponentId, Components},
        reflect::{AppTypeRegistry, ReflectComponent},
    },
    prelude::*,
    reflect::serde::TypedReflectDeserializer,
};
use jackdaw_feathers::{
    icons::{EditorFont, Icon, IconFont},
    panel_header, tokens,
};
use jackdaw_widgets::collapsible::{
    CollapsibleBody, CollapsibleHeader, CollapsibleSection, ToggleCollapsible,
};
use serde::de::DeserializeSeed;

use super::entity_browser::{
    RemoteEntityProxy, RemoteProxyIndex, RemoteSceneCache, RemoteSelection,
};
use crate::inspector::{ComponentDisplay, InspectorGroupSection, component_display};

/// Marker for the remote inspector panel (distinct from `Inspector` to avoid `Single<>` conflict).
#[derive(Component)]
pub struct RemoteInspector;

/// Tracks which `ComponentIds` were temporarily inserted into the proxy for inspection.
#[derive(Component, Default)]
struct PopulatedComponents(Vec<ComponentId>);

/// Tracks the previous remote selection to detect changes.
#[derive(Component, Clone, Copy)]
struct PreviousRemoteSelection(Option<u64>);

/// Flag that triggers phase-2 display building in a normal system context.
#[derive(Component)]
pub struct RemoteInspectorNeedsRebuild {
    proxy_entity: Entity,
    fallback_components: Vec<(String, serde_json::Value)>,
}

/// Build the remote inspector panel bundle.
pub fn remote_inspector() -> impl Bundle {
    (
        Node {
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            panel_header::panel_header("Remote Inspector"),
            (
                RemoteInspector,
                PopulatedComponents::default(),
                PreviousRemoteSelection(None),
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(tokens::SPACING_SM),
                    overflow: Overflow::scroll_y(),
                    flex_grow: 1.0,
                    min_height: Val::Px(0.0),
                    padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
                    ..Default::default()
                },
            ),
        ],
    )
}

/// Phase 1 (exclusive system): detect selection change, populate proxy with real components,
/// set flag for phase 2.
pub fn populate_remote_proxy(world: &mut World) {
    // Find inspector entity
    let inspector_entity = {
        let mut query = world.query_filtered::<Entity, With<RemoteInspector>>();
        let Some(e) = query.iter(world).next() else {
            return;
        };
        e
    };

    let current_selection = world.resource::<RemoteSelection>().selected;
    let prev = world
        .get::<PreviousRemoteSelection>(inspector_entity)
        .map(|p| p.0);

    if prev == Some(current_selection) {
        return;
    }

    // Update stored previous selection
    world
        .entity_mut(inspector_entity)
        .insert(PreviousRemoteSelection(current_selection));

    // Clean up previously populated components from proxy
    cleanup_proxy_components(world, inspector_entity);

    // Despawn existing inspector children
    despawn_inspector_children(world, inspector_entity);

    let Some(selected_bits) = current_selection else {
        spawn_placeholder(world, inspector_entity, "No entity selected");
        return;
    };

    // Look up proxy entity
    let proxy_entity = {
        let index = world.resource::<RemoteProxyIndex>();
        index.map.get(&selected_bits).copied()
    };
    let Some(proxy_entity) = proxy_entity else {
        spawn_placeholder(world, inspector_entity, "Proxy not found");
        return;
    };

    // Look up remote entity data in cache
    let remote_components: Vec<(String, serde_json::Value)> = {
        let cache = world.resource::<RemoteSceneCache>();
        cache
            .entities
            .iter()
            .find(|e| e.entity == selected_bits)
            .map(|e| {
                e.components
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };

    if remote_components.is_empty() {
        spawn_placeholder(world, inspector_entity, "No component data");
        return;
    }

    // Populate proxy entity with real components via reflection
    let mut populated_ids: Vec<ComponentId> = Vec::new();
    let mut fallback_components: Vec<(String, serde_json::Value)> = Vec::new();

    {
        let registry_arc = world.resource::<AppTypeRegistry>().clone();
        let registry = registry_arc.read();

        for (type_path, json_value) in &remote_components {
            let Some(registration) = registry.get_with_type_path(type_path) else {
                fallback_components.push((type_path.clone(), json_value.clone()));
                continue;
            };
            let Some(reflect_component) = registration.data::<ReflectComponent>() else {
                fallback_components.push((type_path.clone(), json_value.clone()));
                continue;
            };

            let deserializer = TypedReflectDeserializer::new(registration, &registry);
            let Ok(reflected) = deserializer.deserialize(json_value) else {
                fallback_components.push((type_path.clone(), json_value.clone()));
                continue;
            };

            // Insert the component onto the proxy entity.
            // Catch panics from components with required components we can't satisfy.
            let insert_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut entity_mut = world.entity_mut(proxy_entity);
                reflect_component.insert(&mut entity_mut, reflected.as_ref(), &registry);
            }));

            if insert_result.is_ok() {
                // Track the component ID for cleanup
                let type_id = registration.type_id();
                if let Some(id) = world.components().get_id(type_id) {
                    populated_ids.push(id);
                }
            } else {
                fallback_components.push((type_path.clone(), json_value.clone()));
            }
        }
    }

    // Store populated component IDs for cleanup
    world
        .entity_mut(inspector_entity)
        .insert(PopulatedComponents(populated_ids));

    // Set flag for phase 2
    world
        .entity_mut(inspector_entity)
        .insert(RemoteInspectorNeedsRebuild {
            proxy_entity,
            fallback_components,
        });
}

/// Phase 2 (normal system): read the rebuild flag and build inspector displays using the
/// shared `build_inspector_displays()` function, which requires normal system params.
pub fn build_remote_inspector_displays(
    mut commands: Commands,
    components: &Components,
    type_registry: Res<AppTypeRegistry>,
    names: Query<&Name>,
    icon_font: Res<IconFont>,
    editor_font: Res<EditorFont>,
    inspector_query: Query<(Entity, &RemoteInspectorNeedsRebuild), With<RemoteInspector>>,
    entity_query: Query<(&Archetype, EntityRef)>,
    materials: Res<Assets<StandardMaterial>>,
) {
    let Ok((inspector_entity, rebuild)) = inspector_query.single() else {
        return;
    };

    let proxy_entity = rebuild.proxy_entity;
    let fallback_components = rebuild.fallback_components.clone();

    commands
        .entity(inspector_entity)
        .remove::<RemoteInspectorNeedsRebuild>();

    // The proxy entity now has real components populated by phase 1.
    // Use the shared build function.
    let Ok((archetype, entity_ref)) = entity_query.get(proxy_entity) else {
        return;
    };

    // Remote entities are read-only, no AST filter
    let empty_jsn_paths = std::collections::HashSet::new();
    component_display::build_inspector_displays(
        &mut commands,
        components,
        &type_registry,
        proxy_entity,
        archetype,
        entity_ref,
        inspector_entity,
        1,
        &names,
        &icon_font,
        &editor_font,
        true, // read_only
        &materials,
        &empty_jsn_paths,
    );

    // Spawn JSON fallback section for unregistered components
    if !fallback_components.is_empty() {
        spawn_fallback_section(
            &mut commands,
            inspector_entity,
            proxy_entity,
            &fallback_components,
            &icon_font,
            &editor_font,
        );
    }
}

fn spawn_fallback_section(
    commands: &mut Commands,
    inspector_entity: Entity,
    source_entity: Entity,
    fallback_components: &[(String, serde_json::Value)],
    icon_font: &IconFont,
    editor_font: &EditorFont,
) {
    let section = commands
        .spawn((
            ComponentDisplay,
            InspectorGroupSection,
            CollapsibleSection { collapsed: false },
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(inspector_entity),
        ))
        .id();

    let header = commands
        .spawn((
            CollapsibleHeader,
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                width: Val::Percent(100.0),
                padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_SM)),
                column_gap: Val::Px(tokens::SPACING_SM),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            ChildOf(section),
        ))
        .id();

    let section_for_toggle = section;
    commands
        .entity(header)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.trigger(ToggleCollapsible {
                entity: section_for_toggle,
            });
        });

    commands.spawn((
        Text::new(String::from(Icon::FileBraces.unicode())),
        TextFont {
            font: icon_font.0.clone(),
            font_size: tokens::FONT_MD,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(header),
    ));

    commands.spawn((
        Text::new("Other Components (JSON)"),
        TextFont {
            font: editor_font.0.clone(),
            font_size: tokens::FONT_MD,
            weight: FontWeight::BOLD,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(header),
    ));

    let group_body = commands
        .spawn((
            CollapsibleBody,
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                border: UiRect::left(Val::Px(1.0)),
                margin: UiRect::left(Val::Px(tokens::SPACING_MD)),
                ..Default::default()
            },
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(section),
        ))
        .id();

    for (type_path, json_value) in fallback_components {
        let short_name = type_path
            .rsplit("::")
            .next()
            .unwrap_or(type_path)
            .to_string();

        let (display_entity, body_entity) = component_display::spawn_component_display(
            commands,
            component_display::ComponentDisplaySpec {
                name: &short_name,
                type_path,
                entity: source_entity,
                component: None,
                is_overridden: false,
                icon_font: &icon_font.0,
                editor_font: &editor_font.0,
            },
        );
        commands.entity(display_entity).insert(ChildOf(group_body));

        let json_text =
            serde_json::to_string_pretty(json_value).unwrap_or_else(|_| format!("{json_value}"));

        commands.spawn((
            Text::new(json_text),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Node {
                max_width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(body_entity),
        ));
    }
}

fn spawn_placeholder(world: &mut World, inspector_entity: Entity, message: &str) {
    world.spawn((
        ComponentDisplay,
        Text::new(message.to_string()),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            padding: UiRect::all(Val::Px(tokens::SPACING_MD)),
            ..Default::default()
        },
        ChildOf(inspector_entity),
    ));
}

fn cleanup_proxy_components(world: &mut World, inspector_entity: Entity) {
    let component_ids: Vec<ComponentId> = world
        .get::<PopulatedComponents>(inspector_entity)
        .map(|p| p.0.clone())
        .unwrap_or_default();

    if component_ids.is_empty() {
        return;
    }

    // Remove populated components from all proxy entities
    let proxies: Vec<Entity> = {
        let mut query = world.query_filtered::<Entity, With<RemoteEntityProxy>>();
        query.iter(world).collect()
    };

    for proxy in proxies {
        for &comp_id in &component_ids {
            world.entity_mut(proxy).remove_by_id(comp_id);
        }
    }

    world
        .entity_mut(inspector_entity)
        .insert(PopulatedComponents::default());
}

fn despawn_inspector_children(world: &mut World, inspector_entity: Entity) {
    let children: Vec<Entity> = world
        .get::<Children>(inspector_entity)
        .map(|c| c.iter().collect())
        .unwrap_or_default();

    for child in children {
        if world.get::<ComponentDisplay>(child).is_some()
            && let Ok(ec) = world.get_entity_mut(child)
        {
            ec.despawn();
        }
    }
}
