//! The dope sheet: the row model both halves of the Timeline tab draw from,
//! the track column on the left, and the sheet on the right.
//!
//! The two columns are separate node trees that have to line up scanline for
//! scanline, so neither builds its own idea of what the rows are. Both are
//! handed the same [`SheetRow`] list, and every row is the same height.

use bevy::prelude::*;
use jackdaw_animation_runtime::ClipEvent;
use jackdaw_feathers::button::{
    ButtonOperatorCall, ButtonProps, ButtonSize, ButtonVariant, button,
};
use jackdaw_feathers::segmented::{segment_background, segment_label, segment_node, segmented_bar};
use jackdaw_feathers::tokens;

use crate::clip::{
    AnimationTrack, F32Keyframe, Interpolation, QuatKeyframe, TimelineView, Vec3Keyframe,
};

/// Width of the left column, wide enough for a group heading over an indented
/// property path with a checkbox and a badge beside it.
pub const TRACK_COLUMN_WIDTH: f32 = 300.0;

/// Height of one row, shared by both columns so they stay in step.
pub const ROW_HEIGHT: f32 = 24.0;

/// Height of the ruler, and of the spacer above the track column.
pub const RULER_HEIGHT: f32 = 24.0;

/// Height of the toolbar above the sheet.
pub const TOOLBAR_HEIGHT: f32 = 32.0;

/// Height of the footer below it.
pub const FOOTER_HEIGHT: f32 = 28.0;

/// Samples drawn across the sheet for one component of a curve.
const CURVE_SAMPLES: usize = 96;

/// What a row is, which decides what the two columns draw for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    /// Every key of the clip on one line.
    Summary,
    /// The clip's named moments.
    Events,
    /// A heading naming the entity the rows under it animate.
    Group,
    /// One property of one entity.
    Track(Entity),
    /// One bone of an imported clip, which cannot be edited here.
    Bone,
    /// The row that starts a new track.
    AddProperty,
}

/// One key drawn on a row.
#[derive(Debug, Clone)]
pub struct RowKey {
    /// The authoring entity the key lives on, so a click can address it.
    pub entity: Entity,
    /// Where it sits in the clip, in seconds.
    pub time: f32,
    /// The name an event key carries; `None` for a keyframe.
    pub name: Option<String>,
}

/// One row of the sheet.
#[derive(Debug, Clone)]
pub struct SheetRow {
    /// What the row is.
    pub kind: RowKind,
    /// What the left column calls it.
    pub label: String,
    /// How far the label is indented, in nesting steps.
    pub depth: u8,
    /// The keys drawn across the row.
    pub keys: Vec<RowKey>,
    /// The track's interpolation, for the badge. `None` off a track row.
    pub interpolation: Option<Interpolation>,
    /// Whether the track drives its property. Always `true` off a track row.
    pub enabled: bool,
}

impl SheetRow {
    fn plain(kind: RowKind, label: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
            depth: 0,
            keys: Vec::new(),
            interpolation: None,
            enabled: true,
        }
    }
}

/// The queries the row model reads, gathered so the systems that build rows
/// stay inside Bevy's parameter count.
#[derive(bevy::ecs::system::SystemParam)]
pub struct ClipContents<'w, 's> {
    /// Tracks and their keyframe children.
    pub tracks: Query<'w, 's, (&'static AnimationTrack, Option<&'static Children>)>,
    /// Keys holding a `Vec3`.
    pub vec3_keyframes: Query<'w, 's, &'static Vec3Keyframe>,
    /// Keys holding a `Quat`.
    pub quat_keyframes: Query<'w, 's, &'static QuatKeyframe>,
    /// Keys holding an `f32`.
    pub f32_keyframes: Query<'w, 's, &'static F32Keyframe>,
    /// The clip's named moments.
    pub events: Query<'w, 's, &'static ClipEvent>,
    /// Display names, for the group heading.
    pub names: Query<'w, 's, &'static Name>,
    /// Parent links, for finding the entity a clip animates.
    pub parents: Query<'w, 's, &'static ChildOf>,
}

impl ClipContents<'_, '_> {
    /// The `(entity, time)` pairs on one track, in the order they were found.
    pub fn keys_of_track(&self, track_children: Option<&Children>) -> Vec<RowKey> {
        track_children
            .into_iter()
            .flatten()
            .filter_map(|key| {
                self.key_time(*key).map(|time| RowKey {
                    entity: *key,
                    time,
                    name: None,
                })
            })
            .collect()
    }

