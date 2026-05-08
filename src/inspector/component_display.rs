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

use jackdaw_runtime::EditorCategory;

use super::{
    AddComponentButton, ComponentDisplay, ComponentDisplayBody, ComponentName, ComponentPicker,
    Inspector, InspectorDirty, InspectorGroupSection, InspectorSearch, InspectorTarget,
    ReflectDisplayable, brush_display, component_tooltip::ReflectedTypeTooltip,
    custom_props_display, extract_module_group, material_display, reflect_fields,
};
use bevy::picking::hover::Hovered;

pub(crate) fn add_component_displays(
    _: On<Add, Selected>,
    mut commands: Commands,
    components: &Components,
    type_registry: Res<AppTypeRegistry>,
    selection: Res<Selection>,
    entity_query: Query<(&Archetype, EntityRef), (With<Selected>, Without<EditorEntity>)>,
    inspector: Single<Entity, With<Inspector>>,
    names: Query<&Name>,
    icon_font: Res<IconFont>,
    editor_font: Res<EditorFont>,
    materials: Res<Assets<StandardMaterial>>,
    ast: Res<jackdaw_jsn::SceneJsnAst>,
) {
    let Some(primary) = selection.primary() else {
        return;
    };
    let Ok((archetype, entity_ref)) = entity_query.get(primary) else {
        return;
    };

    let source_entity = entity_ref.entity();
    let sel_count = selection.entities.len();

    // Collect AST-tracked component type paths
    let jsn_type_paths: HashSet<String> = ast
        .node_for_entity(source_entity)
        .map(|node| node.components.keys().cloned().collect())
        .unwrap_or_default();

    build_inspector_displays(
        &mut commands,
        components,
        &type_registry,
        source_entity,
        archetype,
        entity_ref,
        *inspector,
        sel_count,
        &names,
        &icon_font,
        &editor_font,
        false,
        &materials,
        &jsn_type_paths,
    );

    // Set up monitoring: watch the selected entity for InspectorDirty
    commands.entity(*inspector).insert((
        InspectorTarget(primary),
        Monitor(primary),
        NotifyAdded::<InspectorDirty>::default(),
    ));
}

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
        let is_overridden = baseline.as_ref().is_some_and(|bl| {
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

        let (display_entity, body_entity) = spawn_component_display(
            commands,
            ComponentDisplaySpec {
                name,
                type_path,
                entity: source_entity,
                component: Some(component_id),
                is_overridden,
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
    inspector: Single<(Entity, Option<&Children>), With<Inspector>>,
    displays: Query<
        Entity,
        Or<(
            With<ComponentDisplay>,
            With<AddComponentButton>,
            With<ComponentPicker>,
        )>,
    >,
) {
    let (entity, children) = inspector.into_inner();

    // Clean up monitoring components
    commands
        .entity(entity)
        .remove::<(InspectorTarget, Monitor, NotifyAdded<InspectorDirty>)>();

    let Some(children) = children else {
        return;
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

/// Handles `Addition<InspectorDirty>` on the Inspector entity: despawn existing
/// displays and rebuild from the monitored source entity.
pub(crate) fn on_inspector_dirty(
    _: On<Addition<InspectorDirty>>,
    mut commands: Commands,
    components: &Components,
    type_registry: Res<AppTypeRegistry>,
    inspector: Single<(Entity, &InspectorTarget, Option<&Children>), With<Inspector>>,
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
) {
    let (inspector_entity, target, children) = inspector.into_inner();
    let source_entity = target.0;

    // Collect the old display children, then queue a world-exclusive
    // closure that despawns them synchronously and strips
    // `InspectorDirty` from the source. Doing this in a single
    // queued closure (rather than piecemeal `commands.despawn`
    // calls) guarantees the cascade completes as one atomic unit
    // inside `Commands` flush; no lazy `setup_button` /
    // `setup_combobox` spawns from a previous rebuild can slip in
    // between entity despawns and leave orphaned UI children (the
    // source of the "Inherited" floating label + `ChildOf(...)
    // relates to an entity that does not exist` warnings).
    let old_children: Vec<Entity> = children
        .map(|c| displays.iter_many(c.collection()).collect())
        .unwrap_or_default();
    commands.queue(move |world: &mut World| {
        for child in old_children {
            if let Ok(ec) = world.get_entity_mut(child) {
                ec.despawn();
            }
        }
        if let Ok(mut ec) = world.get_entity_mut(source_entity) {
            ec.remove::<InspectorDirty>();
        }
    });

    // Rebuild
    let Ok((archetype, entity_ref)) = entity_query.get(source_entity) else {
        return;
    };
    let sel_count = selection.entities.len();

    let jsn_type_paths: HashSet<String> = ast
        .node_for_entity(source_entity)
        .map(|node| node.components.keys().cloned().collect())
        .unwrap_or_default();

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
    );
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
        // `Hovered + ButtonOperatorCall` are tooltip data sources for
        // the rich operator popover; the click observer below handles
        // dispatch because this is a raw `Text` spawn (not a feathers
        // button, so it doesn't fire `ButtonClickEvent`).
        if is_overridden {
            let revert_type_path = type_path_owned.clone();
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
                bevy::ui_widgets::observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                    commands
                        .operator(super::ops::ComponentRevertBaselineOp::ID)
                        .param("entity", entity_param)
                        .param("type_path", revert_type_path.clone())
                        .call();
                }),
            ));
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
