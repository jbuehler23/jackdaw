use crate::EditorEntity;
use crate::commands::EditorCommand;
use crate::selection::{Selected, Selection};
use std::any::TypeId;
use std::collections::{BTreeMap, HashSet};

use super::InspectorDirty;

use bevy::{
    ecs::{
        archetype::Archetype,
        component::{ComponentId, Components},
        reflect::{AppTypeRegistry, ReflectComponent},
    },
    feathers::theme::ThemedText,
    prelude::*,
    ui_widgets::observe,
};
use jackdaw_feathers::text_edit::{self, TextEditProps, TextEditValue};
use jackdaw_feathers::tokens;

use super::{
    AddComponentButton, ComponentPicker, ComponentPickerEntry, ComponentPickerSearch,
    ComponentPickerSectionHeader, Inspector, ReflectEditorMeta,
};

/// Grouping key for sorting: custom categories first, then Game, then Bevy.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
enum GroupOrder {
    Custom(String),
    Game,
    Bevy,
}

struct ComponentInfo {
    short_name: String,
    module_path: String,
    category: String,
    description: String,
    type_id: TypeId,
    component_id: ComponentId,
    type_path_full: String,
}

/// Handle click on the "+" button to open the component picker.
pub(crate) fn on_add_component_button_click(
    event: On<jackdaw_feathers::button::ButtonClickEvent>,
    add_buttons: Query<&ChildOf, With<AddComponentButton>>,
    existing_pickers: Query<Entity, With<ComponentPicker>>,
    mut commands: Commands,
    selection: Res<Selection>,
    type_registry: Res<AppTypeRegistry>,
    components: &Components,
    entity_query: Query<&Archetype, (With<Selected>, Without<EditorEntity>)>,
    _inspector: Single<Entity, With<Inspector>>,
) {
    // Check if this click is on an AddComponentButton
    if add_buttons.get(event.entity).is_err() {
        return;
    }

    // Toggle: if picker already open, close it
    if let Some(picker) = existing_pickers.iter().next() {
        commands.entity(picker).despawn();
        return;
    }

    let Some(primary) = selection.primary() else {
        return;
    };
    let Ok(archetype) = entity_query.get(primary) else {
        return;
    };

    // Collect existing component TypeIds on the entity
    let existing_types: HashSet<TypeId> = archetype
        .iter_components()
        .filter_map(|cid| components.get_info(cid).and_then(|info| info.type_id()))
        .collect();

    let registry = type_registry.read();

    // Collect all registered components that have ReflectComponent + ReflectDefault
    let mut grouped: BTreeMap<GroupOrder, Vec<ComponentInfo>> = BTreeMap::new();
    for registration in registry.iter() {
        let type_id = registration.type_id();

        // Must have ReflectComponent and ReflectDefault
        if registration.data::<ReflectComponent>().is_none()
            || registration.data::<ReflectDefault>().is_none()
        {
            continue;
        }

        // Skip components already on the entity
        if existing_types.contains(&type_id) {
            continue;
        }

        // Skip editor-internal types
        let table = registration.type_info().type_path_table();
        let full_path = table.path();
        if full_path.starts_with("jackdaw") && !full_path.starts_with("jackdaw_avian_integration") {
            continue;
        }

        // Get component ID
        let Some(component_id) = components.get_id(type_id) else {
            continue;
        };

        let short_name = table.short_path().to_string();
        let module = table.module_path().unwrap_or("").to_string();

        // Read EditorMeta if present
        let (category, description) = if let Some(meta) = registration.data::<ReflectEditorMeta>() {
            (meta.category.to_string(), meta.description.to_string())
        } else {
            (String::new(), String::new())
        };

        // Determine group
        let group = if !category.is_empty() {
            GroupOrder::Custom(category.clone())
        } else if module.starts_with("bevy") {
            GroupOrder::Bevy
        } else {
            GroupOrder::Game
        };

        grouped.entry(group).or_default().push(ComponentInfo {
            short_name,
            module_path: module,
            category,
            description,
            type_id,
            component_id,
            type_path_full: full_path.to_string(),
        });
    }

    // Sort within each group alphabetically by short name
    for entries in grouped.values_mut() {
        entries.sort_by(|a, b| {
            a.short_name
                .to_lowercase()
                .cmp(&b.short_name.to_lowercase())
        });
    }

    // Spawn as a centered blocking dialog overlay
    // Backdrop absorbs all pointer events and dims the background
    let backdrop = commands
        .spawn((
            ComponentPicker,
            crate::EditorEntity,
            Interaction::default(),
            bevy::ui::FocusPolicy::Block,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(tokens::DIALOG_BACKDROP),
            GlobalZIndex(100),
            crate::BlocksCameraInput,
            // Click on backdrop to close
            observe(
                |_: On<Pointer<Click>>,
                 mut commands: Commands,
                 pickers: Query<Entity, With<ComponentPicker>>| {
                    for picker in &pickers {
                        commands.entity(picker).despawn();
                    }
                },
            ),
        ))
        .id();

    let picker = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Px(tokens::DIALOG_WIDTH),
                max_height: Val::Px(tokens::DIALOG_MAX_HEIGHT),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_LG)),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::PANEL_BORDER),
            BoxShadow(vec![ShadowStyle {
                x_offset: Val::ZERO,
                y_offset: Val::Px(4.0),
                blur_radius: Val::Px(16.0),
                spread_radius: Val::ZERO,
                color: tokens::SHADOW_COLOR,
            }]),
            ChildOf(backdrop),
        ))
        .id();

    // Dialog title bar
    commands.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            width: Val::Percent(100.0),
            padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_SM)),
            border_radius: BorderRadius::top(Val::Px(tokens::BORDER_RADIUS_LG)),
            ..Default::default()
        },
        BackgroundColor(tokens::COMPONENT_CARD_HEADER_BG),
        ChildOf(picker),
        children![(
            Text::new("Add Component"),
            TextFont {
                font_size: tokens::FONT_MD,
                weight: FontWeight::MEDIUM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
        )],
    ));

    // Search input
    commands.spawn((
        ComponentPickerSearch,
        text_edit::text_edit(
            TextEditProps::default()
                .with_placeholder("Search components...")
                .auto_focus()
                .allow_empty(),
        ),
        ChildOf(picker),
    ));

    // Scrollable list
    let list = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                overflow: Overflow::scroll_y(),
                flex_grow: 1.0,
                min_height: Val::Px(0.0),
                ..Default::default()
            },
            ChildOf(picker),
        ))
        .id();

    let source_entity = primary;

    for (group, entries) in &grouped {
        let group_name = match group {
            GroupOrder::Custom(name) => name.clone(),
            GroupOrder::Game => "Game".to_string(),
            GroupOrder::Bevy => "Bevy".to_string(),
        };

        let count = entries.len();

        // Section header
        let header_id = commands
            .spawn((
                ComponentPickerSectionHeader {
                    group: group_name.clone(),
                },
                Node {
                    padding: UiRect::new(
                        Val::Px(tokens::SPACING_LG),
                        Val::Px(tokens::SPACING_LG),
                        Val::Px(tokens::SPACING_MD),
                        Val::Px(tokens::SPACING_XS),
                    ),
                    width: Val::Percent(100.0),
                    border: UiRect::bottom(Val::Px(1.0)),
                    ..Default::default()
                },
                BorderColor::all(tokens::BORDER_SUBTLE),
                ChildOf(list),
            ))
            .id();

        commands.spawn((
            Text::new(format!("{group_name} ({count})")),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(header_id),
        ));

        // Component entries
        for info in entries {
            let type_id = info.type_id;
            let component_id = info.component_id;
            let short_name = info.short_name.clone();
            let category = info.category.clone();
            let description = info.description.clone();
            let module_path = info.module_path.clone();
            let type_path_full = info.type_path_full.clone();

            // Subtitle: description takes priority, otherwise module path
            let subtitle = if !description.is_empty() {
                description.clone()
            } else {
                module_path.clone()
            };

            let entry_id = commands
                .spawn((
                    ComponentPickerEntry {
                        short_name: short_name.clone(),
                        module_path: module_path.clone(),
                        category: category.clone(),
                        description: description.clone(),
                    },
                    Node {
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::axes(
                            Val::Px(tokens::SPACING_LG),
                            Val::Px(tokens::SPACING_SM),
                        ),
                        width: Val::Percent(100.0),
                        ..Default::default()
                    },
                    BackgroundColor(Color::NONE),
                    ChildOf(list),
                    observe({
                        let type_path_full = type_path_full.clone();
                        move |mut click: On<Pointer<Click>>, mut commands: Commands| {
                            click.propagate(false); // Don't let click through to backdrop
                            let tp = type_path_full.clone();
                            commands.queue(move |world: &mut World| {
                                let cmd = crate::commands::AddComponent::new(
                                    source_entity,
                                    type_id,
                                    component_id,
                                    tp,
                                );
                                let mut cmd = Box::new(cmd);
                                cmd.execute(world);
                                let mut history =
                                    world.resource_mut::<crate::commands::CommandHistory>();
                                history.undo_stack.push(cmd);
                                history.redo_stack.clear();

                                // Signal the inspector to rebuild
                                world.entity_mut(source_entity).insert(InspectorDirty);

                                // Close the picker dialog
                                let pickers: Vec<Entity> = world
                                    .query_filtered::<Entity, With<ComponentPicker>>()
                                    .iter(world)
                                    .collect();
                                for picker in pickers {
                                    if let Ok(ec) = world.get_entity_mut(picker) {
                                        ec.despawn();
                                    }
                                }
                            });
                        }
                    }),
                    observe(
                        move |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
                            if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                                bg.0 = tokens::HOVER_BG;
                            }
                        },
                    ),
                    observe(
                        move |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
                            if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                                bg.0 = Color::NONE;
                            }
                        },
                    ),
                ))
                .id();

            // Line 1: short name + optional category badge
            let row = commands
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        width: Val::Percent(100.0),
                        ..Default::default()
                    },
                    ChildOf(entry_id),
                ))
                .id();

            commands.spawn((
                Text::new(short_name),
                TextFont {
                    font_size: tokens::FONT_MD,
                    ..Default::default()
                },
                ThemedText,
                ChildOf(row),
            ));

            if !category.is_empty() {
                commands.spawn((
                    Text::new(category),
                    TextFont {
                        font_size: tokens::FONT_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                    ChildOf(row),
                ));
            }

            // Line 2: subtitle (description or module path)
            if !subtitle.is_empty() {
                commands.spawn((
                    Text::new(subtitle),
                    TextFont {
                        font_size: tokens::FONT_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                    ChildOf(entry_id),
                ));
            }
        }
    }
}

