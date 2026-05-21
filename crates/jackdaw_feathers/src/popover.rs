use bevy_app::prelude::*;
use bevy_asset::prelude::*;
use bevy_camera::prelude::*;
use bevy_ecs::prelude::*;
use bevy_input::prelude::*;
use bevy_math::prelude::*;
use bevy_picking::hover::Hovered;
use bevy_text::prelude::*;
use bevy_ui::UiGlobalTransform;
use bevy_ui::prelude::*;
use bevy_utils::prelude::*;
use bevy_window::{PrimaryWindow, prelude::*};
use lucide_icons::Icon;

use crate::button::{
    ButtonClickEvent, ButtonVariant, IconButtonProps, icon_button, set_button_variant,
};
use crate::tokens::{
    BACKGROUND_COLOR, BORDER_COLOR, CORNER_RADIUS_LG, TEXT_DISPLAY_COLOR, TEXT_SIZE,
};
use crate::utils::is_descendant_of;

const POPOVER_GAP: f32 = 4.0;

pub fn plugin(app: &mut App) {
    app.add_observer(handle_popover_close_click).add_systems(
        Update,
        (
            handle_popover_position,
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

pub fn activate_trigger(
    trigger: Entity,
    button_styles: &mut Query<(&mut BackgroundColor, &mut BorderColor, &mut ButtonVariant)>,
) {
    if let Ok((mut bg, mut border, mut variant)) = button_styles.get_mut(trigger) {
        *variant = ButtonVariant::ActiveAlt;
        set_button_variant(ButtonVariant::ActiveAlt, &mut bg, &mut border);
    }
}

pub fn deactivate_trigger(
    trigger: Entity,
    button_styles: &mut Query<(&mut BackgroundColor, &mut BorderColor, &mut ButtonVariant)>,
) {
    if let Ok((mut bg, mut border, mut variant)) = button_styles.get_mut(trigger) {
        *variant = ButtonVariant::Default;
        set_button_variant(ButtonVariant::Default, &mut bg, &mut border);
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
    fn offset(&self, anchor_size: Vec2, popover_size: Vec2) -> Vec2 {
        match self {
            Self::TopStart => Vec2::new(0.0, -popover_size.y - POPOVER_GAP),
            Self::Top => Vec2::new(
                (anchor_size.x - popover_size.x) / 2.0,
                -popover_size.y - POPOVER_GAP,
            ),
            Self::TopEnd => Vec2::new(
                anchor_size.x - popover_size.x,
                -popover_size.y - POPOVER_GAP,
            ),
            Self::RightStart => Vec2::new(anchor_size.x + POPOVER_GAP, 0.0),
            Self::Right => Vec2::new(
                anchor_size.x + POPOVER_GAP,
                (anchor_size.y - popover_size.y) / 2.0,
            ),
            Self::RightEnd => {
                Vec2::new(anchor_size.x + POPOVER_GAP, anchor_size.y - popover_size.y)
            }
            Self::BottomStart => Vec2::new(0.0, anchor_size.y + POPOVER_GAP),
            Self::Bottom => Vec2::new(
                (anchor_size.x - popover_size.x) / 2.0,
                anchor_size.y + POPOVER_GAP,
            ),
            Self::BottomEnd => {
                Vec2::new(anchor_size.x - popover_size.x, anchor_size.y + POPOVER_GAP)
            }
            Self::LeftStart => Vec2::new(-popover_size.x - POPOVER_GAP, 0.0),
            Self::Left => Vec2::new(
                -popover_size.x - POPOVER_GAP,
                (anchor_size.y - popover_size.y) / 2.0,
            ),
            Self::LeftEnd => Vec2::new(
                -popover_size.x - POPOVER_GAP,
                anchor_size.y - popover_size.y,
            ),
        }
    }

    fn flip(&self) -> Self {
        match self {
            Self::TopStart => Self::BottomStart,
            Self::Top => Self::Bottom,
            Self::TopEnd => Self::BottomEnd,
            Self::RightStart => Self::LeftStart,
            Self::Right => Self::Left,
            Self::RightEnd => Self::LeftEnd,
            Self::BottomStart => Self::TopStart,
            Self::Bottom => Self::Top,
            Self::BottomEnd => Self::TopEnd,
            Self::LeftStart => Self::RightStart,
            Self::Left => Self::Right,
            Self::LeftEnd => Self::RightEnd,
        }
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

fn handle_popover_position(
    mut popovers: Query<
        (
            &PopoverAnchor,
            &PopoverPlacement,
            &ComputedNode,
            &mut Node,
            &mut Visibility,
            &mut PopoverLayoutReady,
        ),
        With<EditorPopover>,
    >,
    anchors: Query<(&ComputedNode, &UiGlobalTransform)>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    let window = window.into_inner();
    let window_size = Vec2::new(window.width(), window.height());

    for (
        anchor_ref,
        placement,
        popover_computed,
        mut popover_node,
        mut visibility,
        mut layout_ready,
    ) in &mut popovers
    {
        let Ok((anchor_computed, anchor_transform)) = anchors.get(anchor_ref.entity) else {
            continue;
        };

        let popover_size = popover_computed.size() * popover_computed.inverse_scale_factor();

        if popover_size.x == 0.0 || popover_size.y == 0.0 {
            continue;
        }

        let (anchor_top_left, anchor_size) = if let Some(pos) = anchor_ref.position {
            (pos, Vec2::ZERO)
        } else {
            let scale = anchor_computed.inverse_scale_factor();
            let anchor_center = anchor_transform.translation * scale;
            let anchor_size = anchor_computed.size() * scale;
            let top_left = Vec2::new(
                anchor_center.x - anchor_size.x * 0.5,
                anchor_center.y - anchor_size.y * 0.5,
            );
            (top_left, anchor_size)
        };

        let mut pos = anchor_top_left + placement.offset(anchor_size, popover_size);

        if pos.x < 0.0
            || pos.x + popover_size.x > window_size.x
            || pos.y < 0.0
            || pos.y + popover_size.y > window_size.y
        {
            let flipped = placement.flip();
            let flipped_pos = anchor_top_left + flipped.offset(anchor_size, popover_size);

            if flipped_pos.x >= 0.0
                && flipped_pos.x + popover_size.x <= window_size.x
                && flipped_pos.y >= 0.0
                && flipped_pos.y + popover_size.y <= window_size.y
            {
                pos = flipped_pos;
            }
        }

        pos.x = pos.x.clamp(0.0, (window_size.x - popover_size.x).max(0.0));
        pos.y = pos.y.clamp(0.0, (window_size.y - popover_size.y).max(0.0));

        popover_node.left = px(pos.x);
        popover_node.top = px(pos.y);

        if layout_ready.0 {
            *visibility = Visibility::Visible;
        } else {
            layout_ready.0 = true;
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
                    font: editor_font.clone(),
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
    mut button_styles: Query<(&mut BackgroundColor, &mut BorderColor, &mut ButtonVariant)>,
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
