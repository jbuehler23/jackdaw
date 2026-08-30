//! The scrollbar beside a scrolling container.
//!
//! The bar is [`Scrollbar`] with a [`ScrollbarThumb`] inside it, painted
//! with the feathers scrollbar tokens: the widget sizes and moves the
//! thumb from the container's [`ScrollPosition`] and drags it back,
//! which the hand-rolled bar it replaces could not do.
//!
//! The container itself carries [`ScrollArea`], so the wheel over it is
//! the widget's too. The editor's own wheel handler
//! (`jackdaw::on_scroll`) leaves a `ScrollArea` alone for that reason;
//! it still answers for every other scrolling container, because it
//! chains a scroll past a container that has reached its limit and turns
//! a vertical wheel into horizontal movement over a strip that only
//! scrolls sideways, neither of which the widget does.

use bevy::feathers::theme::ThemeBackgroundColor;
use bevy::feathers::tokens as feathers_tokens;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{ControlOrientation, ScrollArea, Scrollbar, ScrollbarThumb};

const SCROLLBAR_MIN_THUMB: f32 = 24.0;
const SCROLLBAR_WIDTH: f32 = 3.0;
const SCROLLBAR_MARGIN: f32 = 3.0;

pub fn plugin(app: &mut App) {
    app.add_systems(Update, reveal_scrollbar_on_hover);
}

/// A vertical scrollbar for `container`, sitting just inside its right
/// edge.
///
/// The container needs `Overflow::scroll_y()` and [`scroll_area`].
pub fn scrollbar(container: Entity) -> impl Bundle {
    (
        Scrollbar::new(container, ControlOrientation::Vertical, SCROLLBAR_MIN_THUMB),
        Node {
            position_type: PositionType::Absolute,
            width: px(SCROLLBAR_WIDTH),
            right: px(SCROLLBAR_MARGIN),
            top: px(SCROLLBAR_MARGIN),
            bottom: px(SCROLLBAR_MARGIN),
            border_radius: BorderRadius::all(px(SCROLLBAR_WIDTH / 2.0)),
            ..default()
        },
        ThemeBackgroundColor(feathers_tokens::SCROLLBAR_BG),
        Visibility::Hidden,
        children![(
            ScrollbarThumb {
                border_radius: BorderRadius::all(px(SCROLLBAR_WIDTH / 2.0)),
                ..default()
            },
            Hovered::default(),
            ThemeBackgroundColor(feathers_tokens::SCROLLBAR_THUMB),
        )],
    )
}

/// The container half of a scrolling pair: the wheel over it reaches the
/// widget, and it reports hover so the bar beside it can show itself.
pub fn scroll_area() -> impl Bundle {
    (ScrollArea, Hovered::default(), ScrollPosition::default())
}

/// Show a bar only while its container is hovered and has somewhere to
/// scroll, the way the editor's panels have always shown theirs.
fn reveal_scrollbar_on_hover(
    containers: Query<(&Hovered, &ComputedNode)>,
    mut scrollbars: Query<(&Scrollbar, &mut Visibility)>,
) {
    for (scrollbar, mut visibility) in &mut scrollbars {
        let Ok((hovered, computed)) = containers.get(scrollbar.target) else {
            continue;
        };
        let has_scroll = computed.content_size().y > computed.size().y;
        let wanted = if hovered.get() && has_scroll {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}
