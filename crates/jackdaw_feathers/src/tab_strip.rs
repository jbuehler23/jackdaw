//! A row of labeled tabs sharing one active highlight and one dispatch
//! operator.
//!
//! `jackdaw_panels`' `DockTabBar` renders a similar shape without using this
//! widget: its drag-to-reorder and close-button affordances depend on
//! `jackdaw_panels`-only types this crate cannot depend on.

use std::borrow::Cow;

use bevy::prelude::*;
use jackdaw_scene_types::PropertyValue;

use crate::button::{ButtonOperatorCall, ButtonProps, ButtonVariant, button};
use crate::tokens;

/// Marker for a [`spawn_tab_strip`] row.
#[derive(Component)]
pub struct TabStrip;

/// Marker for one tab spawned by [`spawn_tab_strip`].
#[derive(Component)]
pub struct TabStripTab;

/// Layout direction for a [`spawn_tab_strip`] row.
///
/// A parameter rather than two functions: consumers share one
/// dispatch/highlight mechanism and differ only in the axis tabs stack along.
/// The Terrain panel's and the inspector/dock header's strips run
/// horizontally; the bottom window dock's runs vertically along its edge.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum TabStripOrientation {
    #[default]
    Horizontal,
    Vertical,
}

impl TabStripOrientation {
    fn flex_direction(self) -> FlexDirection {
        match self {
            Self::Horizontal => FlexDirection::Row,
            Self::Vertical => FlexDirection::Column,
        }
    }
}

/// One entry in a [`spawn_tab_strip`] row: its label, whether it is the
/// active tab, and the value the strip's dispatch operator receives for
/// this tab when it is clicked.
pub struct TabStripItem {
    pub label: String,
    pub active: bool,
    pub param: PropertyValue,
}

impl TabStripItem {
    pub fn new(label: impl Into<String>, active: bool, param: impl Into<PropertyValue>) -> Self {
        Self {
            label: label.into(),
            active,
            param: param.into(),
        }
    }
}

/// Spawn a row of tabs under `parent`. Clicking a tab dispatches `op_id` with
/// `param_key` set to that tab's [`TabStripItem::param`], carried by the same
/// [`ButtonOperatorCall`] every other clickable widget in this crate uses, so
/// `dispatch_button_operator_call` fires it with no tab-specific glue and the
/// widget stays independent of the operator API.
///
/// The active tab uses [`ButtonVariant::Active`]; an inactive tab uses
/// [`ButtonVariant::Ghost`].
///
/// `orientation` picks the axis tabs stack along, see
/// [`TabStripOrientation`]. Both the row's layout and the gap between tabs
/// follow it, so a vertical strip carries no leftover horizontal gap.
///
/// Returns the row entity.
pub fn spawn_tab_strip(
    commands: &mut Commands,
    parent: Entity,
    op_id: impl Into<Cow<'static, str>>,
    param_key: impl Into<Cow<'static, str>>,
    orientation: TabStripOrientation,
    items: impl IntoIterator<Item = TabStripItem>,
) -> Entity {
    let op_id = op_id.into();
    let param_key = param_key.into();
    let gap = px(tokens::SPACING_XS);

    let row = commands
        .spawn((
            TabStrip,
            Node {
                flex_direction: orientation.flex_direction(),
                column_gap: gap,
                row_gap: gap,
                ..default()
            },
            ChildOf(parent),
        ))
        .id();

    for item in items {
        let TabStripItem {
            label,
            active,
            param,
        } = item;
        let variant = if active {
            ButtonVariant::Active
        } else {
            ButtonVariant::Ghost
        };
        commands.spawn((
            TabStripTab,
            button(ButtonProps::new(label).with_variant(variant)),
            ButtonOperatorCall::new(op_id.clone()).with_param(param_key.clone(), param),
            ChildOf(row),
        ));
    }

    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_and_flush(
        orientation: TabStripOrientation,
        items: Vec<TabStripItem>,
    ) -> (World, Entity, Entity) {
        let mut world = World::new();
        let parent = world.spawn(Node::default()).id();
        let mut commands_queue = bevy::ecs::world::CommandQueue::default();
        let row = {
            let mut commands = Commands::new(&mut commands_queue, &world);
            spawn_tab_strip(
                &mut commands,
                parent,
                "terrain.panel.tab",
                "tab",
                orientation,
                items,
            )
        };
        commands_queue.apply(&mut world);
        (world, parent, row)
    }

    #[test]
    fn spawns_one_tab_button_per_item_with_a_button_operator_call() {
        let (mut world, _parent, row) = spawn_and_flush(
            TabStripOrientation::Horizontal,
            vec![
                TabStripItem::new("Textures", false, "textures"),
                TabStripItem::new("Scatter", true, "scatter"),
            ],
        );

        let children = world.get::<Children>(row).expect("row has children");
        assert_eq!(children.len(), 2);

        let mut query = world.query::<(&TabStripTab, &ButtonOperatorCall)>();
        let calls: Vec<_> = query.iter(&world).collect();
        assert_eq!(calls.len(), 2);
        for (_, call) in &calls {
            assert_eq!(call.id, "terrain.panel.tab");
            assert_eq!(call.params.len(), 1);
            assert_eq!(call.params[0].0, "tab");
        }
    }

    /// Each tab's dispatched param carries that tab's own value, not a
    /// shared or first-wins one.
    #[test]
    fn each_tab_carries_its_own_param_value() {
        let (mut world, _parent, _row) = spawn_and_flush(
            TabStripOrientation::Horizontal,
            vec![
                TabStripItem::new("Textures", false, "textures"),
                TabStripItem::new("Scatter", true, "scatter"),
            ],
        );

        let mut query = world.query::<&ButtonOperatorCall>();
        let params: Vec<PropertyValue> =
            query.iter(&world).map(|c| c.params[0].1.clone()).collect();
        assert!(params.contains(&PropertyValue::String("textures".into())));
        assert!(params.contains(&PropertyValue::String("scatter".into())));
    }

    /// A vertical strip lays its tabs out top-to-bottom rather than
    /// defaulting to a horizontal row.
    #[test]
    fn vertical_orientation_stacks_the_row_in_a_column() {
        let (world, _parent, row) = spawn_and_flush(
            TabStripOrientation::Vertical,
            vec![TabStripItem::new("One", true, "one")],
        );

        let node = world.get::<Node>(row).expect("row has a Node");
        assert_eq!(node.flex_direction, FlexDirection::Column);
    }
}