    /// Where one keyframe entity sits, whatever value type it holds.
    pub fn key_time(&self, key: Entity) -> Option<f32> {
        if let Ok(key) = self.vec3_keyframes.get(key) {
            Some(key.time)
        } else if let Ok(key) = self.quat_keyframes.get(key) {
            Some(key.time)
        } else {
            self.f32_keyframes.get(key).ok().map(|key| key.time)
        }
    }

    /// The rows an authored clip draws: a summary, its events, then one group
    /// of tracks per entity, and the row that starts a new track.
    pub fn rows_for_clip(
        &self,
        clip_entity: Entity,
        clip_children: Option<&Children>,
    ) -> Vec<SheetRow> {
        let mut tracks = Vec::new();
        let mut events = Vec::new();
        for child in clip_children.into_iter().flatten() {
            if let Ok((track, track_children)) = self.tracks.get(*child) {
                let mut row = SheetRow::plain(RowKind::Track(*child), track.field_path.clone());
                row.depth = 1;
                row.keys = self.keys_of_track(track_children);
                row.interpolation = Some(track.interpolation);
                row.enabled = track.enabled;
                tracks.push(row);
            } else if let Ok(event) = self.events.get(*child) {
                events.push(RowKey {
                    entity: *child,
                    time: event.time,
                    name: Some(event.name.clone()),
                });
            }
        }

        let mut summary = SheetRow::plain(RowKind::Summary, "Summary");
        summary.keys = tracks
            .iter()
            .flat_map(|row| row.keys.iter().cloned())
            .collect();
        let mut event_row = SheetRow::plain(RowKind::Events, "Events");
        event_row.keys = events;

        let mut rows = vec![summary, event_row];
        // Tracks address the clip's parent today, so one group covers them
        // all. The heading is drawn anyway, because that is where a track on
        // a descendant would hang once tracks can name one.
        if !tracks.is_empty() {
            rows.push(SheetRow::plain(
                RowKind::Group,
                self.animated_entity_name(clip_entity),
            ));
            rows.extend(tracks);
        }
        rows.push(SheetRow::plain(RowKind::AddProperty, "Add property"));
        rows
    }

    /// What the group heading calls the entity a clip animates.
    fn animated_entity_name(&self, clip_entity: Entity) -> String {
        self.parents
            .get(clip_entity)
            .ok()
            .and_then(|parent| self.names.get(parent.parent()).ok())
            .map_or_else(|| "Unnamed".to_string(), |name| name.as_str().to_string())
    }

    /// The values one track's keys hold, as one series per component.
    ///
    /// A rotation is read as its four components, which is what the curves
    /// view can draw without deciding what an angle means.
    pub fn series_of_track(
        &self,
        track_children: Option<&Children>,
    ) -> Vec<(&'static str, Vec<(f32, f32)>)> {
        let mut vec3: Vec<(f32, Vec3)> = Vec::new();
        let mut quat: Vec<(f32, Quat)> = Vec::new();
        let mut scalar: Vec<(f32, f32)> = Vec::new();
        for key in track_children.into_iter().flatten() {
            if let Ok(key) = self.vec3_keyframes.get(*key) {
                vec3.push((key.time, key.value));
            } else if let Ok(key) = self.quat_keyframes.get(*key) {
                quat.push((key.time, key.value));
            } else if let Ok(key) = self.f32_keyframes.get(*key) {
                scalar.push((key.time, key.value));
            }
        }
        vec3.sort_by(|a, b| a.0.total_cmp(&b.0));
        quat.sort_by(|a, b| a.0.total_cmp(&b.0));
        scalar.sort_by(|a, b| a.0.total_cmp(&b.0));

        let mut series = Vec::new();
        if !vec3.is_empty() {
            for (axis, read) in [("x", 0usize), ("y", 1), ("z", 2)] {
                series.push((
                    axis,
                    vec3.iter().map(|(t, v)| (*t, v[read])).collect::<Vec<_>>(),
                ));
            }
        }
        if !quat.is_empty() {
            for (axis, read) in [("x", 0usize), ("y", 1), ("z", 2), ("w", 3)] {
                series.push((
                    axis,
                    quat.iter()
                        .map(|(t, v)| (*t, v.to_array()[read]))
                        .collect::<Vec<_>>(),
                ));
            }
        }
        if !scalar.is_empty() {
            series.push(("value", scalar));
        }
        series
    }
}

