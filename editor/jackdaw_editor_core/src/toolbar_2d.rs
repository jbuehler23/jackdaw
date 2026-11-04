//! The 2D viewport toolbar's tool groups: the object tools, and the groups
//! features add through [`Toolbar2dSlots`], each shown in place of the object
//! tools while its feature's mode is on.

use bevy::feathers::controls::ButtonVariant;
use bevy::prelude::*;
use jackdaw_api::prelude::Operator;
use jackdaw_api_internal::operator::RefreshOperatorButtons;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::icons::Icon;

use crate::layout::{toolbar_op_button, toolbar_separator};
use crate::tool_ops::{ToolRotateOp, ToolScaleOp, ToolSelectOp, ToolTranslateOp};
use crate::toolbar_overflow::ToolbarTools;

/// The name of the 2D toolbar's display menu.
pub const CANVAS_VIEW_MENU: &str = "Canvas view";

/// Which tool group of the 2D toolbar a node is: the object tools, or the
/// group of the [`Toolbar2dSlot`] with that id.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toolbar2dGroup {
    Object,
    Mode(&'static str),
}

/// A group's buttons in order; `None` is a separator.
pub type Toolbar2dItems = &'static [Option<(&'static str, Icon)>];

/// A tool group a feature adds to the 2D toolbar.
#[derive(Clone, Copy)]
pub struct Toolbar2dSlot {
    pub id: &'static str,
    /// A button at the end of the object tools, such as the one that turns
    /// the mode on.
    pub entry: Option<(&'static str, Icon)>,
    pub items: Toolbar2dItems,
    /// Whether the mode is on, which shows this group instead of the object
    /// tools.
    pub active: fn(&World) -> bool,
    /// Whether the button calling an operator id shows as on, or `None` for
    /// a button this slot does not light.
    pub lit: fn(&World, &str) -> Option<bool>,
}

/// The tool groups features add to the 2D toolbar, in registration order.
#[derive(Resource, Default, Clone)]
pub struct Toolbar2dSlots(Vec<Toolbar2dSlot>);

impl Toolbar2dSlots {
    pub fn register(&mut self, slot: Toolbar2dSlot) {
        self.0.push(slot);
    }
}

const OBJECT_TOOLS: Toolbar2dItems = &[
    Some((ToolSelectOp::ID, Icon::MousePointer)),
    Some((ToolTranslateOp::ID, Icon::Move)),
    Some((ToolRotateOp::ID, Icon::RotateCw)),
    Some((ToolScaleOp::ID, Icon::Scale3d)),
];

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<Toolbar2dSlots>()
        .add_observer(sync_2d_toolbar);
}

/// Put the object and feature tool groups at the start of `toolbar`, each with
/// the overflow menu the 3D toolbar uses.
pub(crate) fn spawn_tool_groups(world: &mut World, toolbar: Entity) -> Result {
    let slots = world.resource::<Toolbar2dSlots>().clone();
    let mut object: Vec<Option<(&'static str, Icon)>> = OBJECT_TOOLS.to_vec();
    let entries: Vec<_> = slots.0.iter().filter_map(|slot| slot.entry).collect();
    if !entries.is_empty() {
        object.push(None);
        object.extend(entries.into_iter().map(Some));
    }
    let mut groups = Vec::new();
    let all = std::iter::once((Toolbar2dGroup::Object, object.as_slice())).chain(
        slots
            .0
            .iter()
            .map(|slot| (Toolbar2dGroup::Mode(slot.id), slot.items)),
    );
    for (which, items) in all {
        let group = world
            .spawn((
                crate::EditorEntity,
                which,
                ToolbarTools,
                Node {
                    display: if which == Toolbar2dGroup::Object {
                        Display::Flex
                    } else {
                        Display::None
                    },
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(jackdaw_feathers::tokens::TOOLBAR_GAP),
                    flex_shrink: 1.0,
                    min_width: px(0.0),
                    overflow: Overflow::clip_x(),
                    ..default()
                },
            ))
            .id();
        for item in items {
            let mut child = match item {
                Some((id, icon)) => world.spawn_scene(toolbar_op_button(id, *icon))?,
                None => world.spawn_scene(toolbar_separator())?,
            };
            child.insert((crate::EditorEntity, ChildOf(group)));
        }
        groups.push(group);
    }
    world.entity_mut(toolbar).insert_children(0, &groups);
    crate::toolbar_overflow::spawn_toolbar_overflow(world, toolbar);
    Ok(())
}

/// Show the group of the slot whose mode is on, or the object group when
/// none is, and light the buttons the slots name.
fn sync_2d_toolbar(_: On<RefreshOperatorButtons>, mut commands: Commands) {
    commands.queue(sync_2d_toolbar_now);
}

fn sync_2d_toolbar_now(world: &mut World) {
    let slots = world.resource::<Toolbar2dSlots>().clone();
    if slots.0.is_empty() {
        return;
    }
    let active = slots
        .0
        .iter()
        .find(|slot| (slot.active)(world))
        .map(|slot| slot.id);
    let mut groups = world.query::<(&Toolbar2dGroup, &mut Node)>();
    for (group, mut node) in groups.iter_mut(world) {
        let shown = match *group {
            Toolbar2dGroup::Object => active.is_none(),
            Toolbar2dGroup::Mode(id) => active == Some(id),
        };
        let display = if shown { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    let mut buttons = world.query::<(Entity, &ButtonOperatorCall)>();
    let calls: Vec<(Entity, String)> = buttons
        .iter(world)
        .map(|(entity, call)| (entity, call.id.to_string()))
        .collect();
    for (entity, id) in calls {
        let Some(lit) = slots.0.iter().find_map(|slot| (slot.lit)(world, &id)) else {
            continue;
        };
        let target = if lit {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Plain
        };
        if let Some(mut variant) = world.get_mut::<ButtonVariant>(entity)
            && *variant != target
        {
            *variant = target;
        }
    }
}
