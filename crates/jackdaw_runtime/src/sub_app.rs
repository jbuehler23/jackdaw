//! Game-hosting [`SubApp`] infrastructure.
//!
//! When the editor runs a user's game in PIE (Play-In-Editor) mode,
//! the game lives in a [`SubApp`] tagged with the [`GameSubApp`]
//! label. This isolates the game's [`World`](bevy::ecs::world::World)
//! from the editor's authoring [`World`](bevy::ecs::world::World) so:
//!
//! - The user writes their plugin against vanilla Bevy schedules
//!   ([`Update`](bevy::prelude::Update),
//!   [`Startup`](bevy::prelude::Startup),
//!   [`FixedUpdate`](bevy::prelude::FixedUpdate)); no jackdaw-specific
//!   labels leak into game code.
//! - Editor authoring state can't be mutated by gameplay systems.
//! - Stop / reload tears down the `SubApp`'s world without touching the
//!   editor's world.
//!
//! Mirrors the pattern Bevy itself uses for [`RenderApp`] (see
//! `bevy_render-0.18`'s `initialize_render_app`): a separate
//! [`World`](bevy::ecs::world::World) driven on each frame by Bevy's
//! runner via the registered `update_schedule`, with a `set_extract`
//! callback bridging the two worlds.
//!
//! This module is intentionally tiny — it owns the [`SubApp`]
//! construction; the editor's `EditorPlugins::with_game::<P>()`
//! builder is responsible for actually inserting the [`SubApp`] into
//! the editor's [`App`](bevy::app::App).
//!
//! [`RenderApp`]: bevy::render::RenderApp

use bevy::app::{AppLabel, MainSchedulePlugin, SubApp};
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;

/// [`AppLabel`] for the [`SubApp`] hosting the user's game during PIE.
#[derive(AppLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct GameSubApp;

/// Build a [`SubApp`] configured to host a Bevy game.
///
/// The [`SubApp`] is set up with Bevy's standard
/// [`MainSchedulePlugin`] so the user's plugin sees an app identical
/// in shape to a standalone Bevy app —
/// [`Startup`](bevy::prelude::Startup) /
/// [`Update`](bevy::prelude::Update) /
/// [`FixedUpdate`](bevy::prelude::FixedUpdate) /
/// [`PostUpdate`](bevy::prelude::PostUpdate) all behave normally.
/// `update_schedule` is set to [`bevy::app::Main`] so calling
/// [`SubApp::update`](bevy::app::SubApp::update) ticks the
/// `SubApp`'s main loop one frame's worth.
///
/// The caller is responsible for installing the user's plugin
/// against the returned [`SubApp`] before storing it (typically in
/// a [`GameSubAppHolder`] non-send resource on the editor's main
/// world).
pub fn create_game_sub_app() -> SubApp {
    let mut sub = SubApp::new();
    sub.update_schedule = Some(bevy::app::Main.intern());
    sub.add_plugins(MainSchedulePlugin);
    sub.init_resource::<crate::extract::GameEntityMap>();
    // Default extract: walk `SceneEntity`-tagged authoring entities
    // and mirror their `Transform` + `Name` into the SubApp world.
    // See `crate::extract` for the broader auto-sync direction.
    sub.set_extract(|main_world, sub_world| {
        crate::extract::extract_scene_entities(main_world, sub_world);
        crate::extract::extract_input_events(main_world, sub_world);
        crate::extract::extract_input_state(main_world, sub_world);
    });
    sub
}

/// Type-erased factory closure that builds a fresh [`SubApp`]
/// hosting a user game plugin. Stored on [`GameSubAppHolder`] so
/// Stop can rebuild a clean world without the editor needing to
/// know whether the game came from a static plugin type or a dylib
/// FFI factory.
type PluginRebuildFn = Box<dyn Fn() -> SubApp + Send + Sync>;

/// Wrapper holding the [`SubApp`] as a `NonSend` resource on the
/// editor's main world.
///
/// We don't register the [`SubApp`] via
/// [`App::insert_sub_app`](bevy::app::App::insert_sub_app) because
/// that requires `&mut App` access — unavailable from systems and
/// from the runtime dylib-load path. Storing the `SubApp` as a
/// non-send resource on the editor's `World` lets us:
///
/// - Hot-reload by replacing the resource from a `&mut World`
///   exclusive system (no editor restart, no `&mut App` needed).
/// - Drive the `SubApp` manually via a tiny editor system that
///   gates ticking on `PlayState::Playing`.
/// - Reset on Stop by calling [`Self::reset`], which uses the
///   stored rebuild factory to construct a fresh `SubApp`. Next
///   Play starts from a clean game-world.
///
/// `SubApp` owns a `World` containing potentially-`!Send` resources
/// (asset loaders, render queues), so a non-send resource is the
/// right kind. Driver runs on the main thread.
pub struct GameSubAppHolder {
    pub sub: SubApp,
    rebuild: PluginRebuildFn,
}

impl GameSubAppHolder {
    /// Build a holder from a closure that constructs a fresh
    /// `SubApp` (with the user's plugin already added) on demand.
    /// The closure is invoked once immediately to create the
    /// initial `SubApp`, and again on each [`Self::reset`].
    pub fn new(rebuild: PluginRebuildFn) -> Self {
        let sub = rebuild();
        Self { sub, rebuild }
    }

    /// Tick the `SubApp` one frame. The caller is responsible for
    /// gating on `PlayState::Playing`.
    pub fn update(&mut self, world: &mut World) {
        self.sub.extract(world);
        self.sub.update();
        crate::extract::extract_game_mirrors(self.sub.world_mut(), world);
        if let Some(callback) = world.remove_non_send_resource::<PostUpdateCallback>() {
            (callback.0)(self.sub.world_mut(), world);
            world.insert_non_send_resource(callback);
        }
    }

    /// Drop the current `SubApp` and rebuild from the cached
    /// factory. Used on Stop to reset game state to "as if the game
    /// just started".
    pub fn reset(&mut self) {
        self.sub = (self.rebuild)();
    }
}

/// Hook for the editor to install behavior into `GameSubAppHolder::update`
/// without `jackdaw_runtime` depending on the editor crate.
///
/// Stored as a non-send resource on the editor's main world; the holder's
/// `update` invokes it (if present) after each `SubApp` tick.
pub struct PostUpdateCallback(pub Box<dyn Fn(&mut World, &mut World) + Send + Sync>);