/// The colour one component of a curve is drawn in.
fn axis_color(axis: &str) -> Color {
    match axis {
        "x" => tokens::AXIS_X_COLOR,
        "y" => tokens::AXIS_Y_COLOR,
        "z" => tokens::AXIS_Z_COLOR,
        "w" => tokens::AXIS_W_COLOR,
        _ => tokens::ACCENT_BLUE,
    }
}

/// Marker on a rendered keyframe diamond, linking it back to the authoring
/// entity so clicks, drags and the highlight system can address it.
#[derive(Component, Clone, Copy)]
pub struct TimelineKeyframeHandle {
    /// The keyframe entity the diamond stands for.
    pub keyframe: Entity,
}

/// Marker on an event marker, carrying the event entity a click removes.
#[derive(Component, Clone, Copy)]
pub struct TimelineEventHandle {
    /// The [`ClipEvent`] entity the marker stands for.
    pub event: Entity,
}

/// Marker on a track row's label, so clicking one chooses the track the
/// curves view draws.
#[derive(Component, Clone, Copy)]
pub struct TimelineTrackRow {
    /// The track this row stands for.
    pub track: Entity,
}

/// Marker on the field that names a new track's `Component.field`.
#[derive(Component, Clone, Copy)]
pub struct TimelineAddPropertyInput {
    /// The clip the new track joins.
    pub clip: Entity,
}

/// Build the left column: a spacer matching the ruler, then one label row per
/// [`SheetRow`].
pub fn spawn_track_column(
    commands: &mut Commands,
    parent: Entity,
    rows: &[SheetRow],
    layout: SheetLayout,
    selected_track: Option<Entity>,
) {
    let column = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Px(TRACK_COLUMN_WIDTH),
                flex_shrink: 0.0,
                border: UiRect::right(Val::Px(1.0)),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            BackgroundColor(tokens::PANEL_HEADER_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Node {
            height: Val::Px(RULER_HEIGHT),
            width: Val::Percent(100.0),
            flex_shrink: 0.0,
            border: UiRect::bottom(Val::Px(1.0)),
            ..default()
        },
        BorderColor::all(tokens::BORDER_SUBTLE),
        ChildOf(column),
    ));

    for row in rows {
        spawn_label_row(commands, column, row, layout, selected_track);
    }
}

fn spawn_label_row(
    commands: &mut Commands,
    column: Entity,
    row: &SheetRow,
    layout: SheetLayout,
    selected_track: Option<Entity>,
) {
    let read_only = layout.read_only;
    let selected = matches!(row.kind, RowKind::Track(track) if Some(track) == selected_track);
    let entity = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                width: Val::Percent(100.0),
                height: Val::Px(ROW_HEIGHT),
                flex_shrink: 0.0,
                padding: UiRect {
                    left: Val::Px(tokens::SPACING_SM + f32::from(row.depth) * tokens::SPACING_LG),
                    right: Val::Px(tokens::SPACING_SM),
                    ..default()
                },
                border: UiRect::bottom(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(if selected {
                tokens::SELECTED_BG
            } else {
                Color::NONE
            }),
            BorderColor::all(tokens::BORDER_SUBTLE),
            Pickable::default(),
            ChildOf(column),
        ))
        .id();

    match &row.kind {
        RowKind::Track(track) if !read_only => {
            commands
                .entity(entity)
                .insert(TimelineTrackRow { track: *track });
            spawn_enable_box(commands, entity, *track, row.enabled);
            spawn_row_label(commands, entity, &row.label, tokens::TEXT_TERTIARY);
            if let Some(interpolation) = row.interpolation {
                spawn_interpolation_badge(commands, entity, *track, interpolation);
            }
        }
        RowKind::Group => {
            spawn_row_label(commands, entity, &row.label, tokens::TEXT_PRIMARY);
        }
        RowKind::AddProperty if !read_only => {
            spawn_add_property(commands, entity, layout.clip);
        }
        RowKind::AddProperty => {}
        _ => {
            let color = if read_only {
                tokens::TEXT_SECONDARY
            } else {
                tokens::TEXT_TERTIARY
            };
            spawn_row_label(commands, entity, &row.label, color);
        }
    }
}

fn spawn_row_label(commands: &mut Commands, row: Entity, label: &str, color: Color) {
    commands.spawn((
        Text::new(label.to_string()),
        TextColor(color),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        Node {
            flex_grow: 1.0,
            min_width: Val::Px(0.0),
            ..default()
        },
        ChildOf(row),
    ));
}

