//! The Timeline tab: a toolbar over a track column and a dope sheet, with a
//! footer under both.
//!
//! [`timeline_panel`] spawns the root; [`rebuild_timeline`] fills it whenever
//! the clip or its data changed. The row model both columns draw from lives in
//! [`crate::sheet`], and the toolbar in [`crate::toolbar`].
//!
//! The widget reads and displays. Every edit it offers goes out as an operator
//! or as a message the main editor turns into a `SetBsnField` /
//! `DespawnEntity`, so the AST, undo and save all see it; see
//! [`crate::commands`] for why the animation crate mints no commands of its
//! own.

use bevy::prelude::*;
use bevy::ui::ComputedNode;
use bevy::ui::UiScale;
use bevy::ui::ui_transform::UiGlobalTransform;
use jackdaw_feathers::button::{ButtonProps, ButtonVariant, button};
use jackdaw_feathers::icons::IconFont;
use jackdaw_feathers::tokens;
use jackdaw_localization::LocalizedText;
use lucide_icons::Icon;

use crate::blend_graph::AnimationBlendGraph;
use crate::clip::{
    AnimationTrack, Clip, ClipRecording, F32Keyframe, ImportedClipView, OnionSkin, QuatKeyframe,
    SelectedClip, SelectedKeyframes, SelectedTrack, TimelineSnap, TimelineSnapHint, TimelineView,
    TimelineZoom, Vec3Keyframe,
};
use crate::compile::clip_display_duration;
use crate::player::{TimelineCursor, TimelineEngagement};
use crate::sheet::{
    ClipContents, RowKind, SheetLayout, SheetRow, TimelineKeyReadout, TimelineKeyframeHandle,
    TimelinePlayheadIndicator, TimelineScrubber, TimelineSheetBody,
};
use crate::toolbar::ToolbarState;

/// Root marker on the panel container. Its children are rebuilt by
/// [`rebuild_timeline`].
#[derive(Component, Default)]
pub struct TimelinePanelRoot;

/// Marker for the "Create Clip" button shown when nothing is selected.
#[derive(Component, Clone, Copy)]
pub struct TimelineCreateClipButton;

/// Marker for the "Create Blend Graph" button beside it.
#[derive(Component, Clone, Copy)]
pub struct TimelineCreateBlendGraphButton;

/// Bump this to rebuild the timeline on the next frame.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct TimelineDirty(pub bool);

/// Sent when a key has been dragged to a new time, so the main editor can put
/// the move through the AST and into undo.
#[derive(Message, Debug, Clone, Copy)]
pub struct KeyframeRetimed {
    /// The keyframe entity that moved.
    pub keyframe: Entity,
    /// Where it was before the drag.
    pub from: f32,
    /// Where it landed.
    pub to: f32,
}

/// Sent when a marquee closed over a set of keys.
///
/// The widget does not own the selection: the main editor keeps one selection
/// for the whole editor and mirrors the keyframes out of it, so a marquee asks
/// rather than writes.
#[derive(Message, Debug, Clone)]
pub struct KeyframesMarqueeSelected {
    /// The keys the marquee closed over.
    pub keyframes: Vec<Entity>,
    /// Whether they join the selection rather than replace it.
    pub additive: bool,
}

/// Where a marquee began, in the sheet body's own space.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct TimelineMarquee {
    /// The corner the drag started from, or `None` when no drag is open.
    pub from: Option<Vec2>,
    /// Where the pointer stands now.
    pub to: Vec2,
}

/// Marker on the rectangle a marquee drag draws.
#[derive(Component)]
pub struct TimelineMarqueeBox;

/// Where a key stood when its drag began, so the release can report the move.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct KeyframeDragOrigin(Option<(Entity, f32)>);

