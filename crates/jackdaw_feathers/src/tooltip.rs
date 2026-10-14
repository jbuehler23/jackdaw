//! Generic hover-tooltip primitive.
//!
//! Any UI entity that carries a [`Tooltip`] component plus
//! [`bevy::picking::hover::Hovered`] gets a two-stage popover
//! after a short delay: bold title, optional wrapped description,
//! optional dim footer (operator signature, type path, etc.).
//!
//! This module owns nothing about *where* the tooltip data comes
//! from. Domain bridges in the editor crate (operator buttons,
//! inspector headers, ...) attach a small "source" component plus an
//! observer that derives a [`Tooltip`] from it. Call sites that have
//! the data already in hand can also attach a [`Tooltip`] directly ;
//! the renderer doesn't care how the component got there.
//!
//! See `src/operator_tooltip.rs` and `src/inspector/component_tooltip.rs`
//! in the editor crate for two examples of the source-component +
//! `On<Add>` observer pattern this plugin is designed to feed.

use std::time::Duration;

use bevy::{
    picking::hover::Hovered,
    prelude::*,
    ui::{UiGlobalTransform, UiSystems},
    window::PrimaryWindow,
};

use crate::tokens;

/// Delay before the title-only popover appears. Long enough to skip
/// flicker on quick mouse-overs, short enough to feel responsive.
const SHORT_HOVER_DELAY: Duration = Duration::from_millis(300);

/// Additional delay (counted from the start of the hover) before the
/// description + footer are appended to the existing popover.
/// Two-stage tooltips: glance to discover the name,
/// linger to read the manual.
const FULL_HOVER_DELAY: Duration = Duration::from_millis(1200);

/// Maximum width of the popover, and its width once a description is shown,
/// so wrapped text and the popover's height agree. Wider lines wrap.
const TOOLTIP_MAX_WIDTH: f32 = 360.0;

/// Padding around the popover content. Tuned to leave clearance for
/// the descenders in the bottom-most line so wrapped content isn't
/// clipped.
const TOOLTIP_PADDING: f32 = 10.0;

/// Space between the hovered element and its tooltip.
const TOOLTIP_GAP: f32 = 4.0;

/// Closest a tooltip comes to the window edge.
const TOOLTIP_WINDOW_MARGIN: f32 = 8.0;

/// A shown tooltip, spawned at the UI root so it sizes to its text rather
/// than to the element it describes, and placed beside that element.
#[derive(Component)]
pub struct TooltipPopover {
    /// The hovered element.
    pub target: Entity,
}

