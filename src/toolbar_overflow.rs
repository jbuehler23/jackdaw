//! The viewport toolbar's overflow menu: in a viewport too narrow for every
//! tool button, the buttons the tool group cuts off are listed in a menu at
//! the group's end, so each tool stays reachable.

use std::sync::Arc;

use bevy::prelude::*;
use bevy::ui::UiGlobalTransform;
use jackdaw_api_internal::lifecycle::OperatorEntity;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::menu_bar::{OP_ACTION_PREFIX, menu_icon_button};

/// The name the overflow menu's item carries, and its tooltip.
pub const TOOLBAR_OVERFLOW_MENU: &str = "More tools";

/// On the toolbar's group of tool buttons, the part that is cut off when the
/// viewport is narrow.
#[derive(Component, Default, Clone)]
pub struct ToolbarTools;

/// On the overflow menu's button; holds the tool group it lists.
#[derive(Component)]
pub struct ToolbarOverflow(pub Entity);

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        show_overflow_while_tools_are_cut.after(bevy::ui::UiSystems::Layout),
    );
}

/// Put an overflow menu right after the [`ToolbarTools`] group in `toolbar`,
/// hidden until a tool is cut.
pub fn spawn_toolbar_overflow(world: &mut World, toolbar: Entity) -> Option<Entity> {
    let children = world.get::<Children>(toolbar)?;
    let (index, tools) = children
        .iter()
        .enumerate()
        .find(|(_, child)| world.get::<ToolbarTools>(*child).is_some())?;
    let overflow = world
        .spawn((
            crate::EditorEntity,
            ToolbarOverflow(tools),
            menu_icon_button(
                TOOLBAR_OVERFLOW_MENU,
                Icon::Ellipsis,
                Arc::new(move |world: &World| cut_tool_rows(world, tools)),
            ),
            jackdaw_feathers::tooltip::Tooltip::title(TOOLBAR_OVERFLOW_MENU),
        ))
        .id();
    if let Some(mut node) = world.get_mut::<Node>(overflow) {
        node.flex_shrink = 0.0;
        node.display = Display::None;
    }
    world
        .entity_mut(toolbar)
        .insert_children(index + 1, &[overflow]);
    Some(overflow)
}

/// The horizontal extent of a laid-out node, in logical pixels.
fn span_of(world: &World, entity: Entity) -> Option<(f32, f32)> {
    let node = world.get::<ComputedNode>(entity)?;
    let at = world.get::<UiGlobalTransform>(entity)?;
    let scale = node.inverse_scale_factor();
    let centre = at.translation.x * scale;
    let half = node.size().x * scale / 2.0;
    Some((centre - half, centre + half))
}

/// The children of `tools`, buttons and separators alike, that do not fit
/// inside it.
fn cut_children(world: &World, tools: Entity) -> Vec<Entity> {
    let Some((start, end)) = span_of(world, tools) else {
        return Vec::new();
    };
    if end <= start {
        return Vec::new();
    }
    let Some(children) = world.get::<Children>(tools) else {
        return Vec::new();
    };
    children
        .iter()
        .filter(|child| span_of(world, *child).is_some_and(|(_, right)| right > end + 0.5))
        .collect()
}

/// The operator calls of the buttons in `tools` that do not fit inside it.
pub fn cut_tools(world: &World, tools: Entity) -> Vec<ButtonOperatorCall> {
    cut_children(world, tools)
        .into_iter()
        .filter_map(|child| world.get::<ButtonOperatorCall>(child).cloned())
        .collect()
}

fn cut_tool_rows(world: &World, tools: Entity) -> Vec<(String, String)> {
    let cut = cut_tools(world, tools);
    if cut.is_empty() {
        return Vec::new();
    }
    let labels: Vec<(&'static str, &'static str)> = world
        .try_query::<&OperatorEntity>()
        .map(|mut operators| {
            operators
                .iter(world)
                .map(|op| (op.id(), op.label()))
                .collect()
        })
        .unwrap_or_default();
    cut.iter()
        .map(|call| {
            let label = labels
                .iter()
                .find(|(id, _)| *id == call.id)
                .map_or(call.id.as_ref(), |(_, label)| *label);
            (format!("{OP_ACTION_PREFIX}{}", call.id), label.to_string())
        })
        .collect()
}

/// Hide the tools that do not fit, rather than draw them half cut, and show
/// the overflow menu while any are hidden.
fn show_overflow_while_tools_are_cut(world: &mut World) {
    let mut buttons = world.query::<(Entity, &ToolbarOverflow)>();
    let groups: Vec<(Entity, Entity)> = buttons
        .iter(world)
        .map(|(button, overflow)| (button, overflow.0))
        .collect();
    for (button, tools) in groups {
        let cut = cut_children(world, tools);
        let children: Vec<Entity> = world
            .get::<Children>(tools)
            .map(|children| children.iter().collect())
            .unwrap_or_default();
        for child in children {
            let visibility = if cut.contains(&child) {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
            if let Some(mut current) = world.get_mut::<Visibility>(child)
                && *current != visibility
            {
                *current = visibility;
            }
        }
        let display = if cut.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
        if let Some(mut node) = world.get_mut::<Node>(button)
            && node.display != display
        {
            node.display = display;
        }
    }
}