/// Bundle for the panel root. Spawn this wherever the timeline should live.
pub fn timeline_panel() -> impl Bundle {
    (
        TimelinePanelRoot,
        Node {
            flex_direction: FlexDirection::Column,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(tokens::PANEL_BG),
    )
}

/// Flag the timeline dirty whenever authored animation data changed or went
/// away this frame, so an inspector edit, a key added and a key deleted all
/// repaint at once.
///
/// The despawn half is load-bearing: a key removed through `DespawnEntity`
/// fires no `Changed`; the entity just stops existing, and without the
/// removal check its diamond would sit there until something else rebuilt.
pub fn mark_timeline_dirty_on_data_change(
    mut dirty: ResMut<TimelineDirty>,
    changed_clips: Query<(), Changed<Clip>>,
    changed_tracks: Query<(), Changed<AnimationTrack>>,
    changed_vec3: Query<(), Changed<Vec3Keyframe>>,
    changed_quat: Query<(), Changed<QuatKeyframe>>,
    changed_f32: Query<(), Changed<F32Keyframe>>,
    changed_events: Query<(), Changed<jackdaw_animation_runtime::ClipEvent>>,
    mut removed_clips: RemovedComponents<Clip>,
    mut removed_tracks: RemovedComponents<AnimationTrack>,
    mut removed_vec3: RemovedComponents<Vec3Keyframe>,
    mut removed_quat: RemovedComponents<QuatKeyframe>,
    mut removed_f32: RemovedComponents<F32Keyframe>,
    mut removed_events: RemovedComponents<jackdaw_animation_runtime::ClipEvent>,
) {
    let any_changed = !changed_clips.is_empty()
        || !changed_tracks.is_empty()
        || !changed_vec3.is_empty()
        || !changed_quat.is_empty()
        || !changed_f32.is_empty()
        || !changed_events.is_empty();
    let any_removed = removed_clips.read().next().is_some()
        || removed_tracks.read().next().is_some()
        || removed_vec3.read().next().is_some()
        || removed_quat.read().next().is_some()
        || removed_f32.read().next().is_some()
        || removed_events.read().next().is_some();
    if any_changed || any_removed {
        dirty.0 = true;
    }
}

/// What the last rebuild was drawn for, so a frame where nothing the panel
/// draws has moved does not throw it away and build it again.
#[derive(PartialEq, Clone, Copy, Debug)]
pub struct BuiltFor {
    /// The clip that was up.
    clip: Option<Entity>,
    /// Which half of the sheet was drawn.
    view: TimelineView,
    /// Whether the record light was on.
    recording: bool,
    /// Whether the onion skin toggle was on.
    onion_skin: bool,
    /// How wide the sheet was drawn.
    zoom: f32,
    /// Whether snapping was on, and the rate it used.
    snap: (bool, u32),
    /// Which track the curves view was drawing.
    track: Option<Entity>,
    /// Whether an imported clip was up rather than an authored one.
    imported: bool,
}

/// The resources the rebuild reads, gathered so the system stays inside
/// Bevy's parameter count.
#[derive(bevy::ecs::system::SystemParam)]
pub struct TimelineSettings<'w> {
    selected: Res<'w, SelectedClip>,
    selected_track: Res<'w, SelectedTrack>,
    cursor: Res<'w, TimelineCursor>,
    recording: Res<'w, ClipRecording>,
    onion_skin: Res<'w, OnionSkin>,
    snap: Res<'w, TimelineSnap>,
    view: Res<'w, TimelineView>,
    zoom: Res<'w, TimelineZoom>,
    imported: Res<'w, ImportedClipView>,
}

/// Repopulate the timeline whenever the clip or anything it draws changed.
pub fn rebuild_timeline(
    mut commands: Commands,
    settings: TimelineSettings,
    mut dirty: ResMut<TimelineDirty>,
    panels: Query<(Entity, Option<&Children>), With<TimelinePanelRoot>>,
    clips: Query<(&Clip, Option<&Children>)>,
    clip_entities: Query<Entity, With<Clip>>,
    blend_graphs: Query<(), With<AnimationBlendGraph>>,
    contents: ClipContents,
    icon_font: Option<Res<IconFont>>,
    mut last: Local<Option<BuiltFor>>,
) {
    let Some(icon_font) = icon_font else {
        return;
    };
    let built_for = BuiltFor {
        clip: settings.selected.0,
        view: *settings.view,
        recording: settings.recording.0,
        onion_skin: settings.onion_skin.0,
        zoom: settings.zoom.0,
        snap: (settings.snap.enabled, settings.snap.rate.to_bits()),
        track: settings.selected_track.0,
        imported: settings.imported.clip.is_some(),
    };
    let settings_changed = *last != Some(built_for);
    if !settings_changed
        && !dirty.0
        && panels
            .iter()
            .all(|(_, kids)| kids.is_some_and(|kids| !kids.is_empty()))
    {
        return;
    }

    let choices = clip_choices(&contents, &clip_entities);
    for (panel, panel_children) in &panels {
        for child in panel_children.into_iter().flatten() {
            commands.entity(*child).despawn();
        }

        let held = settings
            .selected
            .0
            .and_then(|clip| clips.get(clip).ok().map(|held| (clip, held)));
        match held {
            Some((clip_entity, (clip, clip_children))) if blend_graphs.contains(clip_entity) => {
                spawn_blend_graph_body(
                    &mut commands,
                    panel,
                    clip_entity,
                    clip,
                    &choices,
                    &settings,
                    &icon_font,
                );
                let _ = clip_children;
            }
            Some((clip_entity, (clip, clip_children))) => {
                let rows = contents.rows_for_clip(clip_entity, clip_children);
                spawn_authored_clip(
                    &mut commands,
                    panel,
                    clip_entity,
                    clip,
                    &rows,
                    &choices,
                    &settings,
                    &icon_font,
                    &contents,
                );
            }
            None if settings.imported.clip.is_some() => {
                spawn_imported_clip(&mut commands, panel, &choices, &settings, &icon_font);
            }
            None => spawn_placeholder(&mut commands, panel),
        }
    }

    *last = Some(built_for);
    dirty.0 = false;
}

