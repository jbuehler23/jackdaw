//! The animation editor for the Jackdaw scene editor: the Animation panel and
//! its timeline, clip and keyframe editing, clip preview, event markers, the
//! animation graph canvas and documents, the inspector's keyframe diamonds and
//! the animation graph cards. The clip data and its runtime stay in the
//! `jackdaw_animation` crates, and the glTF clip library in the editor core.
//!
//! Add [`AnimationEditorPlugin`] after the editor's own plugins.

pub mod anim_diamond;
pub mod graph_card;
pub mod graph_doc;
pub mod graph_ops;
pub mod graph_window;
pub mod keyframe_ops;
pub mod markers;
pub mod panel;
pub mod preview;
pub mod timeline_glue;
pub mod timeline_ops;

pub use graph_doc::AnimationGraphDoc;
pub use graph_window::{GRAPH_WINDOW_ID, animation_graph_window_content};
pub use jackdaw_editor_core::animation_mode::{
    ANIMATION_EDITOR_EXTENSION_ID, AnimationLibrary, LibraryClip, LibraryDemand, LibraryFile,
    TIMELINE_WINDOW_ID, library,
};
pub use panel::{AnimationPanelState, AnimationPanelTab, animation_panel_content};
pub use preview::{AnimationPreview, PreviewMannequin};

use bevy::prelude::*;
use jackdaw_api::DefaultArea;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::ExtensionInputContext;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;
use jackdaw_api_internal::lifecycle::OperatorAction;
use jackdaw_editor_core::asset_migration::UnsavedEditChecks;
use jackdaw_editor_core::inspector::{ComponentCardRegistry, InspectorFieldDecorations};
use jackdaw_feathers::icons::Icon;
use jackdaw_input::{ActionOf, bindings};

/// The animation editor's state, systems, hooks into the editor, and its
/// extensions.
pub struct AnimationEditorPlugin;

impl Plugin for AnimationEditorPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            graph_window::plugin,
            markers::plugin,
            panel::plugin,
            preview::plugin,
            timeline_glue::plugin,
        ))
        .add_systems(
            Update,
            (
                keyframe_ops::follow_scene_selection_to_clip,
                keyframe_ops::sync_selected_keyframes_from_selection,
            )
                .run_if(in_state(jackdaw_editor_core::AppState::Editor)),
        )
        .add_systems(
            Update,
            (
                anim_diamond::decorate_animatable_fields,
                anim_diamond::update_anim_diamond_highlights,
            )
                .in_set(InspectorFieldDecorations),
        )
        .add_observer(anim_diamond::on_diamond_click)
        .add_observer(keyframe_ops::on_create_clip_for_selection)
        .add_observer(keyframe_ops::on_create_blend_graph_for_selection)
        .add_observer(keyframe_ops::on_clip_selector_change)
        .add_observer(keyframe_ops::on_duration_input_commit)
        .add_observer(keyframe_ops::on_timeline_keyframe_click);
        let world = app.world_mut();
        {
            let mut cards = world.get_resource_or_init::<ComponentCardRegistry>();
            cards.register_leading::<jackdaw_animation_runtime::AnimationGraphRef>(
                graph_card::fill_graph_reference_picker,
            );
            cards.register_leading::<jackdaw_animation_runtime::AnimationSet>(
                graph_card::fill_animation_set_actions,
            );
        }
        world
            .get_resource_or_init::<UnsavedEditChecks>()
            .register(graph_doc::unsaved_graph_edit);
        app.register_extension::<AnimationEditorExtension>()
            .register_extension::<TimelineExtension>()
            .register_extension::<AnimationGraphExtension>();
    }
}

/// The animation operators and their keys.
#[derive(Default)]
pub struct AnimationEditorExtension;

impl JackdawExtension for AnimationEditorExtension {
    fn id(&self) -> String {
        ANIMATION_EDITOR_EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Animation Editor".to_string()
    }

