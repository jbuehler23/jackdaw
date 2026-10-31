//! The viewport toolbar's overflow menu: in a viewport too narrow for every
//! tool button, the buttons the tool group cuts off are listed in a menu at
//! the group's end, so each tool stays reachable.

use std::sync::Arc;

use bevy::prelude::*;
use bevy::ui::UiGlobalTransform;
use jackdaw_api_internal::lifecycle::OperatorEntity;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::menu_bar::{OP_ACTION_PREFIX, menu_glyph_button};
use jackdaw_feathers::tokens::TOOLBAR_GAP;

/// The name the overflow menu's item carries, and its tooltip.
pub const TOOLBAR_OVERFLOW_MENU: &str = "More tools";

/// On the toolbar's group of tool buttons, the part that is cut off when the
/// viewport is narrow.
#[derive(Component, Default, Clone)]
pub struct ToolbarTools;

/// On the overflow menu's button: the tool group it lists, the button's last
/// laid-out width, and the group's children cut at the last fit, which the
/// menu lists.
#[derive(Component)]
pub struct ToolbarOverflow {
    pub tools: Entity,
    pub width: f32,
    pub cut: Vec<Entity>,
}

/// On the toolbar's flexible gap. The tool groups before it are fitted to the
/// room up to its right edge.
#[derive(Component, Default, Clone)]
pub struct ToolbarSpacer;

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        show_overflow_while_tools_are_cut.after(bevy::ui::UiSystems::Layout),
    );
}

/// Put an overflow menu right after each [`ToolbarTools`] group in `toolbar`,
/// hidden until one of its tools is cut.
pub fn spawn_toolbar_overflow(world: &mut World, toolbar: Entity) -> Vec<Entity> {
    let groups: Vec<Entity> = world
        .get::<Children>(toolbar)
        .map(|children| {
            children
                .iter()
                .filter(|child| world.get::<ToolbarTools>(*child).is_some())
                .collect()
        })
        .unwrap_or_default();
    groups
        .into_iter()
        .filter_map(|tools| spawn_group_overflow(world, toolbar, tools))
        .collect()
}

fn spawn_group_overflow(world: &mut World, toolbar: Entity, tools: Entity) -> Option<Entity> {
    let index = world
        .get::<Children>(toolbar)?
        .iter()
        .position(|child| child == tools)?;
    let overflow = world
        .spawn((
            crate::EditorEntity,
            ToolbarOverflow {
                tools,
                width: OVERFLOW_WIDTH_GUESS,
                cut: Vec::new(),
            },
            menu_glyph_button(
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

/// The operator calls of the buttons in `tools` that do not fit inside it.
pub fn cut_tools(world: &World, tools: Entity) -> Vec<ButtonOperatorCall> {
    let mut overflows = world.try_query::<&ToolbarOverflow>();
    let cut = overflows
        .as_mut()
        .and_then(|query| query.iter(world).find(|overflow| overflow.tools == tools))
        .map(|overflow| overflow.cut.clone())
        .unwrap_or_default();
    cut.into_iter()
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

/// Room the overflow button is given before it has been laid out once.
const OVERFLOW_WIDTH_GUESS: f32 = 44.0;

/// Where the tools of the group at `tools` may end: the right edge of the
/// toolbar's [`ToolbarSpacer`], which does not move as the group narrows.
fn room_end(world: &World, tools: Entity) -> Option<f32> {
    let toolbar = world.get::<ChildOf>(tools)?.parent();
    let spacer = world
        .get::<Children>(toolbar)?
        .iter()
        .find(|child| world.get::<ToolbarSpacer>(*child).is_some())?;
    span_of(world, spacer).map(|(_, right)| right - TOOLBAR_GAP)
}

/// The width the group at `tools` is held to so its last shown tool sits right
/// before the overflow button, or `None` when every tool fits; and the children
/// it cuts. Read from the children's own spans, which a narrowed group clips
/// but does not move, so the answer holds while the group stays narrowed.
fn fit_group(world: &World, tools: Entity, overflow_width: f32) -> (Option<f32>, Vec<Entity>) {
    let (Some((left, _)), Some(end)) = (span_of(world, tools), room_end(world, tools)) else {
        return (None, Vec::new());
    };
    let children: Vec<(Entity, f32, bool)> = world
        .get::<Children>(tools)
        .map(|children| {
            children
                .iter()
                .filter_map(|child| {
                    let (_, right) = span_of(world, child)?;
                    let button = world.get::<ButtonOperatorCall>(child).is_some();
                    Some((child, right, button))
                })
                .collect()
        })
        .unwrap_or_default();
    if children.iter().all(|(_, right, _)| *right <= end + 0.5) {
        return (None, Vec::new());
    }
    let limit = end - overflow_width - TOOLBAR_GAP;
    let shown_end = children
        .iter()
        .take_while(|(_, right, _)| *right <= limit + 0.5)
        .filter(|(_, _, button)| *button)
        .map(|(_, right, _)| *right)
        .last()
        .unwrap_or(left);
    let cut = children
        .iter()
        .filter(|(_, right, _)| *right > shown_end + 0.5)
        .map(|(child, ..)| *child)
        .collect();
    (Some(shown_end - left), cut)
}

/// Fit each tool group to the room it has: the tools that do not fit are
/// hidden rather than drawn half cut, the group ends at its last shown tool,
/// and the overflow menu right after it lists the rest.
fn show_overflow_while_tools_are_cut(world: &mut World) {
    let mut buttons = world.query::<(Entity, &ToolbarOverflow)>();
    let groups: Vec<(Entity, Entity, f32)> = buttons
        .iter(world)
        .map(|(button, overflow)| (button, overflow.tools, overflow.width))
        .collect();
    for (button, tools, stored_width) in groups {
        let overflow_width = span_of(world, button)
            .map(|(left, right)| right - left)
            .filter(|width| *width > 0.0)
            .unwrap_or(stored_width);
        let shown = world
            .get::<Node>(tools)
            .is_some_and(|node| node.display != Display::None);
        let (width, cut) = if shown {
            fit_group(world, tools, overflow_width)
        } else {
            (None, Vec::new())
        };
        if let Some(mut overflow) = world.get_mut::<ToolbarOverflow>(button)
            && (overflow.width != overflow_width || overflow.cut != cut)
        {
            overflow.width = overflow_width;
            overflow.cut.clone_from(&cut);
        }
        let max_width = width.map_or(Val::Auto, Val::Px);
        if let Some(mut node) = world.get_mut::<Node>(tools)
            && node.max_width != max_width
        {
            node.max_width = max_width;
        }
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
