//! Transport and target adoption. Installs/strips Bevy's runtime
//! animation components (`AnimationPlayer`, `AnimationGraphHandle`,
//! `AnimationTargetId`, `AnimatedBy`) based on engagement state.
//! None of these are persisted.

use bevy::animation::{AnimationPlayer, AnimationTargetId};
use bevy::prelude::*;

use crate::blend_graph::{AnimationBlendGraph, ClipNodeRef, OutputNode};
use crate::clip::{Clip, SelectedClip};
use crate::compile::{CompiledClip, clip_display_duration};
use crate::graph_owner::{PlayerLoan, lend_player, return_player};

/// Which (clip, host entity) pair the transport currently drives.
/// `target` is the entity whose player was borrowed.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct ActiveClipBinding {
    pub clip: Option<Entity>,
    pub target: Option<Entity>,
}

/// Whether runtime animation components are installed on the target.
/// `Active` during scrub/play; `Idle` otherwise so the target's
/// Transform is freely editable.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineEngagement {
    #[default]
    Idle,
    Active,
}

/// Editor playhead state. `seek_time` mirrors Bevy's
/// `ActiveAnimation::seek_time`.
#[derive(Resource, Debug, Clone, Copy)]
pub struct TimelineCursor {
    pub seek_time: f32,
    pub is_playing: bool,
}

impl Default for TimelineCursor {
    fn default() -> Self {
        Self {
            seek_time: 0.0,
            is_playing: false,
        }
    }
}

impl TimelineCursor {
    #[inline]
    pub fn is_paused(&self) -> bool {
        !self.is_playing
    }
}

/// Play transport message.
#[derive(Message, Debug, Clone, Copy)]
pub struct AnimationPlay;

/// Pause transport message.
#[derive(Message, Debug, Clone, Copy)]
pub struct AnimationPause;

/// Stop transport message. Rewinds the cursor to 0.
#[derive(Message, Debug, Clone, Copy)]
pub struct AnimationStop;

/// Seek transport message. Sets cursor to the given time.
#[derive(Message, Debug, Clone, Copy)]
pub struct AnimationSeek(pub f32);

/// Borrow or give back the target's animation player based on
/// `TimelineEngagement`. Active = borrow; Idle = give it back so the
/// target's Transform is freely editable again.
///
/// The borrow goes through [`crate::graph_owner`], which is also what the
/// editor's clip preview uses, so neither strips a graph a bound animation
/// set installed.
pub fn auto_bind_player(
    selected: Res<SelectedClip>,
    engagement: Res<TimelineEngagement>,
    mut bound: ResMut<ActiveClipBinding>,
    mut cursor: ResMut<TimelineCursor>,
    compiled: Query<&CompiledClip>,
    blend_graphs: Query<(), With<AnimationBlendGraph>>,
    clip_refs: Query<&ClipNodeRef>,
    outputs: Query<(), With<OutputNode>>,
    graph_connections: Query<&jackdaw_node_graph::Connection>,
    parents: Query<&ChildOf>,
    names: Query<&Name>,
    children_q: Query<&Children>,
    mut commands: Commands,
) {
    let want_bound = *engagement == TimelineEngagement::Active && selected.0.is_some();
    let currently_bound = bound.target.is_some() && bound.clip == selected.0;

    if want_bound == currently_bound && !want_bound {
        // Idle and already stripped - nothing to do.
        return;
    }
    if want_bound && currently_bound {
        // Already bound to the right clip. Nothing to do.
        return;
    }

    // Give back the previous bind (covers both "deactivating" and
    // "switching clips while active") so we can't leave a borrowed player
    // behind.
    if let Some(old_target) = bound.target.take() {
        commands.queue(move |world: &mut World| {
            return_player(world, old_target);
        });
    }
    bound.clip = None;

    if !want_bound {
        cursor.is_playing = false;
        return;
    }

    // From here on: engagement is Active and we need to install on
    // the clip's target.
    let Some(clip_entity) = selected.0 else {
        return;
    };

    // Blend graph passthrough: resolve the selected clip through its
    // single `ClipRef -> Output` edge so runtime binding happens
    // against the *referenced* clip's target instead of the blend
    // graph's own parent. The blend graph's `CompiledClip` already
    // points at the same handles as the referenced clip, so all we
    // need to do is redirect the target resolution.
    let effective_clip = if blend_graphs.contains(clip_entity) {
        match resolve_blend_graph_passthrough_source(
            clip_entity,
            &children_q,
            &graph_connections,
            &clip_refs,
            &outputs,
        ) {
            Some(source) => source,
            None => return,
        }
    } else {
        clip_entity
    };

    // Clip not compiled yet (compile runs in PostUpdate; we're in
    // Update). Retry next frame.
    let Ok(compiled) = compiled.get(clip_entity) else {
        return;
    };
    let Ok(clip_parent) = parents.get(effective_clip) else {
        return;
    };
    let parent_entity = clip_parent.parent();

    let graph = compiled.graph.clone();
    let root_node = compiled.root_node;
    let seek_time = cursor.seek_time;
    let start_playing = cursor.is_playing;

    let Ok(target_name) = names.get(parent_entity) else {
        return;
    };
    let target_id = AnimationTargetId::from_name(target_name);

    commands.queue(move |world: &mut World| {
        // Bevy evaluates paused animations at their `seek_time` without
        // advancing time, so the scrub flow can leave the clip paused and
        // still preview the frame it is parked on.
        lend_player(
            world,
            parent_entity,
            PlayerLoan::new(graph, root_node)
                .at(seek_time, start_playing)
                .addressing_itself(target_id),
        );
    });

    bound.clip = Some(clip_entity);
    bound.target = Some(parent_entity);
}

