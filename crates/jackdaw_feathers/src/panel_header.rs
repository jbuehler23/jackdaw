use bevy_app::prelude::*;
use bevy_color::prelude::*;
use bevy_ecs::prelude::*;
use bevy_text::prelude::*;
use bevy_ui::prelude::*;
use lucide_icons::Icon;

use crate::{icons::IconFont, tokens};

/// Tracks which tab index is active on a panel.
#[derive(Component, Default)]
pub struct PanelActiveTab(pub usize);

/// Marker on individual tab nodes. Stores the tab's index.
#[derive(Component)]
pub struct PanelTab(pub usize);

/// Marker on tab content containers. Stores the tab's index.
#[derive(Component)]
pub struct PanelTabContent(pub usize);

/// Marker on the tab bar container.
#[derive(Component)]
pub struct PanelTabBarMarker;

/// Definition of a tab to create.
pub struct TabDef {
    pub label: String,
    pub icon: Option<Icon>,
    pub active: bool,
}

impl TabDef {
    pub fn new(label: impl Into<String>, active: bool) -> Self {
        Self {
            label: label.into(),
            icon: None,
            active,
        }
    }

    /// Prefix the tab label with a Lucide icon glyph.
    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// Plugin to register tab-related systems.
pub fn plugin(app: &mut App) {
    app.add_systems(Update, (setup_panel_tab_bars, handle_tab_clicks));
}

/// A simple panel header bar with a title label (single-tab convenience).
pub fn panel_header(title: &str) -> impl Bundle {
    (
        PanelActiveTab(0),
        PanelTabBarMarker,
        Node {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            width: Val::Percent(100.0),
            height: Val::Px(tokens::PANEL_TAB_HEIGHT),
            padding: UiRect::new(
                Val::Px(tokens::SPACING_MD),
                Val::Px(tokens::SPACING_MD),
                Val::Px(1.0),
                Val::ZERO,
            ),
            flex_shrink: 0.0,
            border: UiRect {
                left: Val::Px(1.0),
                right: Val::Px(1.0),
                top: Val::Px(1.0),
                bottom: Val::ZERO,
            },
            border_radius: BorderRadius::top(Val::Px(6.0)),
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_HEADER_BG),
        BorderColor::all(tokens::PANEL_BORDER),
        children![(
            PanelTab(0),
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(8.0)),
                height: Val::Percent(100.0),
                border: UiRect {
                    top: Val::Px(2.0),
                    ..Default::default()
                },
                border_radius: BorderRadius::top(Val::Px(2.0)),
                ..Default::default()
            },
            BackgroundColor(tokens::TAB_ACTIVE_BG),
            BorderColor::all(tokens::TAB_ACTIVE_BORDER),
            children![(
                Text::new(title),
                TextFont {
                    font_size: tokens::TEXT_SIZE_LG,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
        )],
    )
}

/// A panel tab bar with multiple tabs, optional plus button, and grip handle.
///
/// Returns a bundle that should be placed as the first child of a panel container.
/// Use `PanelTabContent(idx)` on sibling content containers to enable switching.
pub fn panel_tab_bar(tabs: &[TabDef], show_grip: bool) -> impl Bundle + use<> {
    let active_idx = tabs.iter().position(|t| t.active).unwrap_or(0);

    let tab_defs: Vec<(String, Option<Icon>, bool, usize)> = tabs
        .iter()
        .enumerate()
        .map(|(i, t)| (t.label.clone(), t.icon, t.active, i))
        .collect();

    (
        PanelActiveTab(active_idx),
        PanelTabBarMarker,
        Node {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            width: Val::Percent(100.0),
            height: Val::Px(tokens::PANEL_TAB_HEIGHT),
            padding: UiRect::new(
                Val::Px(tokens::SPACING_MD),
                Val::Px(tokens::SPACING_MD),
                Val::Px(1.0),
                Val::ZERO,
            ),
            flex_shrink: 0.0,
            border: UiRect {
                left: Val::Px(1.0),
                right: Val::Px(1.0),
                top: Val::Px(1.0),
                bottom: Val::ZERO,
            },
            border_radius: BorderRadius::top(Val::Px(6.0)),
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_HEADER_BG),
        BorderColor::all(tokens::PANEL_BORDER),
        PanelTabBarSetup {
            tabs: tab_defs,
            show_grip,
        },
    )
}

/// Temporary component consumed by the setup system to populate tab children.
#[derive(Component)]
struct PanelTabBarSetup {
    tabs: Vec<(String, Option<Icon>, bool, usize)>,
    show_grip: bool,
}

/// System that runs once to populate tab bars that have a `PanelTabBarSetup`.
fn setup_panel_tab_bars(
    mut commands: Commands,
    query: Query<(Entity, &PanelTabBarSetup), Added<PanelTabBarSetup>>,
    icon_font: Option<Res<IconFont>>,
) {
    for (entity, setup) in query.iter() {
        let font = icon_font.as_ref().map(|f| f.0.clone());

        // Left side: tab row
        let mut tab_row_children = Vec::new();

        for (label, icon, active, idx) in &setup.tabs {
            let tab_entity = commands.spawn(tab_shell(*active, *idx)).id();

            // Optional icon glyph, only rendered if both an icon and
            // the Lucide font are present.
            if let (Some(glyph), Some(font_handle)) = (icon, font.clone()) {
                let icon_entity = commands
                    .spawn((
                        Text::new(String::from(glyph.unicode())),
                        TextFont {
                            font: font_handle,
                            font_size: tokens::ICON_SM,
                            ..Default::default()
                        },
                        TextColor(if *active {
                            tokens::TEXT_PRIMARY
                        } else {
                            tokens::TAB_INACTIVE_TEXT
                        }),
                        ChildOf(tab_entity),
                    ))
                    .id();
                let _ = icon_entity;
            }

            commands.spawn((
                Text::new(label.clone()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_LG,
                    ..Default::default()
                },
                TextColor(if *active {
                    tokens::TEXT_PRIMARY
                } else {
                    tokens::TAB_INACTIVE_TEXT
                }),
                ChildOf(tab_entity),
            ));

            tab_row_children.push(tab_entity);
        }

        let tab_row = commands
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                height: Val::Percent(100.0),
                ..Default::default()
            })
            .add_children(&tab_row_children)
            .id();

        // Right side: plus button + grip handle
        let mut right_children = Vec::new();

        if let Some(ref font_handle) = font {
            // Plus button
            let plus = commands
                .spawn((
                    Interaction::default(),
                    Node {
                        width: Val::Px(15.0),
                        height: Val::Px(15.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..Default::default()
                    },
                    children![(
                        Text::new(String::from(Icon::Plus.unicode())),
                        TextFont {
                            font: font_handle.clone(),
                            font_size: tokens::ICON_SM,
                            ..Default::default()
                        },
                        TextColor(tokens::TAB_INACTIVE_TEXT),
                    )],
                ))
                .id();
            right_children.push(plus);

            if setup.show_grip {
                let grip = commands
                    .spawn((
                        Node {
                            width: Val::Px(15.0),
                            height: Val::Px(15.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..Default::default()
                        },
                        children![(
                            Text::new(String::from(Icon::GripVertical.unicode())),
                            TextFont {
                                font: font_handle.clone(),
                                font_size: tokens::ICON_SM,
                                ..Default::default()
                            },
                            TextColor(tokens::TAB_INACTIVE_TEXT),
                        )],
                    ))
                    .id();
                right_children.push(grip);
            }
        }

        let right_row = commands
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                ..Default::default()
            })
            .add_children(&right_children)
            .id();

        crate::utils::attach_children_or_despawn(&mut commands, entity, &[tab_row, right_row]);

        // Remove the setup component
        commands.entity(entity).remove::<PanelTabBarSetup>();
    }
}