/// The checkbox that takes a track out of the compiled clip and puts it back.
fn spawn_enable_box(commands: &mut Commands, row: Entity, track: Entity, enabled: bool) {
    commands.spawn((
        button(
            ButtonProps::new("")
                .with_variant(ButtonVariant::Ghost)
                .with_size(ButtonSize::IconSM)
                .with_left_checkbox(enabled),
        ),
        ButtonOperatorCall::new("clip.track.enable")
            .with_param("track", track)
            .with_param("enabled", !enabled),
        ChildOf(row),
    ));
}

/// The badge that cycles a track through the interpolation modes.
fn spawn_interpolation_badge(
    commands: &mut Commands,
    row: Entity,
    track: Entity,
    interpolation: Interpolation,
) {
    commands.spawn((
        button(
            ButtonProps::new(interpolation.badge())
                .with_variant(ButtonVariant::Ghost)
                .with_size(ButtonSize::IconSM)
                .with_border_radius(BorderRadius::all(tokens::CORNER_RADIUS)),
        ),
        ButtonOperatorCall::new("clip.track.interpolation")
            .with_param("track", track)
            .with_param("mode", interpolation.next().as_str().to_string()),
        ChildOf(row),
    ));
}

/// The row that starts a new track: type `Component.field` and commit.
fn spawn_add_property(commands: &mut Commands, row: Entity, clip: Entity) {
    commands.spawn((
        Text::new("+".to_string()),
        TextColor(tokens::TEXT_SECONDARY),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        ChildOf(row),
    ));
    commands.spawn((
        TimelineAddPropertyInput { clip },
        Node {
            flex_grow: 1.0,
            min_width: Val::Px(0.0),
            ..default()
        },
        children![jackdaw_feathers::text_edit::text_edit(
            jackdaw_feathers::text_edit::TextEditProps::default()
                .with_placeholder("Component.field")
                .allow_empty(),
        )],
        ChildOf(row),
    ));
}

/// What one sheet needs to know about the clip it draws.
#[derive(Clone, Copy)]
pub struct SheetLayout {
    /// The clip whose rows these are.
    pub clip: Entity,
    /// The clip's length in seconds, which is what a time maps against.
    pub duration: f32,
    /// How much wider than the column the sheet is drawn.
    pub zoom: f32,
    /// Frames per second the minor ticks are drawn at.
    pub rate: f32,
    /// Which half of the sheet to draw.
    pub view: TimelineView,
    /// Whether the keys may be moved.
    pub read_only: bool,
}

/// Marker on the sheet's scrolling viewport.
#[derive(Component)]
pub struct TimelineSheetViewport;

/// Marker on the clickable ruler that seeks the playhead.
#[derive(Component)]
pub struct TimelineScrubber {
    /// The clip the ruler measures.
    pub clip: Entity,
}

/// Marker on the sheet body, which a drag marquee-selects across.
#[derive(Component)]
pub struct TimelineSheetBody {
    /// The clip the body draws.
    pub clip: Entity,
}

/// Marker on the moving playhead line.
#[derive(Component)]
pub struct TimelinePlayheadIndicator;

/// Build the right column: a ruler over the rows, with the playhead over
/// both.
pub fn spawn_sheet(
    commands: &mut Commands,
    parent: Entity,
    rows: &[SheetRow],
    layout: SheetLayout,
    series: &[(&'static str, Vec<(f32, f32)>)],
) {
    let viewport = commands
        .spawn((
            TimelineSheetViewport,
            Node {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                min_width: Val::Px(0.0),
                overflow: Overflow::scroll(),
                ..default()
            },
            BackgroundColor(tokens::PANEL_BG),
            ScrollPosition::default(),
            ChildOf(parent),
        ))
        .id();

    let sheet = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0 * layout.zoom.max(1.0)),
                flex_shrink: 0.0,
                position_type: PositionType::Relative,
                ..default()
            },
            ChildOf(viewport),
        ))
        .id();

    spawn_ruler(commands, sheet, layout);
    let body = commands
        .spawn((
            TimelineSheetBody { clip: layout.clip },
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                flex_grow: 1.0,
                position_type: PositionType::Relative,
                ..default()
            },
            Pickable::default(),
            ChildOf(sheet),
        ))
        .id();
    spawn_grid(commands, body, layout);

    match layout.view {
        TimelineView::Dopesheet => {
            for row in rows {
                spawn_strip(commands, body, row, layout);
            }
        }
        TimelineView::Curves => spawn_curves(commands, body, layout, series),
    }

    commands.spawn((
        TimelinePlayheadIndicator,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(0.0),
            top: Val::Px(0.0),
            width: Val::Px(2.0),
            height: Val::Percent(100.0),
            margin: UiRect::left(Val::Px(-1.0)),
            ..default()
        },
        BackgroundColor(tokens::ACCENT_BLUE),
        Pickable::IGNORE,
        ChildOf(sheet),
        children![(
            // The triangle head, drawn as a square turned onto a corner so it
            // reads as a pointer without needing a mesh.
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(-4.0),
                top: Val::Px(0.0),
                width: Val::Px(10.0),
                height: Val::Px(10.0),
                ..default()
            },
            BackgroundColor(tokens::ACCENT_BLUE),
            bevy::ui::ui_transform::UiTransform::from_rotation(Rot2::degrees(45.0)),
            Pickable::IGNORE,
        )],
    ));
}