/// The list the clip selector shows: every authored clip in the open scene,
/// named for the entity it animates so two clips called "Idle" are told apart.
fn clip_choices(
    contents: &ClipContents,
    clip_entities: &Query<Entity, With<Clip>>,
) -> Vec<(Entity, String)> {
    let mut choices: Vec<(Entity, String)> = clip_entities
        .iter()
        .map(|entity| (entity, clip_choice_label(contents, entity)))
        .collect();
    choices.sort_by(|a, b| a.1.cmp(&b.1));
    choices
}

fn clip_choice_label(contents: &ClipContents, clip: Entity) -> String {
    let name = contents
        .names
        .get(clip)
        .map_or_else(|_| "Untitled".to_string(), |name| name.as_str().to_string());
    match contents
        .parents
        .get(clip)
        .ok()
        .and_then(|parent| contents.names.get(parent.parent()).ok())
    {
        Some(owner) => format!("{name} on {}", owner.as_str()),
        None => name,
    }
}

fn toolbar_state<'a>(
    clip_entity: Option<Entity>,
    clip: Option<&Clip>,
    choices: &'a [(Entity, String)],
    settings: &TimelineSettings,
    read_only: bool,
) -> ToolbarState<'a> {
    ToolbarState {
        clip: clip_entity,
        clips: choices,
        time: settings.cursor.seek_time,
        duration: clip.map_or(settings.imported.duration, |clip| clip.duration),
        loop_mode: clip.map_or_else(default, |clip| clip.loop_mode),
        speed: clip.map_or(1.0, |clip| clip.speed),
        recording: settings.recording.0,
        onion_skin: settings.onion_skin.0,
        snap: *settings.snap,
        read_only,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one call site, all of it read-only"
)]
fn spawn_authored_clip(
    commands: &mut Commands,
    panel: Entity,
    clip_entity: Entity,
    clip: &Clip,
    rows: &[SheetRow],
    choices: &[(Entity, String)],
    settings: &TimelineSettings,
    icon_font: &IconFont,
    contents: &ClipContents,
) {
    crate::toolbar::spawn_toolbar(
        commands,
        panel,
        &toolbar_state(Some(clip_entity), Some(clip), choices, settings, false),
        icon_font,
    );
    let layout = SheetLayout {
        clip: clip_entity,
        duration: clip.duration.max(0.01),
        zoom: settings.zoom.0,
        rate: settings.snap.rate,
        view: *settings.view,
        read_only: false,
    };
    let body = spawn_body_row(commands, panel);
    crate::sheet::spawn_track_column(commands, body, rows, layout, settings.selected_track.0);
    let series = settings
        .selected_track
        .0
        .and_then(|track| contents.tracks.get(track).ok())
        .map(|(_, children)| contents.series_of_track(children))
        .unwrap_or_default();
    crate::sheet::spawn_sheet(commands, body, rows, layout, &series);
    crate::sheet::spawn_footer(commands, panel, *settings.view, settings.zoom.0);
}

fn spawn_body_row(commands: &mut Commands, panel: Entity) -> Entity {
    commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                width: Val::Percent(100.0),
                flex_grow: 1.0,
                min_height: Val::Px(0.0),
                ..default()
            },
            ChildOf(panel),
        ))
        .id()
}

/// A blend-graph clip keeps its node canvas where the sheet would be.
fn spawn_blend_graph_body(
    commands: &mut Commands,
    panel: Entity,
    clip_entity: Entity,
    clip: &Clip,
    choices: &[(Entity, String)],
    settings: &TimelineSettings,
    icon_font: &IconFont,
) {
    crate::toolbar::spawn_toolbar(
        commands,
        panel,
        &toolbar_state(Some(clip_entity), Some(clip), choices, settings, false),
        icon_font,
    );
    let wrapper = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_grow: 1.0,
                ..default()
            },
            ChildOf(panel),
        ))
        .id();
    let canvas_root = commands
        .spawn((jackdaw_node_graph::canvas(clip_entity), ChildOf(wrapper)))
        .id();
    commands
        .spawn(jackdaw_node_graph::canvas_world(clip_entity))
        .insert(ChildOf(canvas_root));
}

