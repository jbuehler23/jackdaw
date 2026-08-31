//! Which dock panel a keypress belongs to.
//!
//! Several chords are claimed by more than one panel: `Ctrl+C` copies an
//! entity in the outliner and a keyframe in the timeline, `Delete` removes
//! either, and `Home` frames the canvas or jumps the playhead. Asking whether
//! a panel's tab is active *somewhere* in the dock tree cannot separate them:
//! in a workspace holding both, both answers are yes and the narrower claim
//! wins everywhere, which leaves the wider one dead in every other panel.
//!
//! Focus is the separator, and the dock tree does not track it. What it does
//! have is the pointer and the last press, so that is what this reads: the
//! panel under the cursor, or, when the cursor is over no panel at all, the
//! panel last pressed in. Both are gestures the user made towards one panel.

use bevy::picking::hover::HoverMap;
use bevy::prelude::*;
use jackdaw_panels::area::DockTabContent;

/// The dock panel the user last pressed in, by window id.
///
/// The answer for a cursor that is over no panel: a chord typed with the
/// pointer parked over the menu bar, or off the window entirely, still belongs
/// to the panel the user was last working in.
#[derive(Resource, Default)]
pub struct LastPressedPanel(pub Option<String>);

/// What an availability check asks "is this panel the one the press belongs
/// to?".
#[derive(bevy::ecs::system::SystemParam)]
pub struct PanelFocus<'w, 's> {
    hover_map: Res<'w, HoverMap>,
    parents: Query<'w, 's, &'static ChildOf>,
    contents: Query<'w, 's, (Entity, &'static DockTabContent)>,
    last_pressed: Res<'w, LastPressedPanel>,
}

impl PanelFocus<'_, '_> {
    /// Whether the panel `window_id` names is the one a keypress belongs to.
    ///
    /// The panel under the cursor wins: a workspace can show the timeline and
    /// the outliner at once, and the cursor is what says which of the two the
    /// user is working in. With the cursor over no panel, the last press
    /// answers.
    pub fn is_focused(&self, window_id: &str) -> bool {
        match hovered_panel(&self.hover_map, &self.parents, &self.contents) {
            Some(hovered) => hovered == window_id,
            None => self.last_pressed.0.as_deref() == Some(window_id),
        }
    }
}

/// The window id of the dock panel under the cursor, if any.
fn hovered_panel(
    hover_map: &HoverMap,
    parents: &Query<&ChildOf>,
    contents: &Query<(Entity, &DockTabContent)>,
) -> Option<String> {
    hover_map
        .iter()
        .filter(|(pointer, _)| !pointer.is_custom())
        .flat_map(|(_, hits)| hits.keys())
        .find_map(|hovered| panel_of(*hovered, parents, contents))
}

/// The window id of the panel `entity` sits inside, if it sits in one.
fn panel_of(
    entity: Entity,
    parents: &Query<&ChildOf>,
    contents: &Query<(Entity, &DockTabContent)>,
) -> Option<String> {
    core::iter::successors(Some(entity), |entity| {
        parents.get(*entity).ok().map(ChildOf::parent)
    })
    .find_map(|ancestor| {
        contents
            .get(ancestor)
            .ok()
            .map(|(_, content)| content.window_id.clone())
    })
}

/// Record the panel a press landed in.
fn remember_pressed_panel(
    press: On<Pointer<Press>>,
    parents: Query<&ChildOf>,
    contents: Query<(Entity, &DockTabContent)>,
    mut last_pressed: ResMut<LastPressedPanel>,
) {
    if let Some(window_id) = panel_of(press.entity, &parents, &contents) {
        last_pressed.0 = Some(window_id);
    }
}

pub struct PanelFocusPlugin;

impl Plugin for PanelFocusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LastPressedPanel>()
            .add_observer(remember_pressed_panel);
    }
}