/// The ruler: a label every whole tick, a minor tick every frame.
fn spawn_ruler(commands: &mut Commands, sheet: Entity, layout: SheetLayout) {
    let ruler = commands
        .spawn((
            TimelineScrubber { clip: layout.clip },
            Node {
                height: Val::Px(RULER_HEIGHT),
                width: Val::Percent(100.0),
                flex_shrink: 0.0,
                position_type: PositionType::Relative,
                border: UiRect::bottom(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(tokens::PANEL_HEADER_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            Pickable::default(),
            ChildOf(sheet),
        ))
        .id();
    if layout.duration <= 0.0 {
        return;
    }

    // Minor ticks run at the snap rate, thinned out so a long clip does not
    // draw a tick per pixel.
    let frames = (layout.duration * layout.rate).ceil().max(1.0);
    let every = (frames / 240.0).ceil().max(1.0);
    let mut frame = 0.0_f32;
    while frame <= frames {
        let percent = (frame / frames).clamp(0.0, 1.0) * 100.0;
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(percent),
                bottom: Val::Px(0.0),
                width: Val::Px(1.0),
                height: Val::Px(5.0),
                ..default()
            },
            BackgroundColor(Color::WHITE.with_alpha(0.18)),
            Pickable::IGNORE,
            ChildOf(ruler),
        ));
        frame += every;
    }

    let step = crate::timeline::pick_tick_step(layout.duration);
    let mut time = 0.0_f32;
    while time <= layout.duration + f32::EPSILON {
        let percent = (time / layout.duration).clamp(0.0, 1.0) * 100.0;
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(percent),
                top: Val::Px(0.0),
                height: Val::Percent(100.0),
                margin: UiRect::left(Val::Px(3.0)),
                align_items: AlignItems::Center,
                ..default()
            },
            Pickable::IGNORE,
            ChildOf(ruler),
            children![(
                Text::new(format!("{time:.2}s")),
                TextColor(tokens::TEXT_MUTED_COLOR.into()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_XS,
                    ..default()
                },
            )],
        ));
        time += step;
    }
}

/// The faint vertical lines under the rows, one per labelled tick.
fn spawn_grid(commands: &mut Commands, body: Entity, layout: SheetLayout) {
    if layout.duration <= 0.0 {
        return;
    }
    let step = crate::timeline::pick_tick_step(layout.duration);
    let mut time = step;
    while time < layout.duration - f32::EPSILON {
        let percent = (time / layout.duration).clamp(0.0, 1.0) * 100.0;
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(percent),
                top: Val::Px(0.0),
                width: Val::Px(1.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(Color::WHITE.with_alpha(0.05)),
            Pickable::IGNORE,
            ChildOf(body),
        ));
        time += step;
    }
}