/// A clip the library is previewing: a summary over the bones it drives, none
/// of which can be moved from here.
fn spawn_imported_clip(
    commands: &mut Commands,
    panel: Entity,
    choices: &[(Entity, String)],
    settings: &TimelineSettings,
    icon_font: &IconFont,
) {
    let imported = &*settings.imported;
    crate::toolbar::spawn_toolbar(
        commands,
        panel,
        &toolbar_state(None, None, choices, settings, true),
        icon_font,
    );
    let mut rows = vec![
        read_only_row(RowKind::Summary, imported.name.clone(), 0),
        read_only_row(
            RowKind::Group,
            format!("Skeleton ({} tracks)", imported.curve_count),
            0,
        ),
    ];
    rows.extend(
        imported
            .bones
            .iter()
            .map(|bone| read_only_row(RowKind::Bone, bone.clone(), 1)),
    );

    let layout = SheetLayout {
        clip: Entity::PLACEHOLDER,
        duration: imported.duration.max(0.01),
        zoom: settings.zoom.0,
        rate: settings.snap.rate,
        view: TimelineView::Dopesheet,
        read_only: true,
    };
    let body = spawn_body_row(commands, panel);
    crate::sheet::spawn_track_column(commands, body, &rows, layout, None);
    crate::sheet::spawn_sheet(commands, body, &rows, layout, &[]);
    crate::sheet::spawn_footer(commands, panel, TimelineView::Dopesheet, settings.zoom.0);
}

fn read_only_row(kind: RowKind, label: String, depth: u8) -> SheetRow {
    SheetRow {
        kind,
        label,
        depth,
        keys: Vec::new(),
        interpolation: None,
        enabled: true,
    }
}

fn spawn_placeholder(commands: &mut Commands, parent: Entity) {
    let wrapper = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(tokens::SPACING_MD),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        LocalizedText::new("no-animation-clip-on-selection"),
        TextColor(tokens::TEXT_MUTED_COLOR.into()),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        ChildOf(wrapper),
    ));

    let button_row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(tokens::SPACING_MD),
                ..default()
            },
            ChildOf(wrapper),
        ))
        .id();
    commands.spawn((
        TimelineCreateClipButton,
        button(
            ButtonProps::new("Create Clip")
                .with_variant(ButtonVariant::Default)
                .with_left_icon(Icon::Plus),
        ),
        ChildOf(button_row),
    ));
    commands.spawn((
        TimelineCreateBlendGraphButton,
        button(
            ButtonProps::new("Create Blend Graph")
                .with_variant(ButtonVariant::Ghost)
                .with_left_icon(Icon::GitBranch),
        ),
        ChildOf(button_row),
    ));
}

/// Pick a tick interval for a length, aiming for four to ten labels across it.
///
/// Also used by the editor's arrow-key scrub so stepping lands on the marks
/// the ruler draws.
pub fn pick_tick_step(duration: f32) -> f32 {
    const CANDIDATES: &[f32] = &[0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0];
    for &step in CANDIDATES {
        if duration / step <= 10.0 {
            return step;
        }
    }
    *CANDIDATES.last().unwrap()
}

/// Keep the playhead line in step with [`TimelineCursor`].
pub fn update_playhead_position(
    cursor: Res<TimelineCursor>,
    imported: Res<ImportedClipView>,
    scrubbers: Query<&TimelineScrubber>,
    clips: Query<(&Clip, Option<&Children>)>,
    mut indicators: Query<&mut Node, With<TimelinePlayheadIndicator>>,
) {
    let duration = match scrubbers.iter().next() {
        Some(scrubber) if scrubber.clip != Entity::PLACEHOLDER => {
            clip_display_duration(scrubber.clip, &clips)
        }
        Some(_) => imported.duration.max(0.01),
        None => return,
    };
    let percent = (cursor.seek_time / duration).clamp(0.0, 1.0) * 100.0;
    for mut node in &mut indicators {
        node.left = Val::Percent(percent);
    }
}

