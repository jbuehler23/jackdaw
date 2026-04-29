//! Animation authoring and playback for the Jackdaw editor.
//!
//! Thin UI layer over Bevy's `AnimationClip`, `AnimationGraph`, and
//! `AnimationPlayer`. Authored data (`Clip`, `AnimationTrack`,
//! keyframes) lives in the scene AST and compiles into real Bevy
//! assets at runtime. No custom curve evaluator; everything flows
//! through Bevy's own playback path.
//!
//! All mutations go through `SpawnEntity` / `SetJsnField` /
//! `DespawnEntity`. No custom `EditorCommand` types.

use bevy::prelude::*;

pub mod blend_graph;
pub mod clip;
pub mod commands;
pub mod compile;
pub mod player;
pub mod timeline;

pub use blend_graph::{AdditiveBlendNode, AnimationBlendGraph, BlendNode, ClipNodeRef, OutputNode};
pub use clip::{
    AnimationTrack, Clip, F32Keyframe, GltfClipRef, Interpolation, KeyframeClipboard,
    KeyframeClipboardEntry, KeyframeValue, QuatKeyframe, SelectedClip, SelectedKeyframes,
    TimelineSnap, TimelineSnapHint, Vec3Keyframe,
};
pub use compile::{
    CompiledClip, clip_display_duration, compile_blend_graphs, compile_clips, compile_gltf_clips,
    max_keyframe_time,
};
pub use player::{
    ActiveClipBinding, AnimationPause, AnimationPlay, AnimationSeek, AnimationStop, BindMode,
    TimelineCursor, TimelineEngagement, auto_bind_player, handle_pause, handle_play, handle_seek,
    handle_stop, sync_cursor_from_player,
};
pub use timeline::{
    TimelineAddKeyframeButton, TimelineClipNameInput, TimelineClipSelector,
    TimelineCreateBlendGraphButton, TimelineCreateClipButton, TimelineDirty, TimelineDurationInput,
    TimelineHeaderNewBlendGraphButton, TimelineHeaderNewClipButton, TimelineKeyframeHandle,
    TimelinePanelRoot, TimelinePauseButton, TimelinePlayButton, TimelineStopButton, TrackField,
    clear_snap_hint_on_drag_end, handle_add_keyframe_click, handle_scrubber_click,
    handle_scrubber_drag, handle_scrubber_drag_end, handle_scrubber_drag_start,
    mark_timeline_dirty_on_data_change, pick_tick_step, rebuild_timeline, timeline_panel,
    update_keyframe_highlight, update_playhead_position,
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
            .init_resource::<ActiveClipBinding>()
            .init_resource::<TimelineEngagement>()
            .add_message::<AnimationPlay>()
            .add_message::<AnimationPause>()
            .add_message::<AnimationStop>()
            .add_message::<AnimationSeek>()
            .register_type::<Clip>()
            .register_type::<AnimationTrack>()
            .register_type::<Interpolation>()
            .register_type::<Vec3Keyframe>()
            .register_type::<QuatKeyframe>()
            .register_type::<F32Keyframe>()
            .register_type::<GltfClipRef>()
            .register_type::<AnimationBlendGraph>()
            .register_type::<ClipNodeRef>()
            .register_type::<BlendNode>()
            .register_type::<AdditiveBlendNode>()
            .register_type::<OutputNode>()
            .add_observer(handle_add_keyframe_click)
            .add_observer(handle_scrubber_click)
            .add_observer(handle_scrubber_drag)
            .add_observer(handle_scrubber_drag_start)
            .add_observer(handle_scrubber_drag_end)
            .add_observer(clear_snap_hint_on_drag_end)
            .add_systems(Startup, blend_graph::register_animation_node_types)
            .add_systems(
                PostUpdate,
                (compile_clips, compile_gltf_clips, compile_blend_graphs).chain(),
            )
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
                )
                    .chain()
                    .after(sync_cursor_from_player),
            );
    }
}
