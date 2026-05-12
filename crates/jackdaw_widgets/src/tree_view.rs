use bevy::prelude::*;

/// Marker for the tree view container
#[derive(Component)]
pub struct TreeView;

/// Links a tree row UI entity to the source entity it represents.
///
/// Multiple `TreeNode`s may point at the same source (one per
/// container in a multi-instance Outliner setup), so the inverse
/// `TreeNodeSource` holds a `Vec<Entity>`.
#[derive(Component)]
#[relationship(relationship_target = TreeNodeSource)]
pub struct TreeNode(pub Entity);

/// Inverse relationship: source entity → every tree row referencing it.
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
    /// `(container, source)` → tree row entity. The container is the
    /// host entity carrying [`TreeRoot`]; the source is the scene
    /// entity the row represents.
    map: HashMap<(Entity, Entity), Entity>,
}

impl TreeIndex {
    /// Tree row entity for `source` in `container`, if one exists.
    pub fn get(&self, container: Entity, source: Entity) -> Option<Entity> {
        self.map.get(&(container, source)).copied()
    }

    /// Insert / overwrite the mapping for the `(container, source)` pair.
    pub fn insert(&mut self, container: Entity, source: Entity, tree_row: Entity) {
        self.map.insert((container, source), tree_row);
    }

    /// Drop the mapping for the `(container, source)` pair.
    pub fn remove(&mut self, container: Entity, source: Entity) {
        self.map.remove(&(container, source));
    }

    /// Drop every mapping for `source` across every container. Used
    /// when a scene entity goes away and its rows in every panel
    /// should be forgotten.
    pub fn remove_source(&mut self, source: Entity) {
        self.map.retain(|(_, s), _| *s != source);
    }

    /// True if `source` has a row in `container`.
    pub fn contains(&self, container: Entity, source: Entity) -> bool {
        self.map.contains_key(&(container, source))
    }

    /// True if `source` has a row in any container.
    pub fn contains_anywhere(&self, source: Entity) -> bool {
        self.map.keys().any(|(_, s)| *s == source)
    }

    /// Iterate every row entity for `source` across all containers.
    pub fn rows_for_source(&self, source: Entity) -> impl Iterator<Item = (Entity, Entity)> + '_ {
        self.map
            .iter()
            .filter(move |((_, s), _)| *s == source)
            .map(|((c, _), row)| (*c, *row))
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
    }

    /// Drop every mapping. Used when the host app fully resets state.
    pub fn clear(&mut self) {
        self.map.clear();
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
            .add_systems(PostUpdate, (maintain_tree_index,));
    }
}

/// Keep `TreeIndex` in sync with `TreeNode` additions and removals.
///
/// On a freshly-added node, walks up the parent chain until it hits
/// an entity carrying [`TreeRoot`] and registers `(root, source) →
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