/// One row of the sheet: the keys it holds, at the height of its label.
fn spawn_strip(commands: &mut Commands, body: Entity, row: &SheetRow, layout: SheetLayout) {
    let strip = commands
        .spawn((
            Node {
                position_type: PositionType::Relative,
                width: Val::Percent(100.0),
                height: Val::Px(ROW_HEIGHT),
                flex_shrink: 0.0,
                border: UiRect::bottom(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(match row.kind {
                RowKind::Summary => Color::WHITE.with_alpha(0.03),
                RowKind::Group => Color::WHITE.with_alpha(0.02),
                _ => Color::NONE,
            }),
            BorderColor::all(tokens::BORDER_SUBTLE),
            Pickable::IGNORE,
            ChildOf(body),
        ))
        .id();

    for key in &row.keys {
        let percent = time_percent(key.time, layout.duration);
        match &key.name {
            Some(name) => spawn_event_marker(commands, strip, key.entity, percent, name),
            None => spawn_diamond(commands, strip, key.entity, percent, &row.kind, layout),
        }
    }
}

fn time_percent(time: f32, duration: f32) -> f32 {
    if duration > 0.0 {
        (time / duration).clamp(0.0, 1.0) * 100.0
    } else {
        0.0
    }
}

fn spawn_diamond(
    commands: &mut Commands,
    strip: Entity,
    keyframe: Entity,
    percent: f32,
    kind: &RowKind,
    layout: SheetLayout,
) {
    // A summary diamond stands for a key on some other row, so it is drawn
    // narrower: it is a place to look, not the handle you drag.
    let size = if matches!(kind, RowKind::Summary) {
        7.0
    } else {
        10.0
    };
    let mut diamond = commands.spawn((
        TimelineKeyframeHandle { keyframe },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(percent),
            top: Val::Px((ROW_HEIGHT - size) * 0.5),
            width: Val::Px(size),
            height: Val::Px(size),
            margin: UiRect::left(Val::Px(-size * 0.5)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(tokens::ACCENT_BLUE),
        BorderColor::all(Color::WHITE.with_alpha(0.4)),
        bevy::ui::ui_transform::UiTransform::from_rotation(Rot2::degrees(45.0)),
        ChildOf(strip),
    ));
    if layout.read_only {
        diamond.insert(Pickable::IGNORE);
    } else {
        diamond.insert(Pickable::default());
    }
}

fn spawn_event_marker(
    commands: &mut Commands,
    strip: Entity,
    event: Entity,
    percent: f32,
    name: &str,
) {
    let marker = commands
        .spawn((
            TimelineEventHandle { event },
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(percent),
                top: Val::Px(4.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                ..default()
            },
            Pickable::default(),
            ChildOf(strip),
        ))
        .id();
    commands.spawn((
        Node {
            width: Val::Px(3.0),
            height: Val::Px(ROW_HEIGHT - 8.0),
            ..default()
        },
        BackgroundColor(tokens::TEXT_WARNING),
        Pickable::IGNORE,
        ChildOf(marker),
    ));
    commands.spawn((
        Text::new(name.to_string()),
        TextColor(tokens::TEXT_WARNING),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        Pickable::IGNORE,
        ChildOf(marker),
    ));
}

/// Draw one polyline per component of the chosen track, with its keys marked.
///
/// Each segment is a thin upright bar spanning the values either side of it,
/// which is a polyline that needs no rotation and so stays true whatever the
/// panel is resized to.
fn spawn_curves(
    commands: &mut Commands,
    body: Entity,
    layout: SheetLayout,
    series: &[(&'static str, Vec<(f32, f32)>)],
) {
    let plot = commands
        .spawn((
            Node {
                position_type: PositionType::Relative,
                width: Val::Percent(100.0),
                flex_grow: 1.0,
                min_height: Val::Px(ROW_HEIGHT * 4.0),
                ..default()
            },
            Pickable::IGNORE,
            ChildOf(body),
        ))
        .id();

    let Some((low, high)) = value_range(series) else {
        commands.spawn((
            Text::new("Choose a track to see its curve.".to_string()),
            TextColor(tokens::TEXT_SECONDARY),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            Node {
                margin: UiRect::all(Val::Px(tokens::SPACING_MD)),
                ..default()
            },
            Pickable::IGNORE,
            ChildOf(plot),
        ));
        return;
    };
    let span = (high - low).max(f32::EPSILON);
    // Height runs downwards, so a high value sits near the top.
    let height_percent = |value: f32| (1.0 - (value - low) / span).clamp(0.0, 1.0) * 100.0;

    for (axis, samples) in series {
        let color = axis_color(axis);
        let mut previous: Option<f32> = None;
        for step in 0..=CURVE_SAMPLES {
            let time = layout.duration * step as f32 / CURVE_SAMPLES as f32;
            let value = sample_at(samples, time);
            let at = height_percent(value);
            if let Some(from) = previous {
                let top = at.min(from);
                let height = (at - from).abs().max(0.4);
                commands.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Percent(
                            100.0 * (step as f32 - 1.0).max(0.0) / CURVE_SAMPLES as f32,
                        ),
                        top: Val::Percent(top),
                        width: Val::Px(2.0),
                        height: Val::Percent(height),
                        ..default()
                    },
                    BackgroundColor(color),
                    Pickable::IGNORE,
                    ChildOf(plot),
                ));
            }
            previous = Some(at);
        }
        for (time, value) in samples {
            commands.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(time_percent(*time, layout.duration)),
                    top: Val::Percent(height_percent(*value)),
                    width: Val::Px(6.0),
                    height: Val::Px(6.0),
                    margin: UiRect::new(Val::Px(-3.0), Val::Px(0.0), Val::Px(-3.0), Val::Px(0.0)),
                    border_radius: BorderRadius::all(Val::Px(3.0)),
                    ..default()
                },
                BackgroundColor(color),
                BorderColor::all(Color::WHITE.with_alpha(0.5)),
                Pickable::IGNORE,
                ChildOf(plot),
            ));
        }
    }
}