/// Say what the selected key holds, in the footer.
pub fn update_key_readout(
    selected: Res<SelectedKeyframes>,
    vec3_keyframes: Query<&Vec3Keyframe>,
    quat_keyframes: Query<&QuatKeyframe>,
    f32_keyframes: Query<&F32Keyframe>,
    mut readouts: Query<&mut Text, With<TimelineKeyReadout>>,
) {
    let wanted = selected
        .entities
        .iter()
        .next()
        .copied()
        .map(|key| {
            if let Ok(key) = vec3_keyframes.get(key) {
                let held = key.value;
                format!(
                    "{:.2} s   {:.3}, {:.3}, {:.3}",
                    key.time, held.x, held.y, held.z
                )
            } else if let Ok(key) = quat_keyframes.get(key) {
                let held = key.value;
                format!(
                    "{:.2} s   {:.3}, {:.3}, {:.3}, {:.3}",
                    key.time, held.x, held.y, held.z, held.w
                )
            } else if let Ok(key) = f32_keyframes.get(key) {
                format!("{:.2} s   {:.3}", key.time, key.value)
            } else {
                String::new()
            }
        })
        .unwrap_or_default();
    for mut readout in &mut readouts {
        if readout.0 != wanted {
            readout.0 = wanted.clone();
        }
    }
}

/// Paint every diamond by its state: selected, snap-hovered, or plain.
///
/// Selection wins over snap-hover, since selection is what the author chose.
pub fn update_keyframe_highlight(
    selected: Res<SelectedKeyframes>,
    hint: Res<TimelineSnapHint>,
    mut handles: Query<(
        &TimelineKeyframeHandle,
        &mut BackgroundColor,
        &mut BorderColor,
    )>,
) {
    for (handle, mut bg, mut border) in &mut handles {
        if selected.is_selected(handle.keyframe) {
            bg.0 = Color::srgb(1.0, 0.78, 0.12);
            *border = BorderColor::all(Color::WHITE);
        } else if hint.hovered_keyframe == Some(handle.keyframe) {
            bg.0 = Color::srgb(0.38, 0.72, 1.0);
            *border = BorderColor::all(Color::WHITE);
        } else {
            bg.0 = tokens::ACCENT_BLUE;
            *border = BorderColor::all(Color::WHITE.with_alpha(0.4));
        }
    }
}

/// Clicking the ruler seeks the playhead, snapping to the rate or to a nearby
/// key unless Shift is held.
pub fn handle_scrubber_click(
    mut event: On<Pointer<Click>>,
    scrubbers: Query<(&TimelineScrubber, &ComputedNode, &UiGlobalTransform)>,
    clips: Query<(&Clip, Option<&Children>)>,
    contents: ClipContents,
    snap: Res<TimelineSnap>,
    imported: Res<ImportedClipView>,
    mut hint: ResMut<TimelineSnapHint>,
    keys: Res<ButtonInput<KeyCode>>,
    mut seek: MessageWriter<crate::player::AnimationSeek>,
    ui_scale: Res<UiScale>,
) {
    let Ok((scrubber, computed, global)) = scrubbers.get(event.event_target()) else {
        return;
    };
    let result = seek_for_pointer(
        event.pointer_location.position.x / ui_scale.0,
        computed,
        global,
        scrubber.clip,
        &clips,
        &contents,
        &snap,
        &imported,
        &keys,
    );
    hint.hovered_keyframe = result.hovered_keyframe;
    seek.write(crate::player::AnimationSeek(result.time));
    event.propagate(false);
}

/// Dragging across the ruler seeks as it goes, so the target follows.
pub fn handle_scrubber_drag(
    mut event: On<Pointer<Drag>>,
    scrubbers: Query<(&TimelineScrubber, &ComputedNode, &UiGlobalTransform)>,
    clips: Query<(&Clip, Option<&Children>)>,
    contents: ClipContents,
    snap: Res<TimelineSnap>,
    imported: Res<ImportedClipView>,
    mut hint: ResMut<TimelineSnapHint>,
    keys: Res<ButtonInput<KeyCode>>,
    mut seek: MessageWriter<crate::player::AnimationSeek>,
    ui_scale: Res<UiScale>,
) {
    let Ok((scrubber, computed, global)) = scrubbers.get(event.event_target()) else {
        return;
    };
    let result = seek_for_pointer(
        event.pointer_location.position.x / ui_scale.0,
        computed,
        global,
        scrubber.clip,
        &clips,
        &contents,
        &snap,
        &imported,
        &keys,
    );
    hint.hovered_keyframe = result.hovered_keyframe;
    seek.write(crate::player::AnimationSeek(result.time));
    event.propagate(false);
}

/// Clear the snap hint when the ruler drag ends, so a hover highlight does not
/// linger after the button comes up.
pub fn clear_snap_hint_on_drag_end(
    mut event: On<Pointer<DragEnd>>,
    scrubbers: Query<&TimelineScrubber>,
    mut hint: ResMut<TimelineSnapHint>,
) {
    if scrubbers.get(event.event_target()).is_err() {
        return;
    }
    hint.hovered_keyframe = None;
    event.propagate(false);
}

