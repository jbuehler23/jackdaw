//! A segmented control: two or more mutually exclusive choices joined
//! inside one bordered box, the compact form the editor uses for
//! Play/Select, 3D/2D, Edit/Interact, Scene/Live and the Node card's
//! enum fields.
//!
//! The behaviour is [`bevy_ui_widgets::RadioGroup`] and
//! [`bevy_ui_widgets::RadioButton`]: the bar is the group, each segment
//! is a radio whose caption is its label, a click or an arrow key on the
//! focused group emits [`ValueChange<Entity>`] naming the chosen segment,
//! and [`Checked`] marks the one that is current.
//!
//! Not `bevy_feathers`' `FeathersRadio`: its scene builds a dial (a
//! bordered circle around a mark) beside the caption, and neither a theme
//! token nor a border radius turns that into a segment. The look here is
//! the box: a segment paints its own background when it is
//! [`Checked`] and nothing when it is not.

use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{RadioButton, RadioGroup};

use crate::tokens;

/// The bar's `Node`: segments side by side inside one rounded, clipped
/// box. Spread it (`Node { ..segmented_bar_node() }`) to add a layout
/// property, like the wrap a bar of four variants in a narrow panel
/// needs.
pub fn segmented_bar_node() -> Node {
    Node {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        border: UiRect::all(px(1.0)),
        border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
        overflow: Overflow::clip(),
        flex_shrink: 0.0,
        ..Default::default()
    }
}

/// The bar's behaviour and paint, without its `Node`, so a caller that
/// spreads [`segmented_bar_node`] does not carry two of them.
pub fn segmented_bar_chrome() -> impl Bundle {
    (
        RadioGroup,
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
    )
}

/// A bar with the default layout: `(segmented_bar_node(),
/// segmented_bar_chrome())`.
pub fn segmented_bar() -> impl Bundle {
    (segmented_bar_node(), segmented_bar_chrome())
}

/// One segment's `Node`.
pub fn segment_node() -> Node {
    Node {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        justify_content: JustifyContent::Center,
        padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
        ..Default::default()
    }
}

/// One segment's behaviour and paint, without its `Node`.
pub fn segment_chrome() -> impl Bundle {
    (RadioButton, BackgroundColor(Color::NONE))
}

/// A segment carrying its label: `(segment_node(), segment_chrome(),
/// segment_label(label))`. A caller adds its own marker component and
/// the observer that reads the group's [`ValueChange<Entity>`].
pub fn segment(label: impl Into<String>) -> impl Bundle {
    (
        segment_node(),
        segment_chrome(),
        children![segment_label(label)],
    )
}

/// A segment's caption.
pub fn segment_label(label: impl Into<String>) -> impl Bundle {
    (
        Text::new(label.into()),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
    )
}

/// The background a segment paints while it is the current choice, and
/// [`Color::NONE`] while it is not.
pub fn segment_background(checked: bool) -> Color {
    if checked {
        tokens::TOOLBAR_ACTIVE_BG
    } else {
        Color::NONE
    }
}

/// Mark `segment` as the current choice, or clear it.
///
/// [`RadioGroup`] deliberately does not write [`Checked`] itself: which
/// segment is current is the app's state, not the widget's, so every
/// segmented control syncs it from whatever it drives.
pub fn set_segment_checked(commands: &mut Commands, segment: Entity, checked: bool) {
    let Ok(mut entity) = commands.get_entity(segment) else {
        return;
    };
    if checked {
        entity.insert(Checked);
    } else {
        entity.remove::<Checked>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_is_a_radio_group_and_its_segments_are_radio_buttons() {
        let mut world = World::new();
        let bar = world.spawn(segmented_bar()).id();
        let one = world.spawn((segment("One"), ChildOf(bar))).id();

        assert!(world.get::<RadioGroup>(bar).is_some());
        assert!(world.get::<RadioButton>(one).is_some());
        assert!(world.get::<Interaction>(one).is_none());
    }

    #[test]
    fn checking_a_segment_writes_checked_and_clearing_it_removes_it() {
        let mut world = World::new();
        let segment = world.spawn(segment("One")).id();

        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            set_segment_checked(&mut commands, segment, true);
        }
        queue.apply(&mut world);
        assert!(world.get::<Checked>(segment).is_some());

        {
            let mut commands = Commands::new(&mut queue, &world);
            set_segment_checked(&mut commands, segment, false);
        }
        queue.apply(&mut world);
        assert!(world.get::<Checked>(segment).is_none());
    }
}
