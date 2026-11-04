//! The Animation panel and the clip library behind it.

pub mod graph_doc;
pub mod graph_ops;
pub mod graph_window;
pub mod library;
pub mod markers;
pub mod panel;
pub mod preview;
pub mod timeline_glue;
pub mod timeline_ops;

pub use graph_doc::AnimationGraphDoc;
pub use graph_window::{GRAPH_WINDOW_ID, animation_graph_window_content};
pub use library::{AnimationLibrary, LibraryClip, LibraryDemand, LibraryFile};
pub use panel::{AnimationPanelState, AnimationPanelTab, animation_panel_content};
pub use preview::{AnimationPreview, PreviewMannequin};

use bevy::prelude::*;
use jackdaw_api::prelude::*;

pub(crate) fn plugin(app: &mut App) {
    app.add_plugins((
        graph_window::plugin,
        library::plugin,
        markers::plugin,
        panel::plugin,
        preview::plugin,
        timeline_glue::plugin,
    ));
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<panel::AnimationPanelTabOp>()
        .register_operator::<panel::AnimationLibrarySelectOp>()
        .register_operator::<panel::AnimationLibraryAddStateOp>()
        .register_operator::<preview::AnimationPreviewOp>()
        .register_operator::<preview::AnimationPreviewPauseOp>()
        .register_operator::<preview::AnimationPreviewStopOp>();
    graph_ops::add_to_extension(ctx);
    timeline_ops::add_to_extension(ctx);
}
