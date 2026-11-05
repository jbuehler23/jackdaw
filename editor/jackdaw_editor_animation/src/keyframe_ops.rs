//! Clip and keyframe editing on the timeline: the clip operators, the
//! keyframe clipboard and undoable keyframe commands, and the glue that keeps
//! the timeline's selected clip and keyframes in step with the editor
//! selection.

use bevy::picking::events::PointerClick;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_editor_core::animation_mode::TIMELINE_WINDOW_ID;

/// Spawn a new keyframe clip on the same target as the currently-
/// selected clip and make it the new selection. Backs the
/// [`ClipNewOp`] operator so the timeline header button and any
/// future keybind / menu entry share one path.
fn spawn_new_clip_for_selection(world: &mut World) {
    let Some((target, target_name)) = selected_clip_target_with_name(world) else {
        return;
    };
    let clip = world
        .spawn((
            jackdaw_animation::Clip::default(),
            Name::new(format!("{target_name} Clip")),
            ChildOf(target),
        ))
        .id();
    world.spawn((
        jackdaw_animation::AnimationTrack::new(
            "bevy_transform::components::transform::Transform",
            "translation",
        ),
        Name::new(format!("{target_name} / translation")),
        ChildOf(clip),
    ));
    if let Some(mut selected) = world.get_resource_mut::<jackdaw_animation::SelectedClip>() {
        selected.0 = Some(clip);
    }
    if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
        dirty.0 = true;
    }
}

/// Spawn a new blend-graph clip on the same target as the currently-
/// selected clip. Backs the [`ClipNewBlendGraphOp`] operator.
fn spawn_new_blend_graph_for_selection(world: &mut World) {
    let Some((target, target_name)) = selected_clip_target_with_name(world) else {
        return;
    };
    let clip = world
        .spawn((
            jackdaw_animation::Clip::default(),
            jackdaw_animation::AnimationBlendGraph,
            jackdaw_node_graph::NodeGraph {
                title: format!("{target_name} Blend Graph"),
            },
            jackdaw_node_graph::GraphCanvasView::default(),
            Name::new(format!("{target_name} Blend Graph")),
            ChildOf(target),
        ))
        .id();
    world.spawn((
        jackdaw_node_graph::GraphNode {
            node_type: "anim.output".into(),
            position: Vec2::new(400.0, 160.0),
        },
        jackdaw_animation::OutputNode,
        Name::new("Output"),
        ChildOf(clip),
    ));
    if let Some(mut selected) = world.get_resource_mut::<jackdaw_animation::SelectedClip>() {
        selected.0 = Some(clip);
    }
    if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
        dirty.0 = true;
    }
}

/// Look up the parent target entity of the currently-selected clip
/// along with its `Name`. Returns `None` when no clip is selected or
/// the target has no `Name`.
fn selected_clip_target_with_name(world: &World) -> Option<(Entity, String)> {
    let clip_entity = world.resource::<jackdaw_animation::SelectedClip>().0?;
    let target = world.get::<ChildOf>(clip_entity)?.parent();
    let name = world.get::<Name>(target)?;
    Some((target, name.as_str().to_string()))
}

/// Clip selector combobox changed. Maps the selected index to a
/// clip entity and switches `SelectedClip`.
pub(crate) fn on_clip_selector_change(
    event: On<jackdaw_feathers::combobox::ComboBoxChangeEvent>,
    selectors: Query<&jackdaw_animation::TimelineClipSelector>,
    child_of_query: Query<&ChildOf>,
    mut commands: Commands,
) {
    let mut current = event.entity;
    let mut selector = None;
    for _ in 0..6 {
        if let Ok(s) = selectors.get(current) {
            selector = Some(s);
            break;
        }
        let Ok(parent) = child_of_query.get(current) else {
            break;
        };
        current = parent.parent();
    }
    let Some(selector) = selector else {
        return;
    };
    let idx = event.selected;
    let Some(&clip_entity) = selector.sibling_clips.get(idx) else {
        return;
    };
    commands.queue(move |world: &mut World| {
        if let Some(mut selected) = world.get_resource_mut::<jackdaw_animation::SelectedClip>() {
            selected.0 = Some(clip_entity);
        }
        if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
            dirty.0 = true;
        }
    });
}

