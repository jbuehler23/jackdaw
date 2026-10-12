//! An extension's App plugin is added while the editor is built, and only
//! then: one that arrives or is enabled later waits for a restart.

use crate::util;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::{
    Extension, ExtensionAppExt as _, ExtensionAppPlugins, enable_extension,
    register_dylib_extension,
};

const ID: &str = "app_plugin_sample";

#[derive(Resource, Default)]
struct Ticks(u32);

fn count_ticks(mut ticks: ResMut<Ticks>) {
    ticks.0 += 1;
}

fn sample_plugin(app: &mut App) {
    app.init_resource::<Ticks>()
        .add_systems(Update, count_ticks);
}

#[derive(Resource)]
struct Registered;

#[derive(Default)]
struct PluginExtension;

impl JackdawExtension for PluginExtension {
    fn id(&self) -> String {
        ID.to_string()
    }

    fn app_plugin(&self) -> Option<Box<dyn Plugin>> {
        Some(Box::new(sample_plugin))
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.insert_resource(Registered);
    }
}

/// A rebuild of [`PluginExtension`] that keeps its plugin.
#[derive(Default)]
struct RebuiltExtension;

impl JackdawExtension for RebuiltExtension {
    fn id(&self) -> String {
        ID.to_string()
    }

    fn app_plugin(&self) -> Option<Box<dyn Plugin>> {
        Some(Box::new(sample_plugin))
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.insert_resource(Registered);
    }
}

fn is_loaded(app: &mut App) -> bool {
    let world = app.world_mut();
    world
        .query::<&Extension>()
        .iter(world)
        .any(|extension| extension.id == ID)
}

fn started(mut app: App) -> App {
    app.finish();
    app.update();
    app.update();
    app
}

#[test]
fn an_extension_enabled_at_launch_adds_its_plugin_before_the_app_runs() {
    let mut app = util::headless_app();
    app.register_extension::<PluginExtension>();
    let mut app = started(app);

    assert!(is_loaded(&mut app));
    assert!(app.world().contains_resource::<Registered>());
    assert!(
        app.world().resource::<Ticks>().0 >= 2,
        "the plugin's system runs every frame"
    );
    assert!(
        !app.world()
            .resource::<ExtensionAppPlugins>()
            .restart_required(ID)
    );
}

#[test]
fn an_extension_loaded_after_launch_waits_for_a_restart() {
    let mut app = util::editor_test_app();
    register_dylib_extension(app.world_mut(), || Box::new(PluginExtension));

    assert!(enable_extension(app.world_mut(), ID).is_none());
    app.update();

    assert!(!is_loaded(&mut app));
    assert!(!app.world().contains_resource::<Ticks>());
    assert!(!app.world().contains_resource::<Registered>());
    let app_plugins = app.world().resource::<ExtensionAppPlugins>();
    assert!(app_plugins.restart_required(ID));
    assert_eq!(app_plugins.pending_state(ID), Some(true));
}

#[test]
fn an_extension_turned_off_at_launch_waits_for_a_restart_when_enabled() {
    util::isolate_config_dir();
    jackdaw_api_internal::extensions_config::set_extension_enabled(ID, false);
    let mut app = util::headless_app();
    app.register_extension::<PluginExtension>();
    let mut app = started(app);

    assert!(!is_loaded(&mut app));
    assert!(!app.world().contains_resource::<Ticks>());

    assert!(enable_extension(app.world_mut(), ID).is_none());
    assert!(!is_loaded(&mut app));
    assert!(
        app.world()
            .resource::<ExtensionAppPlugins>()
            .restart_required(ID)
    );
}

#[test]
fn a_rebuild_that_keeps_the_plugin_activates_live() {
    let mut app = util::headless_app();
    app.register_extension::<PluginExtension>();
    let mut app = started(app);
    let ticks = app.world().resource::<Ticks>().0;

    let world = app.world_mut();
    let rebuilt = RebuiltExtension;
    assert!(
        world
            .resource::<ExtensionAppPlugins>()
            .can_activate(&rebuilt)
    );
    jackdaw_api_internal::lifecycle::disable_extension(world, ID);
    register_dylib_extension(world, || Box::new(RebuiltExtension));
    assert!(enable_extension(world, ID).is_some());
    app.update();

    assert!(is_loaded(&mut app));
    assert!(app.world().resource::<Ticks>().0 > ticks);
}
