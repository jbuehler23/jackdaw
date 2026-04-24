//! Unified Add Entity picker, shared by the toolbar Add menu and the
//! scene-tree Add Entity button. Sources items from built-in templates
//! plus extension-contributed `RegisteredMenuEntry` rows under
//! `menu == "Add"`.

use bevy::feathers::theme::ThemedText;
use bevy::prelude::*;
use bevy::ui_widgets::observe;
use jackdaw_feathers::text_edit::{self, TextEditProps, TextEditValue};
use jackdaw_feathers::tokens;

use std::collections::HashSet;

/// Marker for the scene-tree Add Entity button.
#[derive(Component)]
pub struct AddEntityButton;

/// Backdrop and panel root for the picker. Despawning it tears down
/// the whole dialog.
#[derive(Component)]
pub struct AddEntityPicker;

#[derive(Component)]
pub struct AddEntityPickerSearch;

#[derive(Component)]
pub struct AddEntityPickerEntry {
    pub label: String,
    pub category: String,
}

#[derive(Component)]
pub struct AddEntityPickerSectionHeader {
    pub category: String,
}

/// Built-in Add items grouped by category. Order here is the order in
/// the picker and in the toolbar Add menu.
fn builtin_groups() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        (
            "Shapes",
            vec![
                ("op:entity.add.cube", "Cube"),
                ("op:entity.add.sphere", "Sphere"),
            ],
        ),
        (
            "Lights",
            vec![
                ("op:entity.add.point_light", "Point Light"),
                ("op:entity.add.directional_light", "Directional Light"),
                ("op:entity.add.spot_light", "Spot Light"),
            ],
        ),
        (
            "Cameras & Entities",
            vec![
                ("op:entity.add.camera", "Camera"),
                ("op:entity.add.empty", "Empty"),
            ],
        ),
        (
            "Regions",
            vec![
                ("op:entity.add.navmesh", "Navmesh Region"),
                ("op:entity.add.terrain", "Terrain"),
            ],
        ),
        ("Prefabs", vec![("op:entity.add.prefab", "Prefab...")]),
    ]
}

/// One row in the Add menu or Add Entity picker. `action` is handled
/// by `handle_menu_action` (e.g. `"add.cube"` or
/// `"op:viewable_camera.place"`).
#[derive(Clone)]
pub struct AddMenuItem {
    pub action: String,
    pub label: String,
    pub category: String,
}

/// Shared source of truth for Add menu contents, consumed by both the
/// toolbar Add menu and the scene-tree Add Entity picker.
pub fn collect_add_menu_items(world: &mut World) -> Vec<AddMenuItem> {
    let mut items: Vec<AddMenuItem> = builtin_groups()
        .into_iter()
        .flat_map(|(category, entries)| {
            entries.into_iter().map(move |(action, label)| AddMenuItem {
                action: action.to_string(),
                label: label.to_string(),
                category: category.to_string(),
            })
        })
        .collect();

    // Extension items grouped by owning extension so entries cluster by
    // author in the picker.
    let mut q = world.query::<(
        &jackdaw_api_internal::lifecycle::RegisteredMenuEntry,
        Option<&ChildOf>,
    )>();
    let mut ext_entries: Vec<(Entity, String, String)> = Vec::new();
    for (entry, parent) in q.iter(world) {
        if entry.menu != "Add" {
            continue;
        }
        let ext_entity = parent.map(ChildOf::parent).unwrap_or(Entity::PLACEHOLDER);
        ext_entries.push((
            ext_entity,
            format!("op:{}", entry.operator_id),
            entry.label.clone(),
        ));
    }
    for (ext_entity, action, label) in ext_entries {
        let category = world
            .get::<jackdaw_api_internal::lifecycle::Extension>(ext_entity)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| "Extensions".to_string());
        items.push(AddMenuItem {
            action,
            label,
            category,
        });
    }

    items
}