/// Observer: when the placeholder "Create Blend Graph" button is
/// clicked, spawn a `Clip + AnimationBlendGraph + NodeGraph +
/// GraphCanvasView + Name` entity parented to the primary selection,
/// plus a default `OutputNode` inside it so the canvas has
/// something to connect to. Mirror of
/// [`on_create_clip_for_selection`] for the node-canvas path.
pub(crate) fn on_create_blend_graph_for_selection(
    event: On<jackdaw_feathers::button::ButtonClickEvent>,
    buttons: Query<(), With<jackdaw_animation::TimelineCreateBlendGraphButton>>,
    selection: Res<jackdaw_editor_core::selection::Selection>,
    names: Query<&Name>,
    mut commands: Commands,
) {
    if !buttons.contains(event.entity) {
        return;
    }
    let Some(&primary) = selection.entities.last() else {
        warn!("Create Blend Graph: no entity selected");
        return;
    };
    let Ok(name) = names.get(primary) else {
        warn!(
            "Create Blend Graph: selected entity has no Name. Give it one in the inspector first"
        );
        return;
    };
    let target_name = name.as_str().to_string();

    commands.queue(move |world: &mut World| {
        // The blend graph clip is BOTH a `Clip` and a `NodeGraph`.
        // The canvas widget consumes the NodeGraph side of that
        // entity, and the timeline dock consumes the Clip side. That
        // means children are GraphNodes + Connections rather than
        // AnimationTracks, but `compile_clips` already skips entities
        // marked with `AnimationBlendGraph`, and `rebuild_timeline`
        // branches on the same marker to spawn a canvas instead of
        // the keyframe strip.
        let clip_entity = world
            .spawn((
                jackdaw_animation::Clip::default(),
                jackdaw_animation::AnimationBlendGraph,
                jackdaw_node_graph::NodeGraph {
                    title: format!("{target_name} Blend Graph"),
                },
                jackdaw_node_graph::GraphCanvasView::default(),
                Name::new(format!("{target_name} Blend Graph")),
                ChildOf(primary),
            ))
            .id();

        // Default Output node so the canvas isn't empty on creation
        // and the user has a clear target to wire their Clip
        // Reference into. Positioned near the top-right so there's
        // room for source nodes to the left.
        world.spawn((
            jackdaw_node_graph::GraphNode {
                node_type: "anim.output".into(),
                position: Vec2::new(400.0, 160.0),
            },
            jackdaw_animation::OutputNode,
            Name::new("Output"),
            ChildOf(clip_entity),
        ));

        if let Some(mut selected) = world.get_resource_mut::<jackdaw_animation::SelectedClip>() {
            selected.0 = Some(clip_entity);
        }
        if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
            dirty.0 = true;
        }
    });
}

/// Observer: when the placeholder "Create Clip for Selection" button
/// is clicked, spawn a new `Clip` + `Name` + default `AnimationTrack` for
/// the primary selected entity, directly via `SpawnEntity`. The
/// animation crate deliberately exports no custom commands; this is
/// the minimum-wrapping form of "create a clip."
pub(crate) fn on_create_clip_for_selection(
    event: On<jackdaw_feathers::button::ButtonClickEvent>,
    buttons: Query<(), With<jackdaw_animation::TimelineCreateClipButton>>,
    selection: Res<jackdaw_editor_core::selection::Selection>,
    names: Query<&Name>,
    mut commands: Commands,
) {
    if !buttons.contains(event.entity) {
        return;
    }
    let Some(&primary) = selection.entities.last() else {
        warn!("Create Clip: no entity selected");
        return;
    };
    let Ok(name) = names.get(primary) else {
        warn!("Create Clip: selected entity has no Name. Give it one in the inspector first");
        return;
    };
    let target_name = name.as_str().to_string();

    commands.queue(move |world: &mut World| {
        // Spawn clip entity *as a child of the target*. The clip's
        // position in the hierarchy is what encodes "this animates
        // that": compile/bind/snapshot all walk up from the clip to
        // the parent to find the target. Deletion cascades naturally
        // and renaming the target can't silently break the clip
        // because the target is a live Entity reference, not a
        // String.
        let clip_entity = world
            .spawn((
                jackdaw_animation::Clip::default(),
                Name::new(format!("{target_name} Clip")),
                ChildOf(primary),
            ))
            .id();

        // Default translation track as a child of the clip.
        world.spawn((
            jackdaw_animation::AnimationTrack::new(
                "bevy_transform::components::transform::Transform",
                "translation",
            ),
            Name::new(format!("{target_name} / translation")),
            ChildOf(clip_entity),
        ));

        if let Some(mut selected) = world.get_resource_mut::<jackdaw_animation::SelectedClip>() {
            selected.0 = Some(clip_entity);
        }
        if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
            dirty.0 = true;
        }
    });
}

/// Keep [`jackdaw_animation::SelectedClip`] in lockstep with the main
/// editor's [`jackdaw_editor_core::selection::Selection`] resource so the timeline widget
/// shows the clip relevant to whatever the user is currently working
/// with.
///
/// Two cases are actively updated:
/// - **A.** Primary selection is already an animation entity (clip,
///   track, or keyframe): walk up `ChildOf` until we hit the owning
///   `Clip` marker and select that.
/// - **B.** Primary selection is a regular scene entity: find the
///   first `Clip` among its `Children` and select it. Since clips
///   now live parented to their target, this is a structural lookup
///   rather than a name-based scan.
///
/// **Empty selection is deliberately a no-op.** After deleting a
/// keyframe the main `delete_selected` path clears `Selection`; if
/// we also cleared `SelectedClip` here the timeline would bounce to
/// its placeholder after every keyframe delete. The stale case
/// (deleting a brush cascades through `ChildOf` and takes its clip
/// with it) is already handled by `rebuild_timeline`, which falls
/// through to the placeholder when `clips.get(selected.0)` fails.
///
/// Lives here rather than in `jackdaw_animation` because the animation
/// crate must not import the main editor's `Selection` type.
pub(crate) fn follow_scene_selection_to_clip(
    selection: Res<jackdaw_editor_core::selection::Selection>,
    mut selected_clip: ResMut<jackdaw_animation::SelectedClip>,
    parents: Query<&ChildOf>,
    entity_children: Query<&Children>,
    clip_marker: Query<(), With<jackdaw_animation::Clip>>,
) {
    if !selection.is_changed() {
        return;
    }
    // Empty selection: keep the current clip active so keyframe
    // deletes (which clear `Selection`) don't also reset the
    // timeline's context.
    let Some(&primary) = selection.entities.last() else {
        return;
    };

    // Case A: primary is a clip/track/keyframe; walk up to the clip.
    let mut cursor = primary;
    for _ in 0..8 {
        if clip_marker.contains(cursor) {
            if selected_clip.0 != Some(cursor) {
                selected_clip.0 = Some(cursor);
            }
            return;
        }
        let Ok(parent) = parents.get(cursor) else {
            break;
        };
        cursor = parent.parent();
    }

    // Case B: primary is a regular scene entity; pick the first Clip
    // child under it.
    if let Ok(children) = entity_children.get(primary) {
        for child in children.iter() {
            if clip_marker.contains(child) {
                if selected_clip.0 != Some(child) {
                    selected_clip.0 = Some(child);
                }
                return;
            }
        }
    }

    // Case C: the selected entity is not an animation entity and has
    // no clip children. Clear the active clip so the timeline shows
    // the placeholder with "Create Clip" / "Create Blend Graph".
    // This is distinct from the empty-selection guard at the top:
    // empty selection preserves the clip (so keyframe deletes don't
    // bounce the timeline), but selecting a clipless entity is an
    // explicit context switch.
    selected_clip.0 = None;
}

