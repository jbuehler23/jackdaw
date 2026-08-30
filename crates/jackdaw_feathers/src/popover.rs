use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::OverrideClip;
use bevy::ui_widgets::popover::{
    Popover, PopoverAlign, PopoverPlacement as PopoverPosition, PopoverSide,
};
use lucide_icons::Icon;

use crate::button::{ButtonClickEvent, ButtonVariant, IconButtonProps, icon_button};
use crate::tokens::{
    BACKGROUND_COLOR, BORDER_COLOR, CORNER_RADIUS_LG, TEXT_DISPLAY_COLOR, TEXT_SIZE,
};
use crate::utils::is_descendant_of;

const POPOVER_GAP: f32 = 4.0;

/// How close to the window edge the widget lets a popover go.
const WINDOW_MARGIN: f32 = 4.0;

pub fn plugin(app: &mut App) {
    app.add_observer(handle_popover_close_click).add_systems(
        Update,
        (
            anchor_popovers,
            reveal_positioned_popovers,
            handle_popover_dismiss,
            cleanup_tracked_popovers,
        ),
    );
}

#[derive(Component)]
pub struct EditorPopover;

#[derive(Component, Default)]
pub struct PopoverTracker {
    pub popover: Option<Entity>,
    pub trigger: Option<Entity>,
}

impl PopoverTracker {
    pub fn open(&mut self, popover: Entity, trigger: Entity) {
        self.popover = Some(popover);
        self.trigger = Some(trigger);
    }
}

pub fn activate_trigger(trigger: Entity, button_styles: &mut Query<&mut ButtonVariant>) {
    if let Ok(mut variant) = button_styles.get_mut(trigger) {
        *variant = ButtonVariant::ActiveAlt;
    }
}

pub fn deactivate_trigger(trigger: Entity, button_styles: &mut Query<&mut ButtonVariant>) {
    if let Ok(mut variant) = button_styles.get_mut(trigger) {
        *variant = ButtonVariant::Default;
    }
}

#[derive(Component)]
pub struct PopoverAnchor {
    pub entity: Entity,
    pub position: Option<Vec2>,
}

#[derive(Component, Default)]
struct PopoverLayoutReady(bool);

#[derive(Component, Default, Clone, Copy, PartialEq)]
pub enum PopoverPlacement {
    TopStart,
    Top,
    TopEnd,
    RightStart,
    Right,
    RightEnd,
    #[default]
    BottomStart,
    Bottom,
    BottomEnd,
    LeftStart,
    Left,
    LeftEnd,
}

impl PopoverPlacement {
    fn side(&self) -> PopoverSide {
        match self {
            Self::TopStart | Self::Top | Self::TopEnd => PopoverSide::Top,
            Self::RightStart | Self::Right | Self::RightEnd => PopoverSide::Right,
            Self::BottomStart | Self::Bottom | Self::BottomEnd => PopoverSide::Bottom,
            Self::LeftStart | Self::Left | Self::LeftEnd => PopoverSide::Left,
        }
    }

    fn align(&self) -> PopoverAlign {
        match self {
            Self::TopStart | Self::RightStart | Self::BottomStart | Self::LeftStart => {
                PopoverAlign::Start
            }
            Self::Top | Self::Right | Self::Bottom | Self::Left => PopoverAlign::Center,
            Self::TopEnd | Self::RightEnd | Self::BottomEnd | Self::LeftEnd => PopoverAlign::End,
        }
    }

    /// The placement asked for, then its mirror. The widget takes the
    /// first that fits in the window and the least occluded otherwise,
    /// which is the flip the editor used to do by hand.
    fn positions(&self) -> Vec<PopoverPosition> {
        let align = self.align();
        [self.side(), self.side().mirror()]
            .into_iter()
            .map(|side| PopoverPosition {
                side,
                align,
                gap: POPOVER_GAP,
            })
            .collect()
    }
}

pub struct PopoverProps {
    pub placement: PopoverPlacement,
    pub anchor: Entity,
    pub node: Option<Node>,
    pub padding: f32,
    pub gap: f32,
    pub z_index: i32,
    pub position: Option<Vec2>,
}

