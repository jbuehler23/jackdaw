//! Read-only queries over the document: node lookups, hierarchy walks,
//! and component-patch searches.

use bevy::ecs::entity::Entity;

use super::{BsnPatch, BsnPatches, BsnValue, DerivedComponents, SceneBsnAst};

/// Check if `stored_path` is an enum variant of `base_path`.
/// e.g. `foo::Bar::Sphere` is a variant of `foo::Bar`.
fn is_enum_variant_of(stored_path: &str, base_path: &str) -> bool {
    stored_path.starts_with(base_path)
        && stored_path.as_bytes().get(base_path.len()) == Some(&b':')
        && stored_path[base_path.len()..].starts_with("::")
        && !stored_path[base_path.len() + 2..].contains("::")
}

impl SceneBsnAst {
    /// Get the AST entity for an ECS entity.
    pub fn ast_for(&self, ecs_entity: Entity) -> Option<Entity> {
        self.ecs_to_ast.get(&ecs_entity).copied()
    }

    /// Get the ECS entity for an AST patches entity.
    pub fn ecs_for_ast(&self, ast_entity: Entity) -> Option<Entity> {
        self.ast_to_ecs.get(&ast_entity).copied()
    }

    /// Get the patches for an AST entity.
    pub fn get_patches(&self, patches_entity: Entity) -> Option<&BsnPatches> {
        self.world.get::<BsnPatches>(patches_entity)
    }

    /// Get a specific patch component.
    pub fn get_patch(&self, patch_entity: Entity) -> Option<&BsnPatch> {
        self.world.get::<BsnPatch>(patch_entity)
    }

    /// The component type paths authored on `patches_entity`. Skips the
    /// Children relation, base inheritance, and name references, which are
    /// not components.
    pub fn component_type_paths(&self, patches_entity: Entity) -> Vec<String> {
        let Some(patches) = self.get_patches(patches_entity) else {
            return Vec::new();
        };
        patches
            .0
            .iter()
            .filter_map(|&patch_entity| {
                self.get_patch(patch_entity).and_then(|patch| match patch {
                    BsnPatch::Type(tp) | BsnPatch::Template(tp, _) => Some(tp.clone()),
                    BsnPatch::Struct(data) => Some(data.type_path.clone()),
                    BsnPatch::TupleStruct(data) => Some(data.type_path.clone()),
                    _ => None,
                })
            })
            .collect()
    }

    /// Get the [`BsnPatch::Name`] value for an AST entity, if present.
    pub fn get_name(&self, patches_entity: Entity) -> Option<&str> {
        let patches = self.get_patches(patches_entity)?;
        for &pe in &patches.0 {
            if let Some(BsnPatch::Name(name)) = self.get_patch(pe) {
                return Some(name.as_str());
            }
        }
        None
    }

    /// Whether `type_path` is marked derived (computed, not authored) on this
    /// node.
    pub fn is_derived(&self, patches_entity: Entity, type_path: &str) -> bool {
        self.world
            .get::<DerivedComponents>(patches_entity)
            .is_some_and(|d| d.0.contains(type_path))
    }

    /// The stable node id carried by a document node's `SceneNodeId(id)`
    /// tuple-struct patch, if the node has one.
    pub fn stable_id_of(&self, patches_entity: Entity) -> Option<u64> {
        let patches = self.get_patches(patches_entity)?;
        for &pe in &patches.0 {
            if let Some(BsnPatch::TupleStruct(data)) = self.get_patch(pe)
                && data.type_path.ends_with("SceneNodeId")
                && let Some(BsnValue::Int(v)) = data.values.first()
            {
                return u64::try_from(*v).ok();
            }
        }
        None
    }

