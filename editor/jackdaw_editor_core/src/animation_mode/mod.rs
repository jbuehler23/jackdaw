//! Animation state the core reads without the animation editor: the names the
//! animation editor and its windows go by, and the clip library the remote
//! asset listing answers from.

pub mod library;

pub use library::{AnimationLibrary, LibraryClip, LibraryDemand, LibraryFile};

use bevy::prelude::*;

/// Catalog name of the animation editor extension, which owns the clip,
/// keyframe, preview and graph operators.
pub const ANIMATION_EDITOR_EXTENSION_ID: &str = "jackdaw.animation_editor";

/// The Animation panel, which claims the clipboard chords and Delete in its
/// own bounds.
pub const TIMELINE_WINDOW_ID: &str = "jackdaw.timeline";

/// The animation graph canvas.
pub const GRAPH_WINDOW_ID: &str = "jackdaw.animation_graph";

pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(library::plugin);
}