/// Filter the component picker list based on search input.
pub(crate) fn filter_component_picker(
    search_query: Query<&TextEditValue, (With<ComponentPickerSearch>, Changed<TextEditValue>)>,
    entries: Query<(Entity, &ComponentPickerEntry)>,
    headers: Query<(Entity, &ComponentPickerSectionHeader)>,
    mut node_query: Query<&mut Node>,
) {
    let Ok(search) = search_query.single() else {
        return;
    };
    let filter = search.0.trim().to_lowercase();

    // Track which groups have visible entries
    let mut visible_groups: HashSet<String> = HashSet::new();

    for (entity, entry) in &entries {
        let matches = filter.is_empty()
            || entry.short_name.to_lowercase().contains(&filter)
            || entry.module_path.to_lowercase().contains(&filter)
            || entry.category.to_lowercase().contains(&filter)
            || entry.description.to_lowercase().contains(&filter);

        if let Ok(mut node) = node_query.get_mut(entity) {
            node.display = if matches {
                Display::Flex
            } else {
                Display::None
            };
        }

        if matches {
            // Determine which group this entry belongs to
            if !entry.category.is_empty() {
                visible_groups.insert(entry.category.clone());
            } else if entry.module_path.starts_with("bevy") {
                visible_groups.insert("Bevy".to_string());
            } else {
                visible_groups.insert("Game".to_string());
            }
        }
    }

    // Show/hide section headers based on whether their group has visible entries
    for (entity, header) in &headers {
        if let Ok(mut node) = node_query.get_mut(entity) {
            node.display = if filter.is_empty() || visible_groups.contains(&header.group) {
                Display::Flex
            } else {
                Display::None
            };
        }
    }
}

/// Close the component picker dialog when ESC is pressed.
pub(crate) fn close_picker_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    pickers: Query<Entity, With<ComponentPicker>>,
    mut commands: Commands,
) {
    if keys.just_pressed(KeyCode::Escape) && !pickers.is_empty() {
        for picker in &pickers {
            commands.entity(picker).despawn();
        }
    }
}