impl PopoverProps {
    pub fn new(anchor: Entity) -> Self {
        Self {
            placement: PopoverPlacement::default(),
            anchor,
            node: None,
            padding: 6.0,
            gap: 0.0,
            z_index: 100,
            position: None,
        }
    }

    pub fn with_position(mut self, position: impl Into<Option<Vec2>>) -> Self {
        self.position = position.into();
        self
    }

    pub fn with_placement(mut self, placement: PopoverPlacement) -> Self {
        self.placement = placement;
        self
    }

    pub fn with_node(mut self, node: Node) -> Self {
        self.node = Some(node);
        self
    }

    pub fn with_padding(mut self, padding: f32) -> Self {
        self.padding = padding;
        self
    }

    pub fn with_gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    pub fn with_z_index(mut self, z_index: i32) -> Self {
        self.z_index = z_index;
        self
    }
}

pub fn popover(props: PopoverProps) -> impl Bundle {
    let PopoverProps {
        placement,
        anchor,
        node,
        padding,
        gap,
        z_index,
        position,
    } = props;

    let base_node = node.unwrap_or_default();

    (
        EditorPopover,
        PopoverAnchor {
            entity: anchor,
            position,
        },
        PopoverLayoutReady::default(),
        Popover {
            positions: placement.positions(),
            window_margin: WINDOW_MARGIN,
        },
        // The popover is a child of what it points at, so it is inside
        // whatever clips and stacks that: the override and the z index
        // are what lift it back out.
        OverrideClip,
        placement,
        Hovered::default(),
        Interaction::None,
        Node {
            position_type: PositionType::Absolute,
            padding: UiRect::all(px(padding)),
            row_gap: px(gap),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(CORNER_RADIUS_LG),
            flex_direction: FlexDirection::Column,
            ..base_node
        },
        Visibility::Hidden,
        BackgroundColor(BACKGROUND_COLOR.into()),
        BorderColor::all(BORDER_COLOR),
        GlobalZIndex(z_index),
    )
}

/// Hang each popover off what it points at, which is where the widget
/// reads the rectangle to place it against. A popover asked for at a
/// free position gets a zero-size node there to point at instead.
fn anchor_popovers(
    mut commands: Commands,
    popovers: Query<(Entity, &PopoverAnchor), Added<PopoverAnchor>>,
) {
    for (entity, anchor) in &popovers {
        let parent = match anchor.position {
            Some(position) => commands
                .spawn((
                    PopoverPoint(entity),
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(position.x),
                        top: px(position.y),
                        ..default()
                    },
                ))
                .id(),
            None => anchor.entity,
        };
        commands.entity(entity).insert(ChildOf(parent));
    }
}

/// A zero-size node standing in for a free cursor position, despawned
/// with the popover it holds.
#[derive(Component)]
struct PopoverPoint(Entity);

/// A popover is spawned hidden: the widget places it after the frame's
/// layout, so showing it any sooner draws one frame at the origin.
fn reveal_positioned_popovers(
    mut commands: Commands,
    mut popovers: Query<
        (&ComputedNode, &mut Visibility, &mut PopoverLayoutReady),
        With<EditorPopover>,
    >,
    points: Query<(Entity, &PopoverPoint)>,
) {
    for (computed, mut visibility, mut ready) in &mut popovers {
        let size = computed.size();
        if size.x == 0.0 || size.y == 0.0 {
            continue;
        }
        if ready.0 {
            *visibility = Visibility::Visible;
        } else {
            ready.0 = true;
        }
    }

    for (entity, point) in &points {
        if commands.get_entity(point.0).is_err() {
            commands.entity(entity).try_despawn();
        }
    }
}

