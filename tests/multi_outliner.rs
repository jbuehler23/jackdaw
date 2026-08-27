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
//!
//! Also pins that a `UiSceneRoot` is a root for outliner purposes. UI
//! roots carry `UiTransform` (via `Node`), never `Transform`, so the
//! `Transform`-keyed root predicate and root-spawn observer would leave
//! an authored UI scene invisible in every panel.

use bevy::prelude::*;
use jackdaw::hierarchy::{HierarchyShowAll, HierarchyTreeContainer};
use jackdaw_scene_types::UiSceneRoot;
use jackdaw_widgets::tree_view::{TreeIndex, TreeNode, TreeRowContent, TreeRowLabel};

mod util;

/// Spawn a host entity carrying `HierarchyTreeContainer` (which
/// requires `TreeRoot` + `EditorEntity`). Matches the runtime
/// layout's "Outliner panel content" entity.
fn spawn_outliner_container(world: &mut World) -> Entity {
    world
        .spawn((
            HierarchyTreeContainer,
            Node::default(),
            Visibility::Inherited,
        ))
        .id()
}

/// Text of the `TreeRowLabel` under `row` (`TreeNode` -> `TreeRowContent`
/// -> `TreeRowLabel`).
fn row_label(world: &World, row: Entity) -> Option<String> {
    let children: Vec<Entity> = world.get::<Children>(row)?.iter().collect();
    for child in children {
        if world.get::<TreeRowContent>(child).is_none() {
            continue;
        }
        let Some(content_children) = world.get::<Children>(child) else {
            continue;
        };
        for grandchild in content_children.iter() {
            if world.get::<TreeRowLabel>(grandchild).is_some()
                && let Some(text) = world.get::<Text>(grandchild)
            {
                return Some(text.0.clone());
            }
        }
    }
    None
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

#[test]
fn ui_scene_root_gets_a_row_in_every_outliner() {
    let mut app = util::editor_test_app();
    let world = app.world_mut();

    // A UI scene root is a bevy_ui node: `UiTransform`, no `Transform`.
    let root = world
        .spawn((
            Name::new("Overlay"),
            UiSceneRoot::default(),
            Node::default(),
        ))
        .id();

    // Containers mount after the root exists, so the row has to come out
    // of the full rebuild rather than the spawn observers.
    let outliner_a = spawn_outliner_container(world);
    let outliner_b = spawn_outliner_container(world);
    app.update();

    let world = app.world_mut();
    let rows: Vec<Entity> = {
        let index = world.resource::<TreeIndex>();
        [outliner_a, outliner_b]
            .into_iter()
            .map(|container| {
                index.get(container, root).unwrap_or_else(|| {
                    panic!("{container} should host a row for the UI scene root")
                })
            })
            .collect()
    };
    for row in &rows {
        assert_eq!(row_label(world, *row).as_deref(), Some("Overlay"));
    }
}

#[test]
fn unnamed_ui_scene_root_spawns_a_row_that_picks_up_its_name() {
    let mut app = util::editor_test_app();
    let world = app.world_mut();

    let outliner_a = spawn_outliner_container(world);
    let outliner_b = spawn_outliner_container(world);
    world.resource_mut::<HierarchyShowAll>().0 = true;
    app.update();

    // No `Name`, so only the root-added observer can produce the row.
    let root = app
        .world_mut()
        .spawn((UiSceneRoot::default(), Node::default()))
        .id();
    app.update();

    let world = app.world_mut();
    let rows: Vec<Entity> = {
        let index = world.resource::<TreeIndex>();
        [outliner_a, outliner_b]
            .into_iter()
            .map(|container| {
                index.get(container, root).unwrap_or_else(|| {
                    panic!("{container} should host a row for the unnamed UI scene root")
                })
            })
            .collect()
    };

    // Naming the root relabels the row it already owns.
    world.entity_mut(root).insert(Name::new("HUD"));
    app.update();

    let world = app.world();
    for row in &rows {
        assert_eq!(
            row_label(world, *row).as_deref(),
            Some("HUD"),
            "naming the UI scene root should relabel its row",
        );
    }
}