/// Hover-tooltip data. Attach to any entity that also carries
/// [`Hovered`] to make it surface a popover after a short hover
/// delay (300 ms).
///
/// All four fields are plain strings; empty strings render no line
/// (so a title-only tooltip skips the description, keybind, and
/// footer children, leaving a tight one-line popover). Builder
/// methods [`Tooltip::title`] / [`Tooltip::with_description`] /
/// [`Tooltip::with_keybind`] / [`Tooltip::with_footer`] make
/// construction terse.
#[derive(Component, Clone, Debug, Default)]
pub struct Tooltip {
    /// Bold first line. Operator label, component short name, etc.
    pub title: String,
    /// Muted suffix on the title row (operator keybind, etc.).
    /// Rendered right-aligned next to the title in stage 1 so it
    /// shows on the short hover, not just the long-hover full popup.
    /// Empty = skipped.
    pub keybind: String,
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
            keybind: String::new(),
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
    pub fn with_keybind(mut self, keybind: impl Into<String>) -> Self {
        self.keybind = keybind.into();
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
            .add_systems(Update, tick_tooltip)
            .add_systems(PostUpdate, place_tooltips.after(UiSystems::Layout));
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
/// Two-stage: a glance gets the title, lingering
/// expands to the full description + signature.
///
/// Any pointer button held down tears down and blocks the tooltip, checked before
/// the hover lookup, so no popover sits under the cursor over a control the user
/// is operating.
///
/// An open menu blocks it for the same reason: a row's tooltip would cover the
/// rows travelled to next.
fn tick_tooltip(
    time: Res<Time>,
    targets: Query<(Entity, &Tooltip, &Hovered)>,
    mouse: Res<ButtonInput<MouseButton>>,
    menus: Option<Res<jackdaw_widgets::menu_bar::MenuBarState>>,
    default_font: Res<crate::icons::FeathersDefaultFont>,
    mut state: ResMut<TooltipState>,
    mut commands: Commands,
) {
    let menu_is_open = menus.is_some_and(|menus| menus.open_menu.is_some());
    if menu_is_open || mouse.get_pressed().next().is_some() {
        if let Some(active) = state.active.take() {
            commands.entity(active).try_despawn();
        }
        state.pending = None;
        state.stage = TooltipStage::None;
        return;
    }

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
            let popover_entity = commands
                .spawn((
                    TooltipPopover { target: entity },
                    Node {
                        position_type: PositionType::Absolute,
                        flex_direction: FlexDirection::Column,
                        max_width: Val::Px(TOOLTIP_MAX_WIDTH),
                        padding: UiRect::all(px(TOOLTIP_PADDING)),
                        row_gap: px(tokens::SPACING_XS),
                        border: UiRect::all(px(1.0)),
                        border_radius: BorderRadius::all(tokens::CORNER_RADIUS_LG),
                        ..Default::default()
                    },
                    BackgroundColor(tokens::BACKGROUND_COLOR.into()),
                    BorderColor::all(tokens::BORDER_COLOR),
                    GlobalZIndex(300),
                    Visibility::Hidden,
                    // Tooltips are decorative; they must not capture
                    // pointer events. Without this, the popover spawns
                    // at the cursor and blocks the next click on the
                    // hovered row (picker items, operator buttons),
                    // leaving keyboard activation as the only way to
                    // confirm. `Pickable::IGNORE` makes the popover
                    // hit-test transparent so pointer events fall
                    // through to whatever is underneath.
                    bevy::picking::Pickable::IGNORE,
                ))
                .id();
            spawn_title(&mut commands, popover_entity, tip, &default_font.0);
            state.active = Some(popover_entity);
            state.stage = TooltipStage::Title;
        }
        TooltipStage::Title if elapsed >= FULL_HOVER_DELAY => {
            if let Some(popover) = state.active {
                spawn_body(&mut commands, popover, tip, &default_font.0);
                if !tip.description.is_empty() {
                    commands
                        .entity(popover)
                        .entry::<Node>()
                        .and_modify(|mut node| node.width = px(TOOLTIP_MAX_WIDTH));
                }
                state.stage = TooltipStage::Full;
            }
        }
        _ => {}
    }
}

/// Put each tooltip beside the element it describes, inside the window: under
/// it when the element is in the top half of the window, over it otherwise,
/// so a tooltip on a control near the bottom does not cover the bars below.
/// Shown once its size is known.
fn place_tooltips(
    mut tips: Query<(&TooltipPopover, &ComputedNode, &mut Node, &mut Visibility)>,
    targets: Query<(&ComputedNode, &UiGlobalTransform), Without<TooltipPopover>>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    for (tip, computed, mut node, mut visibility) in &mut tips {
        let Ok((target, transform)) = targets.get(tip.target) else {
            continue;
        };
        let size = computed.size() * computed.inverse_scale_factor();
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        let scale = target.inverse_scale_factor();
        let center = transform.translation * scale;
        let half = target.size() * scale / 2.0;
        let (width, height) = (window.width(), window.height());
        let left = (center.x - half.x).clamp(
            TOOLTIP_WINDOW_MARGIN,
            (width - size.x - TOOLTIP_WINDOW_MARGIN).max(TOOLTIP_WINDOW_MARGIN),
        );
        let below = center.y + half.y + TOOLTIP_GAP;
        let above = center.y - half.y - TOOLTIP_GAP - size.y;
        let below_fits = below + size.y <= height - TOOLTIP_WINDOW_MARGIN;
        let above_fits = above >= TOOLTIP_WINDOW_MARGIN;
        let top = if (center.y < height / 2.0 && below_fits) || !above_fits {
            below
        } else {
            above
        };
        if node.left != Val::Px(left) {
            node.left = Val::Px(left);
        }
        if node.top != Val::Px(top) {
            node.top = Val::Px(top);
        }
        visibility.set_if_neq(Visibility::Inherited);
    }
}