    /// Find the document node carrying the given stable node id, i.e. a
    /// `SceneNodeId(id)` tuple-struct patch. Linear over nodes; the stable id
    /// is the cross-process identity used by the play-in-editor mapping.
    pub fn node_by_stable_id(&self, id: u64) -> Option<Entity> {
        let mut found = None;
        let mut nodes: Vec<Entity> = self.roots.clone();
        while let Some(node) = nodes.pop() {
            if let Some(patches) = self.get_patches(node) {
                for &pe in &patches.0 {
                    match self.get_patch(pe) {
                        Some(BsnPatch::TupleStruct(data))
                            if data.type_path.ends_with("SceneNodeId")
                                && matches!(
                                    data.values.first(),
                                    Some(BsnValue::Int(v)) if *v == i128::from(id)
                                ) =>
                        {
                            found = Some(node);
                        }
                        Some(BsnPatch::Children(children)) => {
                            nodes.extend(children.iter().copied());
                        }
                        _ => {}
                    }
                }
            }
            if found.is_some() {
                break;
            }
        }
        found
    }

    /// The live ECS entity for the node carrying the given stable node id.
    pub fn entity_for_stable_id(&self, id: u64) -> Option<Entity> {
        self.node_by_stable_id(id)
            .and_then(|node| self.ecs_for_ast(node))
    }

    /// Get child AST entities from [`BsnPatch::Children`], if present.
    pub fn get_children_ast(&self, patches_entity: Entity) -> Vec<Entity> {
        let Some(patches) = self.get_patches(patches_entity) else {
            return Vec::new();
        };
        for &pe in &patches.0 {
            if let Some(BsnPatch::Children(children)) = self.get_patch(pe) {
                return children.clone();
            }
        }
        Vec::new()
    }

    /// Find the patch of a given type within an entity's patches list.
    /// Returns the patch entity.
    pub fn find_patch_by_type_path(
        &self,
        patches_entity: Entity,
        type_path: &str,
    ) -> Option<Entity> {
        let patches = self.get_patches(patches_entity)?;
        for &patch_entity in &patches.0 {
            if let Some(patch) = self.get_patch(patch_entity) {
                let matches = match patch {
                    BsnPatch::Type(tp) => tp == type_path || is_enum_variant_of(tp, type_path),
                    BsnPatch::Struct(data) => {
                        data.type_path == type_path
                            || is_enum_variant_of(&data.type_path, type_path)
                    }
                    BsnPatch::TupleStruct(data) => {
                        data.type_path == type_path
                            || is_enum_variant_of(&data.type_path, type_path)
                    }
                    BsnPatch::Template(tp, _) => tp == type_path,
                    _ => false,
                };
                if matches {
                    return Some(patch_entity);
                }
            }
        }
        None
    }

    /// Find which AST entity contains `child_ast` in a Children patch.
    /// Returns `None` if `child_ast` is a root (or not found).
    pub(super) fn find_ast_parent_of(&self, child_ast: Entity) -> Option<Entity> {
        if self.roots.contains(&child_ast) {
            return None;
        }
        for &root in &self.roots {
            if let Some(parent) = self.find_parent_in_subtree(root, child_ast) {
                return Some(parent);
            }
        }
        None
    }

    fn find_parent_in_subtree(&self, current: Entity, target: Entity) -> Option<Entity> {
        let patches = self.get_patches(current)?;
        for &patch_entity in &patches.0 {
            if let Some(BsnPatch::Children(children)) = self.get_patch(patch_entity) {
                if children.contains(&target) {
                    return Some(current);
                }
                for &child in children {
                    if let Some(parent) = self.find_parent_in_subtree(child, target) {
                        return Some(parent);
                    }
                }
            }
        }
        None
    }

    /// Every AST node (over all roots and their descendants) that carries a
    /// component patch of `type_path`. The match honours enum-variant type
    /// paths the same way [`find_patch_by_type_path`](Self::find_patch_by_type_path)
    /// does. Nodes are returned in pre-order (each root before its descendants).
    /// Returns an empty vector when no node carries the component.
    pub fn entities_with_component(&self, type_path: &str) -> Vec<Entity> {
        let mut out = Vec::new();
        for &root in &self.roots {
            if self.find_patch_by_type_path(root, type_path).is_some() {
                out.push(root);
            }
            for descendant in self.descendants_of(root) {
                if self
                    .find_patch_by_type_path(descendant, type_path)
                    .is_some()
                {
                    out.push(descendant);
                }
            }
        }
        out
    }

