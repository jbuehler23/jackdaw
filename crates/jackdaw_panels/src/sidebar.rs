use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use jackdaw_feathers::tokens;

use crate::reconcile::LeafBinding;
use crate::tree::{DockTree, TabId};

#[derive(Component)]
pub struct DockSidebarContainer;

#[derive(Component)]
pub struct DockSidebarIcon {
    pub window_id: String,
    pub tab_id: TabId,
}

pub fn spawn_icon_sidebar_world(
    world: &mut World,
    area_entity: Entity,
    windows: &[(TabId, String, String, Option<String>)],
) {
    let first = windows.first().map(|(id, _, _, _)| *id);

    let sidebar = world
        .spawn((
            DockSidebarContainer,
            Node {
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                width: Val::Px(30.0),
                padding: UiRect::new(Val::Px(1.0), Val::ZERO, Val::Px(4.0), Val::Px(9.0)),
                flex_shrink: 0.0,
                border: UiRect {
                    left: Val::Px(1.0),
                    top: Val::Px(1.0),
                    bottom: Val::Px(1.0),
                    right: Val::ZERO,
                },
                border_radius: BorderRadius::left(Val::Px(5.0)),
                ..default()
            },
            BackgroundColor(tokens::WINDOW_BG),
            BorderColor::all(tokens::PANEL_BORDER),
            ChildOf(area_entity),
        ))
        .id();

    let icon_group = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                ..default()
            },
            ChildOf(sidebar),
        ))
        .id();

    for (tab_id, window_id, _name, icon_char) in windows {
        let is_active = Some(*tab_id) == first;
        let icon_text = icon_char.as_deref().unwrap_or("?");

        let icon_entity = world
            .spawn((
                DockSidebarIcon {
                    window_id: window_id.clone(),
                    tab_id: *tab_id,
                },
                Interaction::default(),
                Node {
                    width: Val::Px(29.0),
                    height: Val::Px(30.0),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    border: UiRect::left(Val::Px(2.0)),
                    ..default()
                },
                BorderColor::all(if is_active {
                    tokens::ACCENT_BLUE
                } else {
                    Color::NONE
                }),
                ChildOf(icon_group),
            ))
            .id();

        let mut text_font = TextFont {
            font_size: tokens::ICON_MD,
            ..default()
        };

        if let Some(icon_font_res) = world.get_resource::<crate::IconFontHandle>() {
            text_font.font = icon_font_res.0.clone();
        }

        world.spawn((
            Text::new(icon_text.to_string()),
            text_font,
            TextColor(if is_active {
                tokens::TEXT_PRIMARY
            } else {
                tokens::TAB_INACTIVE_TEXT
            }),
            ChildOf(icon_entity),
        ));
    }

    let _ = first; // ActiveDockWindow is set by reconcile::materialize_area
}

pub fn handle_sidebar_icon_clicks(
    icon_query: Query<(&DockSidebarIcon, &Interaction, &ChildOf), Changed<Interaction>>,
    parent_query: Query<&ChildOf>,
    bindings: Query<&LeafBinding>,
    mut tree: ResMut<DockTree>,
) {
    for (icon, interaction, icon_parent) in icon_query.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }

        // Walk: icon → icon_group → sidebar → area
        let icon_group = icon_parent.parent();
        let Ok(group_parent) = parent_query.get(icon_group) else {
            continue;
        };
        let sidebar = group_parent.parent();
        let Ok(sidebar_parent) = parent_query.get(sidebar) else {
            continue;
        };
        let area_entity = sidebar_parent.parent();

        let Ok(binding) = bindings.get(area_entity) else {
            continue;
        };

        tree.set_active(binding.0, icon.tab_id);
    }
}

/// Right-click on a sidebar icon closes (removes) that tab from its
/// leaf. Sidebar icons don't have a visible X button, so this is the
/// equivalent of clicking X on a tab.
pub fn on_sidebar_icon_right_click(
    trigger: On<Pointer<Click>>,
    icons: Query<&DockSidebarIcon>,
    mut tree: ResMut<DockTree>,
) {
    if trigger.event().button != PointerButton::Secondary {
        return;
    }
    let Ok(icon) = icons.get(trigger.event_target()) else {
        return;
    };
    tree.remove_tab(icon.tab_id);
}
