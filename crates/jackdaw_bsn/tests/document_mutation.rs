//! Document-layer mutation tests for [`SceneBsnAst`]: component patch
//! removal, reparenting, whole-document cloning, single-node cloning, and
//! stable-id lookups. Fixtures are authored as `.bsn` text and parsed through
//! the loader so each test starts from a document the editor could hold.

use bevy::ecs::entity::Entity;
use bevy::ecs::world::World;

use jackdaw_bsn::{
    BsnPatch, BsnValue, SceneBsnAst, clone_node_into, get_bsn_field, parse_bsn_text,
};

const TRANSFORM: &str = "test_types::Transform";
const TAG: &str = "test_types::Tag";
const MATERIAL: &str = "test_types::Material";

/// The root node named `name`, or a panic listing what the document holds.
fn root_named(ast: &SceneBsnAst, name: &str) -> Entity {
    ast.roots
        .iter()
        .copied()
        .find(|&r| ast.get_name(r) == Some(name))
        .unwrap_or_else(|| {
            let names: Vec<_> = ast.roots.iter().map(|&r| ast.get_name(r)).collect();
            panic!("no root named {name}; roots are {names:?}")
        })
}

/// The child of `parent` named `name`.
fn child_named(ast: &SceneBsnAst, parent: Entity, name: &str) -> Entity {
    ast.get_children_ast(parent)
        .into_iter()
        .find(|&c| ast.get_name(c) == Some(name))
        .unwrap_or_else(|| panic!("no child of {parent} named {name}"))
}

// ---------------------------------------------------------------------------
// remove_component_patch
// ---------------------------------------------------------------------------

#[test]
fn remove_component_patch_despawns_patch_and_drops_it_from_the_node() {
    let mut ast = parse_bsn_text(
        "#Node\n\
         test_types::Transform { x: 1.0 }\n\
         test_types::Tag(7)\n",
    )
    .expect("fixture parses");
    let node = root_named(&ast, "Node");

    let patch_entity = ast
        .find_patch_by_type_path(node, TRANSFORM)
        .expect("Transform patch exists before removal");

    ast.remove_component_patch(node, TRANSFORM);

    assert!(
        ast.find_patch_by_type_path(node, TRANSFORM).is_none(),
        "the patch is gone from the node's patch list"
    );
    assert_eq!(
        ast.component_type_paths(node),
        vec![TAG.to_string()],
        "only the untouched component remains"
    );
    assert!(
        ast.world.get_entity(patch_entity).is_err(),
        "the patch entity is despawned from the document world"
    );
    assert_eq!(ast.get_name(node), Some("Node"), "the name patch survives");
}

#[test]
fn remove_component_patch_for_absent_type_is_a_noop() {
    let mut ast = parse_bsn_text("#Node\ntest_types::Tag(7)\n").expect("fixture parses");
    let node = root_named(&ast, "Node");

    ast.remove_component_patch(node, "test_types::NotHere");

    assert_eq!(ast.component_type_paths(node), vec![TAG.to_string()]);
}

// ---------------------------------------------------------------------------
// move_to_parent
// ---------------------------------------------------------------------------

/// Root, with children A (holding grandchild G) and B, plus a second root R2.
fn reparent_fixture() -> SceneBsnAst {
    parse_bsn_text(
        "bevy_ecs::hierarchy::Children [\n\
             #Root\n\
             bevy_ecs::hierarchy::Children [\n\
                 #A\n\
                 bevy_ecs::hierarchy::Children [ #G ]\n\
                 ,\n\
                 #B\n\
             ]\n\
             ,\n\
             #R2\n\
         ]\n",
    )
    .expect("fixture parses")
}

#[test]
fn move_to_parent_between_two_parents() {
    let mut ast = reparent_fixture();
    let root = root_named(&ast, "Root");
    let a = child_named(&ast, root, "A");
    let b = child_named(&ast, root, "B");
    let g = child_named(&ast, a, "G");

    ast.move_to_parent(g, Some(a), Some(b));

    assert!(
        ast.get_children_ast(a).is_empty(),
        "old parent loses the child"
    );
    assert_eq!(
        ast.get_children_ast(b),
        vec![g],
        "new parent gains the child"
    );
    assert_eq!(ast.ast_parent_of(g), Some(b));
}

