use crate::EditorEntity;
use crate::custom_properties::CustomProperties;
use crate::default_style;
use crate::prelude::*;
use crate::selection::{Selected, Selection};
use std::any::TypeId;

use bevy::ecs::component::ComponentInfo;
use bevy::{
    ecs::{
        archetype::Archetype,
        component::{ComponentId, Components},
        reflect::{AppTypeRegistry, ReflectComponent},
    },
    prelude::*,
    reflect::serde::TypedReflectSerializer,
};
use jackdaw_feathers::{
    button::ButtonOperatorCall,
    icons::{EditorFont, Icon, IconFont},
    tokens,
};
use jackdaw_widgets::collapsible::{
    CollapsibleBody, CollapsibleHeader, CollapsibleSection, ToggleCollapsible,
};

use jackdaw_feathers::text_edit::TextEditValue;
use std::collections::HashSet;

use bevy_monitors::prelude::{Addition, Monitor, NotifyAdded};

use jackdaw_avian_integration::AvianCollider;
use jackdaw_geometry::is_convex_topology;
use jackdaw_runtime::EditorCategory;

use super::{
    AddComponentButton, ComponentDisplay, ComponentDisplayBody, ComponentName, ComponentPicker,
    Inspector, InspectorDirty, InspectorFieldRow, InspectorGroupSection, InspectorSearch,
    InspectorTarget, ReflectDisplayable, brush_display, component_tooltip::ReflectedTypeTooltip,
    custom_props_display, extract_module_group, material_display, reflect_fields,
};
use crate::prefab::PrefabAstCache;
use bevy::picking::hover::Hovered;
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
fn inspector_type_paths_for(
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

pub(crate) fn add_component_displays(
    _: On<Add, Selected>,
    mut commands: Commands,
    components: &Components,
    type_registry: Res<AppTypeRegistry>,
    selection: Res<Selection>,
    entity_query: Query<(&Archetype, EntityRef), (With<Selected>, Without<EditorEntity>)>,
    inspectors: Query<Entity, With<Inspector>>,
    names: Query<&Name>,
    icon_font: Res<IconFont>,
    editor_font: Res<EditorFont>,
    materials: Res<Assets<StandardMaterial>>,
    ast: Res<jackdaw_jsn::SceneJsnAst>,
    prefab_cache: Res<PrefabAstCache>,
    child_of_query: Query<&bevy::ecs::hierarchy::ChildOf>,
    isa_query: Query<&crate::prefab::IsA>,
) {
    let Some(primary) = selection.primary() else {
        return;
    };
    let Ok((archetype, entity_ref)) = entity_query.get(primary) else {
        return;
    };

    let source_entity = entity_ref.entity();
    let sel_count = selection.entities.len();

    let jsn_type_paths = inspector_type_paths_for(
        &ast,
        &prefab_cache,
        source_entity,
        entity_ref,
        &child_of_query,
        &isa_query,
    );

    // Build the same component panel into every Inspector instance.
    // Multi-instance dock layouts can host more than one inspector
    // tab; each gets its own UI subtree but mirrors the same data.
    for inspector in &inspectors {
        build_inspector_displays(
            &mut commands,
            components,
            &type_registry,
            source_entity,
            archetype,
            entity_ref,
            inspector,
            sel_count,
            &names,
            &icon_font,
            &editor_font,
            false,
            &materials,
            &jsn_type_paths,
            Some(&ast),
            Some(&prefab_cache),
        );

        // Set up monitoring: watch the selected entity for InspectorDirty
        commands.entity(inspector).insert((
            InspectorTarget(primary),
            Monitor(primary),
            NotifyAdded::<InspectorDirty>::default(),
        ));
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "inspector rebuild needs the full system param set; bundling into a struct would just push the problem one frame down"
)]
pub(crate) fn build_inspector_displays(
    commands: &mut Commands,
    components: &Components,
    type_registry: &Res<AppTypeRegistry>,
    source_entity: Entity,
    archetype: &Archetype,
    entity_ref: EntityRef,
    inspector_entity: Entity,
    selection_count: usize,
    names: &Query<&Name>,
    icon_font: &IconFont,
    editor_font: &EditorFont,
    _read_only: bool,
    materials: &Assets<StandardMaterial>,
    jsn_type_paths: &HashSet<String>,
    scene_ast: Option<&SceneJsnAst>,
    prefab_cache: Option<&PrefabAstCache>,
) {
    // Show multi-selection header when multiple entities are selected
    if selection_count > 1 {
        commands.spawn((
            ComponentDisplay,
            Node {
                padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_SM)),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            BackgroundColor(tokens::SELECTED_BG),
            ChildOf(inspector_entity),
            children![(
                Text::new(format!(
                    "{selection_count} entities selected, edits apply to all"
                )),
                TextFont {
                    font: editor_font.0.clone(),
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
        ));
    }

    let registry = type_registry.read();

    // Check for prefab baseline (override tracking)
    let baseline = entity_ref.get::<jackdaw_jsn::JsnPrefabBaseline>().cloned();

    // Prefab-instance context: if this entity sits inside an IsA
    // subtree, override info comes from the prefab AST + cache and the
    // revert / right-click actions route to the new prefab operators.
    let prefab_ctx: Option<PrefabInstanceCtx> = scene_ast.and_then(|ast| {
        let cache = prefab_cache?;
        let key = ast.key_for_entity(source_entity)?;
        if !crate::prefab::overrides::is_inside_prefab_instance(ast, key) {
            return None;
        }
        let (path, prefab_entity_id) = crate::prefab::overrides::resolve_inheritance(ast, key)?;
        let instance_root = ast.ancestor_with_component(key, "jackdaw::prefab::components::IsA")?;
        let instance_entity = ast.nodes.get(instance_root).and_then(|n| n.ecs_entity)?;
        Some(PrefabInstanceCtx {
            entity_key: key,
            instance_root,
            instance_entity,
            has_cached_prefab: cache.get(&path).is_some(),
            prefab_path: path,
            prefab_entity_id,
        })
    });

    // (short_name, module_group, component_id, full_type_path)
    let mut custom_groups = std::collections::HashSet::new();
    let mut comp_list: Vec<(String, String, ComponentId, String)> = archetype
        .iter_components()
        .filter_map(|component_id| {
            let info = components.get_info(component_id)?;
            let type_id = info.type_id();

            // Try TypeRegistry first for proper names
            if let Some(type_id) = type_id
                && let Some(registration) = registry.get(type_id)
            {
                let table = registration.type_info().type_path_table();
                let full_path = table.path();
                if full_path.starts_with("jackdaw")
                    && !full_path.starts_with("jackdaw_jsn")
                    && !full_path.starts_with("jackdaw_avian_integration")
                    && !full_path.starts_with("jackdaw_animation")
                {
                    return None;
                }
                // AST filter: hide Bevy-internal components that
                // aren't tracked in the scene file. User-defined
                // components (anything outside the `bevy::*`,
                // `core::*`, `std::*`, and `jackdaw_*` namespaces)
                // are always shown so the inspector reflects the
                // actual ECS state. Without this exception, a user
                // component newly added via the picker would be
                // invisible if `AddComponent::execute`'s AST
                // serialization failed silently (e.g., a struct
                // field whose `Reflect` impl can't round-trip
                // through `TypedReflectSerializer`), leaving the
                // user wondering whether the click registered.
                let is_user_type = !full_path.starts_with("bevy")
                    && !full_path.starts_with("core")
                    && !full_path.starts_with("std")
                    && (!full_path.starts_with("jackdaw")
                        || full_path.starts_with("jackdaw_avian_integration"));
                if !is_user_type
                    && !jsn_type_paths.is_empty()
                    && !jsn_type_paths.contains(full_path)
                {
                    return None;
                }
                let short = table.short_path().to_string();
                let info = registration.type_info();
                let attrs = match info {
                    bevy::reflect::TypeInfo::Struct(s) => Some(s.custom_attributes()),
                    bevy::reflect::TypeInfo::TupleStruct(s) => Some(s.custom_attributes()),
                    bevy::reflect::TypeInfo::Enum(e) => Some(e.custom_attributes()),
                    _ => None,
                };
                let module_group = if let Some(cat) = attrs
                    .and_then(|a| a.get::<EditorCategory>())
                    .map(|c| c.0.to_string())
                    .filter(|s| !s.is_empty())
                {
                    custom_groups.insert(cat.clone());
                    cat
                } else {
                    extract_module_group(table.module_path())
                };
                return Some((short, module_group, component_id, full_path.to_string()));
            }

            // Fallback: use Components name
            let name = components.get_name(component_id)?;
            if name.starts_with("jackdaw")
                && !name.starts_with("jackdaw_jsn")
                && !name.starts_with("jackdaw_avian_integration")
                && !name.starts_with("jackdaw_animation")
            {
                return None;
            }
            let full = name.to_string();
            Some((
                name.shortname().to_string(),
                "Other".to_string(),
                component_id,
                full,
            ))
        })
        .collect();

    // Sort: custom-category groups first, then alphabetical within
    // each tier. `AvianCollider` is pinned to the top of its group
    // because it carries the collider-type dropdown the user reaches
    // for most when iterating on physics; ordering it alphabetically
    // (where it'd sit under `RigidBody` in the Avian3d group) buries
    // it under runtime-state components.
    let group_pin_priority = |type_path: &str| -> u8 {
        if type_path == "jackdaw_avian_integration::AvianCollider" {
            0
        } else {
            1
        }
    };
    comp_list.sort_by(|a, b| {
        let a_custom = custom_groups.contains(&a.1);
        let b_custom = custom_groups.contains(&b.1);
        b_custom
            .cmp(&a_custom)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| group_pin_priority(&a.3).cmp(&group_pin_priority(&b.3)))
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });

    // Spawn components with subtle group dividers
    let mut current_group = String::new();
    for (name, module_group, component_id, type_path) in &comp_list {
        // Category group divider with icon
        if *module_group != current_group {
            current_group = module_group.clone();
            let group_icon = if custom_groups.contains(module_group) {
                Icon::Tag
            } else {
                Icon::Package
            };
            commands.spawn((
                ComponentDisplay,
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    column_gap: Val::Px(tokens::SPACING_SM),
                    width: Val::Percent(100.0),
                    padding: UiRect::new(
                        Val::Px(tokens::SPACING_XS),
                        Val::ZERO,
                        Val::Px(tokens::SPACING_MD),
                        Val::ZERO,
                    ),
                    ..Default::default()
                },
                ChildOf(inspector_entity),
                children![
                    (
                        Text::new(String::from(group_icon.unicode())),
                        TextFont {
                            font: icon_font.0.clone(),
                            font_size: tokens::TEXT_SIZE,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_SECONDARY),
                    ),
                    (
                        Text::new(module_group.clone()),
                        TextFont {
                            font: editor_font.0.clone(),
                            font_size: tokens::FONT_SM,
                            weight: FontWeight::MEDIUM,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_SECONDARY),
                    ),
                ],
            ));
        }

        let component_id = *component_id;

        // Detect override: compare current component value vs baseline
        let is_overridden_baseline = baseline.as_ref().is_some_and(|bl| {
            let type_id = components
                .get_info(component_id)
                .and_then(ComponentInfo::type_id);
            if let Some(type_id) = type_id
                && let Some(registration) = registry.get(type_id)
                && let Some(reflect_component) = registration.data::<ReflectComponent>()
                && let Some(component_ref) = reflect_component.reflect(entity_ref)
            {
                let type_path = registration.type_info().type_path_table().path();
                if let Some(baseline_val) = bl.components.get(type_path) {
                    let serializer = TypedReflectSerializer::new(component_ref, &registry);
                    if let Ok(current_val) = serde_json::to_value(&serializer) {
                        return current_val != *baseline_val;
                    }
                }
            }
            false
        });

        let is_overridden_prefab = prefab_ctx.as_ref().is_some_and(|ctx| {
            if !ctx.has_cached_prefab {
                return false;
            }
            let (Some(ast), Some(cache)) = (scene_ast, prefab_cache) else {
                return false;
            };
            crate::prefab::overrides::field_is_overridden(
                ast,
                cache,
                ctx.entity_key,
                type_path,
                None,
            )
        });

        let is_overridden = is_overridden_baseline || is_overridden_prefab;

        // Forward the prefab context whenever the entity sits inside a
        // prefab instance so the right-click menu can offer Revert /
        // Apply on every component. The revert ICON's routing still
        // checks `is_overridden_prefab` below so the legacy
        // `JsnPrefabBaseline` path keeps using its existing operator
        // when both systems coexist.
        let spec_prefab_ctx = prefab_ctx.clone();
        let revert_through_prefab = is_overridden_prefab;

        let (display_entity, body_entity) = spawn_component_display(
            commands,
            ComponentDisplaySpec {
                name,
                type_path,
                entity: source_entity,
                component: Some(component_id),
                is_overridden,
                prefab_ctx: spec_prefab_ctx,
                revert_through_prefab,
                icon_font: &icon_font.0,
                editor_font: &editor_font.0,
            },
        );
        commands
            .entity(display_entity)
            .insert(ChildOf(inspector_entity));

        // Try Displayable first, then reflection, then fallback
        let type_id = components
            .get_info(component_id)
            .and_then(ComponentInfo::type_id);

        if let Some(type_id) = type_id
            && let Some(registration) = registry.get(type_id)
            && let Some(reflect_component) = registration.data::<ReflectComponent>()
            && let Some(reflected) = reflect_component.reflect(entity_ref)
        {
            // Priority 1: Displayable trait override
            if let Some(reflect_displayable) = registration.data::<ReflectDisplayable>()
                && let Some(displayable) = reflect_displayable.get(reflected)
            {
                let mut body_commands = commands.entity(body_entity);
                displayable.display(&mut body_commands, source_entity);
                continue;
            }

            // Priority 2: MeshMaterial3d<StandardMaterial>, display material fields
            if type_id == TypeId::of::<MeshMaterial3d<StandardMaterial>>() {
                material_display::spawn_material_display_deferred(
                    commands,
                    body_entity,
                    source_entity,
                );
                continue;
            }

            // Priority 3: CustomProperties, specialized property editor
            if type_id == TypeId::of::<CustomProperties>() {
                if let Some(cp) = reflected.downcast_ref::<CustomProperties>() {
                    custom_props_display::spawn_custom_properties_display(
                        commands,
                        body_entity,
                        source_entity,
                        cp,
                        &editor_font.0,
                        &icon_font.0,
                    );
                }
                continue;
            }

            // Priority 3b: Brush, show face/vertex info
            if type_id == TypeId::of::<crate::brush::Brush>() {
                if let Some(brush) = reflected.downcast_ref::<crate::brush::Brush>() {
                    brush_display::spawn_brush_display(commands, body_entity, brush, materials);
                    // When this brush is non-convex and has a physics collider, the bridge
                    // forces TriMesh regardless of the user's AvianCollider setting. Show a
                    // read-only note so the change is visible in the inspector.
                    // CONVEX_FUNCTIONAL: different behavior is intentional (mirrors collider-type choice in physics_brush_bridge)
                    if entity_ref.contains::<AvianCollider>()
                        && let Some(brush) = entity_ref.get::<crate::brush::Brush>()
                        && !is_convex_topology(&brush.topology)
                    {
                        commands.spawn((
                            Text::new("Status: non-convex (collider forced to TriMesh)"),
                            TextFont {
                                font_size: tokens::FONT_SM,
                                ..Default::default()
                            },
                            TextColor(tokens::TEXT_DISABLED),
                            Node {
                                margin: UiRect::top(Val::Px(tokens::SPACING_XS)),
                                ..Default::default()
                            },
                            ChildOf(body_entity),
                        ));
                    }
                }
                continue;
            }

            // Priority 3c: Terrain, custom inspector sections
            if type_id == TypeId::of::<jackdaw_jsn::Terrain>() {
                crate::terrain::inspector::spawn_terrain_inspector_container(commands, body_entity);
                continue;
            }

            // Priority 3: Generic reflection display
            let full_path = registration.type_info().type_path_table().path();
            reflect_fields::spawn_reflected_fields(
                commands,
                body_entity,
                reflected,
                0,
                String::new(),
                source_entity,
                full_path,
                names,
                type_registry,
                &editor_font.0,
                &icon_font.0,
            );
            continue;
        }

        // Fallback: no reflection data
        commands.spawn((
            Text::new("(read-only)"),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(body_entity),
        ));
    }

    // Add Component button is in the static layout header (layout.rs entity_inspector)
    // so we don't spawn a dynamic one here.
}

pub(crate) fn remove_component_displays(
    _: On<Remove, Selected>,
    mut commands: Commands,
    inspectors: Query<(Entity, Option<&Children>), With<Inspector>>,
    displays: Query<
        Entity,
        Or<(
            With<ComponentDisplay>,
            With<AddComponentButton>,
            With<ComponentPicker>,
        )>,
    >,
) {
    // Multi-instance: every inspector tab needs its own monitoring
    // teardown and its own children despawned.
    for (entity, children) in &inspectors {
        commands
            .entity(entity)
            .remove::<(InspectorTarget, Monitor, NotifyAdded<InspectorDirty>)>();

        let Some(children) = children else {
            continue;
        };

        // Collect then despawn inside a queued world closure so the
        // cascade runs as one atomic step at flush time. See
        // `on_inspector_dirty` for the rationale; piecemeal deferred
        // despawns can interleave with lazy combobox/button setup
        // spawns and orphan UI text at the root.
        let old_children: Vec<Entity> = displays.iter_many(children.collection()).collect();
        commands.queue(move |world: &mut World| {
            for child in old_children {
                if let Ok(ec) = world.get_entity_mut(child) {
                    ec.despawn();
                }
            }
        });
    }
}

/// Handles `Addition<InspectorDirty>` on the Inspector entity: despawn existing
/// displays and rebuild from the monitored source entity.
pub(crate) fn on_inspector_dirty(
    _: On<Addition<InspectorDirty>>,
    mut commands: Commands,
    components: &Components,
    type_registry: Res<AppTypeRegistry>,
    inspectors: Query<(Entity, &InspectorTarget, Option<&Children>), With<Inspector>>,
    entity_query: Query<(&Archetype, EntityRef), Without<EditorEntity>>,
    selection: Res<Selection>,
    names: Query<&Name>,
    icon_font: Res<IconFont>,
    editor_font: Res<EditorFont>,
    displays: Query<
        Entity,
        Or<(
            With<ComponentDisplay>,
            With<AddComponentButton>,
            With<ComponentPicker>,
        )>,
    >,
    materials: Res<Assets<StandardMaterial>>,
    ast: Res<jackdaw_jsn::SceneJsnAst>,
    prefab_cache: Res<PrefabAstCache>,
    child_of_query: Query<&bevy::ecs::hierarchy::ChildOf>,
    isa_query: Query<&crate::prefab::IsA>,
) {
    // Multi-instance: rebuild every Inspector tab in lockstep. Each
    // inspector entity carries its own `InspectorTarget`; the dirty
    // signal originates from `InspectorDirty` on the source entity
    // and applies to every inspector watching that source.
    let mut clear_dirty_for: Option<Entity> = None;
    for (inspector_entity, target, children) in &inspectors {
        let source_entity = target.0;
        if clear_dirty_for.is_none() {
            clear_dirty_for = Some(source_entity);
        }

        // Collect the old display children, then queue a
        // world-exclusive closure that despawns them synchronously.
        // Doing this in a single queued closure (rather than piecemeal
        // `commands.despawn` calls) guarantees the cascade completes
        // as one atomic unit inside `Commands` flush; no lazy
        // `setup_button` / `setup_combobox` spawns from a previous
        // rebuild can slip in between entity despawns and leave
        // orphaned UI children (the source of "Inherited" floating
        // labels + `ChildOf(...) relates to an entity that does not
        // exist` warnings).
        let old_children: Vec<Entity> = children
            .map(|c| displays.iter_many(c.collection()).collect())
            .unwrap_or_default();
        commands.queue(move |world: &mut World| {
            for child in old_children {
                if let Ok(ec) = world.get_entity_mut(child) {
                    ec.despawn();
                }
            }
        });

        // Rebuild this inspector's contents.
        let Ok((archetype, entity_ref)) = entity_query.get(source_entity) else {
            continue;
        };
        let sel_count = selection.entities.len();

        let jsn_type_paths = inspector_type_paths_for(
            &ast,
            &prefab_cache,
            source_entity,
            entity_ref,
            &child_of_query,
            &isa_query,
        );

        build_inspector_displays(
            &mut commands,
            components,
            &type_registry,
            source_entity,
            archetype,
            entity_ref,
            inspector_entity,
            sel_count,
            &names,
            &icon_font,
            &editor_font,
            false,
            &materials,
            &jsn_type_paths,
            Some(&ast),
            Some(&prefab_cache),
        );
    }

    // Strip `InspectorDirty` from the source entity once after the
    // rebuild fans out. All inspectors watching the same source share
    // a single dirty signal.
    if let Some(source_entity) = clear_dirty_for {
        commands.queue(move |world: &mut World| {
            if let Ok(mut ec) = world.get_entity_mut(source_entity) {
                ec.remove::<InspectorDirty>();
            }
        });
    }
}

/// Inputs to [`spawn_component_display`]. Bundled into a single
/// struct so the call site is readable as a struct literal instead of
/// a long positional argument list.
pub(crate) struct ComponentDisplaySpec<'a> {
    pub name: &'a str,
    pub type_path: &'a str,
    pub entity: Entity,
    pub component: Option<ComponentId>,
    pub is_overridden: bool,
    /// When `Some`, the entity sits inside a prefab instance. Drives
    /// the right-click menu for every component on the entity.
    pub prefab_ctx: Option<PrefabInstanceCtx>,
    /// When true, the revert icon (if shown) routes through the new
    /// prefab operators (`prefab::operators::revert_component`) rather
    /// than the legacy `ComponentRevertBaselineOp` path. False forces
    /// the legacy path even if `prefab_ctx` is present, which preserves
    /// pre-existing baseline overrides.
    pub revert_through_prefab: bool,
    pub icon_font: &'a Handle<Font>,
    pub editor_font: &'a Handle<Font>,
}

