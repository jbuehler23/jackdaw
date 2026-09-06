use bevy::prelude::*;

/// Links a tree row UI entity to the source entity it represents.
///
/// Multiple `TreeNode`s may point at the same source (one per
/// container in a multi-instance Outliner setup), so the inverse
/// `TreeNodeSource` holds a `Vec<Entity>`.
#[derive(Component)]
#[relationship(relationship_target = TreeNodeSource)]
pub struct TreeNode(pub Entity);

/// Inverse relationship: source entity -> every tree row referencing it.
#[derive(Component, Default)]
#[relationship_target(relationship = TreeNode)]
pub struct TreeNodeSource(Vec<Entity>);

/// Marker for expand/collapse toggle button
#[derive(Component)]
pub struct TreeNodeExpandToggle;

/// Tracks whether a tree node is expanded
#[derive(Component, Default)]
pub struct TreeNodeExpanded(pub bool);

/// The clickable content area of a tree row (contains toggle + label)
#[derive(Component)]
pub struct TreeRowContent;

/// Marker on `TreeRowContent` when its source entity is selected
#[derive(Component)]
pub struct TreeRowSelected;

/// Container for displaying the row label
#[derive(Component)]
#[require(Text)]
pub struct TreeRowLabel;

/// Container for child rows (indented)
#[derive(Component)]
pub struct TreeRowChildren;

/// Tracks whether a tree node's children have been lazily populated.
/// Set to `true` after first expansion spawns children; prevents re-population on re-expand.
#[derive(Component, Default)]
pub struct TreeChildrenPopulated(pub bool);

/// Classifies a scene entity by type for sorting and colored dot display.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntityCategory {
    Camera,
    Light,
    Mesh,
    Scene,
    Prefab,
    /// Entity inherited from a prefab (carries `PrefabEntityId` but no
    /// `IsA`). Drawn with a faint tinge to signal it's a materialized
    /// child of an instance rather than authored directly.
    Inherited,
    /// A node of a loaded asset (a glTF scene's own nodes and meshes) rather
    /// than something authored. Shown so the model's structure is inspectable,
    /// but drawn with its own icon and a muted tone because it has no document
    /// node and so cannot be duplicated, deleted or reparented.
    AssetPart,
    /// A container entity: it has children but no more specific type of its
    /// own, so it reads as a grouping node (e.g. a "Trees" parent).
    Group,
    #[default]
    Entity,
}

/// Marker for the colored category dot in a tree row.
#[derive(Component)]
pub struct TreeRowDot;

/// Marker for the visibility toggle icon in a tree row.
#[derive(Component)]
pub struct TreeRowVisibilityToggle;

/// Event fired when a visibility toggle is clicked
#[derive(EntityEvent)]
pub struct TreeRowVisibilityToggled {
    #[event_target]
    pub entity: Entity,
    /// The source (scene) entity to toggle visibility
    pub source_entity: Entity,
}

/// Marker for the lock toggle in a tree row.
#[derive(Component)]
pub struct TreeRowLockToggle;

/// Event fired when a lock toggle is clicked.
#[derive(EntityEvent)]
pub struct TreeRowLockToggled {
    #[event_target]
    pub entity: Entity,
    /// The source (scene) entity to lock or unlock.
    pub source_entity: Entity,
}

/// Marker on the text input during inline rename
#[derive(Component)]
pub struct TreeRowInlineRename;

/// Maps source (scene) entities to their tree row UI entities, keyed
/// by the tree's container so multiple containers (e.g. two open
/// Outliner tabs) each track their own copy of the same source.
///
/// Maintained automatically by [`maintain_tree_index`], which walks
/// each new `TreeNode` up to its `TreeRoot` (matched by the marker
/// component the consumer adds to the container) and inserts an
/// entry under that container's key.
#[derive(Resource, Default)]
pub struct TreeIndex {
    /// `(container, source)` -> tree row entity. The container is the
    /// host entity carrying [`TreeRoot`]; the source is the scene
    /// entity the row represents.
    map: HashMap<(Entity, Entity), Entity>,
    /// `source` -> the containers holding a row for it.
    ///
    /// A second index rather than a scan: the per-frame question is "does
    /// this changed entity have a row anywhere", asked once per changed
    /// entity, and answering it by walking every key makes a filter
    /// keystroke - which dirties every row - cost the square of the tree.
    by_source: HashMap<Entity, Vec<Entity>>,
}

impl TreeIndex {
    /// Tree row entity for `source` in `container`, if one exists.
    pub fn get(&self, container: Entity, source: Entity) -> Option<Entity> {
        self.map.get(&(container, source)).copied()
    }

    /// Insert / overwrite the mapping for the `(container, source)` pair.
    pub fn insert(&mut self, container: Entity, source: Entity, tree_row: Entity) {
        if self.map.insert((container, source), tree_row).is_none() {
            self.by_source.entry(source).or_default().push(container);
        }
    }