fn handle_popover_dismiss(
    mut commands: Commands,
    popovers: Query<(Entity, &PopoverAnchor, &Hovered), With<EditorPopover>>,
    parents: Query<&ChildOf>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    anchor_hovered: Query<&Hovered, Without<EditorPopover>>,
) {
    let esc_pressed = keyboard.just_pressed(KeyCode::Escape);
    let clicked = mouse.get_just_pressed().next().is_some();

    if !esc_pressed && !clicked {
        return;
    }

    let any_hovered = popovers.iter().any(|(_, _, hovered)| hovered.get());

    for (entity, anchor, hovered) in &popovers {
        // Don't dismiss on click if the anchor (trigger) is hovered,
        // let the anchor's click handler manage open/close toggling.
        if clicked && !esc_pressed {
            let anchor_is_hovered = anchor_hovered.get(anchor.entity).is_ok_and(Hovered::get);
            if anchor_is_hovered {
                continue;
            }
        }

        if esc_pressed || !any_hovered {
            commands.entity(entity).try_despawn();
            continue;
        }

        if hovered.get() {
            continue;
        }

        let has_hovered_nested_popover = popovers.iter().any(|(other_entity, _, other_hovered)| {
            other_entity != entity
                && other_hovered.get()
                && is_nested_in_popover(other_entity, entity, &popovers, &parents)
        });

        if !has_hovered_nested_popover {
            commands.entity(entity).try_despawn();
        }
    }
}

fn is_nested_in_popover(
    popover_entity: Entity,
    target: Entity,
    popovers: &Query<(Entity, &PopoverAnchor, &Hovered), With<EditorPopover>>,
    parents: &Query<&ChildOf>,
) -> bool {
    let Ok((_, anchor, _)) = popovers.get(popover_entity) else {
        return false;
    };
    if is_descendant_of(anchor.entity, target, parents) {
        return true;
    }
    for (intermediate, _, _) in popovers.iter() {
        if intermediate == target || intermediate == popover_entity {
            continue;
        }
        if is_descendant_of(anchor.entity, intermediate, parents)
            && is_nested_in_popover(intermediate, target, popovers, parents)
        {
            return true;
        }
    }
    false
}

#[derive(Component)]
pub struct PopoverCloseButton(Entity);

pub struct PopoverHeaderProps {
    pub title: String,
    pub popover: Entity,
}

impl PopoverHeaderProps {
    pub fn new(title: impl Into<String>, popover: Entity) -> Self {
        Self {
            title: title.into(),
            popover,
        }
    }
}

pub fn popover_header(
    props: PopoverHeaderProps,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) -> impl Bundle {
    let PopoverHeaderProps { title, popover } = props;

    (
        Node {
            width: percent(100),
            padding: UiRect::new(px(12.0), px(6.0), px(6.0), px(6.0)),
            border: UiRect::bottom(px(1.0)),
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            ..default()
        },
        BorderColor::all(BORDER_COLOR),
        children![
            (
                Text::new(title),
                TextFont {
                    font: editor_font.clone().into(),
                    font_size: TEXT_SIZE,
                    weight: FontWeight::SEMIBOLD,
                    ..default()
                },
                TextColor(TEXT_DISPLAY_COLOR.into()),
            ),
            (
                PopoverCloseButton(popover),
                icon_button(
                    IconButtonProps::new(Icon::X).variant(ButtonVariant::Ghost),
                    icon_font,
                ),
            ),
        ],
    )
}

pub fn popover_content() -> impl Bundle {
    Node {
        width: percent(100),
        flex_direction: FlexDirection::Column,
        row_gap: px(12.0),
        padding: UiRect::all(px(12.0)),
        ..default()
    }
}

fn cleanup_tracked_popovers(
    mut trackers: Query<&mut PopoverTracker>,
    popovers: Query<Entity, With<EditorPopover>>,
    mut button_styles: Query<&mut ButtonVariant>,
) {
    for mut tracker in &mut trackers {
        let Some(popover_entity) = tracker.popover else {
            continue;
        };

        if popovers.get(popover_entity).is_ok() {
            continue;
        }

        tracker.popover = None;

        if let Some(trigger_entity) = tracker.trigger {
            deactivate_trigger(trigger_entity, &mut button_styles);
        }
    }
}

fn handle_popover_close_click(
    trigger: On<ButtonClickEvent>,
    mut commands: Commands,
    close_buttons: Query<&PopoverCloseButton>,
) {
    let Ok(close_button) = close_buttons.get(trigger.entity) else {
        return;
    };
    commands.entity(close_button.0).try_despawn();
}