/// A ruler drag engages the timeline, so [`crate::auto_bind_player`] installs
/// the runtime components and the target follows the playhead.
pub fn handle_scrubber_drag_start(
    mut event: On<Pointer<DragStart>>,
    scrubbers: Query<&TimelineScrubber>,
    mut engagement: ResMut<TimelineEngagement>,
) {
    if scrubbers.get(event.event_target()).is_err() {
        return;
    }
    *engagement = TimelineEngagement::Active;
    event.propagate(false);
}

/// Releasing the ruler hands the target back.
pub fn handle_scrubber_drag_end(
    mut event: On<Pointer<DragEnd>>,
    scrubbers: Query<&TimelineScrubber>,
    mut engagement: ResMut<TimelineEngagement>,
) {
    if scrubbers.get(event.event_target()).is_err() {
        return;
    }
    *engagement = TimelineEngagement::Idle;
    event.propagate(false);
}

/// Dragging a diamond retimes its key, rounded to the snap rate unless Shift
/// is held.
///
/// The component is written as the drag runs so the diamond follows the
/// pointer; the move reaches the document on release, through
/// [`KeyframeRetimed`].
pub fn handle_keyframe_drag(
    mut event: On<Pointer<Drag>>,
    handles: Query<&TimelineKeyframeHandle>,
    bodies: Query<(&TimelineSheetBody, &ComputedNode, &UiGlobalTransform)>,
    clips: Query<(&Clip, Option<&Children>)>,
    imported: Res<ImportedClipView>,
    snap: Res<TimelineSnap>,
    keys: Res<ButtonInput<KeyCode>>,
    ui_scale: Res<UiScale>,
    mut vec3_keyframes: Query<&mut Vec3Keyframe>,
    mut quat_keyframes: Query<&mut QuatKeyframe>,
    mut f32_keyframes: Query<&mut F32Keyframe>,
) {
    let Ok(handle) = handles.get(event.event_target()) else {
        return;
    };
    let Some((body, computed, global)) = bodies.iter().next() else {
        return;
    };
    let duration = if body.clip == Entity::PLACEHOLDER {
        imported.duration.max(0.01)
    } else {
        clip_display_duration(body.clip, &clips)
    };
    let raw = time_for_cursor(
        event.pointer_location.position.x / ui_scale.0,
        computed,
        global,
        duration,
    );
    let free = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let time = if free { raw } else { snap.round(raw) }.clamp(0.0, duration);

    if let Ok(mut key) = vec3_keyframes.get_mut(handle.keyframe) {
        key.time = time;
    } else if let Ok(mut key) = quat_keyframes.get_mut(handle.keyframe) {
        key.time = time;
    } else if let Ok(mut key) = f32_keyframes.get_mut(handle.keyframe) {
        key.time = time;
    }
    event.propagate(false);
}

/// Remember where a dragged key started.
pub fn handle_keyframe_drag_start(
    mut event: On<Pointer<DragStart>>,
    handles: Query<&TimelineKeyframeHandle>,
    contents: ClipContents,
    mut origin: ResMut<KeyframeDragOrigin>,
) {
    let Ok(handle) = handles.get(event.event_target()) else {
        return;
    };
    origin.0 = contents
        .key_time(handle.keyframe)
        .map(|time| (handle.keyframe, time));
    event.propagate(false);
}

/// Report the finished move so the main editor can put it through the AST.
pub fn handle_keyframe_drag_end(
    mut event: On<Pointer<DragEnd>>,
    handles: Query<&TimelineKeyframeHandle>,
    contents: ClipContents,
    mut origin: ResMut<KeyframeDragOrigin>,
    mut retimed: MessageWriter<KeyframeRetimed>,
) {
    let Ok(handle) = handles.get(event.event_target()) else {
        return;
    };
    let Some((keyframe, from)) = origin.0.take() else {
        return;
    };
    if keyframe != handle.keyframe {
        return;
    }
    if let Some(to) = contents.key_time(keyframe)
        && (to - from).abs() > f32::EPSILON
    {
        retimed.write(KeyframeRetimed { keyframe, from, to });
    }
    event.propagate(false);
}