    /// Drop the mapping for the `(container, source)` pair.
    pub fn remove(&mut self, container: Entity, source: Entity) {
        if self.map.remove(&(container, source)).is_some() {
            self.drop_source_entry(container, source);
        }
    }

    /// Drop every mapping for `source` across every container. Used
    /// when a scene entity goes away and its rows in every panel
    /// should be forgotten.
    pub fn remove_source(&mut self, source: Entity) {
        for container in self.by_source.remove(&source).unwrap_or_default() {
            self.map.remove(&(container, source));
        }
    }

    /// Forget one container from `source`'s list, and the list itself once
    /// it is empty, so an entity with no rows left holds no entry.
    fn drop_source_entry(&mut self, container: Entity, source: Entity) {
        let Some(containers) = self.by_source.get_mut(&source) else {
            return;
        };
        if let Some(at) = containers.iter().position(|held| *held == container) {
            containers.swap_remove(at);
        }
        if containers.is_empty() {
            self.by_source.remove(&source);
        }
    }

    /// True if `source` has a row in `container`.
    pub fn contains(&self, container: Entity, source: Entity) -> bool {
        self.map.contains_key(&(container, source))
    }

    /// True if `source` has a row in any container.
    pub fn contains_anywhere(&self, source: Entity) -> bool {
        self.by_source.contains_key(&source)
    }

    /// Iterate every row entity for `source` across all containers.
    pub fn rows_for_source(&self, source: Entity) -> impl Iterator<Item = (Entity, Entity)> + '_ {
        self.by_source
            .get(&source)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(move |&container| {
                self.map
                    .get(&(container, source))
                    .map(|row| (container, *row))
            })
    }

    /// Iterate every row entity for `container`.
    pub fn rows_in(&self, container: Entity) -> impl Iterator<Item = (Entity, Entity)> + '_ {
        self.map
            .iter()
            .filter(move |((c, _), _)| *c == container)
            .map(|((_, s), row)| (*s, *row))
    }

    /// Drop every mapping for `container`. Used when a panel hosting
    /// a tree is torn down.
    pub fn clear_container(&mut self, container: Entity) {
        self.map.retain(|(c, _), _| *c != container);
        self.by_source.retain(|_, containers| {
            containers.retain(|held| *held != container);
            !containers.is_empty()
        });
    }

    /// Drop every mapping. Used when the host app fully resets state.
    pub fn clear(&mut self) {
        self.map.clear();
        self.by_source.clear();
    }
}

/// Marker the consumer adds to the entity that hosts a tree (every
/// `Outliner` panel content entity, in jackdaw's case). The widget
/// crate uses it during ancestor walks in [`maintain_tree_index`] to
/// find which container a freshly-spawned `TreeNode` belongs to;
/// `TreeIndex` is keyed by `(container, source)` so multiple
/// containers can mirror the same source set without colliding.
#[derive(Component, Default)]
pub struct TreeRoot;

use std::collections::HashMap;

/// Tracks which tree row has keyboard focus (rendered with a focus ring).
#[derive(Resource, Default)]
pub struct TreeFocused(pub Option<Entity>);

/// Event fired when a tree row is clicked
#[derive(EntityEvent)]
pub struct TreeRowClicked {
    #[event_target]
    pub entity: Entity,
    /// The source entity this tree row represents
    pub source_entity: Entity,
}

/// Event fired when a tree row is dropped onto another tree row
#[derive(EntityEvent)]
pub struct TreeRowDropped {
    #[event_target]
    pub entity: Entity,
    /// The scene entity being moved
    pub dragged_source: Entity,
    /// The scene entity to become new parent
    pub target_source: Entity,
}

/// Where the drop line is drawn while a drag is over the tree, and what
/// a release there would mean.
///
/// One entry: one pointer drags at a time. Written by the gap zones as
/// the pointer passes over them and cleared when the drag leaves the
/// tree, so the line follows the pointer instead of appearing once when
/// a zone is first entered.
#[derive(Resource, Default)]
pub struct TreeDropLine {
    /// The gap zone the pointer is over, if any.
    pub zone: Option<Entity>,
    /// How far in the line starts, in logical pixels from the tree's left
    /// edge: the indent of the level the drop would land at.
    pub indent: f32,
}

/// Whether the drag in progress has been called off.
///
/// `bevy_picking` has no notion of a cancelled drag: Escape does not stop
/// the pointer, and the drop still arrives when the button comes up. This
/// is what a list checks before acting on one, and the drop that reads it
/// clears it.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeDragCancelled(
    /// Set while a drag has been called off and its release is still to come.
    pub bool,
);

/// A row the pointer has been resting on during a drag, and for how long.
///
/// Resting over a collapsed parent opens it, so a subtree can be reached
/// without letting go of what is being dragged.
#[derive(Resource, Default)]
pub struct TreeSpringLoad {
    pub row: Option<Entity>,
    pub waited: f32,
}

