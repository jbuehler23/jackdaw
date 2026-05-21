use bevy_app::prelude::*;
use bevy_camera::prelude::*;
use bevy_color::palettes::tailwind;
use bevy_ecs::prelude::*;
use bevy_input::mouse::{MouseScrollUnit, MouseWheel};
use bevy_math::prelude::*;
use bevy_picking::hover::{HoverMap, Hovered};
use bevy_ui::prelude::*;
use bevy_utils::prelude::*;

const SCROLL_SPEED: f32 = 24.0;

const SCROLLBAR_MIN_HEIGHT: f32 = 24.0;
const SCROLLBAR_WIDTH: f32 = 3.0;
const SCROLLBAR_MARGIN: f32 = 3.0;

pub fn plugin(app: &mut App) {
    app.add_systems(Update, (send_scroll_events, update_scrollbar))
        .add_observer(on_scroll_handler);
}

#[derive(EntityEvent, Debug)]
#[entity_event(propagate, auto_propagate)]
pub struct Scroll {
    pub entity: Entity,
    pub delta: Vec2,
}

#[derive(Component)]
pub struct Scrollbar {
    pub container: Entity,
}

pub fn scrollbar(container: Entity) -> impl Bundle {
    (
        Scrollbar { container },
        Node {
            position_type: PositionType::Absolute,
            width: px(SCROLLBAR_WIDTH),
            right: px(SCROLLBAR_MARGIN),
            top: px(SCROLLBAR_MARGIN),
            border_radius: BorderRadius::all(px(SCROLLBAR_WIDTH / 2.0)),
            ..default()
        },
        BackgroundColor(tailwind::ZINC_600.into()),
        Visibility::Hidden,
    )
}

fn send_scroll_events(
    mut mouse_wheel_reader: MessageReader<MouseWheel>,
    hover_map: Res<HoverMap>,
    mut commands: Commands,
) {
    for mouse_wheel in mouse_wheel_reader.read() {
        let mut delta = -Vec2::new(mouse_wheel.x, mouse_wheel.y);

        if mouse_wheel.unit == MouseScrollUnit::Line {
            delta *= SCROLL_SPEED;
        }

        for pointer_map in hover_map.values() {
            for entity in pointer_map.keys().copied() {
                commands.trigger(Scroll { entity, delta });
            }
        }
    }
}

fn on_scroll_handler(
    mut scroll: On<Scroll>,
    mut query: Query<(&mut ScrollPosition, &Node, &ComputedNode)>,
) {
    let Ok((mut scroll_position, node, computed)) = query.get_mut(scroll.entity) else {
        return;
    };

    let max_offset = (computed.content_size() - computed.size()) * computed.inverse_scale_factor();
    let max_offset = max_offset.max(Vec2::ZERO);

    let delta = &mut scroll.delta;
    if node.overflow.x == OverflowAxis::Scroll && delta.x != 0. {
        let old_x = scroll_position.x;
        scroll_position.x = (scroll_position.x + delta.x).clamp(0., max_offset.x);
        if scroll_position.x != old_x {
            delta.x = 0.;
        }
    }

    if node.overflow.y == OverflowAxis::Scroll && delta.y != 0. {
        let old_y = scroll_position.y;
        scroll_position.y = (scroll_position.y + delta.y).clamp(0., max_offset.y);
        if scroll_position.y != old_y {
            delta.y = 0.;
        }
    }

    if *delta == Vec2::ZERO {
        scroll.propagate(false);
    }
}

fn update_scrollbar(
    containers: Query<(&Hovered, &ScrollPosition, &ComputedNode)>,
    mut scrollbars: Query<(&Scrollbar, &mut Node, &mut Visibility)>,
) {
    for (scrollbar, mut node, mut visibility) in &mut scrollbars {
        let Ok((hovered, scroll_position, computed)) = containers.get(scrollbar.container) else {
            continue;
        };

        let content_height = computed.content_size().y * computed.inverse_scale_factor();
        let visible_height = computed.size().y * computed.inverse_scale_factor();
        let has_scroll = content_height > visible_height;

        let should_show = hovered.get() && has_scroll;
        let new_visibility = if should_show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };

        if *visibility != new_visibility {
            *visibility = new_visibility;
        }

        if !has_scroll {
            continue;
        }

        let track_height = visible_height - (SCROLLBAR_MARGIN * 2.0);
        let thumb_ratio = visible_height / content_height;
        let thumb_height = (track_height * thumb_ratio).max(SCROLLBAR_MIN_HEIGHT);

        let max_scroll = content_height - visible_height;
        let scroll_ratio = if max_scroll > 0.0 {
            scroll_position.y / max_scroll
        } else {
            0.0
        };
        let thumb_offset = scroll_ratio * (track_height - thumb_height);

        node.top = px(SCROLLBAR_MARGIN + thumb_offset);
        node.height = px(thumb_height);
    }
}