/// Spawn the outer tab container (background + border + click
/// target). Children (icon + label) are attached separately by
/// [`setup_panel_tab_bars`] because `children!` can't express a
/// conditional icon element.
fn tab_shell(active: bool, idx: usize) -> impl Bundle {
    let bg = if active {
        tokens::TAB_ACTIVE_BG
    } else {
        Color::NONE
    };

    let border_top = if active { Val::Px(2.0) } else { Val::ZERO };

    let border_color = if active {
        tokens::TAB_ACTIVE_BORDER
    } else {
        Color::NONE
    };

    (
        PanelTab(idx),
        Interaction::default(),
        Node {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            column_gap: Val::Px(tokens::SPACING_XS),
            padding: UiRect::horizontal(Val::Px(8.0)),
            height: Val::Percent(100.0),
            border: UiRect {
                top: border_top,
                ..Default::default()
            },
            border_radius: BorderRadius::top(Val::Px(2.0)),
            ..Default::default()
        },
        BackgroundColor(bg),
        BorderColor::all(border_color),
    )
}

/// System: detect tab clicks via `Interaction` and switch active tab + content.
fn handle_tab_clicks(
    tab_query: Query<(Entity, &PanelTab, &Interaction, &ChildOf), Changed<Interaction>>,
    mut tab_bar_query: Query<&mut PanelActiveTab>,
    all_tabs: Query<(Entity, &PanelTab, &ChildOf)>,
    mut bg_query: Query<&mut BackgroundColor>,
    mut border_query: Query<&mut BorderColor>,
    mut node_query: Query<&mut Node>,
    children_query: Query<&Children>,
    mut text_color_query: Query<&mut TextColor>,
    // Content containers
    content_query: Query<(Entity, &PanelTabContent)>,
    parent_query: Query<&ChildOf>,
) {
    for (_clicked_entity, clicked_tab, interaction, tab_parent) in tab_query.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }

        let new_idx = clicked_tab.0;

        // Navigate up: tab -> tab_row -> tab_bar
        let tab_row_entity = tab_parent.parent();
        let Ok(tab_row_parent) = parent_query.get(tab_row_entity) else {
            continue;
        };
        let tab_bar_entity = tab_row_parent.parent();

        let Ok(mut active_tab) = tab_bar_query.get_mut(tab_bar_entity) else {
            continue;
        };

        if active_tab.0 == new_idx {
            continue;
        }

        active_tab.0 = new_idx;

        // Update all sibling tabs' visual state
        for (tab_entity, tab, tab_child_of) in all_tabs.iter() {
            if tab_child_of.parent() != tab_row_entity {
                continue;
            }

            let is_active = tab.0 == new_idx;

            if let Ok(mut bg) = bg_query.get_mut(tab_entity) {
                bg.0 = if is_active {
                    tokens::TAB_ACTIVE_BG
                } else {
                    Color::NONE
                };
            }

            if let Ok(mut bc) = border_query.get_mut(tab_entity) {
                *bc = BorderColor::all(if is_active {
                    tokens::TAB_ACTIVE_BORDER
                } else {
                    Color::NONE
                });
            }

            if let Ok(mut node) = node_query.get_mut(tab_entity) {
                node.border.top = if is_active { Val::Px(2.0) } else { Val::ZERO };
            }

            // Update text color on children
            if let Ok(tab_children) = children_query.get(tab_entity) {
                for child in tab_children.iter() {
                    if let Ok(mut tc) = text_color_query.get_mut(child) {
                        tc.0 = if is_active {
                            tokens::TEXT_PRIMARY
                        } else {
                            tokens::TAB_INACTIVE_TEXT
                        };
                    }
                }
            }
        }

        // Toggle content visibility
        let Ok(panel_parent) = parent_query.get(tab_bar_entity) else {
            continue;
        };
        let panel_entity = panel_parent.parent();

        for (content_entity, content_tab) in content_query.iter() {
            let Ok(content_parent) = parent_query.get(content_entity) else {
                continue;
            };
            if content_parent.parent() != panel_entity {
                continue;
            }

            if let Ok(mut node) = node_query.get_mut(content_entity) {
                node.display = if content_tab.0 == new_idx {
                    Display::Flex
                } else {
                    Display::None
                };
            }
        }
    }
}
