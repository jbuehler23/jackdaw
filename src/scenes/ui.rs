//! Scene tab bar. Renders one button per `SceneTab` in `Scenes`,
//! plus a trailing "+" button for `scene.new`. Left-click switches,
//! middle/X-button closes, right-click opens the context menu. The
//! strip is rebuilt structurally when the tab list changes; per-tab
//! highlight + dirty-dot updates happen in-place to keep observer
//! identities stable across normal editing.

use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use jackdaw_feathers::{
    context_menu::spawn_context_menu,
    icons::{EditorFont, Icon, IconFont},
    tokens,
};
use jackdaw_widgets::context_menu::{ContextMenuAction, ContextMenuState};

use crate::scenes::{
    Scenes,
    operators::{scene_close_system, scene_new_system, scene_switch_system},
};

const TAB_ACTIVE_BG: Color = tokens::DOC_TAB_ACTIVE_BG;
const TAB_INACTIVE_BG: Color = Color::NONE;
const TAB_ACTIVE_LABEL: Color = tokens::DOC_TAB_ACTIVE_LABEL;
const TAB_INACTIVE_LABEL: Color = tokens::DOC_TAB_INACTIVE_LABEL;
const TAB_ACTIVE_BORDER: Color = tokens::DOC_TAB_ACTIVE_BORDER;
const TAB_DIRTY_DOT: Color = tokens::DOC_TAB_DIRTY_DOT;
const TAB_ACCENT: Color = tokens::DOC_TAB_SCENE_ACCENT;
const ADD_BTN_LABEL: Color = tokens::DOC_TAB_INACTIVE_LABEL;

const TAB_LABEL_FONT_PX: f32 = 12.0;
const TAB_ICON_FONT_PX: f32 = 12.0;
const TAB_CLOSE_ICON_PX: f32 = 9.0;
const TAB_PAD_X: f32 = 9.0;
const TAB_PAD_Y: f32 = 5.0;
const TAB_RADIUS: f32 = 4.0;
const TAB_GAP: f32 = 6.0;

/// Marker for the scene-tab-bar container. The layout owner spawns one
/// of these; the rebuild system reuses it.
#[derive(Component)]
pub struct SceneTabStrip;

/// Per-tab marker. Stores the tab index so click observers and the
/// in-place visual updater can locate it.
#[derive(Component, Clone, Copy)]
pub struct SceneTabIndex(pub usize);

/// Marker on the close (`x`) child of a scene tab. Carries the tab
/// index so it can dispatch the close operator independently of the
/// parent tab's click observer.
#[derive(Component, Clone, Copy)]
pub struct SceneTabCloseButton(pub usize);

/// Marker on the per-tab dirty dot so `update_scene_tab_visuals` can
/// hide/show it without rebuilding the strip.
#[derive(Component, Clone, Copy)]
pub struct SceneTabDirtyDot(pub usize);

/// Marker on the trailing "+" button at the end of the strip.
#[derive(Component)]
pub struct SceneTabAddButton;

/// Rebuilds the tab strip's children when the tab list changes.
/// In-place visual updates (active highlight, dirty dot) go through
/// `update_scene_tab_visuals` so the observers stay attached across
/// normal edits.
pub fn rebuild_scene_tab_strip(
    mut commands: Commands,
    scenes: Res<Scenes>,
    strip_q: Query<(Entity, Option<&Children>), With<SceneTabStrip>>,
    tab_q: Query<&SceneTabIndex>,
    editor_font: Option<Res<EditorFont>>,
    icon_font: Option<Res<IconFont>>,
) {
    if !scenes.is_changed() {
        return;
    }
    let Ok((strip_entity, children)) = strip_q.single() else {
        return;
    };

    let current_count = children
        .map(|c| c.iter().filter(|e| tab_q.get(*e).is_ok()).count())
        .unwrap_or(0);
    if current_count == scenes.tabs.len() && current_count > 0 {
        // Structure unchanged. Visual updater handles active/dirty
        // state without despawning observer-bearing entities.
        return;
    }

    if let Some(children) = children {
        for child in children.iter() {
            if let Ok(mut ec) = commands.get_entity(child) {
                ec.despawn();
            }
        }
    }

    let editor_font_handle = editor_font.map(|f| f.0.clone());
    let icon_font_handle = icon_font.map(|f| f.0.clone());
    let active = scenes.active;

    for (idx, tab) in scenes.tabs.iter().enumerate() {
        spawn_scene_tab(
            &mut commands,
            strip_entity,
            idx,
            &tab.display_name,
            tab.dirty,
            idx == active,
            editor_font_handle.clone(),
            icon_font_handle.clone(),
        );
    }

    spawn_add_tab_button(&mut commands, strip_entity, icon_font_handle);
}