/// Open a marquee where the drag on the sheet body began.
pub fn handle_marquee_start(
    mut event: On<Pointer<DragStart>>,
    bodies: Query<(&ComputedNode, &UiGlobalTransform), With<TimelineSheetBody>>,
    ui_scale: Res<UiScale>,
    mut marquee: ResMut<TimelineMarquee>,
) {
    let Ok((computed, global)) = bodies.get(event.event_target()) else {
        return;
    };
    let at = local_point(
        event.pointer_location.position / ui_scale.0,
        computed,
        global,
    );
    marquee.from = Some(at);
    marquee.to = at;
    event.propagate(false);
}

/// Track the open corner of the marquee.
pub fn handle_marquee_drag(
    mut event: On<Pointer<Drag>>,
    bodies: Query<(&ComputedNode, &UiGlobalTransform), With<TimelineSheetBody>>,
    ui_scale: Res<UiScale>,
    mut marquee: ResMut<TimelineMarquee>,
) {
    let Ok((computed, global)) = bodies.get(event.event_target()) else {
        return;
    };
    if marquee.from.is_none() {
        return;
    }
    marquee.to = local_point(
        event.pointer_location.position / ui_scale.0,
        computed,
        global,
    );
    event.propagate(false);
}

/// Close the marquee and ask for every key it covered.
pub fn handle_marquee_end(
    mut event: On<Pointer<DragEnd>>,
    diamonds: Query<(&TimelineKeyframeHandle, &ComputedNode, &UiGlobalTransform)>,
    bodies: Query<(&ComputedNode, &UiGlobalTransform), With<TimelineSheetBody>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut marquee: ResMut<TimelineMarquee>,
    mut selected: MessageWriter<KeyframesMarqueeSelected>,
) {
    let Ok((body_node, body_global)) = bodies.get(event.event_target()) else {
        return;
    };
    let Some(from) = marquee.from.take() else {
        return;
    };
    let low = from.min(marquee.to);
    let high = from.max(marquee.to);
    let covered: Vec<Entity> = diamonds
        .iter()
        .filter_map(|(handle, computed, global)| {
            let (_, _, centre) = global.to_scale_angle_translation();
            let at = local_point(
                centre * computed.inverse_scale_factor(),
                body_node,
                body_global,
            );
            (at.x >= low.x && at.x <= high.x && at.y >= low.y && at.y <= high.y)
                .then_some(handle.keyframe)
        })
        .collect();
    if !covered.is_empty() {
        selected.write(KeyframesMarqueeSelected {
            keyframes: covered,
            additive: keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]),
        });
    }
    event.propagate(false);
}

/// Draw the open marquee, and take it away once the drag closes.
pub fn update_marquee_box(
    marquee: Res<TimelineMarquee>,
    bodies: Query<Entity, With<TimelineSheetBody>>,
    boxes: Query<Entity, With<TimelineMarqueeBox>>,
    mut nodes: Query<&mut Node, With<TimelineMarqueeBox>>,
    mut commands: Commands,
) {
    let Some(from) = marquee.from else {
        for drawn in &boxes {
            commands.entity(drawn).despawn();
        }
        return;
    };
    let low = from.min(marquee.to);
    let size = (marquee.to - from).abs();
    if boxes.is_empty() {
        let Some(body) = bodies.iter().next() else {
            return;
        };
        commands.spawn((
            TimelineMarqueeBox,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(low.x),
                top: Val::Px(low.y),
                width: Val::Px(size.x),
                height: Val::Px(size.y),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(tokens::SELECTED_BG.with_alpha(0.25)),
            BorderColor::all(tokens::SELECTED_BORDER),
            Pickable::IGNORE,
            ChildOf(body),
        ));
        return;
    }
    for mut node in &mut nodes {
        node.left = Val::Px(low.x);
        node.top = Val::Px(low.y);
        node.width = Val::Px(size.x);
        node.height = Val::Px(size.y);
    }
}

/// The result of a snap attempt: the time it landed on, and the key it landed
/// on when it landed on one.
#[derive(Debug, Clone, Copy)]
struct SnapResult {
    time: f32,
    hovered_keyframe: Option<Entity>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "two call sites, and every argument is one query the snap reads"
)]
fn seek_for_pointer(
    logical_cursor_x: f32,
    computed: &ComputedNode,
    global: &UiGlobalTransform,
    clip_entity: Entity,
    clips: &Query<(&Clip, Option<&Children>)>,
    contents: &ClipContents,
    snap: &TimelineSnap,
    imported: &ImportedClipView,
    keys: &ButtonInput<KeyCode>,
) -> SnapResult {
    let duration = if clip_entity == Entity::PLACEHOLDER {
        imported.duration.max(0.01)
    } else {
        clip_display_duration(clip_entity, clips)
    };
    let raw = time_for_cursor(logical_cursor_x, computed, global, duration);
    // Shift holds snapping off for precise placement, as it does for the grid
    // and the viewport elsewhere in the editor.
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) || !snap.enabled {
        return SnapResult {
            time: raw,
            hovered_keyframe: None,
        };
    }
    let keyframes = if clip_entity == Entity::PLACEHOLDER {
        Vec::new()
    } else {
        all_keyframes_for_clip(clip_entity, clips, contents)
    };
    apply_snap(raw, duration, snap, &keyframes)
}