/// Marker on the strip that stands for the gap above or below a
/// tree row. A drop there reorders; a drop on the row itself reparents.
#[derive(Component)]
pub struct TreeRowInsertZone {
    /// `true` for the strip below the row (and below everything nested
    /// under it), `false` for the strip above it.
    pub after: bool,
}

/// Event fired when a drag is dropped on the gap between two tree rows,
/// which reorders rather than reparents.
///
/// The widget reports the gap it was dropped in as a row and a side; the
/// consumer owns the scene, so it is the one that turns that into a parent
/// and a sibling index.
#[derive(EntityEvent)]
pub struct TreeRowInserted {
    #[event_target]
    pub entity: Entity,
    /// The scene entity being moved
    pub dragged_source: Entity,
    /// The scene entity whose row the gap sits against
    pub target: Entity,
    /// Which side of `target` the gap is: `0` immediately before it among
    /// its siblings, `1` immediately after.
    pub index: usize,
}

/// Event fired when a tree row is dropped onto the root container (deparent)
#[derive(EntityEvent)]
pub struct TreeRowDroppedOnRoot {
    #[event_target]
    pub entity: Entity,
    /// The scene entity being moved back to root
    pub dragged_source: Entity,
}

/// Event fired when an inline rename is committed
#[derive(EntityEvent)]
pub struct TreeRowRenamed {
    #[event_target]
    pub entity: Entity,
    /// The source (scene) entity
    pub source_entity: Entity,
    /// The new name entered by the user
    pub new_name: String,
}

/// Event fired to request starting an inline rename
#[derive(EntityEvent)]
pub struct TreeRowStartRename {
    #[event_target]
    pub entity: Entity,
    /// The source (scene) entity to rename
    pub source_entity: Entity,
}

pub struct TreeViewPlugin;

impl Plugin for TreeViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TreeIndex>()
            .init_resource::<TreeFocused>()
            .init_resource::<TreeDropLine>()
            .init_resource::<TreeSpringLoad>()
            .init_resource::<TreeDragCancelled>()
            .add_systems(PostUpdate, (maintain_tree_index,));
    }
}

/// Keep `TreeIndex` in sync with `TreeNode` additions and removals.
///
/// On a freshly-added node, walks up the parent chain until it hits
/// an entity carrying [`TreeRoot`] and registers `(root, source) ->
/// row`. Multiple roots in the same world (e.g. two Outliner tabs)
/// each maintain their own independent mapping.
pub fn maintain_tree_index(
    mut index: ResMut<TreeIndex>,
    added: Query<(Entity, &TreeNode), Added<TreeNode>>,
    parents: Query<&ChildOf>,
    roots: Query<(), With<TreeRoot>>,
    mut removed: RemovedComponents<TreeNode>,
) {
    for (tree_row, tree_node) in &added {
        let mut current = tree_row;
        let container = loop {
            if roots.get(current).is_ok() {
                break Some(current);
            }
            match parents.get(current) {
                Ok(parent) => current = parent.parent(),
                Err(_) => break None,
            }
        };
        if let Some(container) = container {
            index.insert(container, tree_node.0, tree_row);
        }
    }

    for removed_entity in removed.read() {
        // Scan the map to find which (container, source) maps to this
        // removed tree row. Quadratic in worst case; only runs on
        // removal frames, not every frame.
        let key = index
            .map
            .iter()
            .find(|(_, tree_row)| **tree_row == removed_entity)
            .map(|(k, _)| *k);
        if let Some((container, source)) = key {
            index.remove(container, source);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The source index answers "does this entity have a row anywhere"
    /// without walking every key, so it has to be taken down by every path
    /// that takes a row out of the map. A stale entry would keep the icon
    /// pass working on a row that is gone.
    #[test]
    fn the_source_index_follows_every_way_a_row_leaves() {
        let mut world = World::new();
        let one = world.spawn_empty().id();
        let two = world.spawn_empty().id();
        let source = world.spawn_empty().id();
        let row_one = world.spawn_empty().id();
        let row_two = world.spawn_empty().id();

        let mut index = TreeIndex::default();
        index.insert(one, source, row_one);
        index.insert(two, source, row_two);
        assert!(index.contains_anywhere(source));
        assert_eq!(index.rows_for_source(source).count(), 2);

        index.remove(one, source);
        assert!(
            index.contains_anywhere(source),
            "the other panel still has one"
        );
        assert_eq!(
            index.rows_for_source(source).collect::<Vec<_>>(),
            vec![(two, row_two)]
        );

        index.clear_container(two);
        assert!(!index.contains_anywhere(source), "no panel holds a row now");
        assert_eq!(index.rows_for_source(source).count(), 0);

        index.insert(one, source, row_one);
        index.remove_source(source);
        assert!(!index.contains_anywhere(source));
        assert!(index.get(one, source).is_none(), "the map went with it");

        index.insert(one, source, row_one);
        index.clear();
        assert!(!index.contains_anywhere(source));
    }
}
