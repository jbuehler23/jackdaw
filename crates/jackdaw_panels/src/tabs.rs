use bevy::prelude::*;
use jackdaw_feathers::{icons::IconFont, tokens};
use lucide_icons::Icon;

use crate::area::{DockTab, DockTabBar};
use crate::reconcile::LeafBinding;
use crate::tree::{DockTree, TabId};

#[derive(Component)]
pub struct DockTabAddButton {
    pub area_entity: Entity,
}

#[derive(Component)]
pub struct DockTabGrip;

#[derive(Component)]
pub struct DockTabRow;

pub struct DockTabPlugin;

impl Plugin for DockTabPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (handle_dock_tab_clicks, show_close_on_hover))
            .add_observer(on_close_button_click);
    }
}

pub fn spawn_tab_bar_world(
    world: &mut World,
    area_entity: Entity,
    tabs: &[(TabId, String, String)],
) {
    let first_tab = tabs.first().map(|(id, _, _)| *id);

    let tab_bar = world
        .spawn((
            DockTabBar,
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
                ..default()
            },
            BackgroundColor(tokens::PANEL_HEADER_BG),
            BorderColor::all(tokens::PANEL_BORDER),
            ChildOf(area_entity),
        ))
        .id();

    let tab_row = world
        .spawn((
            DockTabRow,
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                height: Val::Percent(100.0),
                overflow: Overflow::scroll_x(),
                flex_shrink: 1.0,
                min_width: Val::Px(0.0),
                ..default()
            },
            ChildOf(tab_bar),
        ))
        .id();

    for (tab_id, window_id, label) in tabs {
        let is_active = Some(*tab_id) == first_tab;
        spawn_tab(world, tab_row, *tab_id, window_id, label, is_active);
    }

    let icon_font = world.get_resource::<IconFont>().map(|f| f.0.clone());

    let right_row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                flex_shrink: 0.0,
                ..default()
            },
            ChildOf(tab_bar),
        ))
        .id();

    if let Some(ref font_handle) = icon_font {
        world.spawn((
            DockTabAddButton { area_entity },
            Interaction::default(),
            Node {
                width: Val::Px(15.0),
                height: Val::Px(15.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            ChildOf(right_row),
            children![(
                Text::new(String::from(Icon::Plus.unicode())),
                TextFont {
                    font: font_handle.clone(),
                    font_size: tokens::ICON_SM,
                    ..default()
                },
                TextColor(tokens::TAB_INACTIVE_TEXT),
            )],
        ));

        world.spawn((
            DockTabGrip,
            Interaction::default(),
            Node {
                width: Val::Px(15.0),
                height: Val::Px(15.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            ChildOf(right_row),
            children![(
                Text::new(String::from(Icon::GripVertical.unicode())),
                TextFont {
                    font: font_handle.clone(),
                    font_size: tokens::ICON_SM,
                    ..default()
                },
                TextColor(tokens::TAB_INACTIVE_TEXT),
            )],
        ));
    }
}

pub fn spawn_tab_in_world(
    world: &mut World,
    tab_row: Entity,
    tab_id: TabId,
    window_id: &str,
    label: &str,
    is_active: bool,
) {
    spawn_tab(world, tab_row, tab_id, window_id, label, is_active);
}

fn spawn_tab(
    world: &mut World,
    tab_row: Entity,
    tab_id: TabId,
    window_id: &str,
    label: &str,
    is_active: bool,
) {
    let tab_bg = if is_active {
        tokens::TAB_ACTIVE_BG
    } else {
        Color::NONE
    };
    let border_top = if is_active { Val::Px(2.0) } else { Val::ZERO };
    let border_color = if is_active {
        tokens::TAB_ACTIVE_BORDER
    } else {
        Color::NONE
    };
    let text_color = if is_active {
        tokens::TEXT_PRIMARY
    } else {
        tokens::TAB_INACTIVE_TEXT
    };

    let tab_entity = world
        .spawn((
            DockTab {
                window_id: window_id.to_string(),
                tab_id,
            },
            Interaction::default(),
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                padding: UiRect::horizontal(Val::Px(8.0)),
                height: Val::Percent(100.0),
                flex_shrink: 0.0,
                border: UiRect {
                    top: border_top,
                    ..default()
                },
                border_radius: BorderRadius::top(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(tab_bg),
            BorderColor::all(border_color),
            ChildOf(tab_row),
        ))
        .id();

    world.spawn((
        Text::new(label.to_string()),
        TextLayout::new_with_linebreak(LineBreak::NoWrap),
        TextFont {
            font_size: tokens::TEXT_SIZE_LG,
            ..default()
        },
        TextColor(text_color),
        ChildOf(tab_entity),
    ));

    let icon_font = world.get_resource::<IconFont>().map(|f| f.0.clone());

    if let Some(font_handle) = icon_font {
        world.spawn((
            crate::area::DockTabCloseButton {
                window_id: window_id.to_string(),
                tab_id,
            },
            Interaction::default(),
            Node {
                width: Val::Px(14.0),
                height: Val::Px(14.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(2.0)),
                display: Display::None,
                ..default()
            },
            ChildOf(tab_entity),
            children![(
                Text::new(String::from(Icon::X.unicode())),
                TextFont {
                    font: font_handle,
                    font_size: 10.0,
                    ..default()
                },
                TextColor(tokens::TAB_INACTIVE_TEXT),
            )],
        ));
    }
}

fn handle_dock_tab_clicks(
    tab_query: Query<(&DockTab, &Interaction, &ChildOf), Changed<Interaction>>,
    parent_query: Query<&ChildOf>,
    bindings: Query<&LeafBinding>,
    mut tree: ResMut<DockTree>,
) {
    for (tab, interaction, tab_child_of) in tab_query.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }

        // Walk: tab → tab_row → tab_bar → area
        let tab_row = tab_child_of.parent();
        let Ok(row_parent) = parent_query.get(tab_row) else {
            continue;
        };
        let tab_bar = row_parent.parent();
        let Ok(bar_parent) = parent_query.get(tab_bar) else {
            continue;
        };
        let area_entity = bar_parent.parent();

        let Ok(binding) = bindings.get(area_entity) else {
            continue;
        };

        tree.set_active(binding.0, tab.tab_id);
    }
}

fn show_close_on_hover(
    tabs: Query<(Entity, &Interaction, &Children), (Changed<Interaction>, With<DockTab>)>,
    drag_state: Option<Res<crate::drag::DockDragState>>,
    mut close_buttons: Query<&mut Node, With<crate::area::DockTabCloseButton>>,
) {
    let hide = drag_state.is_none_or(|s| matches!(*s, crate::drag::DockDragState::Dragging { .. }));

    for (_tab_entity, interaction, children) in tabs.iter() {
        let show =
            (*interaction == Interaction::Hovered || *interaction == Interaction::Pressed) && !hide;
        for child in children.iter() {
            if let Ok(mut node) = close_buttons.get_mut(child) {
                node.display = if show { Display::Flex } else { Display::None };
            }
        }
    }
}

fn on_close_button_click(
    trigger: On<Pointer<Click>>,
    close_buttons: Query<&crate::area::DockTabCloseButton>,
    mut tree: ResMut<DockTree>,
) {
    let entity = trigger.event_target();
    let Ok(close_btn) = close_buttons.get(entity) else {
        return;
    };
    tree.remove_tab(close_btn.tab_id);
}