pub(crate) fn spawn_component_display(
    commands: &mut Commands,
    spec: ComponentDisplaySpec<'_>,
) -> (Entity, Entity) {
    let ComponentDisplaySpec {
        name,
        type_path,
        entity,
        component,
        is_overridden,
        prefab_ctx,
        revert_through_prefab,
        icon_font,
        editor_font,
    } = spec;
    let font = icon_font.clone();
    let body_font = editor_font.clone();

    let body_entity = commands
        .spawn((
            ComponentDisplayBody,
            CollapsibleBody,
            Node {
                padding: UiRect::new(
                    Val::Px(tokens::SPACING_MD),
                    Val::Px(tokens::SPACING_SM),
                    Val::Px(tokens::SPACING_XS),
                    Val::Px(tokens::SPACING_XS),
                ),
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                ..Default::default()
            },
        ))
        .id();

    let section_entity = commands
        .spawn((
            ComponentDisplay,
            ComponentName(name.to_string()),
            CollapsibleSection { collapsed: false },
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::COMPONENT_CARD_RADIUS)),
                ..Default::default()
            },
            BackgroundColor(tokens::COMPONENT_CARD_BG),
            BorderColor::all(tokens::COMPONENT_CARD_BORDER),
            BoxShadow(vec![ShadowStyle {
                x_offset: Val::ZERO,
                y_offset: Val::ZERO,
                blur_radius: Val::Px(1.0),
                spread_radius: Val::ZERO,
                color: tokens::SHADOW_COLOR,
            }]),
        ))
        .id();

    // Header (Figma: space-between with [chevron] [icon+name] [ellipsis])
    let header = commands
        .spawn((
            CollapsibleHeader,
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                width: Val::Percent(100.0),
                padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_SM)),
                column_gap: Val::Px(tokens::SPACING_SM),
                border_radius: BorderRadius::top(Val::Px(tokens::COMPONENT_CARD_RADIUS)),
                ..Default::default()
            },
            BackgroundColor(tokens::COMPONENT_CARD_HEADER_BG),
            ChildOf(section_entity),
        ))
        .id();

    // Toggle area (chevron + icon + title) -- click to collapse/expand.
    // The hover-tooltip source sits on this row so the popover
    // surface matches the click target; the auto-attach observer in
    // `component_tooltip.rs` resolves the reflected type and inserts
    // a `Tooltip` with the short name, optional `ReflectEditorMeta`
    // description, and full type path.
    let toggle_area = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                flex_grow: 1.0,
                ..Default::default()
            },
            Hovered::default(),
            ReflectedTypeTooltip::new(type_path.to_string()),
            ChildOf(header),
        ))
        .id();

    // Chevron icon
    commands.spawn((
        Text::new(String::from(Icon::ChevronDown.unicode())),
        TextFont {
            font: font.clone(),
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(toggle_area),
    ));

    // Component icon (matching Figma: lucide/move-3d style icon)
    commands.spawn((
        Text::new(String::from(Icon::Move3d.unicode())),
        TextFont {
            font: font.clone(),
            font_size: tokens::TEXT_SIZE,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(toggle_area),
    ));

    // Component name (orange if overridden).
    let name_color = if is_overridden {
        default_style::INSPECTOR_OVERRIDE
    } else {
        tokens::TEXT_DISPLAY_COLOR.into()
    };
    commands.spawn((
        Text::new(name.to_string()),
        TextFont {
            font: body_font,
            font_size: tokens::FONT_SM,
            weight: FontWeight::MEDIUM,
            ..Default::default()
        },
        TextColor(name_color),
        ChildOf(toggle_area),
    ));

    // Toggle on click (on toggle area, not on the X button)
    let section = section_entity;
    commands
        .entity(toggle_area)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.trigger(ToggleCollapsible { entity: section });
        });

    if component.is_some() {
        let type_path_owned = type_path.to_string();
        let entity_param = entity;

        // Revert button (only shown for overridden prefab components).
        // Two code paths share the visual: the legacy
        // `JsnPrefabBaseline` system dispatches through
        // `ComponentRevertBaselineOp` (and uses `ButtonOperatorCall`
        // for the rich tooltip popover); the new prefab system calls
        // `prefab::operators::revert_component` directly with the
        // entity's AST key, so it skips the tooltip wiring.
        if is_overridden {
            let revert_type_path = type_path_owned.clone();
            let revert_through_new_prefab = revert_through_prefab && prefab_ctx.is_some();
            if revert_through_new_prefab {
                let prefab_type_path = revert_type_path.clone();
                commands.spawn((
                    Text::new(String::from(Icon::RotateCcw.unicode())),
                    TextFont {
                        font: font.clone(),
                        font_size: tokens::FONT_SM,
                        ..Default::default()
                    },
                    TextColor(default_style::INSPECTOR_OVERRIDE),
                    Hovered::default(),
                    ChildOf(header),
                    bevy::ui_widgets::observe(
                        move |_: On<Pointer<Click>>, mut commands: Commands| {
                            let revert_path = prefab_type_path.clone();
                            commands
                                .operator("prefab.revert_component")
                                .settings(CallOperatorSettings {
                                    creates_history_entry: true,
                                    ..default()
                                })
                                .param("entity", entity_param)
                                .param("type_path", revert_path)
                                .call();
                            commands.queue(move |world: &mut World| {
                                if let Ok(mut ec) = world.get_entity_mut(entity_param) {
                                    ec.insert(InspectorDirty);
                                }
                            });
                        },
                    ),
                ));
            } else {
                let bo_call = ButtonOperatorCall::new(super::ops::ComponentRevertBaselineOp::ID)
                    .with_param("entity", entity_param)
                    .with_param("type_path", revert_type_path.clone());
                commands.spawn((
                    Text::new(String::from(Icon::RotateCcw.unicode())),
                    TextFont {
                        font: font.clone(),
                        font_size: tokens::FONT_SM,
                        ..Default::default()
                    },
                    TextColor(default_style::INSPECTOR_OVERRIDE),
                    Hovered::default(),
                    bo_call,
                    ChildOf(header),
                    bevy::ui_widgets::observe(
                        move |_: On<Pointer<Click>>, mut commands: Commands| {
                            commands
                                .operator(super::ops::ComponentRevertBaselineOp::ID)
                                .param("entity", entity_param)
                                .param("type_path", revert_type_path.clone())
                                .call();
                        },
                    ),
                ));
            }
        }

        // Remove component button (X icon). See revert button for the
        // tooltip-data + manual-dispatch pattern.
        let remove_path = type_path_owned.clone();
        let remove_call = ButtonOperatorCall::new(super::ops::ComponentRemoveOp::ID)
            .with_param("entity", entity_param)
            .with_param("type_path", remove_path.clone());
        commands.spawn((
            Text::new(String::from(Icon::X.unicode())),
            TextFont {
                font: font.clone(),
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Hovered::default(),
            remove_call,
            ChildOf(header),
            bevy::ui_widgets::observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                commands
                    .operator(super::ops::ComponentRemoveOp::ID)
                    .param("entity", entity_param)
                    .param("type_path", type_path_owned.clone())
                    .call();
            }),
        ));
    }

    // Right-click context menu on prefab-instance component headers.
    // Wires the "Revert Component" / "Apply Component to Prefab Source"
    // actions; both route through `prefab_menu::on_prefab_menu_action`,
    // which reads the captured target context from
    // `prefab_menu::PrefabMenuTarget`.
    if let Some(menu_ctx) = prefab_ctx.clone() {
        let menu_type_path = type_path.to_string();
        commands.entity(header).observe(
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
                target.entity = Some(entity);
                target.instance_entity = Some(menu_ctx.instance_entity);
                target.entity_key = Some(menu_ctx.entity_key);
                target.instance_root = Some(menu_ctx.instance_root);
                target.prefab_entity_id = Some(menu_ctx.prefab_entity_id);
                target.prefab_path = Some(menu_ctx.prefab_path.clone());
                target.type_path = Some(menu_type_path.clone());
                target.field_path = None;
                let items: [(&str, &str); 3] = [
                    (super::prefab_menu::REVERT_COMPONENT, "Revert Component"),
                    (
                        super::prefab_menu::APPLY_TO_SOURCE,
                        "Apply Component to Prefab Source",
                    ),
                    (
                        super::prefab_menu::BULK_APPLY,
                        "Apply to All Instances in Scene",
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

    // Hover effect on header
    commands.entity(header).observe(
        |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor, With<CollapsibleHeader>>| {
            if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                bg.0 = tokens::HOVER_BG;
            }
        },
    );
    commands.entity(header).observe(
        |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor, With<CollapsibleHeader>>| {
            if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                bg.0 = tokens::COMPONENT_CARD_HEADER_BG;
            }
        },
    );

    // Attach body to section
    commands.entity(body_entity).insert(ChildOf(section_entity));

    (section_entity, body_entity)
}

/// Filter inspector components based on the search input.
pub(crate) fn filter_inspector_components(
    search_query: Query<&TextEditValue, (With<InspectorSearch>, Changed<TextEditValue>)>,
    components: Query<(Entity, &ComponentName), With<ComponentDisplay>>,
    groups: Query<(Entity, &Children), With<InspectorGroupSection>>,
    mut node_query: Query<&mut Node>,
) {
    let Ok(search) = search_query.single() else {
        return;
    };
    let filter = search.0.trim().to_lowercase();

    // Track which component entities are visible
    let mut visible_components: HashSet<Entity> = HashSet::new();

    // Filter individual component displays by name
    for (entity, comp_name) in &components {
        let matches = filter.is_empty() || comp_name.0.to_lowercase().contains(&filter);

        if let Ok(mut node) = node_query.get_mut(entity) {
            node.display = if matches {
                Display::Flex
            } else {
                Display::None
            };
        }

        if matches {
            visible_components.insert(entity);
        }
    }

    // Hide group sections where all children are hidden
    for (group_entity, children) in &groups {
        let has_visible_child = children
            .iter()
            .any(|child| visible_components.contains(&child));

        if let Ok(mut node) = node_query.get_mut(group_entity) {
            node.display = if filter.is_empty() || has_visible_child {
                Display::Flex
            } else {
                Display::None
            };
        }
    }
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
