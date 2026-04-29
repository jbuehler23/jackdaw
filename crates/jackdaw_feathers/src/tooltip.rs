//! Generic hover-tooltip primitive.
//!
//! Any UI entity that carries a [`Tooltip`] component plus
//! [`bevy::picking::hover::Hovered`] gets a Blender-style popover
//! after a short delay: bold title, optional wrapped description,
//! optional dim footer (operator signature, type path, etc.).
//!
//! This module owns nothing about *where* the tooltip data comes
//! from. Domain bridges in the editor crate (operator buttons,
//! inspector headers, …) attach a small "source" component plus an
//! observer that derives a [`Tooltip`] from it. Call sites that have
//! the data already in hand can also attach a [`Tooltip`] directly ;
//! the renderer doesn't care how the component got there.
//!
//! See `src/operator_tooltip.rs` and `src/inspector/component_tooltip.rs`
//! in the editor crate for two examples of the source-component +
//! `On<Add>` observer pattern this plugin is designed to feed.

use std::time::Duration;

use bevy::{picking::hover::Hovered, prelude::*, window::PrimaryWindow};

use crate::{
    popover::{self, PopoverPlacement, PopoverProps},
    tokens,
};

/// Delay before the title-only popover appears. Long enough to skip
/// flicker on quick mouse-overs, short enough to feel responsive.
const SHORT_HOVER_DELAY: Duration = Duration::from_millis(300);

/// Additional delay (counted from the start of the hover) before the
/// description + footer are appended to the existing popover.
/// Blender-style two-stage tooltips: glance to discover the name,
/// linger to read the manual.
const FULL_HOVER_DELAY: Duration = Duration::from_millis(1200);

/// Maximum width of the popover. Wider lines wrap; taller content
/// grows the popover vertically without re-positioning.
const TOOLTIP_MAX_WIDTH: f32 = 360.0;

/// Padding around the popover content. Tuned to leave clearance for
/// the descenders in the bottom-most line so wrapped content isn't
/// clipped.
const TOOLTIP_PADDING: f32 = 10.0;

/// Hover-tooltip data. Attach to any entity that also carries
/// [`Hovered`] to make it surface a popover after a short hover
/// delay (300 ms).
///
/// All three fields are plain strings; empty strings render no line
/// (so a title-only tooltip skips the description and footer
/// children, leaving a tight one-line popover). Builder methods
/// [`Tooltip::title`] / [`Tooltip::with_description`] /
/// [`Tooltip::with_footer`] make construction terse.
#[derive(Component, Clone, Debug, Default)]
pub struct Tooltip {
    /// Bold first line. Operator label, component short name, etc.
    pub title: String,
    /// Wrapped paragraph below the title. Empty = skipped.
    pub description: String,
    /// Dim trailing line (operator signature, rust type path, etc.).
    /// Empty = skipped.
    pub footer: String,
}

impl Tooltip {
    pub fn title(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            description: String::new(),
            footer: String::new(),
        }
    }

    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    #[must_use]
    pub fn with_footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = footer.into();
        self
    }
}

pub struct TooltipPlugin;

impl Plugin for TooltipPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut().register_component::<Tooltip>();
        app.init_resource::<TooltipState>()
            .add_systems(Update, tick_tooltip);
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum TooltipStage {
    #[default]
    None,
    /// Title-only popover spawned (after `SHORT_HOVER_DELAY`).
    Title,
    /// Title + description + footer spawned (after `FULL_HOVER_DELAY`).
    Full,
}

#[derive(Resource, Default)]
struct TooltipState {
    /// Currently-hovered tagged entity, with elapsed hover time.
    pending: Option<(Entity, Duration)>,
    /// Spawned popover entity, if the tooltip is currently visible.
    active: Option<Entity>,
    /// Which stage of the popover is currently rendered for `active`.
    stage: TooltipStage,
}

/// Tick the hover delay and spawn / despawn the tooltip popover.
/// Two-stage Blender-style: a glance gets the title, lingering
/// expands to the full description + signature.
fn tick_tooltip(
    time: Res<Time>,
    targets: Query<(Entity, &Tooltip, &Hovered)>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut state: ResMut<TooltipState>,
    mut commands: Commands,
) {
    let hovered = targets
        .iter()
        .find_map(|(entity, tip, hover)| hover.get().then_some((entity, tip)));

    let Some((entity, tip)) = hovered else {
        // Mouse left every tagged entity. Cancel the timer and tear
        // down any active tooltip.
        state.pending = None;
        if let Some(active) = state.active.take() {
            commands.entity(active).try_despawn();
        }
        state.stage = TooltipStage::None;
        return;
    };

    // Reset the timer if the hover target changed.
    if state.pending.is_none_or(|(prev, _)| prev != entity) {
        state.pending = Some((entity, Duration::ZERO));
        if let Some(active) = state.active.take() {
            commands.entity(active).try_despawn();
        }
        state.stage = TooltipStage::None;
    }

    let Some((_, elapsed)) = state.pending.as_mut() else {
        return;
    };
    *elapsed += time.delta();
    let elapsed = *elapsed;

    match state.stage {
        TooltipStage::None if elapsed >= SHORT_HOVER_DELAY => {
            let cursor_pos = window.cursor_position();
            let popover_entity = commands
                .spawn(popover::popover(
                    PopoverProps::new(entity)
                        .with_position(cursor_pos)
                        .with_placement(PopoverPlacement::BottomStart)
                        .with_padding(TOOLTIP_PADDING)
                        .with_gap(tokens::SPACING_XS)
                        .with_z_index(300)
                        .with_node(Node {
                            flex_direction: FlexDirection::Column,
                            max_width: Val::Px(TOOLTIP_MAX_WIDTH),
                            ..Default::default()
                        }),
                ))
                .id();
            spawn_title(&mut commands, popover_entity, tip);
            state.active = Some(popover_entity);
            state.stage = TooltipStage::Title;
        }
        TooltipStage::Title if elapsed >= FULL_HOVER_DELAY => {
            if let Some(popover) = state.active {
                spawn_body(&mut commands, popover, tip);
                state.stage = TooltipStage::Full;
            }
        }
        _ => {}
    }
}

/// Title-only popover content (stage 1).
fn spawn_title(commands: &mut Commands, popover: Entity, tip: &Tooltip) {
    if tip.title.is_empty() {
        return;
    }
    commands.spawn((
        Text::new(tip.title.clone()),
        TextFont {
            font_size: tokens::FONT_SM,
            weight: FontWeight::MEDIUM,
            ..default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(popover),
    ));
}

/// Description + footer appended below the title (stage 2). Description
/// is the meaningful body the reader is here for, so it gets primary
/// weight; the footer (signature / type path) is dim metadata and gets
/// the darker grey.
fn spawn_body(commands: &mut Commands, popover: Entity, tip: &Tooltip) {
    if !tip.description.is_empty() {
        commands.spawn((
            Text::new(tip.description.clone()),
            TextFont {
                font_size: tokens::FONT_SM,
                ..default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            ChildOf(popover),
        ));
    }
    if !tip.footer.is_empty() {
        commands.spawn((
            Text::new(tip.footer.clone()),
            TextFont {
                font_size: tokens::FONT_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(popover),
        ));
    }
}