fn spawn_scene_tab(
    commands: &mut Commands,
    strip: Entity,
    idx: usize,
    display_name: &str,
    dirty: bool,
    is_active: bool,
    editor_font: Option<Handle<Font>>,
    icon_font: Option<Handle<Font>>,
) {
    let bg = if is_active { TAB_ACTIVE_BG } else { TAB_INACTIVE_BG };
    let border = if is_active { TAB_ACTIVE_BORDER } else { Color::NONE };
    let label_color = if is_active {
        TAB_ACTIVE_LABEL
    } else {
        TAB_INACTIVE_LABEL
    };

    let tab_entity = commands
        .spawn((
            SceneTabIndex(idx),
            Interaction::default(),
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(TAB_GAP),
                padding: UiRect::axes(Val::Px(TAB_PAD_X), Val::Px(TAB_PAD_Y)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(TAB_RADIUS)),
                ..Default::default()
            },
            BackgroundColor(bg),
            BorderColor::all(border),
            ChildOf(strip),
        ))
        .observe(
            move |click: On<Pointer<Click>>,
                  mut commands: Commands,
                  mut state: ResMut<ContextMenuState>,
                  windows: Query<&Window>| {
                match click.event().button {
                    PointerButton::Primary => {
                        commands.queue(move |world: &mut World| {
                            scene_switch_system(world, idx);
                        });
                    }
                    PointerButton::Middle => {
                        commands.queue(move |world: &mut World| {
                            scene_close_system(world, idx);
                        });
                    }
                    PointerButton::Secondary => {
                        let cursor_pos = windows
                            .single()
                            .ok()
                            .and_then(|w| w.cursor_position())
                            .unwrap_or_default();
                        if let Some(menu) = state.menu_entity.take()
                            && let Ok(mut ec) = commands.get_entity(menu)
                        {
                            ec.despawn();
                        }
                        let owned_items: Vec<(String, String)> = vec![
                            (format!("scene.tab.save.{}", idx), "Save".into()),
                            (format!("scene.tab.save_as.{}", idx), "Save As...".into()),
                            (format!("scene.tab.close.{}", idx), "Close".into()),
                            (
                                format!("scene.tab.close_others.{}", idx),
                                "Close Others".into(),
                            ),
                        ];
                        let item_refs: Vec<(&str, &str)> = owned_items
                            .iter()
                            .map(|(a, l)| (a.as_str(), l.as_str()))
                            .collect();
                        let menu =
                            spawn_context_menu(&mut commands, cursor_pos, None, &item_refs);
                        state.menu_entity = Some(menu);
                    }
                }
            },
        )
        .id();

    // Small colour accent stripe (matches the workspace-tab accent column).
    commands.spawn((
        Node {
            width: Val::Px(2.5),
            height: Val::Px(12.0),
            border_radius: BorderRadius::all(Val::Px(5.0)),
            ..Default::default()
        },
        BackgroundColor(TAB_ACCENT),
        Pickable::IGNORE,
        ChildOf(tab_entity),
    ));

    // File icon prefix (only if icon font is available).
    if let Some(handle) = icon_font.clone() {
        commands.spawn((
            Text::new(String::from(Icon::File.unicode())),
            TextFont {
                font: handle,
                font_size: TAB_ICON_FONT_PX,
                ..Default::default()
            },
            TextColor(label_color),
            Pickable::IGNORE,
            ChildOf(tab_entity),
        ));
    }

    // Label.
    let mut label_font = TextFont {
        font_size: TAB_LABEL_FONT_PX,
        ..Default::default()
    };
    if let Some(handle) = editor_font {
        label_font.font = handle;
    }
    commands.spawn((
        Text::new(display_name.to_string()),
        label_font,
        TextColor(label_color),
        Pickable::IGNORE,
        ChildOf(tab_entity),
    ));

    // Dirty dot (always present; hidden when not dirty so the visual
    // updater can flip display without rebuilding).
    commands.spawn((
        SceneTabDirtyDot(idx),
        Node {
            width: Val::Px(6.0),
            height: Val::Px(6.0),
            border_radius: BorderRadius::all(Val::Px(3.0)),
            display: if dirty { Display::Flex } else { Display::None },
            ..Default::default()
        },
        BackgroundColor(TAB_DIRTY_DOT),
        Pickable::IGNORE,
        ChildOf(tab_entity),
    ));

    // Close button.
    let close_btn = commands
        .spawn((
            SceneTabCloseButton(idx),
            Interaction::default(),
            Node {
                width: Val::Px(16.0),
                height: Val::Px(16.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(3.0)),
                margin: UiRect::left(Val::Px(2.0)),
                ..Default::default()
            },
            BackgroundColor(Color::NONE),
            ChildOf(tab_entity),
        ))
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.queue(move |world: &mut World| {
                scene_close_system(world, idx);
            });
        })
        .id();

    if let Some(handle) = icon_font {
        commands.spawn((
            Text::new(String::from(Icon::X.unicode())),
            TextFont {
                font: handle,
                font_size: TAB_CLOSE_ICON_PX,
                ..Default::default()
            },
            TextColor(TAB_INACTIVE_LABEL),
            Pickable::IGNORE,
            ChildOf(close_btn),
        ));
    }
}

