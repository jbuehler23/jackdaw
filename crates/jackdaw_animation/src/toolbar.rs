//! The Timeline tab's toolbar: which clip, whether edits are recorded, the
//! transport, where the playhead is, and how the clip is played back.
//!
//! Every control here dispatches an operator, so a script drives the tab the
//! same way a click does.

use bevy::prelude::*;
use jackdaw_feathers::button::{
    ButtonOperatorCall, ButtonProps, ButtonSize, ButtonVariant, IconButtonProps, button,
    icon_button,
};
use jackdaw_feathers::icons::IconFont;
use jackdaw_feathers::segmented::{segment_background, segment_label, segment_node, segmented_bar};
use jackdaw_feathers::text_edit::{TextEditProps, text_edit};
use jackdaw_feathers::tokens;
use lucide_icons::Icon;

use crate::clip::{LoopMode, TimelineSnap};
use crate::sheet::TOOLBAR_HEIGHT;

/// Marker on the combobox listing every authored clip in the open scene.
#[derive(Component, Clone)]
pub struct TimelineClipSelector {
    /// The clips the combobox lists, in the order it lists them.
    pub sibling_clips: Vec<Entity>,
}

/// Marker on the field holding the playhead's time in seconds.
#[derive(Component, Clone, Copy)]
pub struct TimelineTimeInput;

/// Marker on the field holding the playhead's frame at the snap rate.
#[derive(Component, Clone, Copy)]
pub struct TimelineFrameInput;

/// Marker on the field holding the clip's length.
#[derive(Component, Clone, Copy)]
pub struct TimelineDurationInput {
    /// The clip whose length the field writes.
    pub clip: Entity,
}

/// Marker on the field holding the preview speed multiplier.
#[derive(Component, Clone, Copy)]
pub struct TimelineSpeedInput {
    /// The clip whose speed the field writes.
    pub clip: Entity,
}

/// Marker on one half of the loop-mode toggle.
#[derive(Component, Clone, Copy)]
pub struct TimelineLoopSegment {
    /// The mode this half asks for.
    pub mode: LoopMode,
}

/// Marker on the field holding the snap rate in frames per second.
#[derive(Component, Clone, Copy)]
pub struct TimelineSnapRateInput;

/// What the toolbar draws itself from.
pub struct ToolbarState<'a> {
    /// The clip being edited, or the imported clip being shown.
    pub clip: Option<Entity>,
    /// Every authored clip in the scene, as `(entity, "Clip on Entity")`.
    pub clips: &'a [(Entity, String)],
    /// Where the playhead stands, in seconds.
    pub time: f32,
    /// The clip's length in seconds.
    pub duration: f32,
    /// What playback does at the end.
    pub loop_mode: LoopMode,
    /// The multiplier the preview plays at.
    pub speed: f32,
    /// Whether an inspector edit writes a key.
    pub recording: bool,
    /// Whether the pose either side of the playhead is drawn. Reserved.
    pub onion_skin: bool,
    /// Snapping, and the rate the time reads its frame at.
    pub snap: TimelineSnap,
    /// Whether the clip can be edited at all.
    pub read_only: bool,
}