/// Typed, undo-aware delete command for animation keyframes.
///
/// We don't reuse `DespawnEntity` for keyframes because
/// that path round-trips through Bevy's `DynamicScene::write_to_world`,
/// which doesn't play well with entity ID reuse: after despawn,
/// Bevy may reissue the keyframe's slot to a later-spawned entity,
/// and an undo that restores the snapshot at the original ID can
/// end up clobbering whatever is living at that slot now (the user
/// saw this as "Ctrl+Z deletes my brush").
///
/// This command captures the keyframe's fields directly (`time`,
/// `value`, and parent `track`) and on undo spawns a **fresh**
/// entity with those fields parented to the original track. No
/// ID reuse, no `DynamicScene`, no surprises.
enum DespawnKeyframeCmd {
    Vec3 {
        /// Current entity id. Updated after each undo so redo knows
        /// which live entity to despawn.
        keyframe: Entity,
        track: Entity,
        time: f32,
        value: Vec3,
    },
    Quat {
        keyframe: Entity,
        track: Entity,
        time: f32,
        value: Quat,
    },
    F32 {
        keyframe: Entity,
        track: Entity,
        time: f32,
        value: f32,
    },
}

impl DespawnKeyframeCmd {
    fn relive(&mut self, world: &World) {
        match self {
            Self::Vec3 {
                keyframe, track, ..
            }
            | Self::Quat {
                keyframe, track, ..
            }
            | Self::F32 {
                keyframe, track, ..
            } => {
                *keyframe = jackdaw_editor_core::scene_nodes::live_entity(world, *keyframe);
                *track = jackdaw_editor_core::scene_nodes::live_entity(world, *track);
            }
        }
    }
}

impl jackdaw_commands::EditorCommand for DespawnKeyframeCmd {
    fn execute(&mut self, world: &mut World) {
        self.relive(world);
        let entity = match self {
            Self::Vec3 { keyframe, .. }
            | Self::Quat { keyframe, .. }
            | Self::F32 { keyframe, .. } => *keyframe,
        };
        if let Ok(ent) = world.get_entity_mut(entity) {
            ent.despawn();
        }
    }

    fn undo(&mut self, world: &mut World) {
        self.relive(world);
        let new_id = match self {
            Self::Vec3 {
                track, time, value, ..
            } => world
                .spawn((
                    jackdaw_animation::Vec3Keyframe {
                        time: *time,
                        value: *value,
                    },
                    ChildOf(*track),
                ))
                .id(),
            Self::Quat {
                track, time, value, ..
            } => world
                .spawn((
                    jackdaw_animation::QuatKeyframe {
                        time: *time,
                        value: *value,
                    },
                    ChildOf(*track),
                ))
                .id(),
            Self::F32 {
                track, time, value, ..
            } => world
                .spawn((
                    jackdaw_animation::F32Keyframe {
                        time: *time,
                        value: *value,
                    },
                    ChildOf(*track),
                ))
                .id(),
        };
        match self {
            Self::Vec3 { keyframe, .. }
            | Self::Quat { keyframe, .. }
            | Self::F32 { keyframe, .. } => *keyframe = new_id,
        }
    }

    fn description(&self) -> &str {
        "Delete keyframe"
    }
}

impl DespawnKeyframeCmd {
    /// Try to build a despawn command for `entity`. Returns `None`
    /// if the entity doesn't have any of the known keyframe
    /// component types, so the caller can fall through to a
    /// generic despawn.
    fn try_from_entity(world: &World, entity: Entity) -> Option<Self> {
        let track = world.get::<ChildOf>(entity).map(ChildOf::parent)?;
        if let Some(kf) = world.get::<jackdaw_animation::Vec3Keyframe>(entity) {
            return Some(Self::Vec3 {
                keyframe: entity,
                track,
                time: kf.time,
                value: kf.value,
            });
        }
        if let Some(kf) = world.get::<jackdaw_animation::QuatKeyframe>(entity) {
            return Some(Self::Quat {
                keyframe: entity,
                track,
                time: kf.time,
                value: kf.value,
            });
        }
        if let Some(kf) = world.get::<jackdaw_animation::F32Keyframe>(entity) {
            return Some(Self::F32 {
                keyframe: entity,
                track,
                time: kf.time,
                value: kf.value,
            });
        }
        None
    }
}