/// Title-only popover content (stage 1). When the tooltip carries a
/// keybind, lay it out as a row: bold title on the left, muted
/// keybind on the right with a small gap. The popover's column
/// flow shrinks the row to its content width, so the keybind stays
/// adjacent to the title rather than stretching across the full
/// `TOOLTIP_MAX_WIDTH`.
///
/// Every spawned descendant carries `Pickable::IGNORE`. Bevy's UI
/// picking backend treats absent-`Pickable` nodes as blocking, so the
/// popover root being click-through is not enough; without this, a
/// text run hovering over a picker row would still capture the click
/// and the underlying row would never see it.
fn spawn_title(
    commands: &mut Commands,
    popover: Entity,
    tip: &Tooltip,
    default_font: &Handle<Font>,
) {
    if tip.title.is_empty() {
        return;
    }
    if tip.keybind.is_empty() {
        commands.spawn((
            Text::new(tip.title.clone()),
            TextFont {
                font: default_font.into(),
                font_size: tokens::TEXT_SIZE_SM,
                weight: FontWeight::MEDIUM,
                ..default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            bevy::picking::Pickable::IGNORE,
            ChildOf(popover),
        ));
        return;
    }
    commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_MD),
                ..default()
            },
            bevy::picking::Pickable::IGNORE,
            ChildOf(popover),
        ))
        .with_child((
            Text::new(tip.title.clone()),
            TextFont {
                font: default_font.into(),
                font_size: tokens::TEXT_SIZE_SM,
                weight: FontWeight::MEDIUM,
                ..default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            bevy::picking::Pickable::IGNORE,
        ))
        .with_child((
            Text::new(tip.keybind.clone()),
            TextFont {
                font: default_font.into(),
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            bevy::picking::Pickable::IGNORE,
        ));
}

/// Description + footer appended below the title (stage 2). Description
/// is the meaningful body the reader is here for, so it gets primary
/// weight; the footer (signature / type path) is dim metadata and gets
/// the darker grey.
fn spawn_body(
    commands: &mut Commands,
    popover: Entity,
    tip: &Tooltip,
    default_font: &Handle<Font>,
) {
    if !tip.description.is_empty() {
        commands.spawn((
            Text::new(tip.description.clone()),
            TextFont {
                font: default_font.into(),
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            bevy::picking::Pickable::IGNORE,
            ChildOf(popover),
        ));
    }
    if !tip.footer.is_empty() {
        commands
            .spawn((
                Node {
                    max_width: px(TOOLTIP_MAX_WIDTH - 2.0 * TOOLTIP_PADDING),
                    overflow: Overflow::clip(),
                    ..default()
                },
                bevy::picking::Pickable::IGNORE,
                ChildOf(popover),
            ))
            .with_child((
                Text::new(tip.footer.clone()),
                TextFont {
                    font: default_font.into(),
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                TextLayout {
                    linebreak: LineBreak::NoWrap,
                    ..default()
                },
                bevy::picking::Pickable::IGNORE,
            ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Bare `World` with what `tick_tooltip` reads: `Time`, the mouse
    /// button table, `TooltipState`, and a stand-in primary window (the
    /// system only reads its cursor position, so a bare `Window` with no
    /// backend is enough).
    fn test_world() -> World {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<ButtonInput<MouseButton>>();
        world.init_resource::<TooltipState>();
        world.insert_resource(crate::icons::FeathersDefaultFont(Handle::default()));
        world.spawn((Window::default(), PrimaryWindow));
        world
    }

    fn tick(world: &mut World) {
        world.run_system_once(tick_tooltip).unwrap();
    }

    fn advance(world: &mut World, dur: Duration) {
        world.resource_mut::<Time>().advance_by(dur);
    }

    fn popover_count(world: &mut World) -> usize {
        world
            .query_filtered::<Entity, With<TooltipPopover>>()
            .iter(world)
            .count()
    }

    /// Hovering a tagged entity past `SHORT_HOVER_DELAY` spawns the
    /// popover; this is the baseline the suppression tests build on.
    #[test]
    fn hover_past_delay_spawns_popover() {
        let mut world = test_world();
        world.spawn((
            Tooltip::title("Persistence").with_description("desc"),
            Hovered(true),
        ));

        tick(&mut world);
        assert_eq!(popover_count(&mut world), 0, "not spawned before the delay");

        advance(&mut world, SHORT_HOVER_DELAY);
        tick(&mut world);
        assert_eq!(
            popover_count(&mut world),
            1,
            "spawned once the delay elapses"
        );
    }

    /// A tooltip already showing disappears the instant a mouse button
    /// goes down, even while still hovering its anchor.
    #[test]
    fn mouse_down_dismisses_an_open_tooltip() {
        let mut world = test_world();
        world.spawn((
            Tooltip::title("Persistence").with_description("desc"),
            Hovered(true),
        ));
        advance(&mut world, SHORT_HOVER_DELAY);
        tick(&mut world);
        assert_eq!(popover_count(&mut world), 1);

        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        tick(&mut world);
        assert_eq!(popover_count(&mut world), 0, "mouse-down must close it");
    }

    /// A menu row's tooltip would cover the rows below it, so an open
    /// menu blocks tooltips outright and closing it re-arms them.
    #[test]
    fn an_open_menu_suppresses_the_tooltip_over_its_rows() {
        let mut world = test_world();
        world.init_resource::<jackdaw_widgets::menu_bar::MenuBarState>();
        let row = world
            .spawn((
                Tooltip::title("New Scene").with_description("desc"),
                Hovered(true),
            ))
            .id();
        world
            .resource_mut::<jackdaw_widgets::menu_bar::MenuBarState>()
            .open_menu = Some(row);

        advance(&mut world, FULL_HOVER_DELAY * 2);
        tick(&mut world);
        assert_eq!(
            popover_count(&mut world),
            0,
            "no popover lands over the rows of an open menu",
        );

        world
            .resource_mut::<jackdaw_widgets::menu_bar::MenuBarState>()
            .open_menu = None;
        advance(&mut world, SHORT_HOVER_DELAY);
        tick(&mut world);
        assert_eq!(
            popover_count(&mut world),
            1,
            "and the same hover is answered once the menu is down",
        );
    }

    /// While a button stays held (the span of a drag-scrub gesture), the
    /// tooltip does not reappear even though the hover delay has long
    /// since elapsed.
    #[test]
    fn held_button_suppresses_the_tooltip_through_a_long_hover() {
        let mut world = test_world();
        world.spawn((
            Tooltip::title("Persistence").with_description("desc"),
            Hovered(true),
        ));
        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);

        tick(&mut world);
        advance(&mut world, FULL_HOVER_DELAY * 2);
        tick(&mut world);
        assert_eq!(
            popover_count(&mut world),
            0,
            "held button must block the tooltip for as long as the drag lasts"
        );
    }

    /// Once the button is released, the tooltip re-arms and shows again
    /// after the normal hover delay: suppression is a hold, not a one-shot
    /// latch.
    #[test]
    fn releasing_the_button_re_arms_the_tooltip() {
        let mut world = test_world();
        world.spawn((
            Tooltip::title("Persistence").with_description("desc"),
            Hovered(true),
        ));
        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        tick(&mut world);

        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        advance(&mut world, SHORT_HOVER_DELAY);
        tick(&mut world);
        assert_eq!(popover_count(&mut world), 1, "re-arms after release");
    }
}
