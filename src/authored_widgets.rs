//! Value behaviour for the authored UI widgets.
//!
//! The defaults and the observers live in [`jackdaw_widgets_runtime`], shared
//! with the game runtime so an authored widget behaves the same in both. The
//! editor supplies which entities count as authored: the observers fire only
//! for [`AuthoredWidget`], and here that marker mirrors [`AstNodeRef`]. An
//! entity the scene document has a node for is authored content; editor chrome
//! has none, so the Extensions dialog, the material panel, and the inspector
//! keep their own checkbox state machines.

use bevy::prelude::*;
use jackdaw_bsn::AstNodeRef;
use jackdaw_widgets_runtime::AuthoredWidget;

pub use jackdaw_widgets_runtime::register_widget_defaults;

pub struct AuthoredWidgetPlugin;

impl Plugin for AuthoredWidgetPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(jackdaw_widgets_runtime::AuthoredWidgetPlugin)
            .add_observer(mark_authored_node)
            .add_observer(unmark_authored_node);
    }
}

/// An observer rather than a system: a document load inserts `AstNodeRef` and
/// the first click can arrive in the same frame, before an `Update` pass runs.
fn mark_authored_node(insert: On<Insert, AstNodeRef>, mut commands: Commands) {
    if let Ok(mut entity) = commands.get_entity(insert.event_target()) {
        entity.try_insert(AuthoredWidget);
    }
}

/// A despawn removes `AstNodeRef` too, so the entity is usually gone by the
/// time the command runs; `try_remove` tolerates that.
fn unmark_authored_node(remove: On<Remove, AstNodeRef>, mut commands: Commands) {
    if let Ok(mut entity) = commands.get_entity(remove.event_target()) {
        entity.try_remove::<AuthoredWidget>();
    }
}