fn spawn_add_tab_button(
    commands: &mut Commands,
    strip: Entity,
    icon_font: Option<Handle<Font>>,
) {
    let btn = commands
        .spawn((
            SceneTabAddButton,
            Interaction::default(),
            Node {
                width: Val::Px(22.0),
                height: Val::Px(22.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(4.0)),
                margin: UiRect::left(Val::Px(4.0)),
                ..Default::default()
            },
            BackgroundColor(Color::NONE),
            ChildOf(strip),
        ))
        .observe(|_: On<Pointer<Click>>, mut commands: Commands| {
            commands.queue(|world: &mut World| {
                scene_new_system(world);
            });
        })
        .id();

    if let Some(handle) = icon_font {
        commands.spawn((
            Text::new(String::from(Icon::Plus.unicode())),
            TextFont {
                font: handle,
                font_size: 12.0,
                ..Default::default()
            },
            TextColor(ADD_BTN_LABEL),
            Pickable::IGNORE,
            ChildOf(btn),
        ));
    }
}

/// Handle context menu actions that originated from a right-click on a scene tab.
///
/// Only processes actions whose IDs begin with `"scene.tab."`. Any other
/// `ContextMenuAction` events (e.g., from the hierarchy panel) are silently
/// ignored so both observers can coexist.
pub fn on_scene_tab_context_action(event: On<ContextMenuAction>, mut commands: Commands) {
    let action = event.action.as_str();
    if !action.starts_with("scene.tab.") {
        return;
    }
    let Some((prefix, idx_str)) = action.rsplit_once('.') else {
        return;
    };
    let Ok(idx) = idx_str.parse::<usize>() else {
        return;
    };
    let prefix = prefix.to_string();
    commands.queue(move |world: &mut World| {
        match prefix.as_str() {
            "scene.tab.save" => {
                let current = world.resource::<crate::scenes::Scenes>().active;
                if idx != current {
                    crate::scenes::swap::swap_active_tab(world, idx);
                }
                if let Some(path) = world
                    .resource::<crate::scenes::Scenes>()
                    .tabs
                    .get(idx)
                    .and_then(|t| t.path.clone())
                {
                    if let Some(mut spath) =
                        world.get_resource_mut::<crate::scene_io::SceneFilePath>()
                    {
                        spath.path = Some(path.to_string_lossy().into_owned());
                    }
                }
                crate::scene_io::save_scene(world);
                if let Some(tab) = world
                    .resource_mut::<crate::scenes::Scenes>()
                    .tabs
                    .get_mut(idx)
                {
                    tab.dirty = false;
                }
            }
            "scene.tab.save_as" => {
                let current = world.resource::<crate::scenes::Scenes>().active;
                if idx != current {
                    crate::scenes::swap::swap_active_tab(world, idx);
                }
                crate::scene_io::save_scene_as(world);
            }
            "scene.tab.close" => {
                crate::scenes::operators::scene_close_system(world, idx);
            }
            "scene.tab.close_others" => {
                let count = world.resource::<crate::scenes::Scenes>().tabs.len();
                for i in (0..count).rev() {
                    if i == idx {
                        continue;
                    }
                    crate::scenes::operators::scene_close_system(world, i);
                }
            }
            _ => {}
        }
    });
}

/// Per-frame system: update tab visuals (bg, border, label color, dirty
/// dot) in-place. The structural rebuild only fires when the number of
/// tabs changes, so this is the path that handles flips between
/// active/inactive and clean/dirty without disrupting per-entity
/// observers.
pub fn update_scene_tab_visuals(
    scenes: Res<Scenes>,
    tabs: Query<(Entity, &SceneTabIndex)>,
    mut bg_query: Query<&mut BackgroundColor>,
    mut border_query: Query<&mut BorderColor>,
    children_query: Query<&Children>,
    mut text_color_query: Query<&mut TextColor>,
    mut node_query: Query<&mut Node>,
    close_buttons: Query<&SceneTabCloseButton>,
    dirty_dots: Query<&SceneTabDirtyDot>,
) {
    if !scenes.is_changed() {
        return;
    }
    let active = scenes.active;

    for (tab_entity, &SceneTabIndex(idx)) in tabs.iter() {
        let is_active = idx == active;
        let dirty = scenes
            .tabs
            .get(idx)
            .map(|t| t.dirty)
            .unwrap_or(false);

        if let Ok(mut bg) = bg_query.get_mut(tab_entity) {
            bg.0 = if is_active { TAB_ACTIVE_BG } else { TAB_INACTIVE_BG };
        }
        if let Ok(mut bc) = border_query.get_mut(tab_entity) {
            *bc = BorderColor::all(if is_active {
                TAB_ACTIVE_BORDER
            } else {
                Color::NONE
            });
        }
        if let Ok(children) = children_query.get(tab_entity) {
            for child in children.iter() {
                if close_buttons.get(child).is_ok() {
                    continue;
                }
                if let Ok(dot) = dirty_dots.get(child) {
                    let _ = dot;
                    if let Ok(mut node) = node_query.get_mut(child) {
                        node.display = if dirty { Display::Flex } else { Display::None };
                    }
                    continue;
                }
                if let Ok(mut tc) = text_color_query.get_mut(child) {
                    tc.0 = if is_active {
                        TAB_ACTIVE_LABEL
                    } else {
                        TAB_INACTIVE_LABEL
                    };
                }
            }
        }
    }
}
