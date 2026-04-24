use bevy::{feathers::theme::ThemedText, prelude::*, ui::ui_transform::UiGlobalTransform};
use jackdaw_widgets::menu_bar::{
    MenuAction, MenuBar, MenuBarDropdown, MenuBarDropdownItem, MenuBarItem, MenuBarState,
};

use crate::button::{ButtonClickEvent, ButtonOperatorCall, ButtonProps, ButtonVariant, button};
use crate::tokens;
use crate::tooltip::Tooltip;

/// Action strings in menu entries that start with this prefix are
/// interpreted as operator ids; the suffix becomes the [`ButtonOperatorCall`] id
/// attached to the dropdown button so the editor dispatches through the
/// operator API instead of firing a generic [`MenuAction`].
pub const OP_ACTION_PREFIX: &str = "op:";

pub fn plugin(app: &mut App) {
    app.add_observer(on_dropdown_item_click)
        .add_observer(on_menu_bar_item_click)
        .add_observer(on_menu_bar_item_over)
        .add_observer(on_menu_bar_item_out);
}

/// When a dropdown item is clicked, fire the [`MenuAction`] — unless the
/// item carries a [`ButtonOperatorCall`] component, in which case the editor's
/// operator observer will handle dispatch and a `MenuAction` would
/// double-fire.
fn on_dropdown_item_click(
    event: On<ButtonClickEvent>,
    items: Query<(&MenuBarDropdownItem, Option<&ButtonOperatorCall>)>,
    mut commands: Commands,
) {
    let Ok((item, button_op)) = items.get(event.entity) else {
        return;
    };
    if button_op.is_some() {
        return;
    }
    commands.trigger(MenuAction {
        action: item.action.clone(),
    });
}

/// Handle click on a [`MenuBarItem`]: find the item by walking up from the event target.
fn on_menu_bar_item_click(
    mut click: On<Pointer<Click>>,
    mut commands: Commands,
    mut state: ResMut<MenuBarState>,
    items: Query<(&MenuBarItem, &ComputedNode, &UiGlobalTransform)>,
    item_check: Query<Entity, With<MenuBarItem>>,
    parents: Query<&ChildOf>,
) {
    let Some(entity) = find_ancestor(click.event_target(), &item_check, &parents) else {
        return;
    };
    let Ok((item, computed, global_tf)) = items.get(entity) else {
        return;
    };

    click.propagate(false);

    // Close existing dropdown
    if let Some(dropdown) = state.dropdown_entity.take() {
        commands.entity(dropdown).despawn();
    }

    if state.open_menu == Some(entity) {
        // Toggle off
        state.open_menu = None;
        return;
    }

    // Open dropdown
    state.open_menu = Some(entity);

    let (_, _, pos) = global_tf.to_scale_angle_translation();
    let size = computed.size() * computed.inverse_scale_factor();
    let x = pos.x - size.x / 2.0;
    let y = pos.y + size.y / 2.0;

    let dropdown = spawn_dropdown(&mut commands, x, y, &item.actions);
    state.dropdown_entity = Some(dropdown);
}

fn on_menu_bar_item_over(
    hover: On<Pointer<Over>>,
    items: Query<Entity, With<MenuBarItem>>,
    parents: Query<&ChildOf>,
    mut bg_query: Query<&mut BackgroundColor>,
) {
    if let Some(entity) = find_ancestor(hover.event_target(), &items, &parents)
        && let Ok(mut bg) = bg_query.get_mut(entity)
    {
        bg.0 = tokens::HOVER_BG;
    }
}

fn on_menu_bar_item_out(
    out: On<Pointer<Out>>,
    items: Query<Entity, With<MenuBarItem>>,
    parents: Query<&ChildOf>,
    mut bg_query: Query<&mut BackgroundColor>,
) {
    if let Some(entity) = find_ancestor(out.event_target(), &items, &parents)
        && let Ok(mut bg) = bg_query.get_mut(entity)
    {
        bg.0 = Color::NONE;
    }
}

