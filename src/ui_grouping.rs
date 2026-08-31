//! Wrapping a selection in a container, and taking one apart again.
//!
//! `ui.group_into` puts a new container where the selection's bounding box
//! is and moves the selection into it; `ui.ungroup` lifts a container's
//! children out into its own place and takes the empty container away. Each
//! is one history entry, and neither moves anything on the canvas: an
//! authored rect is re-expressed against its new parent's offset box, which
//! is the same arithmetic a canvas drag writes back through.

use bevy::{ecs::entity::hash_map::EntityHashMap, prelude::*, world_serialization::DynamicWorld};
use jackdaw_api::prelude::*;

use crate::{
    EditorEntity,
    commands::{
        CommandHistory, EditorCommand, HierarchyLocation, despawn_scene_entity,
        filtered_scene_builder, set_hierarchy_location, snapshot_rebuild, sync_component_to_ast,
    },
    selection::Selection,
    ui_stage::{global_node_rect, parent_offset_box},
};

/// What a new container is called before the namer makes the name free.
const GROUP_NAME: &str = "Group";

/// Selected nodes that a group can act on: authored, laid out, and under one
/// parent, in the order the parent holds them.
///
/// One parent because the members keep their place on the canvas, and a rect
/// re-expressed against a different offset box is only the same rect when it
/// started from the box the container replaces.
fn group_members(world: &mut World) -> Option<(Option<Entity>, Vec<Entity>)> {
    let selected: Vec<Entity> = world.resource::<Selection>().entities.clone();
    let members: Vec<Entity> = selected
        .into_iter()
        .filter(|&entity| {
            world.get::<Node>(entity).is_some()
                && world.get::<EditorEntity>(entity).is_none()
                && world
                    .resource::<jackdaw_bsn::SceneBsnAst>()
                    .ast_for(entity)
                    .is_some()
        })
        .collect();
    if members.is_empty() {
        return None;
    }
    let parent = world.get::<ChildOf>(members[0]).map(ChildOf::parent);
    if members
        .iter()
        .any(|&entity| world.get::<ChildOf>(entity).map(ChildOf::parent) != parent)
    {
        warn!("ui.group_into: the selection spans more than one parent");
        return None;
    }
    let mut members = members;
    members.sort_by_key(|&entity| HierarchyLocation::from_world(world, entity).index);
    Some((parent, members))
}

/// The box every member sits inside, in global authored pixels.
fn bounding_rect(world: &World, members: &[Entity]) -> Option<Rect> {
    members
        .iter()
        .filter_map(|&entity| global_node_rect(world, entity))
        .reduce(|left, right| left.union(right))
}

/// Write `node` onto `entity` and into the document, the way an undone
/// layout edit is put back.
fn write_node(world: &mut World, entity: Entity, node: &Node) {
    if let Some(mut live) = world.get_mut::<Node>(entity) {
        *live = node.clone();
    }
    sync_component_to_ast::<Node>(
        world,
        entity,
        crate::inspector::node_card::node_type_path(),
        node,
    );
}

/// `entity`'s current rect expressed as authored `left`/`top` against an
/// offset box whose corner sits at `origin`.
fn offsets_against(world: &World, entity: Entity, origin: Vec2) -> Option<Vec2> {
    Some(global_node_rect(world, entity)?.min - origin)
}

/// A single entity's components, without its children, so undo can put a
/// container back with whatever else was on it.
fn snapshot_one(world: &World, entity: Entity) -> DynamicWorld {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    filtered_scene_builder(world, &registry)
        .extract_entities([entity].into_iter())
        .build()
}

/// Put a snapshot back into the world and say which entity it landed on.
fn restore_one(world: &mut World, snapshot: &DynamicWorld, was: Entity) -> Entity {
    let scene = snapshot_rebuild(snapshot);
    let mut map = EntityHashMap::default();
    let _ = scene.write_to_world(world, &mut map);
    map.get(&was).copied().unwrap_or(was)
}

/// The container `members` would be wrapped in: absolutely placed over their
/// bounding box, flowing along whichever side of it is longer.
fn container_node(world: &World, members: &[Entity], bounds: Rect) -> Node {
    let origin = members
        .first()
        .map(|&member| parent_offset_box(world, member).min)
        .unwrap_or(Vec2::ZERO);
    let offset = bounds.min - origin;
    Node {
        position_type: PositionType::Absolute,
        left: px(offset.x),
        top: px(offset.y),
        width: px(bounds.width()),
        height: px(bounds.height()),
        flex_direction: if bounds.width() > bounds.height() {
            FlexDirection::Row
        } else {
            FlexDirection::Column
        },
        ..default()
    }
}

/// Wrap a selection in a container.
struct GroupIntoContainer {
    /// The nodes to move in, in the order they will hold.
    members: Vec<Entity>,
    parent: Option<Entity>,
    /// Where the container goes in `parent`: the first member's slot.
    index: usize,
    name: String,
    node: Node,
    /// Each member's slot and `Node` before the group, for undo.
    before: Vec<(Entity, HierarchyLocation, Node)>,
    /// Each member's `Node` once it is measured against the container.
    after: Vec<(Entity, Node)>,
    /// The container this made, once it has made one.
    container: Option<Entity>,
    label: String,
}

