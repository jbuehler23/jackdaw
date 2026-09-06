//! The Animation panel and the clip library behind it.

pub mod library;
pub mod panel;
pub mod preview;

pub use library::{AnimationLibrary, LibraryClip, LibraryFile};
pub use panel::{AnimationPanelState, AnimationPanelTab, animation_panel_content};
pub use preview::{AnimationPreview, PreviewMannequin};

use bevy::prelude::*;
use jackdaw_api::prelude::*;

pub(crate) fn plugin(app: &mut App) {
    app.add_plugins((library::plugin, panel::plugin, preview::plugin));
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<panel::AnimationPanelTabOp>()
        .register_operator::<panel::AnimationLibrarySelectOp>()
        .register_operator::<panel::AnimationLibraryAddStateOp>()
        .register_operator::<preview::AnimationPreviewOp>()
        .register_operator::<preview::AnimationPreviewPauseOp>()
        .register_operator::<preview::AnimationPreviewStopOp>();
}