/// Walk up from `start` through [`ChildOf`] to find an entity with `MenuBarItem`.
fn find_ancestor(
    start: Entity,
    items: &Query<Entity, With<MenuBarItem>>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    let mut entity = start;
    for _ in 0..10 {
        if items.contains(entity) {
            return Some(entity);
        }
        if let Ok(child_of) = parents.get(entity) {
            entity = child_of.parent();
        } else {
            return None;
        }
    }
    None
}

/// Marker for the menu bar root so we can find and populate it.
#[derive(Component)]
pub struct MenuBarRoot;

/// Build the styled menu bar shell. Items are spawned by
/// `populate_menu_bar` system.
///
/// The shell sizes to its content width (menu items + padding) so it
/// composes cleanly inside a horizontal flex row alongside siblings like
/// a document tab strip or transport controls.
pub fn menu_bar_shell() -> impl Bundle {
    (
        MenuBarRoot,
        MenuBar,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            // Auto width so siblings (tab strips, transport pills) get
            // their share of the row; `flex_shrink: 0` keeps our menu
            // items from being squeezed if the window is narrow.
            width: Val::Auto,
            height: Val::Px(tokens::MENU_BAR_HEIGHT),
            flex_shrink: 0.0,
            padding: UiRect::horizontal(Val::Px(tokens::SPACING_SM)),
            ..Default::default()
        },
        BackgroundColor(tokens::WINDOW_BG),
    )
}

/// Populate a menu bar entity with items. Call from the app layer after spawning the shell.
///
/// Actions are `(action_id, label)` pairs. `action_id` can be an
/// operator id wrapped in the [`OP_ACTION_PREFIX`] or any free-form
/// identifier the host matches in a `MenuAction` observer. Action
/// strings are owned so callers can pass `format!("op:{}", Op::ID)`
/// without leaking operator-id string literals into UI code.
pub fn populate_menu_bar(
    world: &mut World,
    menu_bar_entity: Entity,
    menus: Vec<(&str, Vec<(String, String)>)>,
) {
    for (label, actions) in menus {
        spawn_menu_bar_item(world, menu_bar_entity, label, actions);
    }
}

fn spawn_menu_bar_item(
    world: &mut World,
    parent: Entity,
    label: &str,
    actions: Vec<(String, String)>,
) {
    world.spawn((
        MenuBarItem {
            label: label.to_string(),
            actions,
        },
        Node {
            padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_XS)),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        BackgroundColor(Color::NONE),
        children![(
            Text::new(label),
            TextFont {
                font_size: tokens::FONT_MD,
                ..Default::default()
            },
            ThemedText,
        )],
        ChildOf(parent),
    ));
}

fn spawn_dropdown(commands: &mut Commands, x: f32, y: f32, actions: &[(String, String)]) -> Entity {
    let dropdown = commands
        .spawn((
            MenuBarDropdown,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(x),
                top: Val::Px(y),
                flex_direction: FlexDirection::Column,
                min_width: Val::Px(180.0),
                padding: UiRect::axes(Val::Px(tokens::SPACING_XS), Val::Px(tokens::SPACING_SM)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::MENU_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ZIndex(1000),
        ))
        .id();

    for (action, label) in actions {
        if action == "---" {
            // Separator
            commands.spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(1.0),
                    margin: UiRect::axes(Val::Px(0.0), Val::Px(tokens::SPACING_XS)),
                    ..Default::default()
                },
                BackgroundColor(tokens::BORDER_SUBTLE),
                ChildOf(dropdown),
            ));
            continue;
        }

        let item = MenuBarDropdownItem {
            action: action.clone(),
        };
        let btn = button(
            ButtonProps::new(label.clone())
                .with_variant(ButtonVariant::Ghost)
                // TODO: add keybind as subtitle
                .align_left(),
        );
        // TODO: show this tooltip only when the user has opted into dev stuff
        let tooltip = Tooltip(action.to_string());

        if let Some(op_id) = action.strip_prefix(OP_ACTION_PREFIX) {
            commands.entity(dropdown).with_child((
                item,
                btn,
                tooltip,
                ButtonOperatorCall::new(op_id.to_string()),
            ));
        } else {
            commands.entity(dropdown).with_child((item, btn, tooltip));
        }
    }

    dropdown
}