/// Interceptor that runs before the entity-delete operator fires and
/// steals the Delete key for any selected keyframe entities.
/// Each keyframe gets wrapped in a [`DespawnKeyframeCmd`], the
/// commands are grouped and pushed onto the history, and the
/// keyframes are removed from [`jackdaw_editor_core::selection::Selection`] so the
/// downstream generic delete handler ignores them.
///
/// Mixed selections (keyframes + a scene entity) work: this system
/// handles the keyframes, then `handle_entity_keys` handles the
/// remaining non-keyframe entities normally. Both halves land on
/// the history as independent commands, which is fine: undo
/// reverses them in push order.
/// Delete all keyframe entities currently in [`jackdaw_editor_core::selection::Selection`]
/// as a single undoable group. Strips them from the selection first so
/// any non-keyframe entities still in the selection get processed by
/// the generic `EntityDeleteOp` in the same press.
#[operator(
    id = "clip.delete_keyframes",
    label = "Delete Keyframes",
    description = "Remove the selected animation keyframes.",
    is_available = has_selected_keyframes,
)]
pub(crate) fn clip_delete_keyframes(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let selected: Vec<Entity> = world
            .resource::<jackdaw_editor_core::selection::Selection>()
            .entities
            .clone();
        let mut kf_cmds: Vec<Box<dyn jackdaw_commands::EditorCommand>> = Vec::new();
        let mut keyframe_ids: Vec<Entity> = Vec::new();
        for &entity in &selected {
            if let Some(cmd) = DespawnKeyframeCmd::try_from_entity(world, entity) {
                keyframe_ids.push(entity);
                kf_cmds.push(Box::new(cmd));
            }
        }
        if kf_cmds.is_empty() {
            return;
        }
        {
            let mut selection = world.resource_mut::<jackdaw_editor_core::selection::Selection>();
            selection.entities.retain(|e| !keyframe_ids.contains(e));
        }
        for entity in &keyframe_ids {
            if let Ok(mut ent) = world.get_entity_mut(*entity) {
                ent.remove::<jackdaw_editor_core::selection::Selected>();
            }
        }
        for cmd in &mut kf_cmds {
            cmd.execute(world);
        }
        let group = jackdaw_editor_core::commands::CommandGroup {
            commands: kf_cmds,
            label: "Delete keyframes".to_string(),
        };
        let mut history = world.resource_mut::<jackdaw_commands::CommandHistory>();
        history.push_executed(Box::new(group));
    });
    OperatorResult::Finished
}

/// Keyframes are selected and the timeline is the focused window. The
/// entity clipboard refuses under the same condition, so one `Ctrl+C`
/// copies keyframes or a subtree and never both.
fn has_selected_keyframes(
    input_focus: Res<bevy::input_focus::InputFocus>,
    selection: Res<jackdaw_editor_core::selection::Selection>,
    panel_focus: jackdaw_editor_core::panel_focus::PanelFocus,
    keyframes: Query<
        (),
        bevy::ecs::query::Or<(
            With<jackdaw_animation::Vec3Keyframe>,
            With<jackdaw_animation::QuatKeyframe>,
            With<jackdaw_animation::F32Keyframe>,
        )>,
    >,
) -> bool {
    if input_focus.get().is_some() {
        return false;
    }
    if !panel_focus.is_focused(TIMELINE_WINDOW_ID) {
        return false;
    }
    selection.entities.iter().any(|&e| keyframes.contains(e))
}

fn timeline_with_clip(
    input_focus: Res<bevy::input_focus::InputFocus>,
    active: ActiveModalQuery,
    panel_focus: jackdaw_editor_core::panel_focus::PanelFocus,
    selected_clip: Res<jackdaw_animation::SelectedClip>,
) -> bool {
    if input_focus.get().is_some() || active.is_modal_running() {
        return false;
    }
    if !panel_focus.is_focused(TIMELINE_WINDOW_ID) {
        return false;
    }
    selected_clip.0.is_some()
}

fn timeline_paste_available(
    input_focus: Res<bevy::input_focus::InputFocus>,
    active: ActiveModalQuery,
    panel_focus: jackdaw_editor_core::panel_focus::PanelFocus,
    selected_clip: Res<jackdaw_animation::SelectedClip>,
    clipboard: Res<jackdaw_animation::KeyframeClipboard>,
) -> bool {
    if input_focus.get().is_some() || active.is_modal_running() {
        return false;
    }
    if !panel_focus.is_focused(TIMELINE_WINDOW_ID) {
        return false;
    }
    selected_clip.0.is_some() && !clipboard.entries.is_empty()
}

/// Step the playhead one ruler tick to the left.
#[operator(
    id = "clip.timeline.step_left",
    label = "Step Left",
    description = "Step the playhead one tick back.",
    is_available = timeline_with_clip,
    allows_undo = false,
)]
pub(crate) fn clip_timeline_step_left(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| step_timeline(world, -1));
    OperatorResult::Finished
}

/// Step the playhead one ruler tick to the right.
#[operator(
    id = "clip.timeline.step_right",
    label = "Step Right",
    description = "Step the playhead one tick forward.",
    is_available = timeline_with_clip,
    allows_undo = false,
)]
pub(crate) fn clip_timeline_step_right(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| step_timeline(world, 1));
    OperatorResult::Finished
}

/// Jump the playhead to the previous keyframe in the selected clip.
#[operator(
    id = "clip.timeline.jump_prev_keyframe",
    label = "Jump To Previous Keyframe",
    description = "Snap the playhead to the previous keyframe.",
    is_available = timeline_with_clip,
    allows_undo = false,
)]
pub(crate) fn clip_timeline_jump_prev(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| jump_to_keyframe(world, false));
    OperatorResult::Finished
}

