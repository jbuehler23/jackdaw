//! Build custom standalone Jackdaw editors with normal Bevy composition.
//!
//! The official GUI and custom editors use the same plugin group. Game
//! applications should depend on `jackdaw_runtime`, not this crate.

pub use jackdaw::prelude::{editor_window_plugin, primary_window_attributes};
pub use jackdaw::{
    AppState, DylibLoaderPlugin, EditorCorePlugin, ExtensionPlugin, JackdawEditorPlugins,
};

pub mod prelude {
    pub use jackdaw::prelude::{
        AppState, DylibLoaderPlugin, EditorCorePlugin, EnhancedInputPlugin, ExtensionPlugin,
        JackdawEditorPlugins, PhysicsPlugins, editor_window_plugin, primary_window_attributes,
    };
}