/// Open the Add Entity picker as a centered blocking dialog. Styled
/// to match the Add Component dialog. Toggles off if already open.
pub fn open_add_entity_picker(
    world: &mut World,
    entity_pickers: &mut QueryState<Entity, With<AddEntityPicker>>,
) {
    let existing: Vec<Entity> = entity_pickers.iter(world).collect();
    if !existing.is_empty() {
        for e in existing {
            if let Ok(ec) = world.get_entity_mut(e) {
                ec.despawn();
            }
        }
        return;
    }

    let items = collect_add_menu_items(world);

    // Group by category, preserving insertion order so built-in groups
    // render before any extension groups.
    let mut grouped: Vec<(String, Vec<AddMenuItem>)> = Vec::new();
    for item in items {
        if let Some((_, entries)) = grouped.iter_mut().find(|(cat, _)| *cat == item.category) {
            entries.push(item);
        } else {
            grouped.push((item.category.clone(), vec![item]));
        }
    }

    let mut commands = world.commands();

    let backdrop = commands
        .spawn((
            AddEntityPicker,
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
            observe(
                |_: On<Pointer<Click>>,
                 mut commands: Commands,
                 pickers: Query<Entity, With<AddEntityPicker>>| {
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
            // Stop clicks inside the panel from closing the dialog.
            observe(|mut click: On<Pointer<Click>>| {
                click.propagate(false);
            }),
        ))
        .id();

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
            Text::new("Add Entity"),
            TextFont {
                font_size: tokens::FONT_MD,
                weight: FontWeight::MEDIUM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
        )],
    ));

    commands.spawn((
        AddEntityPickerSearch,
        text_edit::text_edit(
            TextEditProps::default()
                .with_placeholder("Search entities...")
                .auto_focus()
                .allow_empty(),
        ),
        ChildOf(picker),
    ));

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

    for (category, entries) in &grouped {
        let count = entries.len();

        let header_id = commands
            .spawn((
                AddEntityPickerSectionHeader {
                    category: category.clone(),
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
            Text::new(format!("{category} ({count})")),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(header_id),
        ));

        for item in entries {
            let label = item.label.clone();
            let category = item.category.clone();
            let action = item.action.clone();

            let entry_id = commands
                .spawn((
                    AddEntityPickerEntry {
                        label: label.clone(),
                        category: category.clone(),
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
                        let action = action.clone();
                        move |mut click: On<Pointer<Click>>, mut commands: Commands| {
                            click.propagate(false);
                            // Route through the menu-bar dispatch path
                            // so the toolbar Add menu and this picker
                            // share one code path.
                            commands.trigger(jackdaw_widgets::menu_bar::MenuAction {
                                action: action.clone(),
                            });
                            fn despawn_pickers(
                                mut commands: Commands,
                                pickers: Query<Entity, With<AddEntityPicker>>,
                            ) {
                                for picker in &pickers {
                                    commands.entity(picker).try_despawn();
                                }
                            }
                            commands.run_system_cached(despawn_pickers);
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
                Text::new(label),
                TextFont {
                    font_size: tokens::FONT_MD,
                    ..Default::default()
                },
                ThemedText,
                ChildOf(row),
            ));

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
    }
}

/// Filter picker entries by the search input.
pub fn filter_add_entity_picker(
    search_query: Query<&TextEditValue, (With<AddEntityPickerSearch>, Changed<TextEditValue>)>,
    entries: Query<(Entity, &AddEntityPickerEntry)>,
    headers: Query<(Entity, &AddEntityPickerSectionHeader)>,
    mut node_query: Query<&mut Node>,
) {
    let Ok(search) = search_query.single() else {
        return;
    };
    let filter = search.0.trim().to_lowercase();

    let mut visible_categories: HashSet<String> = HashSet::new();

    for (entity, entry) in &entries {
        let matches = filter.is_empty()
            || entry.label.to_lowercase().contains(&filter)
            || entry.category.to_lowercase().contains(&filter);

        if let Ok(mut node) = node_query.get_mut(entity) {
            node.display = if matches {
                Display::Flex
            } else {
                Display::None
            };
        }

        if matches {
            visible_categories.insert(entry.category.clone());
        }
    }

    for (entity, header) in &headers {
        if let Ok(mut node) = node_query.get_mut(entity) {
            node.display = if filter.is_empty() || visible_categories.contains(&header.category) {
                Display::Flex
            } else {
                Display::None
            };
        }
    }
}

/// Close on Escape.
pub fn close_add_entity_picker_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    pickers: Query<Entity, With<AddEntityPicker>>,
    mut commands: Commands,
) {
    if keys.just_pressed(KeyCode::Escape) && !pickers.is_empty() {
        for picker in &pickers {
            commands.entity(picker).despawn();
        }
    }
}