    /// All AST descendants of `root_ast`, excluding `root_ast` itself. Walks the
    /// [`BsnPatch::Children`] relation recursively via
    /// [`get_children_ast`](Self::get_children_ast). Returns an empty vector when
    /// the node has no children.
    pub fn descendants_of(&self, root_ast: Entity) -> Vec<Entity> {
        let mut out = Vec::new();
        let mut stack = vec![root_ast];
        while let Some(current) = stack.pop() {
            for child in self.get_children_ast(current) {
                out.push(child);
                stack.push(child);
            }
        }
        out
    }

    /// The parent AST node that lists `ast` in its [`BsnPatch::Children`], or
    /// `None` when `ast` is a root (or is not present in the document). Public
    /// wrapper over the internal parentage walk.
    pub fn ast_parent_of(&self, ast: Entity) -> Option<Entity> {
        self.find_ast_parent_of(ast)
    }

    /// The nearest ancestor of `ast` (inclusive of `ast` itself) that carries a
    /// component patch of `type_path`, walking parentage upward via
    /// [`ast_parent_of`](Self::ast_parent_of). Returns `ast` when `ast` itself
    /// has the component, or `None` when no node on the chain carries it.
    pub fn ancestor_with_component(&self, ast: Entity, type_path: &str) -> Option<Entity> {
        let mut current = ast;
        loop {
            if self.find_patch_by_type_path(current, type_path).is_some() {
                return Some(current);
            }
            current = self.ast_parent_of(current)?;
        }
    }

    /// The first AST node (over all roots and their descendants, in pre-order)
    /// whose `type_path` component reads as a single integer equal to `value`.
    /// The component is read whole via `get_bsn_field(node, type_path, "")` and
    /// accepts either a bare integer scalar or a tuple struct wrapping exactly
    /// one integer, which is how a `u32`-newtype marker such as a prefab entity
    /// id serialises. Returns `None` when no node matches.
    pub fn find_node_by_component_int(&self, type_path: &str, value: u64) -> Option<Entity> {
        let mut nodes: Vec<Entity> = Vec::new();
        for &root in &self.roots {
            nodes.push(root);
            nodes.extend(self.descendants_of(root));
        }
        for node in nodes {
            let Some(whole) = crate::apply::get_bsn_field(self, node, type_path, "") else {
                continue;
            };
            if bsn_value_as_int(&whole) == Some(i128::from(value)) {
                return Some(node);
            }
        }
        None
    }
}

