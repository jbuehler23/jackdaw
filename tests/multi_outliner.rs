//! Multi-instance Outliner: two `HierarchyTreeContainer`s should both
//! reflect every scene-graph change in lockstep.
//!
//! Pins three contracts the per-`(container, source)` `TreeIndex`
//! refactor introduced:
//!  - adding a new root scene entity spawns one row in every container,
//!    not zero (single-instance fallthrough) and not two in any one
//!    panel (the duplicate-row regression);
//!  - reparenting a scene entity moves its row under the new parent's
//!    `TreeRowChildren` in every container;
//!  - despawning the source removes the row in every container.

use bevy::prelude::*;
use jackdaw::hierarchy::HierarchyTreeContainer;
use jackdaw_widgets::tree_view::{TreeIndex, TreeNode};

mod util;

/// Spawn a host entity carrying `HierarchyTreeContainer` (which
/// requires `TreeRoot` + `EditorEntity`). Mirrors the runtime layout's
/// "Outliner panel content" entity.
fn spawn_outliner_container(world: &mut World) -> Entity {
    world
        .spawn((
            HierarchyTreeContainer,
            Node::default(),
            Visibility::Inherited,
        ))
        .id()
}

#[test]
fn add_root_entity_spawns_one_row_per_container() {
    let mut app = util::editor_test_app();
    let world = app.world_mut();

    let outliner_a = spawn_outliner_container(world);
    let outliner_b = spawn_outliner_container(world);

    let entity = world.spawn((Name::new("Brush"), Transform::default())).id();

    // Flush the queued `commands` from the `On<Add, ...>` observers.
    app.update();
    let world = app.world_mut();

    let index = world.resource::<TreeIndex>();
    assert!(
        index.contains(outliner_a, entity),
        "outliner A should have a row for the new root",
    );
    assert!(
        index.contains(outliner_b, entity),
        "outliner B should have a row for the new root",
    );

    // Exactly one row per container, never two.
    let mut q = world.query::<(Entity, &TreeNode)>();
    let rows: Vec<(Entity, Entity)> = q
        .iter(world)
        .filter(|(_, tree_node)| tree_node.0 == entity)
        .map(|(e, t)| (e, t.0))
        .collect();
    assert_eq!(
        rows.len(),
        2,
        "expected exactly one row per outliner container (2 total), got {}",
        rows.len(),
    );
}

#[test]
fn reparent_scene_entity_moves_row_in_every_outliner() {
    let mut app = util::editor_test_app();
    let world = app.world_mut();

    let outliner_a = spawn_outliner_container(world);
    let outliner_b = spawn_outliner_container(world);

    let parent = world
        .spawn((Name::new("Parent"), Transform::default()))
        .id();
    let child = world.spawn((Name::new("Child"), Transform::default())).id();
    app.update();

    // Sanity: both containers initially see both as roots.
    let world = app.world_mut();
    {
        let index = world.resource::<TreeIndex>();
        for c in [outliner_a, outliner_b] {
            assert!(index.contains(c, parent), "{c} should host parent row");
            assert!(index.contains(c, child), "{c} should host child row");
        }
    }

    // Mark the parent as having children populated so the reparent
    // observer reseats existing rows instead of treating it as a
    // not-yet-expanded subtree. (`spawn_single_tree_row` defaults
    // `TreeChildrenPopulated(false)`.)
    {
        let mut q = world.query::<(
            &TreeNode,
            &mut jackdaw_widgets::tree_view::TreeChildrenPopulated,
        )>();
        for (tree_node, mut populated) in q.iter_mut(world) {
            if tree_node.0 == parent {
                populated.0 = true;
            }
        }
    }

    // Reparent child under parent.
    world.entity_mut(child).insert(ChildOf(parent));
    app.update();

    let world = app.world_mut();
    let index = world.resource::<TreeIndex>();

    // Parent's row in each container has a `TreeRowChildren` descendant
    // that should be the new ancestor of the child's row.
    for container in [outliner_a, outliner_b] {
        let parent_row = index
            .get(container, parent)
            .expect("parent row in container");
        let child_row = index.get(container, child).expect("child row in container");

        // Walk up from child_row's ChildOf chain; we must hit parent_row.
        let mut current = child_row;
        let mut found_parent = false;
        for _ in 0..6 {
            let Some(co) = world.get::<ChildOf>(current) else {
                break;
            };
            if co.parent() == parent_row {
                found_parent = true;
                break;
            }
            current = co.parent();
        }
        assert!(
            found_parent,
            "child row in {container} should reparent under {parent_row} after the source was reparented",
        );
    }
}

#[test]
fn despawn_scene_entity_drops_row_in_every_outliner() {
    let mut app = util::editor_test_app();
    let world = app.world_mut();

    let outliner_a = spawn_outliner_container(world);
    let outliner_b = spawn_outliner_container(world);

    let entity = world.spawn((Name::new("Brush"), Transform::default())).id();
    app.update();

    let world = app.world_mut();
    {
        let index = world.resource::<TreeIndex>();
        assert!(index.contains(outliner_a, entity));
        assert!(index.contains(outliner_b, entity));
    }

    world.entity_mut(entity).despawn();
    app.update();

    let world = app.world_mut();
    let index = world.resource::<TreeIndex>();
    assert!(
        !index.contains(outliner_a, entity),
        "row should be cleaned out of outliner A",
    );
    assert!(
        !index.contains(outliner_b, entity),
        "row should be cleaned out of outliner B",
    );
}