fn time_for_cursor(
    logical_cursor_x: f32,
    computed: &ComputedNode,
    global: &UiGlobalTransform,
    duration: f32,
) -> f32 {
    let (_, _, centre) = global.to_scale_angle_translation();
    let inverse = computed.inverse_scale_factor();
    let centre = centre * inverse;
    let size = computed.size() * inverse;
    let left = centre.x - size.x * 0.5;
    ((logical_cursor_x - left) / size.x.max(1.0)).clamp(0.0, 1.0) * duration
}

/// A point in a node's own space, with the origin at its top left corner.
fn local_point(logical: Vec2, computed: &ComputedNode, global: &UiGlobalTransform) -> Vec2 {
    let (_, _, centre) = global.to_scale_angle_translation();
    let inverse = computed.inverse_scale_factor();
    let size = computed.size() * inverse;
    logical - (centre * inverse - size * 0.5)
}

/// Snap a raw time to the nearest frame or key, whichever is nearer, so long
/// as it falls inside the threshold.
///
/// A key wins a tie, because a key is somewhere the author put something, and
/// it is the only case the caller shows feedback for.
fn apply_snap(
    raw_time: f32,
    duration: f32,
    snap: &TimelineSnap,
    keyframes: &[(Entity, f32)],
) -> SnapResult {
    if !snap.enabled || duration <= 0.0 {
        return SnapResult {
            time: raw_time,
            hovered_keyframe: None,
        };
    }
    let threshold = snap.threshold_ratio * duration;
    let mut best = raw_time;
    let mut best_dist = threshold;
    let mut hovered = None;

    if snap.snap_to_ticks {
        let snapped = snap.round(raw_time);
        let dist = (snapped - raw_time).abs();
        if dist < best_dist {
            best_dist = dist;
            best = snapped.clamp(0.0, duration);
        }
    }
    if snap.snap_to_keyframes {
        for &(entity, time) in keyframes {
            let dist = (time - raw_time).abs();
            if dist <= best_dist {
                best_dist = dist;
                best = time;
                hovered = Some(entity);
            }
        }
    }
    SnapResult {
        time: best,
        hovered_keyframe: hovered,
    }
}

/// Every `(entity, time)` pair in a clip, across all its tracks.
fn all_keyframes_for_clip(
    clip_entity: Entity,
    clips: &Query<(&Clip, Option<&Children>)>,
    contents: &ClipContents,
) -> Vec<(Entity, f32)> {
    let Ok((_, clip_children)) = clips.get(clip_entity) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for track in clip_children.into_iter().flatten() {
        let Ok((_, track_children)) = contents.tracks.get(*track) else {
            continue;
        };
        for key in track_children.into_iter().flatten() {
            if let Some(time) = contents.key_time(*key) {
                out.push((*key, time));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap_at(rate: f32) -> TimelineSnap {
        TimelineSnap { rate, ..default() }
    }

    #[test]
    fn dragging_a_key_snaps_to_the_rate() {
        let snap = snap_at(30.0);

        // A third of a second is ten frames at thirty, and a time just off it
        // has to land back on the frame rather than between two.
        assert!(
            (snap.round(0.334) - 10.0 / 30.0).abs() < 1e-5,
            "{}",
            snap.round(0.334)
        );
        assert!((snap.round(0.5) - 15.0 / 30.0).abs() < 1e-5);
        assert_eq!(snap.frame_of(0.5), 15);
    }

    #[test]
    fn snapping_switched_off_leaves_a_time_where_it_was_dragged() {
        let snap = TimelineSnap {
            enabled: false,
            ..snap_at(30.0)
        };
        assert!((snap.round(0.3337) - 0.3337).abs() < f32::EPSILON);
    }

    #[test]
    fn a_key_within_the_threshold_wins_the_snap_over_the_frame_beside_it() {
        let snap = snap_at(30.0);
        let key = Entity::from_raw_u32(7).expect("a test entity");

        let result = apply_snap(0.505, 2.0, &snap, &[(key, 0.5051)]);

        assert_eq!(result.hovered_keyframe, Some(key));
        assert!((result.time - 0.5051).abs() < 1e-6);
    }
}