#[test]
fn move_to_parent_from_roots_to_a_child_slot() {
    let mut ast = reparent_fixture();
    let root = root_named(&ast, "Root");
    let r2 = root_named(&ast, "R2");

    ast.move_to_parent(r2, None, Some(root));

    assert!(!ast.roots.contains(&r2), "the node left the root list");
    assert_eq!(ast.ast_parent_of(r2), Some(root));
    let children = ast.get_children_ast(root);
    assert_eq!(
        children.last().copied(),
        Some(r2),
        "the node is appended to the new parent's children"
    );
    assert_eq!(children.len(), 3);
}

#[test]
fn move_to_parent_from_a_child_slot_to_roots() {
    let mut ast = reparent_fixture();
    let root = root_named(&ast, "Root");
    let b = child_named(&ast, root, "B");

    ast.move_to_parent(b, Some(root), None);

    assert!(ast.roots.contains(&b), "the node joined the root list");
    assert_eq!(ast.ast_parent_of(b), None);
    assert!(
        !ast.get_children_ast(root).contains(&b),
        "the old parent no longer lists the node"
    );
}

// ---------------------------------------------------------------------------
// deep_clone
// ---------------------------------------------------------------------------

/// Two roots with component values, a nested child, and ECS links.
fn clone_fixture() -> (SceneBsnAst, Entity, Entity) {
    let mut ast = parse_bsn_text(
        "bevy_ecs::hierarchy::Children [\n\
             #Root\n\
             test_types::Transform { translation: glam::Vec3 { x: 1.0, y: 2.0, z: 3.0 } }\n\
             test_types::Tag(7, \"label\")\n\
             bevy_ecs::hierarchy::Children [\n\
                 #Child\n\
                 test_types::Material { base: \"#Mat0\", strength: 0.5 }\n\
             ]\n\
             ,\n\
             #Sibling\n\
             test_types::Transform { translation: glam::Vec3 { x: 9.0 } }\n\
         ]\n",
    )
    .expect("fixture parses");

    // Link ECS entities (from a separate world) to the root and its child.
    let mut ecs = World::new();
    let ecs_root = ecs.spawn_empty().id();
    let ecs_child = ecs.spawn_empty().id();
    let root = root_named(&ast, "Root");
    let child = child_named(&ast, root, "Child");
    ast.link(ecs_root, root);
    ast.link(ecs_child, child);
    (ast, ecs_root, ecs_child)
}

#[test]
fn deep_clone_preserves_values_names_hierarchy_and_links() {
    let (src, ecs_root, ecs_child) = clone_fixture();
    let src_root = root_named(&src, "Root");
    let src_child = child_named(&src, src_root, "Child");

    let clone = src.deep_clone();

    assert_eq!(clone.roots.len(), src.roots.len());
    let clone_root = root_named(&clone, "Root");
    let clone_child = child_named(&clone, clone_root, "Child");
    root_named(&clone, "Sibling");

    // Component values compare deeply equal, patch by patch.
    for (tp, src_node, clone_node) in [
        (TRANSFORM, src_root, clone_root),
        (TAG, src_root, clone_root),
        (MATERIAL, src_child, clone_child),
    ] {
        let src_value = get_bsn_field(&src, src_node, tp, "").expect("source value");
        let clone_value = get_bsn_field(&clone, clone_node, tp, "").expect("clone value");
        assert_eq!(clone_value, src_value, "clone value for {tp}");
    }

    // ECS links carry over, re-keyed to the clone's node entities.
    assert_eq!(clone.ast_for(ecs_root), Some(clone_root));
    assert_eq!(clone.ast_for(ecs_child), Some(clone_child));
    assert_eq!(clone.ecs_for_ast(clone_root), Some(ecs_root));
    assert_eq!(clone.ecs_for_ast(clone_child), Some(ecs_child));
}

