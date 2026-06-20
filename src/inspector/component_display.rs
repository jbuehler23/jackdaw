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
use jackdaw_localization::LocalizedText;
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
    ComponentDisplay, ComponentDisplayBody, ComponentDisplayTypePath, ComponentName,
    ComponentPicker, Inspector, InspectorDirty, InspectorGroupSection, InspectorSearch,
    InspectorTarget, ReflectDisplayable, brush_display, category_strip::ActiveInspectorCategory,
    component_tooltip::ReflectedTypeTooltip, custom_props_display, extract_module_group,
    material_display, modifier_display, reflect_fields,
};
use crate::inspector::prefab_field_dots::{PrefabInstanceCtx, inspector_type_paths_for};
use crate::prefab::PrefabAstCache;
use bevy::picking::hover::Hovered;
use jackdaw_jsn::SceneJsnAst;

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
    collapse_state: Res<super::InspectorCollapseState>,
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
            &collapse_state,
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
    collapse_state: &super::InspectorCollapseState,
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
                    && !full_path.starts_with("jackdaw_geometry")
                    && !full_path.starts_with("jackdaw::reference_image")
                    && !full_path.starts_with("jackdaw_avian_integration")
                    && !full_path.starts_with("jackdaw_animation")
                    && !full_path.starts_with("jackdaw_multiplayer")
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
                        || full_path.starts_with("jackdaw_avian_integration")
                        || full_path.starts_with("jackdaw_multiplayer"));
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
                && !name.starts_with("jackdaw_geometry")
                && !name.starts_with("jackdaw::reference_image")
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

    for (name, _module_group, component_id, type_path) in &comp_list {
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

        // ModifierStack gets its own top-level cards (one per modifier entry)
        // rather than a single generic wrapper. Detect it here, before creating
        // the generic card, and emit per-modifier cards directly under the
        // inspector scroll body.
        if *type_path == *<jackdaw_geometry::ModifierStack as bevy::reflect::TypePath>::type_path()
        {
            let type_id = components
                .get_info(component_id)
                .and_then(ComponentInfo::type_id);
            if let Some(type_id) = type_id
                && let Some(registration) = registry.get(type_id)
                && let Some(reflect_component) = registration.data::<ReflectComponent>()
                && let Some(reflected) = reflect_component.reflect(entity_ref)
                && let Some(stack) = reflected.downcast_ref::<jackdaw_geometry::ModifierStack>()
            {
                modifier_display::spawn_modifier_display(
                    commands,
                    inspector_entity,
                    source_entity,
                    stack,
                    names,
                    type_registry,
                    &editor_font.0,
                    &icon_font.0,
                    collapse_state,
                );
            }
            continue;
        }

        // MeshMaterial3d<StandardMaterial> gets four dedicated material cards
        // (Preview, Surface, Textures, Settings) rather than a single generic
        // wrapper. Skip the generic card and inject the four cards directly.
        if *type_path == BRUSH_MATERIAL_TYPE_PATH {
            material_display::inject_material_cards(
                commands,
                source_entity,
                inspector_entity,
                &icon_font.0,
                collapse_state,
            );
            continue;
        }

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
                collapse_state,
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

            // Priority 2: CustomProperties, specialized property editor
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
            LocalizedText::new("read-only"),
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

    // If the selected entity is a brush, inject the four material cards into the
    // Material tab. The brush entity itself carries no MeshMaterial3d; its face
    // data carries the handles. Shells are spawned synchronously (same flush as
    // every other card) so the "material" category is present on the rebuild
    // frame before `resolve_active_on_rebuild` runs. Body fills are deferred.
    if entity_ref.contains::<crate::brush::Brush>() {
        material_display::inject_material_cards(
            commands,
            source_entity,
            inspector_entity,
            &icon_font.0,
            collapse_state,
        );
    }
}

/// The type path used to route the brush material card to the Material inspector tab.
/// Also the `ComponentDisplayTypePath` of the entity-bound `MeshMaterial3d` card,
/// so a targeted refresh keyed on this string finds both material card variants.
pub(crate) const BRUSH_MATERIAL_TYPE_PATH: &str =
    "bevy_pbr::mesh_material::MeshMaterial3d<bevy_pbr::pbr_material::StandardMaterial>";

