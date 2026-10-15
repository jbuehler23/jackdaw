//! The Navmesh module's editor tooling.
//!
//! [`NavmeshExtension`] adds a Navmesh tool to the terrain tool palette. Its
//! options bar sets the agent the selected terrain is baked for and bakes a
//! navigation mesh from the terrain and the scene's static meshes with
//! `jackdaw_navmesh`, writing it beside the scene as `<scene>.jdnav`. The
//! viewport can draw the baked surface over the ground.
//!
//! The game side needs no crate of its own: `jackdaw_runtime` with its
//! `navmesh` feature reads the baked file onto the scene root as a
//! `JackdawNavmesh`.

mod bake;
mod tool;

use bevy::prelude::*;
use jackdaw_api::prelude::*;

pub use bake::{
    BakeStatus, BakedNavmesh, ShowNavmesh, Staleness, TerrainNavmeshBakeOp, TerrainNavmeshState,
    TerrainNavmeshToggleOverlayOp, navmesh_colors,
};
pub use tool::TerrainToolNavmeshOp;

/// The id the Navmesh module's extension registers under.
pub const EXTENSION_ID: &str = "jackdaw.navmesh";

/// The Navmesh module's editor extension.
#[derive(Default)]
pub struct NavmeshExtension;

impl JackdawExtension for NavmeshExtension {
    fn id(&self) -> String {
        EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Navmesh".to_string()
    }

    fn description(&self) -> String {
        "Bake a navigation mesh from the terrain and draw it over the ground.".to_string()
    }

    fn register(&self, ctx: &mut ExtensionRegistrar<'_>) {
        bake::register(ctx);
        tool::register(ctx);
    }

    fn app_plugin(&self) -> Option<Box<dyn Plugin>> {
        Some(Box::new(NavmeshEditorPlugin))
    }
}

/// The systems the extension runs whatever window is open.
pub struct NavmeshEditorPlugin;

impl Plugin for NavmeshEditorPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((bake::plugin, tool::plugin));
    }
}