/// Build the toolbar row.
pub fn spawn_toolbar(
    commands: &mut Commands,
    parent: Entity,
    state: &ToolbarState<'_>,
    icon_font: &IconFont,
) {
    let bar = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                width: Val::Percent(100.0),
                height: Val::Px(TOOLBAR_HEIGHT),
                flex_shrink: 0.0,
                padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_XS)),
                border: UiRect::bottom(Val::Px(1.0)),
                // A dock narrower than the row scrolls to the rest of it,
                // rather than squeezing the fields down to nothing.
                overflow: Overflow::scroll_x(),
                ..default()
            },
            BackgroundColor(tokens::PANEL_HEADER_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ScrollPosition::default(),
            ChildOf(parent),
        ))
        .id();

    spawn_clip_selector(commands, bar, state);
    commands.spawn((
        icon_button(IconButtonProps::new(Icon::Plus), &icon_font.0),
        ButtonOperatorCall::new("clip.new"),
        ChildOf(bar),
    ));
    separator(commands, bar);

    if !state.read_only {
        commands.spawn((
            button(
                ButtonProps::new("Rec")
                    .with_size(ButtonSize::MD)
                    .with_variant(if state.recording {
                        ButtonVariant::Destructive
                    } else {
                        ButtonVariant::Ghost
                    })
                    .with_left_icon(Icon::Circle),
            ),
            ButtonOperatorCall::new("clip.record.toggle"),
            ChildOf(bar),
        ));
        separator(commands, bar);
    }

    for (icon, operator) in [
        (Icon::SkipBack, "clip.timeline.jump_start"),
        (Icon::ChevronLeft, "clip.timeline.jump_prev_keyframe"),
        (Icon::Play, "clip.play"),
        (Icon::Pause, "clip.pause"),
        (Icon::ChevronRight, "clip.timeline.jump_next_keyframe"),
        (Icon::SkipForward, "clip.timeline.jump_end"),
    ] {
        commands.spawn((
            icon_button(IconButtonProps::new(icon), &icon_font.0),
            ButtonOperatorCall::new(operator),
            ChildOf(bar),
        ));
    }
    separator(commands, bar);

    // Time and frame are the same reading twice: seconds for authoring
    // against a length, frames for landing on the rate the sheet snaps to.
    numeric_field(
        commands,
        bar,
        TimelineTimeInput,
        56.0,
        TextEditProps::default()
            .numeric_f32()
            .with_suffix("s")
            .with_min(0.0)
            .with_max(3600.0)
            .with_default_value(format!("{:.2}", state.time)),
    );
    numeric_field(
        commands,
        bar,
        TimelineFrameInput,
        48.0,
        TextEditProps::default()
            .numeric_f32()
            .with_suffix("f")
            .with_min(0.0)
            .with_max(216_000.0)
            .with_default_value(format!("{}", state.snap.frame_of(state.time))),
    );

    if let Some(clip) = state.clip.filter(|_| !state.read_only) {
        label(commands, bar, "len");
        numeric_field(
            commands,
            bar,
            TimelineDurationInput { clip },
            64.0,
            TextEditProps::default()
                .numeric_f32()
                .with_suffix("s")
                .with_min(0.01)
                .with_max(3600.0)
                .with_default_value(format!("{:.2}", state.duration)),
        );

        let loop_bar = commands.spawn((segmented_bar(), ChildOf(bar))).id();
        for mode in [LoopMode::Clamp, LoopMode::Wrap] {
            let chosen = state.loop_mode == mode;
            let half = commands
                .spawn((
                    TimelineLoopSegment { mode },
                    segment_node(),
                    bevy::ui_widgets::RadioButton,
                    BackgroundColor(segment_background(chosen)),
                    Pickable::default(),
                    children![segment_label(if mode == LoopMode::Clamp {
                        "Clamp"
                    } else {
                        "Wrap"
                    })],
                    ChildOf(loop_bar),
                ))
                .id();
            if chosen {
                commands.entity(half).insert(bevy::ui::Checked);
            }
        }

        label(commands, bar, "x");
        numeric_field(
            commands,
            bar,
            TimelineSpeedInput { clip },
            48.0,
            TextEditProps::default()
                .numeric_f32()
                .with_min(0.05)
                .with_max(10.0)
                .with_default_value(format!("{:.2}", state.speed)),
        );
    }

    separator(commands, bar);
    commands.spawn((
        button(
            ButtonProps::new("Snap")
                .with_size(ButtonSize::MD)
                .with_variant(if state.snap.enabled {
                    ButtonVariant::Active
                } else {
                    ButtonVariant::Ghost
                }),
        ),
        ButtonOperatorCall::new("clip.snap").with_param("enabled", !state.snap.enabled),
        ChildOf(bar),
    ));
    numeric_field(
        commands,
        bar,
        TimelineSnapRateInput,
        56.0,
        TextEditProps::default()
            .numeric_f32()
            .with_suffix("fps")
            .with_min(1.0)
            .with_max(240.0)
            .with_default_value(format!("{:.0}", state.snap.rate)),
    );

    // Onion skin is present so the toolbar reads as finished; nothing draws
    // the neighbouring poses yet.
    commands.spawn((
        button(
            ButtonProps::new("")
                .with_size(ButtonSize::IconSM)
                .with_variant(if state.onion_skin {
                    ButtonVariant::Active
                } else {
                    ButtonVariant::Ghost
                })
                .with_left_icon(Icon::Layers),
        ),
        ButtonOperatorCall::new("clip.onion_skin").with_param("enabled", !state.onion_skin),
        ChildOf(bar),
    ));
}

/// The clip list: every authored clip in the open scene, named for the entity
/// it animates so two clips called "Idle" are told apart.
fn spawn_clip_selector(commands: &mut Commands, bar: Entity, state: &ToolbarState<'_>) {
    let selected = state
        .clip
        .and_then(|clip| state.clips.iter().position(|(entity, _)| *entity == clip))
        .unwrap_or(0);
    let options: Vec<jackdaw_feathers::combobox::ComboBoxOptionData> = state
        .clips
        .iter()
        .map(|(_, label)| jackdaw_feathers::combobox::ComboBoxOptionData::new(label.clone()))
        .collect();
    let wrapper = commands
        .spawn((
            Node {
                min_width: Val::Px(140.0),
                max_width: Val::Px(220.0),
                flex_shrink: 0.0,
                ..default()
            },
            ChildOf(bar),
        ))
        .id();
    if options.is_empty() {
        commands.spawn((
            Text::new("No clip".to_string()),
            TextColor(tokens::TEXT_SECONDARY),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            ChildOf(wrapper),
        ));
        return;
    }
    commands.spawn((
        TimelineClipSelector {
            sibling_clips: state.clips.iter().map(|(entity, _)| *entity).collect(),
        },
        jackdaw_feathers::combobox::combobox_with_selected(options, selected),
        ChildOf(wrapper),
    ));
}

/// A field with a marker on its wrapper, which is where the commit observers
/// look: the text edit spawns its own node, so the marker cannot share it.
fn numeric_field(
    commands: &mut Commands,
    bar: Entity,
    marker: impl Component,
    width: f32,
    props: TextEditProps,
) {
    commands.spawn((
        marker,
        Node {
            width: Val::Px(width),
            flex_shrink: 0.0,
            ..default()
        },
        children![text_edit(props)],
        ChildOf(bar),
    ));
}

fn label(commands: &mut Commands, bar: Entity, text: &str) {
    commands.spawn((
        Node {
            flex_shrink: 0.0,
            ..default()
        },
        Text::new(text.to_string()),
        TextColor(tokens::TEXT_MUTED_COLOR.into()),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        ChildOf(bar),
    ));
}

fn separator(commands: &mut Commands, bar: Entity) {
    commands.spawn((
        Node {
            width: Val::Px(1.0),
            height: Val::Px(TOOLBAR_HEIGHT - 10.0),
            flex_shrink: 0.0,
            margin: UiRect::horizontal(Val::Px(tokens::SPACING_XS)),
            ..default()
        },
        BackgroundColor(tokens::BORDER_SUBTLE),
        ChildOf(bar),
    ));
}
