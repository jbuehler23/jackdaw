//! Wrapping a selection in a container, and taking one apart again.
//!
//! `ui.group_into` puts a new container where the selection's bounding box is
//! and moves the selection into it; `ui.ungroup` lifts a container's children
//! out into its own place and takes the empty container away. Each is one
//! history entry. An absolutely placed member keeps its spot by having its
//! rect re-expressed against the container; a flowed member flows again
//! inside it. A transformed member is refused, since the bounding rect is
//! measured before the transform. Every refusal says so in the status bar.

use bevy::{
    ecs::entity::hash_map::EntityHashMap, math::Rot2, prelude::*, ui::UiTransform,
    world_serialization::DynamicWorld,
};
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

/// Whether `entity` is a scene's own root rather than a node inside one.
/// Both operators refuse a root, which the document is anchored on.
fn is_scene_root(world: &World, entity: Entity) -> bool {
    world
        .get::<jackdaw_scene_types::UiSceneRoot>(entity)
        .is_some()
        || world
            .get::<jackdaw_scene_types::Scene2dRoot>(entity)
            .is_some()
}

/// Selected nodes that a group can act on: authored, laid out, and under one
/// parent, in the order the parent holds them.
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
        crate::status_bar::notify_error(
            world,
            "grouping needs every selected node under one parent",
        );
        return None;
    }
    let mut members = members;
    members.sort_by_key(|&entity| HierarchyLocation::from_world(world, entity).index);
    Some((parent, members))
}

/// What a notice calls `entity`.
fn name_of(world: &World, entity: Entity) -> String {
    world
        .get::<Name>(entity)
        .map_or_else(|| "the node".to_string(), |name| name.as_str().to_owned())
}

/// Whether `entity` carries a `UiTransform` that is not the identity. A
/// translation counts: all three are drawn around the laid-out box, so the
/// container would apply the transform a second time.
fn is_transformed(world: &World, entity: Entity) -> bool {
    let identity = UiTransform::default();
    world.get::<UiTransform>(entity).is_some_and(|transform| {
        transform.rotation != Rot2::IDENTITY
            || transform.scale != Vec2::ONE
            || transform.translation != identity.translation
    })
}

/// Whether `entity` is laid out by its parent rather than placed. Only an
/// absolutely placed member has its `left`/`top` rewritten.
fn is_flowed(world: &World, entity: Entity) -> bool {
    world
        .get::<Node>(entity)
        .is_none_or(|node| node.position_type != PositionType::Absolute)
}