    fn description(&self) -> String {
        "Clip, keyframe, preview and animation graph editing.".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        use keyframe_ops::*;

        ctx.register_operator::<ClipDeleteKeyframesOp>()
            .register_operator::<ClipTimelineStepLeftOp>()
            .register_operator::<ClipTimelineStepRightOp>()
            .register_operator::<ClipTimelineJumpPrevOp>()
            .register_operator::<ClipTimelineJumpNextOp>()
            .register_operator::<ClipTimelineJumpStartOp>()
            .register_operator::<ClipTimelineJumpEndOp>()
            .register_operator::<ClipCopyKeyframesOp>()
            .register_operator::<ClipPasteKeyframesOp>()
            .register_operator::<ClipPlayOp>()
            .register_operator::<ClipPauseOp>()
            .register_operator::<ClipStopOp>()
            .register_operator::<ClipNewOp>()
            .register_operator::<ClipNewBlendGraphOp>()
            .register_operator::<AnimationToggleKeyframeOp>()
            .register_operator::<panel::AnimationPanelTabOp>()
            .register_operator::<panel::AnimationLibrarySelectOp>()
            .register_operator::<panel::AnimationLibraryAddStateOp>()
            .register_operator::<preview::AnimationPreviewOp>()
            .register_operator::<preview::AnimationPreviewPauseOp>()
            .register_operator::<preview::AnimationPreviewStopOp>();
        graph_ops::add_to_extension(ctx);
        timeline_ops::add_to_extension(ctx);

        let ext = ctx.id();
        ctx.bind_operator::<ExtensionInputContext, ClipDeleteKeyframesOp>([
            PresetInput::key("Delete"),
            PresetInput::key("Backspace"),
        ]);
        // No press condition on Step Left / Right: deferred (hold-to-repeat, not a bare press).
        ctx.spawn((
            OperatorAction(ClipTimelineStepLeftOp::ID),
            ActionOf(ext),
            bindings![KeyCode::ArrowLeft],
        ));
        ctx.spawn((
            OperatorAction(ClipTimelineStepRightOp::ID),
            ActionOf(ext),
            bindings![KeyCode::ArrowRight],
        ));
        ctx.bind_operator::<ExtensionInputContext, ClipTimelineJumpPrevOp>([PresetInput::key(
            "ArrowLeft",
        )
        .shift()]);
        ctx.bind_operator::<ExtensionInputContext, ClipTimelineJumpNextOp>([PresetInput::key(
            "ArrowRight",
        )
        .shift()]);
        ctx.bind_operator::<ExtensionInputContext, ClipTimelineJumpStartOp>([PresetInput::key(
            "Home",
        )]);
        ctx.bind_operator::<ExtensionInputContext, ClipTimelineJumpEndOp>([PresetInput::key(
            "End",
        )]);
        ctx.bind_operator::<ExtensionInputContext, ClipCopyKeyframesOp>([
            PresetInput::key("KeyC").ctrl_or_super()
        ]);
        ctx.bind_operator::<ExtensionInputContext, ClipPasteKeyframesOp>([PresetInput::key(
            "KeyV",
        )
        .ctrl_or_super()]);
    }
}

/// Clip library and keyframe timeline in the bottom dock.
///
/// The window keeps its `jackdaw.timeline` id so a saved layout still finds
/// it; only what it is called changed.
#[derive(Default)]
pub struct TimelineExtension;

impl JackdawExtension for TimelineExtension {
    fn id(&self) -> String {
        TIMELINE_WINDOW_ID.to_string()
    }

    fn label(&self) -> String {
        "Animation".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(
            WindowDescriptor::new(TIMELINE_WINDOW_ID)
                .with_name("Animation")
                .with_icon(Icon::Film.unicode())
                .with_default_area(DefaultArea::BottomDock)
                .with_priority(1)
                .with_build(|window| {
                    window.spawn(animation_panel_content());
                }),
        );
    }
}

/// The animation graph canvas in the bottom dock.
#[derive(Default)]
pub struct AnimationGraphExtension;

impl JackdawExtension for AnimationGraphExtension {
    fn id(&self) -> String {
        GRAPH_WINDOW_ID.to_string()
    }

    fn label(&self) -> String {
        "Animation Graph".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(
            WindowDescriptor::new(GRAPH_WINDOW_ID)
                .with_name("Graph")
                .with_icon(Icon::Workflow.unicode())
                .with_default_area(DefaultArea::BottomDock)
                .with_priority(2)
                .with_build(|window| {
                    window.spawn(animation_graph_window_content());
                }),
        );
    }
}