/// Jump the playhead to the next keyframe in the selected clip.
#[operator(
    id = "clip.timeline.jump_next_keyframe",
    label = "Jump To Next Keyframe",
    description = "Snap the playhead to the next keyframe.",
    is_available = timeline_with_clip,
    allows_undo = false,
)]
pub(crate) fn clip_timeline_jump_next(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| jump_to_keyframe(world, true));
    OperatorResult::Finished
}

/// Move the playhead to the start of the selected clip.
#[operator(
    id = "clip.timeline.jump_start",
    label = "Jump To Start",
    description = "Move the playhead to the start of the clip.",
    is_available = timeline_with_clip,
    allows_undo = false,
)]
pub(crate) fn clip_timeline_jump_start(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        world.write_message(jackdaw_animation::AnimationSeek(0.0));
    });
    OperatorResult::Finished
}

/// Move the playhead to the end of the selected clip.
#[operator(
    id = "clip.timeline.jump_end",
    label = "Jump To End",
    description = "Move the playhead to the end of the clip.",
    is_available = timeline_with_clip,
    allows_undo = false,
)]
pub(crate) fn clip_timeline_jump_end(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let Some(clip_entity) = world.resource::<jackdaw_animation::SelectedClip>().0 else {
            return;
        };
        let Some(clip) = world.get::<jackdaw_animation::Clip>(clip_entity).copied() else {
            return;
        };
        world.write_message(jackdaw_animation::AnimationSeek(clip.duration.max(0.01)));
    });
    OperatorResult::Finished
}

/// Copy the selected keyframes into the animation clipboard so they
/// can be pasted at a different time.
#[operator(
    id = "clip.copy_keyframes",
    label = "Copy Keyframes",
    description = "Copy the selected keyframes to the clipboard.",
    is_available = has_selected_keyframes,
    allows_undo = false,
)]
pub(crate) fn clip_copy_keyframes(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(copy_selected_keyframes);
    OperatorResult::Finished
}

/// Paste the clipboard keyframes onto the selected clip starting at
/// the playhead.
#[operator(
    id = "clip.paste_keyframes",
    label = "Paste Keyframes",
    description = "Paste keyframes from the clipboard at the playhead.",
    is_available = timeline_paste_available,
)]
pub(crate) fn clip_paste_keyframes(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(paste_clipboard_keyframes);
    OperatorResult::Finished
}

/// Start playback on the active clip.
#[operator(
    id = "clip.play",
    label = "Play",
    description = "Start animation playback.",
    allows_undo = false
)]
pub(crate) fn clip_play(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        world.write_message(jackdaw_animation::AnimationPlay);
    });
    OperatorResult::Finished
}

/// Pause playback on the active clip.
#[operator(
    id = "clip.pause",
    label = "Pause",
    description = "Pause animation playback.",
    allows_undo = false
)]
pub(crate) fn clip_pause(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        world.write_message(jackdaw_animation::AnimationPause);
    });
    OperatorResult::Finished
}

/// Stop playback and rewind the playhead to the start of the clip.
#[operator(
    id = "clip.stop",
    label = "Stop",
    description = "Stop playback and rewind the playhead to the start of the clip.",
    allows_undo = false
)]
pub(crate) fn clip_stop(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        world.write_message(jackdaw_animation::AnimationStop);
    });
    OperatorResult::Finished
}

/// Spawn a new keyframe clip on the same target as the currently
/// selected clip, then make it the new selection.
#[operator(
    id = "clip.new",
    label = "New Clip",
    description = "Create a new keyframe clip alongside the currently-selected clip.",
    is_available = clip_new_available,
)]
pub(crate) fn clip_new(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(spawn_new_clip_for_selection);
    OperatorResult::Finished
}

/// Spawn a new blend-graph clip on the same target as the currently
/// selected clip.
#[operator(
    id = "clip.new_blend_graph",
    label = "New Blend Graph",
    description = "Create a new blend-graph clip alongside the currently-selected clip.",
    is_available = clip_new_available,
)]
pub(crate) fn clip_new_blend_graph(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(spawn_new_blend_graph_for_selection);
    OperatorResult::Finished
}

/// Both [`ClipNewOp`] and [`ClipNewBlendGraphOp`] need a currently-
/// selected clip to source the parent target from.
fn clip_new_available(selected_clip: Res<jackdaw_animation::SelectedClip>) -> bool {
    selected_clip.0.is_some()
}

fn step_timeline(world: &mut World, direction: i32) {
    let Some(clip_entity) = world.resource::<jackdaw_animation::SelectedClip>().0 else {
        return;
    };
    let Some(clip) = world.get::<jackdaw_animation::Clip>(clip_entity).copied() else {
        return;
    };
    let duration = clip.duration.max(0.01);
    let current_time = world
        .resource::<jackdaw_animation::TimelineCursor>()
        .seek_time;
    let step = jackdaw_animation::pick_tick_step(duration);
    let new_time = (current_time + direction as f32 * step).clamp(0.0, duration);
    world.write_message(jackdaw_animation::AnimationSeek(new_time));
}

