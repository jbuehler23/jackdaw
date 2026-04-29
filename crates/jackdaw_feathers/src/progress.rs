//! Simple horizontal progress-bar widget.
//!
//! Used by the scaffold + hot-reload flows to surface build
//! progress while cargo runs. Render determinate (known fraction)
//! or indeterminate (unknown total; bar stays at 0%, caller
//! typically shows a spinner separately or leaves the bar empty).

use bevy::prelude::*;

use crate::tokens;

/// Height of the track.
const TRACK_HEIGHT_PX: f32 = 6.0;

/// Marker on the outer track node so callers can query by type.
#[derive(Component)]
pub struct ProgressBar;

/// Marker on the inner fill node so updates can target it directly.
#[derive(Component)]
pub struct ProgressBarFill;

/// Build a progress bar with `fraction` in `[0.0, 1.0]`. Callers
/// can later update the fill by querying `ProgressBarFill` and
/// setting its `Node.width` to `Val::Percent(fraction * 100.0)`.
pub fn progress_bar(fraction: f32) -> impl Bundle {
    let clamped = fraction.clamp(0.0, 1.0);
    (
        ProgressBar,
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(TRACK_HEIGHT_PX),
            border: UiRect::all(Val::Px(1.0)),
            overflow: Overflow::clip(),
            ..default()
        },
        BorderColor::all(tokens::BORDER_SUBTLE),
        BackgroundColor(tokens::PANEL_BG),
        children![(
            ProgressBarFill,
            Node {
                width: Val::Percent(clamped * 100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(tokens::TEXT_ACCENT),
        )],
    )
}

/// Helper for callers that want to update a progress bar's fill
/// width given its root entity and a new fraction. Walks the
/// single expected child.
pub fn set_progress_fill(
    bar_entity: Entity,
    fraction: f32,
    children_q: &Query<&Children>,
    fill_q: &mut Query<&mut Node, With<ProgressBarFill>>,
) {
    let Ok(children) = children_q.get(bar_entity) else {
        return;
    };
    let clamped = fraction.clamp(0.0, 1.0);
    for child in children.iter() {
        if let Ok(mut node) = fill_q.get_mut(child) {
            node.width = Val::Percent(clamped * 100.0);
        }
    }
}