#[test]
fn mutating_a_deep_clone_leaves_the_source_untouched() {
    let (src, _, _) = clone_fixture();
    let src_root = root_named(&src, "Root");
    let before = get_bsn_field(&src, src_root, TRANSFORM, "").expect("source value");

    let mut clone = src.deep_clone();
    let clone_root = root_named(&clone, "Root");

    // Remove one component and overwrite another on the clone.
    clone.remove_component_patch(clone_root, TAG);
    let transform_patch = clone
        .find_patch_by_type_path(clone_root, TRANSFORM)
        .expect("clone Transform patch");
    clone.set_patch(transform_patch, BsnPatch::Type(TRANSFORM.to_string()));

    assert!(
        src.find_patch_by_type_path(src_root, TAG).is_some(),
        "removing a clone patch must not touch the source"
    );
    let after = get_bsn_field(&src, src_root, TRANSFORM, "").expect("source value");
    assert_eq!(
        after, before,
        "overwriting a clone patch must not touch the source"
    );
}

// ---------------------------------------------------------------------------
// clone_node_into
// ---------------------------------------------------------------------------

#[test]
fn clone_node_into_preserves_nested_and_handle_bearing_values() {
    let src = parse_bsn_text(
        "#Node\n\
         test_types::Transform { translation: glam::Vec3 { x: 1.0, y: 2.0, z: 3.0 } }\n\
         test_types::Material { base: \"#Mat0\", strength: 0.5 }\n\
         test_types::Tag(7, \"label\")\n",
    )
    .expect("fixture parses");
    let src_node = root_named(&src, "Node");

    let mut dst = SceneBsnAst::default();
    let dst_root = dst.create_entity_node(vec![BsnPatch::Name("Target".to_string())]);
    dst.add_to_roots(dst_root);

    let cloned = clone_node_into(&mut dst, &src, src_node, dst_root);

    assert_eq!(dst.get_children_ast(dst_root), vec![cloned]);
    assert_eq!(dst.get_name(cloned), Some("Node"));

    // Nested struct values (Transform), the handle reference string carried
    // by the Material patch, and tuple values all compare deeply equal.
    for tp in [TRANSFORM, MATERIAL, TAG] {
        let src_value = get_bsn_field(&src, src_node, tp, "").expect("source value");
        let dst_value = get_bsn_field(&dst, cloned, tp, "").expect("cloned value");
        assert_eq!(dst_value, src_value, "cloned value for {tp}");
    }
    assert_eq!(
        get_bsn_field(&dst, cloned, MATERIAL, "base"),
        Some(BsnValue::String("#Mat0".to_string())),
        "the handle reference string survives the copy verbatim"
    );
}

// ---------------------------------------------------------------------------
// Stable-id lookups
// ---------------------------------------------------------------------------

#[test]
fn stable_id_lookup_for_an_absent_id_is_none() {
    let ast = parse_bsn_text(
        "#Node\n\
         jackdaw_scene_types::node_id::SceneNodeId(11)\n",
    )
    .expect("fixture parses");

    assert!(ast.node_by_stable_id(999).is_none());
    assert!(ast.entity_for_stable_id(999).is_none());
}

/// An id that resolves to a node without an ECS link still has no entity.
#[test]
fn entity_for_stable_id_without_a_link_is_none() {
    let ast = parse_bsn_text(
        "#Node\n\
         jackdaw_scene_types::node_id::SceneNodeId(11)\n",
    )
    .expect("fixture parses");

    assert!(ast.node_by_stable_id(11).is_some());
    assert!(ast.entity_for_stable_id(11).is_none());
}

/// Two roots carrying the same stable id: the traversal walks the root list
/// as a stack, so the LAST root in document order wins. Stable ids are minted
/// unique by the editor; this pins the current tie-break for documents that
/// violate that invariant rather than endorsing it.
#[test]
fn duplicate_stable_ids_resolve_to_the_last_root_in_document_order() {
    let ast = parse_bsn_text(
        "bevy_ecs::hierarchy::Children [\n\
             #First\n\
             jackdaw_scene_types::node_id::SceneNodeId(7)\n\
             ,\n\
             #Second\n\
             jackdaw_scene_types::node_id::SceneNodeId(7)\n\
         ]\n",
    )
    .expect("fixture parses");

    let winner = ast.node_by_stable_id(7).expect("a node is found");
    assert_eq!(ast.get_name(winner), Some("Second"));
}