/// Gather every keyframe time on the clip, across all tracks and
/// all typed keyframe components. Used by the shift+arrow "step to
/// adjacent keyframe" path.
fn collect_clip_keyframe_times(world: &World, clip_entity: Entity) -> Vec<f32> {
    let mut times = Vec::new();
    let Some(clip_children) = world.get::<Children>(clip_entity) else {
        return times;
    };
    let track_entities: Vec<Entity> = clip_children.iter().collect();
    for track in track_entities {
        let Some(track_children) = world.get::<Children>(track) else {
            continue;
        };
        for kf in track_children.iter() {
            if let Some(k) = world.get::<jackdaw_animation::Vec3Keyframe>(kf) {
                times.push(k.time);
            } else if let Some(k) = world.get::<jackdaw_animation::QuatKeyframe>(kf) {
                times.push(k.time);
            } else if let Some(k) = world.get::<jackdaw_animation::F32Keyframe>(kf) {
                times.push(k.time);
            }
        }
    }
    times
}

fn jump_to_keyframe(world: &mut World, forward: bool) {
    let Some(clip_entity) = world.resource::<jackdaw_animation::SelectedClip>().0 else {
        return;
    };
    let Some(clip) = world.get::<jackdaw_animation::Clip>(clip_entity).copied() else {
        return;
    };
    let duration = clip.duration.max(0.01);
    let current_time = world
        .resource::<jackdaw_animation::TimelineCursor>()
        .seek_time;
    let times = collect_clip_keyframe_times(world, clip_entity);
    let new_time = if forward {
        times
            .iter()
            .copied()
            .filter(|t| *t > current_time + 1e-4)
            .fold(duration, f32::min)
    } else {
        times
            .iter()
            .copied()
            .filter(|t| *t < current_time - 1e-4)
            .fold(0.0_f32, f32::max)
    };
    world.write_message(jackdaw_animation::AnimationSeek(new_time));
}

fn copy_selected_keyframes(world: &mut World) {
    let selected: Vec<Entity> = world
        .resource::<jackdaw_editor_core::selection::Selection>()
        .entities
        .clone();
    if selected.is_empty() {
        return;
    }
    let mut entries: Vec<(f32, jackdaw_animation::KeyframeClipboardEntry)> = Vec::new();
    for &entity in &selected {
        let Some(track_entity) = world.get::<ChildOf>(entity).map(ChildOf::parent) else {
            continue;
        };
        let Some(track) = world.get::<jackdaw_animation::AnimationTrack>(track_entity) else {
            continue;
        };
        let component_type_path = track.component_type_path.clone();
        let field_path = track.field_path.clone();

        if let Some(kf) = world.get::<jackdaw_animation::Vec3Keyframe>(entity) {
            entries.push((
                kf.time,
                jackdaw_animation::KeyframeClipboardEntry {
                    component_type_path,
                    field_path,
                    relative_time: kf.time,
                    value: jackdaw_animation::KeyframeValue::Vec3(kf.value),
                },
            ));
        } else if let Some(kf) = world.get::<jackdaw_animation::QuatKeyframe>(entity) {
            entries.push((
                kf.time,
                jackdaw_animation::KeyframeClipboardEntry {
                    component_type_path,
                    field_path,
                    relative_time: kf.time,
                    value: jackdaw_animation::KeyframeValue::Quat(kf.value),
                },
            ));
        } else if let Some(kf) = world.get::<jackdaw_animation::F32Keyframe>(entity) {
            entries.push((
                kf.time,
                jackdaw_animation::KeyframeClipboardEntry {
                    component_type_path,
                    field_path,
                    relative_time: kf.time,
                    value: jackdaw_animation::KeyframeValue::F32(kf.value),
                },
            ));
        }
    }
    if entries.is_empty() {
        return;
    }
    let base = entries
        .iter()
        .map(|(t, _)| *t)
        .fold(f32::INFINITY, f32::min);
    let mut normalized: Vec<jackdaw_animation::KeyframeClipboardEntry> = entries
        .into_iter()
        .map(|(_, mut entry)| {
            entry.relative_time -= base;
            entry
        })
        .collect();
    normalized.sort_by(|a, b| a.relative_time.partial_cmp(&b.relative_time).unwrap());
    let count = normalized.len();
    world
        .resource_mut::<jackdaw_animation::KeyframeClipboard>()
        .entries = normalized;
    info!("Copied {count} keyframe(s) to animation clipboard");
}