/// Walk a blend graph's single `ClipRef` -> Output connection to find
/// the clip being passed through. Only recognizes "one clip ref, one
/// output, one connection." Returns `None` if incomplete.
fn resolve_blend_graph_passthrough_source(
    blend_graph_entity: Entity,
    children_q: &Query<&Children>,
    connections: &Query<&jackdaw_node_graph::Connection>,
    clip_refs: &Query<&ClipNodeRef>,
    outputs: &Query<(), With<OutputNode>>,
) -> Option<Entity> {
    let graph_children = children_q.get(blend_graph_entity).ok()?;
    let output_node = graph_children.iter().find(|c| outputs.contains(*c))?;
    let incoming: Vec<&jackdaw_node_graph::Connection> = graph_children
        .iter()
        .filter_map(|c| connections.get(c).ok())
        .filter(|c| c.target_node == output_node)
        .collect();
    if incoming.len() != 1 {
        return None;
    }
    let source_node = incoming[0].source_node;
    let clip_ref = clip_refs.get(source_node).ok()?;
    if clip_ref.clip_entity == Entity::PLACEHOLDER {
        return None;
    }
    Some(clip_ref.clip_entity)
}

pub fn handle_play(
    mut events: MessageReader<AnimationPlay>,
    mut cursor: ResMut<TimelineCursor>,
    mut engagement: ResMut<TimelineEngagement>,
    bound: Res<ActiveClipBinding>,
    clips: Query<&CompiledClip>,
    mut players: Query<&mut AnimationPlayer>,
) {
    if events.read().count() == 0 {
        return;
    }
    cursor.is_playing = true;
    *engagement = TimelineEngagement::Active;

    // If we happen to already be bound (e.g. coming out of a pause),
    // resume the player in place. If we're Idle, auto_bind_player
    // will install a freshly-unpaused player on the next frame based
    // on `cursor.is_playing == true`.
    let (Some(clip_entity), Some(target_entity)) = (bound.clip, bound.target) else {
        return;
    };
    let Ok(compiled) = clips.get(clip_entity) else {
        return;
    };
    if let Ok(mut player) = players.get_mut(target_entity) {
        if player.animation_mut(compiled.root_node).is_none() {
            player.play(compiled.root_node);
        }
        if let Some(active) = player.animation_mut(compiled.root_node) {
            active.seek_to(cursor.seek_time);
            active.resume();
        }
    }
}

pub fn handle_pause(
    mut events: MessageReader<AnimationPause>,
    mut cursor: ResMut<TimelineCursor>,
    bound: Res<ActiveClipBinding>,
    clips: Query<&CompiledClip>,
    mut players: Query<&mut AnimationPlayer>,
) {
    if events.read().count() == 0 {
        return;
    }
    cursor.is_playing = false;
    // Deliberately leave engagement alone: pausing keeps the target
    // bound so the user can see the frozen frame. Stop is the action
    // that releases the target.
    let (Some(clip_entity), Some(target_entity)) = (bound.clip, bound.target) else {
        return;
    };
    let Ok(compiled) = clips.get(clip_entity) else {
        return;
    };
    if let Ok(mut player) = players.get_mut(target_entity)
        && let Some(active) = player.animation_mut(compiled.root_node)
    {
        active.pause();
    }
}

pub fn handle_stop(
    mut events: MessageReader<AnimationStop>,
    mut cursor: ResMut<TimelineCursor>,
    mut engagement: ResMut<TimelineEngagement>,
) {
    if events.read().count() == 0 {
        return;
    }
    cursor.seek_time = 0.0;
    cursor.is_playing = false;
    // Drop engagement to Idle - auto_bind_player will strip the
    // runtime components on the next frame, releasing the target so
    // the user can edit its Transform via gizmos again.
    *engagement = TimelineEngagement::Idle;
}

pub fn handle_seek(
    mut events: MessageReader<AnimationSeek>,
    mut cursor: ResMut<TimelineCursor>,
    bound: Res<ActiveClipBinding>,
    clips: Query<&CompiledClip>,
    mut players: Query<&mut AnimationPlayer>,
) {
    let Some(AnimationSeek(time)) = events.read().last().copied() else {
        return;
    };
    cursor.seek_time = time;
    let (Some(clip_entity), Some(target_entity)) = (bound.clip, bound.target) else {
        return;
    };
    let Ok(compiled) = clips.get(clip_entity) else {
        return;
    };
    if let Ok(mut player) = players.get_mut(target_entity)
        && let Some(active) = player.animation_mut(compiled.root_node)
    {
        active.seek_to(time);
    }
}

/// While playing, mirror the Bevy animation's seek time back into the
/// cursor so the timeline widget draws an accurate playhead. The clip
/// duration is derived from the keyframe data at every call, not
/// stored as authored data.
pub fn sync_cursor_from_player(
    mut cursor: ResMut<TimelineCursor>,
    bound: Res<ActiveClipBinding>,
    compiled: Query<&CompiledClip>,
    clips: Query<(&Clip, Option<&Children>)>,
    players: Query<&AnimationPlayer>,
) {
    if !cursor.is_playing {
        return;
    }
    let (Some(clip_entity), Some(target_entity)) = (bound.clip, bound.target) else {
        return;
    };
    let Ok(compiled) = compiled.get(clip_entity) else {
        return;
    };
    let duration = clip_display_duration(clip_entity, &clips);
    if let Ok(player) = players.get(target_entity)
        && let Some(active) = player.animation(compiled.root_node)
    {
        cursor.seek_time = active.seek_time().clamp(0.0, duration);
    }
}
