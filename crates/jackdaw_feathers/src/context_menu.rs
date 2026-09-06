use bevy::feathers::controls::{FeathersMenuItem, FeathersMenuPopup};
use bevy::feathers::rounded_corners::RoundedCorners;
use bevy::feathers::theme::ThemedText;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use jackdaw_widgets::context_menu::{ContextMenuAction, ContextMenuItem};

use crate::button::ButtonOperatorCall;

pub fn plugin(app: &mut App) {
    app.add_observer(on_context_menu_item_activate);
}

fn on_context_menu_item_activate(
    event: On<Activate>,
    items: Query<(&ContextMenuItem, Option<&ButtonOperatorCall>)>,
    mut commands: Commands,
) {
    let Ok((item, button_op)) = items.get(event.entity) else {
        return;
    };
    // Items that dispatch an operator are handled by the editor-side
    // ButtonOperatorCall observer; firing ContextMenuAction here would
    // double-dispatch.
    if button_op.is_some() {
        return;
    }
    commands.trigger(ContextMenuAction {
        action: item.action.clone(),
        target_entity: item.target_entity,
    });
}

/// Spawn a context menu at the given position with the given items.
///
/// The menu is a [`FeathersMenuPopup`] and each item a
/// [`FeathersMenuItem`], so the frame, the row painting and the
/// activation are the widget's. It is placed by hand rather than by the
/// popup's own [`bevy::ui_widgets::popover::Popover`], which places a
/// popup against the rectangle of its parent: a menu opened at the
/// cursor has no such parent, so it is spawned at the root and the
/// placement component the scene carries stays inert.
///
/// Each item is `(action_id, label)`. Actions starting with `op:` are
/// parsed via [`ButtonOperatorCall`]'s `TryFrom<&str>` impl into a
/// `ButtonOperatorCall` (id + any embedded `?key=value` params)
/// attached to the item.
pub fn spawn_context_menu(
    commands: &mut Commands,
    position: Vec2,
    target_entity: Option<Entity>,
    items: &[(&str, &str)],
) -> Entity {
    let menu = commands
        .spawn_scene(bsn! { @FeathersMenuPopup })
        .insert((
            jackdaw_widgets::context_menu::ContextMenu,
            Node {
                position_type: PositionType::Absolute,
                left: px(position.x),
                top: px(position.y),
                flex_direction: FlexDirection::Column,
                min_width: px(160.0),
                padding: UiRect::axes(px(0.0), px(4.0)),
                border: UiRect::all(px(1.0)),
                border_radius: RoundedCorners::All.to_border_radius(4.0),
                ..default()
            },
            Visibility::Visible,
            GlobalZIndex(1000),
        ))
        .id();

    for &(action, label) in items {
        let item = ContextMenuItem {
            action: action.to_string(),
            target_entity,
        };
        let mut row = commands.spawn_scene(bsn! {
            @FeathersMenuItem {
                @caption: bsn! { Text({label.to_string()}) ThemedText },
            }
        });
        row.insert((item, ChildOf(menu)));
        if let Ok(call) = ButtonOperatorCall::try_from(action) {
            row.insert(call);
        }
    }

    menu
}