impl EditorCommand for GroupIntoContainer {
    fn execute(&mut self, world: &mut World) {
        let mut container = world.spawn((Name::new(self.name.clone()), self.node.clone()));
        if let Some(parent) = self.parent {
            container.insert(ChildOf(parent));
        }
        let container = container.id();
        crate::scene_io::register_entity_in_ast(world, container);
        set_hierarchy_location(
            world,
            container,
            HierarchyLocation {
                parent: self.parent,
                index: self.index,
            },
        );

        for (index, &member) in self.members.iter().enumerate() {
            set_hierarchy_location(
                world,
                member,
                HierarchyLocation {
                    parent: Some(container),
                    index,
                },
            );
            if let Some((_, node)) = self.after.iter().find(|(entity, _)| *entity == member) {
                let node = node.clone();
                write_node(world, member, &node);
            }
        }
        self.container = Some(container);
        crate::hierarchy::sync_outliner_row_order(world, self.parent);
        crate::selection::select_only(world, container);
    }

    fn undo(&mut self, world: &mut World) {
        for (member, location, node) in self.before.clone() {
            write_node(world, member, &node);
            set_hierarchy_location(world, member, location);
        }
        if let Some(container) = self.container.take() {
            crate::commands::deselect_entities(world, &[container]);
            despawn_scene_entity(world, container);
        }
        crate::hierarchy::sync_outliner_row_order(world, self.parent);
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// Lift a container's children out and take the container away.
struct UngroupContainer {
    container: Entity,
    parent: Option<Entity>,
    index: usize,
    /// The container's own components, so undo puts back what was there and
    /// not merely a node of the same shape.
    snapshot: DynamicWorld,
    /// Each child, its `Node` inside the container, and its `Node` outside.
    children: Vec<(Entity, Node, Node)>,
    label: String,
}

impl EditorCommand for UngroupContainer {
    fn execute(&mut self, world: &mut World) {
        for (offset, (child, _, outside)) in self.children.clone().into_iter().enumerate() {
            set_hierarchy_location(
                world,
                child,
                HierarchyLocation {
                    parent: self.parent,
                    index: self.index + offset,
                },
            );
            write_node(world, child, &outside);
        }
        crate::commands::deselect_entities(world, &[self.container]);
        despawn_scene_entity(world, self.container);
        crate::hierarchy::sync_outliner_row_order(world, self.parent);
        let members: Vec<Entity> = self.children.iter().map(|(child, _, _)| *child).collect();
        select_many(world, &members);
    }

    fn undo(&mut self, world: &mut World) {
        self.container = restore_one(world, &self.snapshot, self.container);
        if let Some(parent) = self.parent {
            world.entity_mut(self.container).insert(ChildOf(parent));
        }
        crate::scene_io::register_entity_in_ast(world, self.container);
        set_hierarchy_location(
            world,
            self.container,
            HierarchyLocation {
                parent: self.parent,
                index: self.index,
            },
        );
        for (index, (child, inside, _)) in self.children.clone().into_iter().enumerate() {
            set_hierarchy_location(
                world,
                child,
                HierarchyLocation {
                    parent: Some(self.container),
                    index,
                },
            );
            write_node(world, child, &inside);
        }
        crate::hierarchy::sync_outliner_row_order(world, self.parent);
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// Wrap the selection in a container at its bounding rect.
pub(crate) fn group_selection(world: &mut World) {
    let Some((parent, members)) = group_members(world) else {
        return;
    };
    let Some(bounds) = bounding_rect(world, &members) else {
        warn!("ui.group_into: the selection has not been laid out yet");
        return;
    };
    let node = container_node(world, &members, bounds);

    // Along the axis the container flows, so the order the outliner shows
    // matches the order the canvas does.
    let mut members = members;
    let horizontal = node.flex_direction == FlexDirection::Row;
    members.sort_by(|&left, &right| {
        let key = |entity: Entity| {
            global_node_rect(world, entity)
                .map(|rect| if horizontal { rect.min.x } else { rect.min.y })
                .unwrap_or(0.0)
        };
        key(left).total_cmp(&key(right))
    });

    let before: Vec<(Entity, HierarchyLocation, Node)> = members
        .iter()
        .map(|&member| {
            (
                member,
                HierarchyLocation::from_world(world, member),
                world.get::<Node>(member).cloned().unwrap_or_default(),
            )
        })
        .collect();
    // The container has neither border nor padding, so the box its children
    // measure their offsets from starts at the bounding rect's own corner.
    let after: Vec<(Entity, Node)> = members
        .iter()
        .filter_map(|&member| {
            let mut node = world.get::<Node>(member).cloned()?;
            if node.position_type != PositionType::Absolute {
                // A flowed child had no offsets to keep; it flows in the
                // container instead, in the order it was put in.
                return Some((member, node));
            }
            let offset = offsets_against(world, member, bounds.min)?;
            node.left = px(offset.x);
            node.top = px(offset.y);
            node.right = Val::Auto;
            node.bottom = Val::Auto;
            Some((member, node))
        })
        .collect();

    let index = before
        .iter()
        .map(|(_, location, _)| location.index)
        .min()
        .unwrap_or(0);
    let mut taken = crate::entity_ops::scene_entity_names(world);
    let name = crate::entity_ops::claim_free_name(&mut taken, GROUP_NAME)
        .unwrap_or_else(|| GROUP_NAME.to_string());

    let mut command = GroupIntoContainer {
        members,
        parent,
        index,
        name,
        node,
        before,
        after,
        container: None,
        label: "Group into container".to_string(),
    };
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(command));
}

/// Lift the selected container's children into its place and remove it.
pub(crate) fn ungroup_selection(world: &mut World) {
    let Some(container) = world.resource::<Selection>().primary() else {
        return;
    };
    if world.get::<Node>(container).is_none()
        || world.get::<EditorEntity>(container).is_some()
        || world
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .ast_for(container)
            .is_none()
    {
        return;
    }
    let children: Vec<Entity> = world
        .get::<Children>(container)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    if children.is_empty() {
        warn!("ui.ungroup: the selected node holds nothing to lift out");
        return;
    }

    let location = HierarchyLocation::from_world(world, container);
    let origin = parent_offset_box(world, container).min;
    let plan: Vec<(Entity, Node, Node)> = children
        .iter()
        .filter_map(|&child| {
            let inside = world.get::<Node>(child).cloned()?;
            let mut outside = inside.clone();
            if outside.position_type == PositionType::Absolute
                && let Some(offset) = offsets_against(world, child, origin)
            {
                outside.left = px(offset.x);
                outside.top = px(offset.y);
                outside.right = Val::Auto;
                outside.bottom = Val::Auto;
            }
            Some((child, inside, outside))
        })
        .collect();

    let mut command = UngroupContainer {
        container,
        parent: location.parent,
        index: location.index,
        snapshot: snapshot_one(world, container),
        children: plan,
        label: "Ungroup container".to_string(),
    };
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(command));
}

/// Make `entities` the whole selection.
fn select_many(world: &mut World, entities: &[Entity]) {
    let mut state: bevy::ecs::system::SystemState<(Commands, ResMut<Selection>)> =
        bevy::ecs::system::SystemState::new(world);
    let Ok((mut commands, mut selection)) = state.get_mut(world) else {
        return;
    };
    selection.select_multiple(&mut commands, entities);
    state.apply(world);
}

/// A group needs an open UI scene and at least one authored node selected.
fn can_group(
    keybind_focus: crate::keybind_focus::KeybindFocus,
    active: ActiveModalQuery,
    selection: Res<Selection>,
    ui_scenes: Query<(), crate::prefab::AuthoredUiSceneRoot>,
    nodes: Query<(), (With<Node>, Without<EditorEntity>)>,
) -> bool {
    if keybind_focus.is_typing() || active.is_modal_running() || ui_scenes.is_empty() {
        return false;
    }
    selection
        .entities
        .iter()
        .any(|&entity| nodes.contains(entity))
}

/// Ungrouping needs the selection to be a node with children.
fn can_ungroup(
    keybind_focus: crate::keybind_focus::KeybindFocus,
    active: ActiveModalQuery,
    selection: Res<Selection>,
    ui_scenes: Query<(), crate::prefab::AuthoredUiSceneRoot>,
    containers: Query<&Children, (With<Node>, Without<EditorEntity>)>,
) -> bool {
    if keybind_focus.is_typing() || active.is_modal_running() || ui_scenes.is_empty() {
        return false;
    }
    selection
        .primary()
        .and_then(|entity| containers.get(entity).ok())
        .is_some_and(|children| !children.is_empty())
}

#[operator(
    id = "ui.group_into",
    label = "Group Into Container",
    description = "Wrap the selection in a container at its bounding rect.",
    is_available = can_group
)]
pub(crate) fn ui_group_into(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(group_selection);
    OperatorResult::Finished
}

#[operator(
    id = "ui.ungroup",
    label = "Ungroup",
    description = "Lift a container's children into its place and remove it.",
    is_available = can_ungroup
)]
pub(crate) fn ui_ungroup(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(ungroup_selection);
    OperatorResult::Finished
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<UiGroupIntoOp>()
        .register_operator::<UiUngroupOp>();
    ctx.bind_operator::<crate::core_extension::CoreExtensionInputContext, UiGroupIntoOp>([
        jackdaw_api_internal::keymap::PresetInput::key("KeyG").ctrl(),
    ]);
    ctx.bind_operator::<crate::core_extension::CoreExtensionInputContext, UiUngroupOp>([
        jackdaw_api_internal::keymap::PresetInput::key("KeyG")
            .ctrl()
            .shift(),
    ]);
}