/// How many entries the list `parent` names holds. `None` is the scene's own
/// root list.
fn list_len(world: &World, parent: Option<Entity>) -> usize {
    match parent {
        Some(parent) => world.get::<Children>(parent).map_or(0, Children::len),
        None => world
            .get_resource::<jackdaw_bsn::SceneBsnAst>()
            .map_or(0, |ast| ast.roots.len()),
    }
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

/// The container itself, so undo can put it back with whatever else was on
/// it. The container alone: ungroup lifts every child out before this is
/// taken.
fn snapshot_one(world: &World, entity: Entity) -> DynamicWorld {
    let subtree = [entity];
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    filtered_scene_builder(world, &registry)
        .extract_entities(subtree.into_iter())
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

    /// Undo in three passes: move the members out, despawn the container,
    /// then replay the slots lowest first. A slot is an index into the list
    /// as it was, so the container has to be gone and every lower member
    /// back before the next one is placed.
    fn undo(&mut self, world: &mut World) {
        for (member, location, node) in self.before.clone() {
            write_node(world, member, &node);
            let end = list_len(world, location.parent);
            set_hierarchy_location(
                world,
                member,
                HierarchyLocation {
                    parent: location.parent,
                    index: end,
                },
            );
        }
        if let Some(container) = self.container.take() {
            crate::commands::deselect_entities(world, &[container]);
            despawn_scene_entity(world, container);
        }
        let mut restore = self.before.clone();
        restore.sort_by_key(|(_, location, _)| location.index);
        for (member, location, _) in restore {
            set_hierarchy_location(world, member, location);
        }
        crate::hierarchy::sync_outliner_row_order(world, self.parent);
        select_many(world, &self.members);
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// One child on its way out of a container. `layout` is `None` for a child
/// carrying no `Node`, which is moved as it is; it still moves, since a child
/// left behind is despawned with the container.
#[derive(Clone)]
struct UngroupChild {
    entity: Entity,
    /// The child's `Node` inside the container and outside it, when it has one.
    layout: Option<(Node, Node)>,
}

/// Lift a container's children out and take the container away.
struct UngroupContainer {
    container: Entity,
    parent: Option<Entity>,
    index: usize,
    /// The container and whatever is still under it once the children are
    /// out. Taken during `execute`, since what is left is only known then.
    snapshot: Option<DynamicWorld>,
    children: Vec<UngroupChild>,
    label: String,
}

impl EditorCommand for UngroupContainer {
    fn execute(&mut self, world: &mut World) {
        for (offset, child) in self.children.clone().into_iter().enumerate() {
            set_hierarchy_location(
                world,
                child.entity,
                HierarchyLocation {
                    parent: self.parent,
                    index: self.index + offset,
                },
            );
            if let Some((_, outside)) = &child.layout {
                write_node(world, child.entity, outside);
            }
        }
        self.snapshot = Some(snapshot_one(world, self.container));
        crate::commands::deselect_entities(world, &[self.container]);
        despawn_scene_entity(world, self.container);
        crate::hierarchy::sync_outliner_row_order(world, self.parent);
        let members: Vec<Entity> = self.children.iter().map(|child| child.entity).collect();
        select_many(world, &members);
    }

    fn undo(&mut self, world: &mut World) {
        let Some(snapshot) = self.snapshot.take() else {
            return;
        };
        self.container = restore_one(world, &snapshot, self.container);
        if let Some(parent) = self.parent {
            world.entity_mut(self.container).insert(ChildOf(parent));
        }
        crate::ui_palette::register_authored_subtree(world, self.container);
        set_hierarchy_location(
            world,
            self.container,
            HierarchyLocation {
                parent: self.parent,
                index: self.index,
            },
        );
        for (index, child) in self.children.clone().into_iter().enumerate() {
            set_hierarchy_location(
                world,
                child.entity,
                HierarchyLocation {
                    parent: Some(self.container),
                    index,
                },
            );
            if let Some((inside, _)) = &child.layout {
                write_node(world, child.entity, inside);
            }
        }
        crate::hierarchy::sync_outliner_row_order(world, self.parent);
        crate::selection::select_only(world, self.container);
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// Wrap the selection in a container at its bounding rect.
pub fn group_selection(world: &mut World) {
    let selected: Vec<Entity> = world.resource::<Selection>().entities.clone();
    if selected.iter().any(|&entity| is_scene_root(world, entity)) {
        crate::status_bar::notify_error(
            world,
            "a scene's own root cannot be grouped; select the nodes inside it",
        );
        return;
    }
    let Some((parent, members)) = group_members(world) else {
        return;
    };
    if let Some(&turned) = members
        .iter()
        .find(|&&member| is_transformed(world, member))
    {
        let name = name_of(world, turned);
        crate::status_bar::notify_error(
            world,
            format!(
                "{name} carries a transform, so a container around it would not be the box it fills"
            ),
        );
        return;
    }
    let Some(bounds) = bounding_rect(world, &members) else {
        crate::status_bar::notify_error(
            world,
            "the selection has not been laid out yet, so it has no box to group at",
        );
        return;
    };
    let node = container_node(world, &members, bounds);

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
    // The container has neither border nor padding, so its children measure
    // their offsets from the bounding rect's own corner. A flowed member is
    // absent: the container's layout places it.
    let after: Vec<(Entity, Node)> = members
        .iter()
        .filter(|&&member| !is_flowed(world, member))
        .filter_map(|&member| {
            let mut node = world.get::<Node>(member).cloned()?;
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
pub fn ungroup_selection(world: &mut World) {
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
    if is_scene_root(world, container) {
        crate::status_bar::notify_error(
            world,
            "a scene's own root cannot be ungrouped; select a container inside it",
        );
        return;
    }
    let children: Vec<Entity> = world
        .get::<Children>(container)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    if children.is_empty() {
        let name = name_of(world, container);
        crate::status_bar::notify_error(world, format!("{name} holds nothing to lift out"));
        return;
    }

    let location = HierarchyLocation::from_world(world, container);
    let origin = parent_offset_box(world, container).min;
    let moves: Vec<UngroupChild> = children
        .iter()
        .map(|&child| {
            let layout = world.get::<Node>(child).cloned().map(|inside| {
                let mut outside = inside.clone();
                if outside.position_type == PositionType::Absolute
                    && let Some(offset) = offsets_against(world, child, origin)
                {
                    outside.left = px(offset.x);
                    outside.top = px(offset.y);
                    outside.right = Val::Auto;
                    outside.bottom = Val::Auto;
                }
                (inside, outside)
            });
            UngroupChild {
                entity: child,
                layout,
            }
        })
        .collect();

    let mut command = UngroupContainer {
        container,
        parent: location.parent,
        index: location.index,
        snapshot: None,
        children: moves,
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

/// Entities a scene hangs off: neither operator touches one.
type SceneRoots = Or<(
    With<jackdaw_scene_types::UiSceneRoot>,
    With<jackdaw_scene_types::Scene2dRoot>,
)>;

/// A group needs an open UI scene and at least one authored node selected
/// that is not a scene's own root.
fn can_group(
    keybind_focus: crate::keybind_focus::KeybindFocus,
    active: ActiveModalQuery,
    selection: Res<Selection>,
    ui_scenes: Query<(), crate::prefab::AuthoredUiSceneRoot>,
    nodes: Query<(), (With<Node>, Without<EditorEntity>)>,
    roots: Query<(), SceneRoots>,
) -> bool {
    if keybind_focus.keyboard_is_spoken_for() || active.is_modal_running() || ui_scenes.is_empty() {
        return false;
    }
    if selection.entities.iter().any(|&e| roots.contains(e)) {
        return false;
    }
    selection
        .entities
        .iter()
        .any(|&entity| nodes.contains(entity))
}

/// Ungrouping needs the selection to be a node with children, and not a
/// scene's own root.
fn can_ungroup(
    keybind_focus: crate::keybind_focus::KeybindFocus,
    active: ActiveModalQuery,
    selection: Res<Selection>,
    ui_scenes: Query<(), crate::prefab::AuthoredUiSceneRoot>,
    containers: Query<&Children, (With<Node>, Without<EditorEntity>)>,
    roots: Query<(), SceneRoots>,
) -> bool {
    if keybind_focus.keyboard_is_spoken_for() || active.is_modal_running() || ui_scenes.is_empty() {
        return false;
    }
    let Some(primary) = selection.primary() else {
        return false;
    };
    if roots.contains(primary) {
        return false;
    }
    containers
        .get(primary)
        .is_ok_and(|children| !children.is_empty())
}

#[operator(
    id = "ui.group_into",
    label = "Group Into Container",
    description = "Wrap the selection in a container at its bounding rect.",
    // The command it queues pushes the undo entry itself.
    allows_undo = false,
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
    allows_undo = false,
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