fn paste_clipboard_keyframes(world: &mut World) {
    let entries = world
        .resource::<jackdaw_animation::KeyframeClipboard>()
        .entries
        .clone();
    if entries.is_empty() {
        return;
    }
    let Some(clip_entity) = world.resource::<jackdaw_animation::SelectedClip>().0 else {
        return;
    };
    let cursor_time = world
        .resource::<jackdaw_animation::TimelineCursor>()
        .seek_time;

    let mut tracks: Vec<(Entity, String, String)> = Vec::new();
    if let Some(children) = world.get::<Children>(clip_entity) {
        for child in children.iter() {
            if let Some(track) = world.get::<jackdaw_animation::AnimationTrack>(child) {
                tracks.push((
                    child,
                    track.component_type_path.clone(),
                    track.field_path.clone(),
                ));
            }
        }
    }

    let mut cmds: Vec<Box<dyn jackdaw_commands::EditorCommand>> = Vec::new();
    let mut max_paste_time = cursor_time;
    for entry in &entries {
        let track_entity = tracks.iter().find_map(|(e, tp, fp)| {
            (tp == &entry.component_type_path && fp == &entry.field_path).then_some(*e)
        });
        let Some(track_entity) = track_entity else {
            warn!(
                "Paste keyframe: no track for {}.{} on selected clip. Add one via the inspector diamond first",
                entry.component_type_path, entry.field_path,
            );
            continue;
        };
        let paste_time = cursor_time + entry.relative_time;
        max_paste_time = max_paste_time.max(paste_time);
        let cmd: Box<dyn jackdaw_commands::EditorCommand> = match entry.value {
            jackdaw_animation::KeyframeValue::Vec3(v) => Box::new(SpawnKeyframeCmd::Vec3 {
                keyframe: None,
                track: track_entity,
                time: paste_time,
                value: v,
            }),
            jackdaw_animation::KeyframeValue::Quat(q) => Box::new(SpawnKeyframeCmd::Quat {
                keyframe: None,
                track: track_entity,
                time: paste_time,
                value: q,
            }),
            jackdaw_animation::KeyframeValue::F32(f) => Box::new(SpawnKeyframeCmd::F32 {
                keyframe: None,
                track: track_entity,
                time: paste_time,
                value: f,
            }),
        };
        cmds.push(cmd);
    }

    if cmds.is_empty() {
        return;
    }

    if let Some(mut clip) = world.get_mut::<jackdaw_animation::Clip>(clip_entity)
        && max_paste_time > clip.duration
    {
        clip.duration = max_paste_time;
    }

    for cmd in &mut cmds {
        cmd.execute(world);
    }
    let count = cmds.len();
    let group = jackdaw_editor_core::commands::CommandGroup {
        commands: cmds,
        label: "Paste keyframes".to_string(),
    };
    let mut history = world.resource_mut::<jackdaw_commands::CommandHistory>();
    history.push_executed(Box::new(group));

    if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
        dirty.0 = true;
    }
    info!("Pasted {count} keyframe(s) from animation clipboard");
}

/// Typed, undo-aware spawn command for animation keyframes. Mirror of
/// [`DespawnKeyframeCmd`]: execute spawns a fresh entity with the
/// stored fields parented to the track, undo despawns it. Same ID-
/// reuse avoidance rationale: direct `world.spawn` rather than
/// `DynamicScene`.
///
/// Internal primitive used by the keyframe paste path
/// (`clip.paste_keyframes`). The user-facing entry point for spawning
/// a keyframe at the current playhead time for a selected entity is
/// the `animation.toggle_keyframe` operator.
enum SpawnKeyframeCmd {
    Vec3 {
        /// Filled in by `execute`; `None` before the first execute.
        keyframe: Option<Entity>,
        track: Entity,
        time: f32,
        value: Vec3,
    },
    Quat {
        keyframe: Option<Entity>,
        track: Entity,
        time: f32,
        value: Quat,
    },
    F32 {
        keyframe: Option<Entity>,
        track: Entity,
        time: f32,
        value: f32,
    },
}

impl SpawnKeyframeCmd {
    fn relive(&mut self, world: &World) {
        match self {
            Self::Vec3 {
                keyframe, track, ..
            }
            | Self::Quat {
                keyframe, track, ..
            }
            | Self::F32 {
                keyframe, track, ..
            } => {
                *keyframe = keyframe
                    .map(|keyframe| jackdaw_editor_core::scene_nodes::live_entity(world, keyframe));
                *track = jackdaw_editor_core::scene_nodes::live_entity(world, *track);
            }
        }
    }
}

impl jackdaw_commands::EditorCommand for SpawnKeyframeCmd {
    fn execute(&mut self, world: &mut World) {
        self.relive(world);
        let new_id = match self {
            Self::Vec3 {
                track, time, value, ..
            } => world
                .spawn((
                    jackdaw_animation::Vec3Keyframe {
                        time: *time,
                        value: *value,
                    },
                    ChildOf(*track),
                ))
                .id(),
            Self::Quat {
                track, time, value, ..
            } => world
                .spawn((
                    jackdaw_animation::QuatKeyframe {
                        time: *time,
                        value: *value,
                    },
                    ChildOf(*track),
                ))
                .id(),
            Self::F32 {
                track, time, value, ..
            } => world
                .spawn((
                    jackdaw_animation::F32Keyframe {
                        time: *time,
                        value: *value,
                    },
                    ChildOf(*track),
                ))
                .id(),
        };
        match self {
            Self::Vec3 { keyframe, .. }
            | Self::Quat { keyframe, .. }
            | Self::F32 { keyframe, .. } => *keyframe = Some(new_id),
        }
    }

    fn undo(&mut self, world: &mut World) {
        self.relive(world);
        let entity = match self {
            Self::Vec3 { keyframe, .. }
            | Self::Quat { keyframe, .. }
            | Self::F32 { keyframe, .. } => *keyframe,
        };
        if let Some(entity) = entity
            && let Ok(ent) = world.get_entity_mut(entity)
        {
            ent.despawn();
        }
    }

    fn description(&self) -> &str {
        "Paste keyframe"
    }
}

