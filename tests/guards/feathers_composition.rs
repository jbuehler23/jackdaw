//! The editor's UI plugins over a game that brought its own.
//!
//! A game whose interface is authored with feathers adds `FeathersPlugins`
//! itself, and the editor loads that game's plugin into its own app. Bevy
//! panics on a plugin added twice, so the editor has to take the group
//! only where nothing else already has.

use crate::util;

use bevy::feathers::{FeathersCorePlugin, FeathersPlugins};
use bevy::input_focus::tab_navigation::TabNavigationPlugin;

#[test]
fn the_editor_builds_over_an_app_that_already_has_feathers() {
    let mut app = util::ambient_app();
    app.add_plugins(FeathersPlugins);
    util::add_editor_plugins(&mut app);

    assert!(
        app.is_plugin_added::<FeathersCorePlugin>(),
        "the group the game added is the one in play",
    );
}

#[test]
fn the_editor_brings_feathers_when_nothing_else_does() {
    let mut app = util::ambient_app();
    util::add_editor_plugins(&mut app);

    assert!(app.is_plugin_added::<FeathersCorePlugin>());
}

/// `FeathersPlugins` is tab navigation plus the feathers core, and tab
/// navigation is a plugin a game can reach for on its own.
#[test]
fn the_editor_builds_over_an_app_that_has_only_tab_navigation() {
    let mut app = util::ambient_app();
    app.add_plugins(TabNavigationPlugin);
    util::add_editor_plugins(&mut app);

    assert!(
        app.is_plugin_added::<FeathersCorePlugin>(),
        "the half the game left out still arrives",
    );
}
