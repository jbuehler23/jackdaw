//! The editor's look for a disabled feathers checkbox.

use bevy::feathers::theme::UiTheme;
use bevy::feathers::tokens;
use bevy::prelude::*;

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, outline_disabled_checkboxes);
}

/// Feathers draws a disabled checkbox with no fill and a border in the
/// disabled fill colour, which is the colour of the panels the editor puts
/// checkboxes on, so the box disappears and only the dimmed caption is left.
/// The border takes the disabled text colour instead, like the caption and
/// the check mark.
fn outline_disabled_checkboxes(theme: Option<ResMut<UiTheme>>) {
    let Some(mut theme) = theme else {
        return;
    };
    let Some(disabled) = theme
        .0
        .token_assignments
        .get(&tokens::CHECKBOX_TEXT_DISABLED)
        .cloned()
    else {
        return;
    };
    for token in [
        tokens::CHECKBOX_BORDER_DISABLED,
        tokens::CHECKBOX_BORDER_CHECKED_DISABLED,
    ] {
        theme.0.token_assignments.insert(token, disabled.clone());
    }
}