/// The integer a whole-component [`BsnValue`] represents, when it is either a
/// bare integer scalar or a tuple struct wrapping exactly one integer scalar.
/// `None` for every other shape.
pub fn bsn_value_as_int(value: &BsnValue) -> Option<i128> {
    match value {
        BsnValue::Int(v) => Some(*v),
        BsnValue::TupleStruct(data) => match data.values.as_slice() {
            [BsnValue::Int(v)] => Some(*v),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod query_tests {
    use super::*;
    use crate::document::{BsnTupleStructData, clone_node_into};

    const TRANSFORM: &str = "test::Transform";
    const MESH: &str = "test::Mesh";
    const PREFAB_ID: &str = "test::PrefabEntityId";

    fn prefab_id_patch(id: i128) -> BsnPatch {
        BsnPatch::TupleStruct(BsnTupleStructData {
            type_path: PREFAB_ID.to_string(),
            values: vec![BsnValue::Int(id)],
        })
    }

    /// Builds this tree, returning the node entities in a fixed order:
    ///
    /// ```text
    /// root       [Transform, PrefabEntityId(0)]
    ///   child_a  [Mesh,      PrefabEntityId(1)]
    ///     grand  [Transform, PrefabEntityId(2)]
    ///   child_b  [Transform, PrefabEntityId(3)]
    /// ```
    struct Tree {
        ast: SceneBsnAst,
        root: Entity,
        child_a: Entity,
        grand: Entity,
        child_b: Entity,
    }

    fn build_tree() -> Tree {
        let mut ast = SceneBsnAst::default();

        let root = ast.create_entity_node(vec![
            BsnPatch::Type(TRANSFORM.to_string()),
            prefab_id_patch(0),
        ]);
        let child_a =
            ast.create_entity_node(vec![BsnPatch::Type(MESH.to_string()), prefab_id_patch(1)]);
        let grand = ast.create_entity_node(vec![
            BsnPatch::Type(TRANSFORM.to_string()),
            prefab_id_patch(2),
        ]);
        let child_b = ast.create_entity_node(vec![
            BsnPatch::Type(TRANSFORM.to_string()),
            prefab_id_patch(3),
        ]);

        ast.add_to_roots(root);
        ast.add_child_to_ast(root, child_a);
        ast.add_child_to_ast(root, child_b);
        ast.add_child_to_ast(child_a, grand);

        Tree {
            ast,
            root,
            child_a,
            grand,
            child_b,
        }
    }

    #[test]
    fn entities_with_component_spans_all_depths() {
        let t = build_tree();
        let mut found = t.ast.entities_with_component(TRANSFORM);
        found.sort();
        let mut expected = vec![t.root, t.grand, t.child_b];
        expected.sort();
        assert_eq!(found, expected);

        assert_eq!(t.ast.entities_with_component(MESH), vec![t.child_a]);
        assert!(t.ast.entities_with_component("test::Absent").is_empty());
    }

    #[test]
    fn descendants_exclude_root() {
        let t = build_tree();
        let mut descendants = t.ast.descendants_of(t.root);
        descendants.sort();
        let mut expected = vec![t.child_a, t.child_b, t.grand];
        expected.sort();
        assert_eq!(descendants, expected);
        assert!(!descendants.contains(&t.root));

        assert!(t.ast.descendants_of(t.grand).is_empty());
    }

    #[test]
    fn ast_parent_of_walks_children_relation() {
        let t = build_tree();
        assert_eq!(t.ast.ast_parent_of(t.root), None);
        assert_eq!(t.ast.ast_parent_of(t.child_a), Some(t.root));
        assert_eq!(t.ast.ast_parent_of(t.child_b), Some(t.root));
        assert_eq!(t.ast.ast_parent_of(t.grand), Some(t.child_a));
    }

    #[test]
    fn ancestor_with_component_is_inclusive_of_self() {
        let t = build_tree();
        // The node itself carries Transform, so it is its own nearest match.
        assert_eq!(
            t.ast.ancestor_with_component(t.grand, TRANSFORM),
            Some(t.grand)
        );
        // Mesh lives only on child_a, an ancestor of grand.
        assert_eq!(
            t.ast.ancestor_with_component(t.grand, MESH),
            Some(t.child_a)
        );
        // No node on child_b's chain carries Mesh.
        assert_eq!(t.ast.ancestor_with_component(t.child_b, MESH), None);
    }

    #[test]
    fn find_node_by_component_int_matches_tuple_struct_newtype() {
        let t = build_tree();
        assert_eq!(
            t.ast.find_node_by_component_int(PREFAB_ID, 2),
            Some(t.grand)
        );
        assert_eq!(t.ast.find_node_by_component_int(PREFAB_ID, 0), Some(t.root));
        assert_eq!(t.ast.find_node_by_component_int(PREFAB_ID, 99), None);
    }

    #[test]
    fn clone_node_into_copies_components_but_not_children() {
        let src = build_tree();

        let mut dst = SceneBsnAst::default();
        let dst_root = dst.create_entity_node(vec![BsnPatch::Type("test::Root".to_string())]);
        dst.add_to_roots(dst_root);

        // child_a has a Mesh + PrefabEntityId patch and one child (grand).
        let cloned = clone_node_into(&mut dst, &src.ast, src.child_a, dst_root);

        assert_eq!(dst.get_children_ast(dst_root), vec![cloned]);

        let mut components = dst.component_type_paths(cloned);
        components.sort();
        let mut expected = vec![MESH.to_string(), PREFAB_ID.to_string()];
        expected.sort();
        assert_eq!(components, expected);

        // The single-node clone must not drag the source's children across.
        assert!(dst.get_children_ast(cloned).is_empty());
    }
}