/// The lowest and highest value across every component, padded so a flat
/// curve still has a band to sit in the middle of.
fn value_range(series: &[(&'static str, Vec<(f32, f32)>)]) -> Option<(f32, f32)> {
    let mut low = f32::INFINITY;
    let mut high = f32::NEG_INFINITY;
    for (_, samples) in series {
        for (_, value) in samples {
            low = low.min(*value);
            high = high.max(*value);
        }
    }
    if !low.is_finite() || !high.is_finite() {
        return None;
    }
    let pad = ((high - low) * 0.1).max(0.05);
    Some((low - pad, high + pad))
}

/// The value a series holds at `time`, read linearly between its keys.
fn sample_at(samples: &[(f32, f32)], time: f32) -> f32 {
    match samples {
        [] => 0.0,
        [(_, only)] => *only,
        _ => {
            let first = samples[0];
            let last = samples[samples.len() - 1];
            if time <= first.0 {
                return first.1;
            }
            if time >= last.0 {
                return last.1;
            }
            let at = samples.partition_point(|(t, _)| *t <= time).max(1);
            let (t0, v0) = samples[at - 1];
            let (t1, v1) = samples[at];
            let span = t1 - t0;
            if span <= f32::EPSILON {
                v1
            } else {
                v0 + (v1 - v0) * (time - t0) / span
            }
        }
    }
}

/// Marker on one half of the view toggle.
#[derive(Component, Clone, Copy)]
pub struct TimelineViewSegment {
    /// The view this half asks for.
    pub view: TimelineView,
}

/// Marker on the footer's readout of the key the selection is on.
#[derive(Component)]
pub struct TimelineKeyReadout;

/// Marker on the zoom slider, which widens the sheet under the ruler.
#[derive(Component)]
pub struct TimelineZoomSlider;

/// Build the footer: the view toggle, what the selected key holds, and zoom.
pub fn spawn_footer(commands: &mut Commands, parent: Entity, view: TimelineView, zoom: f32) {
    let footer = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_MD),
                width: Val::Percent(100.0),
                height: Val::Px(FOOTER_HEIGHT),
                flex_shrink: 0.0,
                padding: UiRect::horizontal(Val::Px(tokens::SPACING_SM)),
                border: UiRect::top(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(tokens::PANEL_HEADER_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(parent),
        ))
        .id();

    let bar = commands.spawn((segmented_bar(), ChildOf(footer))).id();
    for (label, mode) in [
        ("Dopesheet", TimelineView::Dopesheet),
        ("Curves", TimelineView::Curves),
    ] {
        let segment = commands
            .spawn((
                TimelineViewSegment { view: mode },
                segment_node(),
                bevy::ui_widgets::RadioButton,
                BackgroundColor(segment_background(view == mode)),
                Pickable::default(),
                children![segment_label(label)],
                ChildOf(bar),
            ))
            .id();
        if view == mode {
            commands.entity(segment).insert(bevy::ui::Checked);
        }
    }

    commands.spawn((
        TimelineKeyReadout,
        Text::new(String::new()),
        TextColor(tokens::TEXT_SECONDARY),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        Node {
            flex_grow: 1.0,
            min_width: Val::Px(0.0),
            ..default()
        },
        ChildOf(footer),
    ));

    commands.spawn((
        Text::new("Zoom".to_string()),
        TextColor(tokens::TEXT_MUTED_COLOR.into()),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        ChildOf(footer),
    ));
    commands.spawn((
        TimelineZoomSlider,
        Node {
            width: Val::Px(120.0),
            ..default()
        },
        children![jackdaw_feathers::text_edit::text_edit(
            jackdaw_feathers::text_edit::TextEditProps::default()
                .numeric_f32()
                .with_min(1.0)
                .with_max(20.0)
                .with_default_value(format!("{zoom:.1}")),
        )],
        ChildOf(footer),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::Clip;

    /// The rows one clip draws, read out of a real world so the row model is
    /// exercised through the same queries the panel uses.
    fn rows_of(
        clip: In<Entity>,
        contents: ClipContents,
        children: Query<&Children>,
    ) -> Vec<SheetRow> {
        contents.rows_for_clip(*clip, children.get(*clip).ok())
    }

    /// A door with one clip on it: two keys on the translation, one on the
    /// scale, and one named moment.
    fn world_with_a_clip() -> (App, Entity) {
        let mut app = App::new();
        let door = app.world_mut().spawn(Name::new("Door")).id();
        let clip = app
            .world_mut()
            .spawn((Clip::default(), Name::new("Open"), ChildOf(door)))
            .id();
        let translation = app
            .world_mut()
            .spawn((
                AnimationTrack::new("Transform", "translation"),
                ChildOf(clip),
            ))
            .id();
        for time in [0.0, 1.5] {
            app.world_mut().spawn((
                Vec3Keyframe {
                    time,
                    value: Vec3::ZERO,
                },
                ChildOf(translation),
            ));
        }
        let scale = app
            .world_mut()
            .spawn((AnimationTrack::new("Transform", "scale"), ChildOf(clip)))
            .id();
        app.world_mut().spawn((
            Vec3Keyframe {
                time: 0.75,
                value: Vec3::ONE,
            },
            ChildOf(scale),
        ));
        app.world_mut().spawn((
            ClipEvent {
                time: 0.5,
                name: "step".to_string(),
            },
            ChildOf(clip),
        ));
        (app, clip)
    }

    #[test]
    fn the_summary_row_holds_every_key_of_the_clip() {
        let (mut app, clip) = world_with_a_clip();

        let rows = app
            .world_mut()
            .run_system_cached_with(rows_of, clip)
            .expect("the rows read");

        let summary = rows
            .iter()
            .find(|row| row.kind == RowKind::Summary)
            .expect("a summary row");
        let mut times: Vec<f32> = summary.keys.iter().map(|key| key.time).collect();
        times.sort_by(f32::total_cmp);
        assert_eq!(
            times,
            vec![0.0, 0.75, 1.5],
            "the summary should hold every key of every track"
        );

        let events = rows
            .iter()
            .find(|row| row.kind == RowKind::Events)
            .expect("an events row");
        assert_eq!(events.keys.len(), 1);
        assert_eq!(events.keys[0].name.as_deref(), Some("step"));
        assert!(
            !summary
                .keys
                .iter()
                .any(|key| key.entity == events.keys[0].entity),
            "an event is not a keyframe, so it does not join the summary"
        );
    }

    #[test]
    fn the_tracks_hang_under_a_heading_naming_what_they_animate() {
        let (mut app, clip) = world_with_a_clip();

        let rows = app
            .world_mut()
            .run_system_cached_with(rows_of, clip)
            .expect("the rows read");

        let heading = rows
            .iter()
            .position(|row| row.kind == RowKind::Group)
            .expect("a group heading");
        assert_eq!(rows[heading].label, "Door");
        assert!(
            rows[heading + 1..]
                .iter()
                .filter(|row| matches!(row.kind, RowKind::Track(_)))
                .count()
                == 2,
            "both tracks belong under the heading"
        );
        assert_eq!(
            rows.last().map(|row| row.kind.clone()),
            Some(RowKind::AddProperty),
            "the row that starts a new track comes last"
        );
    }

    #[test]
    fn a_flat_series_still_gets_a_band_to_sit_in() {
        let series = vec![("x", vec![(0.0, 1.0), (1.0, 1.0)])];
        let (low, high) = value_range(&series).expect("a range");
        assert!(low < 1.0 && high > 1.0, "{low}..{high}");
    }

    #[test]
    fn a_series_reads_linearly_between_its_keys() {
        let samples = [(0.0, 0.0), (2.0, 4.0)];
        assert!((sample_at(&samples, 1.0) - 2.0).abs() < 1e-5);
        assert!((sample_at(&samples, -1.0) - 0.0).abs() < 1e-5);
        assert!((sample_at(&samples, 9.0) - 4.0).abs() < 1e-5);
    }
}
