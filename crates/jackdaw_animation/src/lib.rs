//! Animation authoring and playback for the Jackdaw editor.
//!
//! Thin UI layer over Bevy's `AnimationClip`, `AnimationGraph`, and
//! `AnimationPlayer`. Authored data (`Clip`, `AnimationTrack`,
//! keyframes) lives in the scene AST and compiles into real Bevy
//! assets at runtime. No custom curve evaluator; everything flows
//! through Bevy's own playback path.
//!
//! All mutations go through `SpawnEntity` / `SetBsnField` /
//! `DespawnEntity`. No custom `EditorCommand` types.

use bevy::prelude::*;

pub mod blend_graph;
pub mod clip;
pub mod commands;
pub mod compile;
pub mod graph_owner;
pub mod player;
pub mod sheet;
pub mod timeline;
pub mod toolbar;

pub use blend_graph::{AdditiveBlendNode, AnimationBlendGraph, BlendNode, ClipNodeRef, OutputNode};
pub use clip::{
    AnimationTrack, Clip, ClipRecording, F32Keyframe, GltfClipRef, ImportedClipView, Interpolation,
    KeyframeClipboard, KeyframeClipboardEntry, KeyframeValue, LoopMode, OnionSkin, QuatKeyframe,
    SelectedClip, SelectedKeyframes, SelectedTrack, TimelineSnap, TimelineSnapHint, TimelineView,
    TimelineZoom, Vec3Keyframe,
};
pub use compile::{
    CompiledClip, clip_display_duration, compile_blend_graphs, compile_clips, max_keyframe_time,
};
pub use graph_owner::{LoanedPlayer, PlayerLoan, lend_player, return_player};
pub use jackdaw_animation_runtime::ClipEvent;
pub use player::{
    ActiveClipBinding, AnimationPause, AnimationPlay, AnimationSeek, AnimationStop, TimelineCursor,
    TimelineEngagement, auto_bind_player, handle_pause, handle_play, handle_seek, handle_stop,
    sync_cursor_from_player,
};
pub use sheet::{
    SheetRow, TimelineAddPropertyInput, TimelineEventHandle, TimelineKeyframeHandle,
    TimelineTrackRow, TimelineViewSegment, TimelineZoomSlider,
};
pub use timeline::{
    KeyframeRetimed, KeyframesMarqueeSelected, TimelineCreateBlendGraphButton,
    TimelineCreateClipButton, TimelineDirty, TimelinePanelRoot, clear_snap_hint_on_drag_end,
    handle_scrubber_click, handle_scrubber_drag, handle_scrubber_drag_end,
    handle_scrubber_drag_start, mark_timeline_dirty_on_data_change, pick_tick_step,
    rebuild_timeline, timeline_panel, update_key_readout, update_keyframe_highlight,
    update_playhead_position,
};
pub use toolbar::{
    TimelineClipSelector, TimelineDurationInput, TimelineFrameInput, TimelineLoopSegment,
    TimelineSnapRateInput, TimelineSpeedInput, TimelineTimeInput,
};

/// Plugin that registers the animation authoring data model and wires
/// up the compile + playback systems. Add this to the editor app once,
/// after the Bevy default plugins and the JSN AST layer.
pub struct AnimationPlugin;

impl Plugin for AnimationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SelectedClip>()
            .init_resource::<SelectedKeyframes>()
            .init_resource::<KeyframeClipboard>()
            .init_resource::<TimelineCursor>()
            .init_resource::<TimelineDirty>()
            .init_resource::<TimelineSnap>()
            .init_resource::<TimelineSnapHint>()
            .init_resource::<TimelineView>()
            .init_resource::<TimelineZoom>()
            .init_resource::<SelectedTrack>()
            .init_resource::<ClipRecording>()
            .init_resource::<OnionSkin>()
            .init_resource::<ImportedClipView>()
            .init_resource::<timeline::TimelineMarquee>()
            .init_resource::<timeline::KeyframeDragOrigin>()
            .init_resource::<ActiveClipBinding>()
            .init_resource::<TimelineEngagement>()
            .add_message::<AnimationPlay>()
            .add_message::<AnimationPause>()
            .add_message::<AnimationStop>()
            .add_message::<AnimationSeek>()
            .add_message::<KeyframeRetimed>()
            .add_message::<KeyframesMarqueeSelected>()
            .register_type::<Clip>()
            .register_type::<AnimationTrack>()
            .register_type::<Interpolation>()
            .register_type::<LoopMode>()
            .register_type::<Vec3Keyframe>()
            .register_type::<QuatKeyframe>()
            .register_type::<F32Keyframe>()
            .register_type::<GltfClipRef>()
            .register_type::<AnimationBlendGraph>()
            .register_type::<ClipNodeRef>()
            .register_type::<BlendNode>()
            .register_type::<AdditiveBlendNode>()
            .register_type::<OutputNode>()
            .add_observer(handle_scrubber_click)
            .add_observer(handle_scrubber_drag)
            .add_observer(handle_scrubber_drag_start)
            .add_observer(handle_scrubber_drag_end)
            .add_observer(clear_snap_hint_on_drag_end)
            .add_observer(timeline::handle_keyframe_drag)
            .add_observer(timeline::handle_keyframe_drag_start)
            .add_observer(timeline::handle_keyframe_drag_end)
            .add_observer(timeline::handle_marquee_start)
            .add_observer(timeline::handle_marquee_drag)
            .add_observer(timeline::handle_marquee_end)
            .add_systems(Startup, blend_graph::register_animation_node_types)
            .add_systems(PostUpdate, (compile_clips, compile_blend_graphs).chain())
            .add_systems(
                Update,
                (
                    auto_bind_player,
                    handle_play,
                    handle_pause,
                    handle_stop,
                    handle_seek,
                    sync_cursor_from_player,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    mark_timeline_dirty_on_data_change,
                    rebuild_timeline,
                    update_playhead_position,
                    update_keyframe_highlight,
                    update_key_readout,
                    timeline::update_marquee_box,
                )
                    .chain()
                    .after(sync_cursor_from_player),
            );
    }
}
