//! Blender-style hover tooltip for operator-bound UI: bold label,
//! wrapped description, and an `ID: ...` footer pulled from the
//! operator registry.
//!
//! Auto-attached to any UI entity that carries [`ButtonOperatorCall`],
//! so editor buttons and menu entries get the tooltip without extra
//! wiring. The plain [`jackdaw_feathers::tooltip::Tooltip`] widget
//! still works for non-operator buttons.

use std::borrow::Cow;
use std::time::Duration;

use bevy::{picking::hover::Hovered, prelude::*, window::PrimaryWindow};
use jackdaw_api_internal::lifecycle::OperatorEntity;
use jackdaw_feathers::{
    button::ButtonOperatorCall,
    popover::{self, PopoverPlacement, PopoverProps},
    tokens,
    tooltip::Tooltip,
};

/// Delay before the tooltip appears. Long enough to skip flicker on
/// quick mouse-overs, short enough to feel responsive.
const HOVER_DELAY: Duration = Duration::from_millis(300);

pub struct OperatorTooltipPlugin;

impl Plugin for OperatorTooltipPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OperatorTooltipState>()
            .add_observer(auto_attach_operator_tooltip)
            .add_systems(Update, tick_operator_tooltip);
    }
}

/// Marker on a UI entity that should show the operator's hover
/// tooltip. Auto-attached alongside [`ButtonOperatorCall`]; insert it
/// manually for non-button UI (picker rows, custom hover targets).
#[derive(Component, Clone, Debug)]
pub struct OperatorTooltip(pub Cow<'static, str>);

#[derive(Resource, Default)]
struct OperatorTooltipState {
    /// Currently-hovered tagged entity, with elapsed hover time.
    pending: Option<(Entity, Duration)>,
    /// Spawned popover entity, if the tooltip is currently visible.
    active: Option<Entity>,
}

/// Mirror a [`ButtonOperatorCall`]'s id into [`OperatorTooltip`] when
/// the button is spawned, so any operator button gets the rich tooltip
/// for free.
fn auto_attach_operator_tooltip(
    trigger: On<Add, ButtonOperatorCall>,
    calls: Query<&ButtonOperatorCall>,
    existing: Query<&OperatorTooltip>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    if existing.contains(entity) {
        return;
    }
    let Ok(call) = calls.get(entity) else {
        return;
    };
    // Remove any plain `Tooltip(String)` so the rich operator tooltip
    // doesn't double up on feathers menu/dropdown items that attach
    // both as a dev-time hint.
    commands
        .entity(entity)
        .remove::<Tooltip>()
        .insert(OperatorTooltip(call.0.clone()));
}

/// Tick the hover delay and spawn / despawn the tooltip popover.
fn tick_operator_tooltip(
    time: Res<Time>,
    targets: Query<(Entity, &OperatorTooltip, &Hovered)>,
    operators: Query<&OperatorEntity>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut state: ResMut<OperatorTooltipState>,
    mut commands: Commands,
) {
    // Find the currently-hovered tagged entity, if any.
    let hovered = targets
        .iter()
        .find_map(|(entity, tip, hover)| hover.get().then_some((entity, tip)));

    let Some((entity, tip)) = hovered else {
        // Mouse left every tagged entity. Cancel timer and tear down
        // any active tooltip.
        state.pending = None;
        if let Some(active) = state.active.take() {
            commands.entity(active).try_despawn();
        }
        return;
    };

    // Reset the timer if the hover target changed.
    if state.pending.is_none_or(|(prev, _)| prev != entity) {
        state.pending = Some((entity, Duration::ZERO));
        if let Some(active) = state.active.take() {
            commands.entity(active).try_despawn();
        }
    }

    let already_visible = state.active.is_some();
    let Some((_, elapsed)) = state.pending.as_mut() else {
        return;
    };
    *elapsed += time.delta();

    if already_visible || *elapsed < HOVER_DELAY {
        return;
    }

    // Look up the operator metadata. If the id doesn't resolve (e.g.
    // an extension dynamically unregistered the operator) we just skip.
    let Some(op) = operators.iter().find(|o| o.id() == tip.0.as_ref()) else {
        return;
    };

    let cursor_pos = window.cursor_position();
    let popover_entity = commands
        .spawn(popover::popover(
            PopoverProps::new(entity)
                .with_position(cursor_pos)
                .with_placement(PopoverPlacement::BottomStart)
                .with_padding(10.0)
                .with_z_index(300)
                .with_node(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(tokens::SPACING_XS),
                    max_width: Val::Px(360.0),
                    ..Default::default()
                }),
        ))
        .id();

    // Title (bold, primary).
    commands.spawn((
        Text::new(op.label().to_string()),
        TextFont {
            font_size: tokens::FONT_SM,
            weight: FontWeight::MEDIUM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(popover_entity),
    ));

    // Description (wrapped, secondary). Skipped if blank so we don't
    // render an empty paragraph.
    if !op.description().is_empty() {
        commands.spawn((
            Text::new(op.description().to_string()),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(popover_entity),
        ));
    }

    // Id footer (dim, like Blender's `Python: bpy.ops.X.Y()`).
    commands.spawn((
        Text::new(format!("ID: {}", op.id())),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_TERTIARY),
        ChildOf(popover_entity),
    ));

    state.active = Some(popover_entity);
}
