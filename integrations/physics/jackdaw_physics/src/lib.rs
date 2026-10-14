//! The Physics module's editor tooling.
//!
//! [`PhysicsExtension`] brings avian into the editor: scenes' rigid bodies
//! and colliders become live avian components, brushes start with a static
//! body and a collider, the inspector's Physics category gets Rigid Body and
//! Collider toggles, the viewport can draw collider wireframes, and the
//! physics tool simulates the selected bodies until Space keeps where they
//! settled.
//!
//! The game side is [`jackdaw_avian_integration`], which builds the same
//! colliders from the same authored components.

mod bodies;
mod components;
mod overlays;
mod simulation;
mod tool;

use avian3d::prelude::{PhysicsPlugins, PhysicsSchedulePlugin};
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_avian_integration::AvianColliderBridgePlugin;

pub use components::{PHYSICS_CATEGORY, PhysicsDisableOp, PhysicsEnableOp, is_physics_type_path};
pub use overlays::{
    PhysicsOverlayConfig, ViewToggleColliderGizmosOp, ViewToggleHierarchyArrowsOp, physics_colors,
};
pub use simulation::{PhysicsDrag, PhysicsToolState};
pub use tool::{PhysicsActivateOp, PhysicsCommitOp, STATUS, is_physics_tool_running};

/// The id the Physics module's extension registers under.
pub const EXTENSION_ID: &str = "jackdaw.physics";

/// The Physics module's editor extension.
#[derive(Default)]
pub struct PhysicsExtension;

impl JackdawExtension for PhysicsExtension {
    fn id(&self) -> String {
        EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Physics".to_string()
    }

    fn description(&self) -> String {
        "Rigid bodies and colliders with avian: the physics tool, collider overlays and physics \
         toggles in the inspector."
            .to_string()
    }

    fn register(&self, ctx: &mut ExtensionRegistrar<'_>) {
        tool::register(ctx);
        components::register(ctx);
        overlays::register(ctx);
    }

    fn app_plugin(&self) -> Option<Box<dyn Plugin>> {
        Some(Box::new(PhysicsEditorPlugin))
    }
}

/// Avian and the systems the extension runs whatever window is open.
pub struct PhysicsEditorPlugin;

impl Plugin for PhysicsEditorPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<PhysicsSchedulePlugin>() {
            app.add_plugins(PhysicsPlugins::default());
        }
        app.add_plugins(AvianColliderBridgePlugin).add_plugins((
            simulation::plugin,
            overlays::plugin,
            tool::plugin,
            bodies::plugin,
            components::plugin,
        ));
    }
}