pub(crate) fn remove_component_displays(
    _: On<Remove, Selected>,
    mut commands: Commands,
    inspectors: Query<(Entity, Option<&Children>), With<Inspector>>,
    displays: Query<Entity, Or<(With<ComponentDisplay>, With<ComponentPicker>)>>,
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
    displays: Query<Entity, Or<(With<ComponentDisplay>, With<ComponentPicker>)>>,
    materials: Res<Assets<StandardMaterial>>,
    ast: Res<jackdaw_jsn::SceneJsnAst>,
    prefab_cache: Res<PrefabAstCache>,
    child_of_query: Query<&bevy::ecs::hierarchy::ChildOf>,
    isa_query: Query<&crate::prefab::IsA>,
    collapse_state: Res<super::InspectorCollapseState>,
) {
    // Multi-instance: rebuild every Inspector tab in lockstep. Each
    // inspector entity carries its own `InspectorTarget`; the dirty
    // signal originates from `InspectorDirty` on the source entity
    // and applies to every inspector watching that source.
    let mut clear_dirty_for: Option<Entity> = None;
    for (inspector_entity, target, children) in &inspectors {
        let mut source_entity = target.0;

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

        // Rebuild this inspector's contents. If the monitored target is gone
        // (despawned/respawned by CSG, undo, or prefab install), fall back to
        // the live primary selection and re-point this inspector, rather than
        // despawning the cards and rebuilding an empty panel.
        let (archetype, entity_ref) = match entity_query.get(source_entity) {
            Ok(found) => found,
            Err(_) => {
                let Some(primary) = selection.primary() else {
                    continue;
                };
                let Ok(found) = entity_query.get(primary) else {
                    continue;
                };
                commands.entity(inspector_entity).insert((
                    InspectorTarget(primary),
                    Monitor(primary),
                    NotifyAdded::<InspectorDirty>::default(),
                ));
                source_entity = primary;
                found
            }
        };
        if clear_dirty_for.is_none() {
            clear_dirty_for = Some(source_entity);
        }
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
            &collapse_state,
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
    /// Per-session collapsed-state map; used to restore the card's
    /// open/closed state across inspector rebuilds.
    pub collapse_state: &'a super::InspectorCollapseState,
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
        collapse_state,
    } = spec;
    let font = icon_font.clone();
    let body_font = editor_font.clone();

    let collapsed = collapse_state.collapsed(name);
    let body_display = if collapsed {
        Display::None
    } else {
        Display::Flex
    };

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
                display: body_display,
                ..Default::default()
            },
        ))
        .id();

    let section_entity = commands
        .spawn((
            ComponentDisplay,
            ComponentName(name.to_string()),
            ComponentDisplayTypePath(type_path.to_string()),
            CollapsibleSection { collapsed },
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

/// Filter inspector component cards based on both the active category and the
/// search input. A card is visible only when its category matches the active
/// category AND its short name passes the search text predicate (either the
/// search field is empty or the name contains the filter string).
///
/// The system re-runs whenever the search text changes OR the active category
/// changes. Group-section visibility follows: a group hides when all of its
/// cards are hidden.
pub(crate) fn filter_inspector_components(
    search_query: Query<&TextEditValue, With<InspectorSearch>>,
    active: Res<ActiveInspectorCategory>,
    registry: Res<jackdaw_api_internal::inspector::InspectorRegistry>,
    components: Query<(Entity, &ComponentName, &ComponentDisplayTypePath), With<ComponentDisplay>>,
    groups: Query<(Entity, &Children), With<InspectorGroupSection>>,
    mut node_query: Query<&mut Node>,
    changed_search: Query<(), (With<InspectorSearch>, Changed<TextEditValue>)>,
) {
    // Re-run only when the search text or the active category changed.
    // Running every frame is cheap but this avoids unnecessary Node mutations.
    if changed_search.is_empty() && !active.is_changed() {
        return;
    }

    let filter = search_query
        .single()
        .map(|v| v.0.trim().to_lowercase())
        .unwrap_or_default();

    let active_cat = active.0.as_ref();

    // Track which component entities are visible.
    let mut visible_components: HashSet<Entity> = HashSet::new();

    for (entity, comp_name, type_path) in &components {
        let category_ok = registry.category_for(&type_path.0) == active_cat;
        let search_ok = filter.is_empty() || comp_name.0.to_lowercase().contains(&filter);
        let visible = category_ok && search_ok;

        if let Ok(mut node) = node_query.get_mut(entity) {
            node.display = if visible {
                Display::Flex
            } else {
                Display::None
            };
        }

        if visible {
            visible_components.insert(entity);
        }
    }

    // Hide group sections where all children are hidden.
    for (group_entity, children) in &groups {
        let has_visible_child = children
            .iter()
            .any(|child| visible_components.contains(&child));

        if let Ok(mut node) = node_query.get_mut(group_entity) {
            node.display = if has_visible_child {
                Display::Flex
            } else {
                Display::None
            };
        }
    }
}