/// Observer: clicking a timeline keyframe diamond routes through
/// the main editor's [`jackdaw_editor_core::selection::Selection`] resource. Ctrl+click
/// toggles into the existing selection; plain click replaces with
/// just the keyframe. Delete is then handled by the main editor's
/// existing `delete_selected` path, which wraps despawns in
/// `DespawnEntity` commands for undo safety. The animation crate
/// deliberately does NOT own a delete key handler, so there's no
/// risk of double-delete when the user has both a scene entity and
/// a keyframe "selected."
///
/// Propagation is stopped so the click doesn't also hit the
/// scrubber and seek the playhead.
pub(crate) fn on_timeline_keyframe_click(
    mut event: On<PointerClick>,
    handles: Query<&jackdaw_animation::TimelineKeyframeHandle>,
    keys: Res<ButtonInput<KeyCode>>,
    mut selection: ResMut<jackdaw_editor_core::selection::Selection>,
    mut commands: Commands,
) {
    let Ok(handle) = handles.get(event.event_target()) else {
        return;
    };
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if ctrl {
        selection.toggle(&mut commands, handle.keyframe);
    } else {
        selection.select_single(&mut commands, handle.keyframe);
    }
    event.propagate(false);
}

/// Mirror the main [`jackdaw_editor_core::selection::Selection`] into the animation crate's
/// [`jackdaw_animation::SelectedKeyframes`] so the timeline
/// highlight system can tell which diamonds to light up without
/// the animation crate needing to import `Selection` itself.
///
/// Runs only when `Selection` changes. Also filters out entities
/// whose keyframe component type isn't one we know about; non-
/// keyframe selections simply don't land in `SelectedKeyframes`.
pub(crate) fn sync_selected_keyframes_from_selection(
    selection: Res<jackdaw_editor_core::selection::Selection>,
    mut selected_keyframes: ResMut<jackdaw_animation::SelectedKeyframes>,
    vec3_keyframes: Query<(), With<jackdaw_animation::Vec3Keyframe>>,
    quat_keyframes: Query<(), With<jackdaw_animation::QuatKeyframe>>,
    f32_keyframes: Query<(), With<jackdaw_animation::F32Keyframe>>,
) {
    if !selection.is_changed() {
        return;
    }
    selected_keyframes.entities.clear();
    for &entity in &selection.entities {
        if vec3_keyframes.contains(entity)
            || quat_keyframes.contains(entity)
            || f32_keyframes.contains(entity)
        {
            selected_keyframes.entities.insert(entity);
        }
    }
}

/// Observer: when the timeline header's duration field commits,
/// route the edit through `SetBsnField` so it flows through the
/// document and participates in undo/redo + save/load. This is the
/// hand-off point between the animation crate (which can't import
/// `SetBsnField`) and the editor binary.
pub(crate) fn on_duration_input_commit(
    event: On<jackdaw_feathers::text_edit::TextEditCommitEvent>,
    duration_inputs: Query<&jackdaw_animation::TimelineDurationInput>,
    child_of_query: Query<&ChildOf>,
    clips: Query<&jackdaw_animation::Clip>,
    mut commands: Commands,
) {
    // The commit event fires on the inner text_input entity; the
    // `TimelineDurationInput` marker sits on the wrapper, so walk
    // up one step to find it. Matches the pattern used by
    // `on_material_param_commit` in material_browser.rs.
    let mut current = event.entity;
    let mut marker_clip: Option<Entity> = None;
    for _ in 0..4 {
        if let Ok(marker) = duration_inputs.get(current) {
            marker_clip = Some(marker.clip);
            break;
        }
        let Ok(child_of) = child_of_query.get(current) else {
            break;
        };
        current = child_of.parent();
    }

    let Some(clip_entity) = marker_clip else {
        return;
    };
    let Ok(new_value) = event.text.trim().parse::<f32>() else {
        return;
    };
    let Ok(clip) = clips.get(clip_entity) else {
        return;
    };
    if (new_value - clip.duration).abs() < f32::EPSILON {
        return;
    }
    let old_duration = clip.duration;
    commands.queue(move |world: &mut World| {
        let mut history = world
            .remove_resource::<jackdaw_commands::CommandHistory>()
            .unwrap_or_default();
        history.execute(
            Box::new(jackdaw_editor_core::commands::SetBsnField {
                entity: clip_entity,
                type_path: "jackdaw_animation::clip::Clip".to_string(),
                field_path: "duration".to_string(),
                old_value: Some(jackdaw_bsn::BsnValue::from_f32(old_duration)),
                new_value: jackdaw_bsn::BsnValue::from_f32(new_value),
                was_derived: false,
            }),
            world,
        );
        world.insert_resource(history);
    });
}

fn has_primary_selection(selection: Res<jackdaw_editor_core::selection::Selection>) -> bool {
    selection.primary().is_some()
}

/// Spawn (or replace) a keyframe at the current timeline cursor for one
/// of the entity's animatable properties. Creates the clip and track
/// lazily if they don't exist yet.
#[operator(
    id = "animation.toggle_keyframe",
    label = "Toggle Keyframe",
    description = "Add or replace a keyframe for this property at the current timeline cursor.",
    is_available = has_primary_selection,
    params(
        entity(Entity, doc = "Source entity whose property is being animated."),
        component_type_path(String, doc = "Fully-qualified Bevy reflected type path of the component that owns the property."),
        field_path(String, doc = "Dotted path to the field within the component (e.g. \"translation\")."),
    ),
)]
pub(crate) fn animation_toggle_keyframe(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> Option<OperatorResult> {
    let entity = params.as_entity("entity")?;
    let type_path = params.as_str("component_type_path").map(str::to_string)?;
    let field_path = params.as_str("field_path").map(str::to_string)?;
    commands.queue(move |world: &mut World| {
        world
            .run_system_cached_with(
                crate::anim_diamond::toggle_keyframe,
                (entity, type_path, field_path),
            )
            .ok();
    });
    Some(OperatorResult::Finished)
}
