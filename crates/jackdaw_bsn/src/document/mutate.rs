//! Document mutation: node creation, hierarchy edits, patch writes, and
//! deep cloning.

use bevy::ecs::entity::Entity;

use super::{BsnPatch, BsnPatches, SceneBsnAst};

/// How deep a document walk follows `Children` before it gives up.
///
/// A clipboard payload whose `Children` lists form a cycle would otherwise
/// recurse until the stack ran out. Hitting the cap is a refusal, logged and
/// abandoned, never a partial walk that carries on.
pub const MAX_AST_DEPTH: usize = 256;

impl SceneBsnAst {
    /// Create a new AST node for an entity with the given patches.
    pub fn create_entity_node(&mut self, patches: Vec<BsnPatch>) -> Entity {
        let patch_entities: Vec<Entity> = patches
            .into_iter()
            .map(|patch| self.world.spawn(patch).id())
            .collect();
        self.world.spawn(BsnPatches(patch_entities)).id()
    }

    /// Add a patches entity to the root list.
    pub fn add_to_roots(&mut self, patches_entity: Entity) {
        self.roots.push(patches_entity);
    }

    /// Remove a patches entity from the root list.
    pub fn remove_from_roots(&mut self, patches_entity: Entity) {
        self.roots.retain(|&e| e != patches_entity);
    }

    /// Register an ECS entity to AST node mapping (both directions).
    pub fn link(&mut self, ecs_entity: Entity, ast_entity: Entity) {
        self.ecs_to_ast.insert(ecs_entity, ast_entity);
        self.ast_to_ecs.insert(ast_entity, ecs_entity);
    }

    /// Remove an ECS entity mapping.
    pub fn unlink(&mut self, ecs_entity: Entity) {
        if let Some(ast_entity) = self.ecs_to_ast.remove(&ecs_entity) {
            self.ast_to_ecs.remove(&ast_entity);
        }
    }

    /// Remove the mapping for an AST node, clearing both directions. The
    /// counterpart to [`unlink`](Self::unlink) keyed by the AST node so a
    /// subtree teardown can drop every descendant's link, not just the top.
    fn unlink_ast(&mut self, ast_entity: Entity) {
        if let Some(ecs_entity) = self.ast_to_ecs.remove(&ast_entity) {
            self.ecs_to_ast.remove(&ecs_entity);
        }
    }

    /// Get a mutable reference to patches for an AST entity.
    pub fn get_patches_mut(&mut self, patches_entity: Entity) -> Option<&mut BsnPatches> {
        self.world
            .get_mut::<BsnPatches>(patches_entity)
            .map(bevy::ecs::change_detection::Mut::into_inner)
    }

    /// Remove the component patch for `type_path` from a node (the undo of
    /// authoring a component that did not exist before). The patch entity is
    /// despawned and dropped from the node's patch list.
    pub fn remove_component_patch(&mut self, patches_entity: Entity, type_path: &str) {
        let Some(patch_entity) = self.find_patch_by_type_path(patches_entity, type_path) else {
            return;
        };
        if let Some(patches) = self.get_patches_mut(patches_entity) {
            patches.0.retain(|&pe| pe != patch_entity);
        }
        self.world.despawn(patch_entity);
    }

    /// Replace a patch component on an existing entity.
    pub fn set_patch(&mut self, patch_entity: Entity, patch: BsnPatch) {
        if let Ok(mut entity_mut) = self.world.get_entity_mut(patch_entity) {
            entity_mut.insert(patch);
        }
    }

    /// Move an AST node from one parent's Children to another.
    pub fn move_to_parent(
        &mut self,
        node: Entity,
        old_parent: Option<Entity>,
        new_parent: Option<Entity>,
    ) {
        self.move_to_parent_at(node, old_parent, new_parent, usize::MAX);
    }

    /// Move an AST node to an exact position in a parent's ordered child list.
    ///
    /// `index` is clamped to the destination length. [`usize::MAX`] therefore
    /// means "append" and preserves the behavior of [`Self::move_to_parent`].
    pub fn move_to_parent_at(
        &mut self,
        node: Entity,
        old_parent: Option<Entity>,
        new_parent: Option<Entity>,
        index: usize,
    ) {
        if let Some(old_parent_ast) = old_parent {
            self.remove_child_from_ast(old_parent_ast, node);
        } else {
            self.remove_from_roots(node);
        }

        if let Some(new_parent_ast) = new_parent {
            self.insert_child_in_ast(new_parent_ast, node, index);
        } else {
            let index = index.min(self.roots.len());
            self.roots.insert(index, node);
        }
    }

    /// Removes a child from every `Children` patch its parent carries, not just
    /// the first, or a move out of a parent with two lists duplicates it.
    pub fn remove_child_from_ast(&mut self, parent_ast: Entity, child_ast: Entity) {
        let Some(patches) = self.get_patches(parent_ast) else {
            return;
        };
        let patch_ids: Vec<Entity> = patches.0.clone();

        for &patch_entity in &patch_ids {
            if let Some(patch) = self.world.get_mut::<BsnPatch>(patch_entity)
                && let BsnPatch::Children(children) = patch.into_inner()
            {
                children.retain(|&e| e != child_ast);
            }
        }
    }

    /// Remove an entity's AST node entirely: detach from parent (or roots),
    /// recursively despawn all AST sub-entities, and unlink the ECS mapping for
    /// the node and every linked descendant.
    ///
    /// No-ops gracefully if the entity is not in `ecs_to_ast`.
    pub fn remove_entity_node(&mut self, ecs_entity: Entity) {
        let Some(node_ast) = self.ecs_to_ast.get(&ecs_entity).copied() else {
            return;
        };

        if let Some(parent_ast) = self.find_ast_parent_of(node_ast) {
            self.remove_child_from_ast(parent_ast, node_ast);
        } else {
            self.remove_from_roots(node_ast);
        }

        self.despawn_recursive(node_ast);
    }

    /// Recursively despawn an AST node and all its children/patches, dropping
    /// the ECS link for every node torn down (not just the top) so no stale
    /// `ecs_to_ast`/`ast_to_ecs` entries survive for a linked descendant.
    fn despawn_recursive(&mut self, node: Entity) {
        self.despawn_recursive_to_depth(node, 0);
    }

    fn despawn_recursive_to_depth(&mut self, node: Entity, depth: usize) {
        if depth >= MAX_AST_DEPTH {
            log::warn!("document node {node} is deeper than {MAX_AST_DEPTH}; not torn down");
            return;
        }
        // Drop this node's link before its entity id is despawned and possibly
        // recycled for an unrelated AST node.
        self.unlink_ast(node);

        let children: Vec<Entity> = if let Some(patches) = self.get_patches(node) {
            patches
                .0
                .iter()
                .filter_map(|&pe| {
                    if let Some(BsnPatch::Children(child_list)) = self.get_patch(pe) {
                        Some(child_list.clone())
                    } else {
                        None
                    }
                })
                .flatten()
                .collect()
        } else {
            Vec::new()
        };

        for child in children {
            self.despawn_recursive_to_depth(child, depth + 1);
        }

        if let Some(patches) = self.get_patches(node) {
            let patch_ids: Vec<Entity> = patches.0.clone();
            for pe in patch_ids {
                if let Ok(em) = self.world.get_entity_mut(pe) {
                    em.despawn();
                }
            }
        }
        if let Ok(em) = self.world.get_entity_mut(node) {
            em.despawn();
        }
    }

    /// Add a child to a parent's Children patch (creating one if needed).
    pub fn add_child_to_ast(&mut self, parent_ast: Entity, child_ast: Entity) {
        self.insert_child_in_ast(parent_ast, child_ast, usize::MAX);
    }

    /// Inserts a child into a parent's ordered child list.
    ///
    /// `index` counts over every `Children` patch the parent carries, in the
    /// order [`Self::get_children_ast`] reports them.
    pub fn insert_child_in_ast(&mut self, parent_ast: Entity, child_ast: Entity, index: usize) {
        let Some(patches) = self.get_patches(parent_ast) else {
            return;
        };
        let patch_ids: Vec<Entity> = patches.0.clone();

        let mut remaining = index;
        let mut last_list: Option<Entity> = None;
        for &patch_entity in &patch_ids {
            let Some(BsnPatch::Children(children)) = self.get_patch(patch_entity) else {
                continue;
            };
            let len = children.len();
            if remaining <= len {
                if let Some(patch) = self.world.get_mut::<BsnPatch>(patch_entity)
                    && let BsnPatch::Children(children) = patch.into_inner()
                {
                    children.insert(remaining.min(children.len()), child_ast);
                }
                return;
            }
            remaining -= len;
            last_list = Some(patch_entity);
        }

        if let Some(patch_entity) = last_list
            && let Some(patch) = self.world.get_mut::<BsnPatch>(patch_entity)
            && let BsnPatch::Children(children) = patch.into_inner()
        {
            children.push(child_ast);
            return;
        }

        let children_patch = self.world.spawn(BsnPatch::Children(vec![child_ast])).id();
        if let Some(patches) = self.get_patches_mut(parent_ast) {
            patches.0.push(children_patch);
        }
    }
}

impl SceneBsnAst {
    /// Deep-copy the whole document into a fresh one: every root and its
    /// subtree, all component patches, and the node-to-ECS links (re-keyed to
    /// the clone's node entities). The clone's node entities are new; only
    /// the linked ECS entities carry over. Useful for read-only emission or
    /// resolution passes that must not mutate the live document.
    pub fn deep_clone(&self) -> SceneBsnAst {
        let mut dst = SceneBsnAst::default();
        for &root in &self.roots {
            let new_root = clone_subtree_into(&mut dst, self, root, None);
            if let Some(ecs) = self.ecs_for_ast(root) {
                dst.link(ecs, new_root);
            }
            link_cloned_subtree(&mut dst, self, root, new_root);
        }
        dst
    }

    /// Clone `node`'s component patches into a fresh vector, dropping the
    /// [`BsnPatch::Children`] relation (callers rebuild the hierarchy
    /// separately). Name, base, and every component patch pass through.
    /// Returns an empty vector when `node` has no patches.
    pub fn cloned_component_patches(&self, node: Entity) -> Vec<BsnPatch> {
        match self.get_patches(node) {
            Some(patches) => patches
                .0
                .iter()
                .filter_map(|&pe| self.get_patch(pe))
                .filter(|patch| !matches!(patch, BsnPatch::Children(_)))
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    }
}

/// Graft `src_node`'s full subtree into `dst` under `dst_parent` (`None` = new root).
///
/// The walk stops at [`MAX_AST_DEPTH`], so a `src` whose `Children` lists form
/// a cycle costs a bounded graft and a warning rather than the stack.
pub fn clone_subtree_into(
    dst: &mut SceneBsnAst,
    src: &SceneBsnAst,
    src_node: Entity,
    dst_parent: Option<Entity>,
) -> Entity {
    clone_subtree_to_depth(dst, src, src_node, dst_parent, 0)
}

fn clone_subtree_to_depth(
    dst: &mut SceneBsnAst,
    src: &SceneBsnAst,
    src_node: Entity,
    dst_parent: Option<Entity>,
    depth: usize,
) -> Entity {
    let new_node = match dst_parent {
        Some(parent) => clone_node_into(dst, src, src_node, parent),
        None => {
            let patches = src.cloned_component_patches(src_node);
            let node = dst.create_entity_node(patches);
            dst.add_to_roots(node);
            node
        }
    };
    if depth >= MAX_AST_DEPTH {
        log::warn!(
            "document node {src_node} is deeper than {MAX_AST_DEPTH}; its children were not copied"
        );
        return new_node;
    }
    for child in src.get_children_ast(src_node) {
        clone_subtree_to_depth(dst, src, child, Some(new_node), depth + 1);
    }
    new_node
}

/// Create a new AST node in `dst` under `dst_parent`, deep-copying the component
/// patches of `src_node` from `src`.
pub fn clone_node_into(
    dst: &mut SceneBsnAst,
    src: &SceneBsnAst,
    src_node: Entity,
    dst_parent: Entity,
) -> Entity {
    let cloned_patches = src.cloned_component_patches(src_node);
    let new_node = dst.create_entity_node(cloned_patches);
    dst.add_child_to_ast(dst_parent, new_node);
    new_node
}

/// Copy ECS links from `src`'s subtree onto the already-cloned nodes in `dst`
fn link_cloned_subtree(
    dst: &mut SceneBsnAst,
    src: &SceneBsnAst,
    src_node: Entity,
    dst_node: Entity,
) {
    let src_children = src.get_children_ast(src_node);
    let dst_children = dst.get_children_ast(dst_node);
    for (src_child, dst_child) in src_children.into_iter().zip(dst_children) {
        if let Some(ecs) = src.ecs_for_ast(src_child) {
            dst.link(ecs, dst_child);
        }
        link_cloned_subtree(dst, src, src_child, dst_child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A parent whose children are split across two `Children` patches, which
    /// is what two `Children [ ... ]` relations parse to.
    fn parent_with_two_child_lists() -> (SceneBsnAst, Entity, Vec<Entity>) {
        let mut ast = SceneBsnAst::default();
        let kids: Vec<Entity> = (0..4)
            .map(|index| ast.create_entity_node(vec![BsnPatch::Name(format!("Child{index}"))]))
            .collect();
        let first = ast
            .world
            .spawn(BsnPatch::Children(kids[0..2].to_vec()))
            .id();
        let second = ast
            .world
            .spawn(BsnPatch::Children(kids[2..4].to_vec()))
            .id();
        let name = ast.world.spawn(BsnPatch::Name("Parent".to_string())).id();
        let parent = ast.world.spawn(BsnPatches(vec![name, first, second])).id();
        ast.add_to_roots(parent);
        (ast, parent, kids)
    }

    #[test]
    fn every_child_list_contributes_to_the_child_order() {
        let (ast, parent, kids) = parent_with_two_child_lists();
        assert_eq!(
            ast.get_children_ast(parent),
            kids,
            "reading only the first list hides half the children"
        );
    }

    #[test]
    fn a_child_in_the_second_list_can_be_removed() {
        let (mut ast, parent, kids) = parent_with_two_child_lists();
        ast.remove_child_from_ast(parent, kids[3]);
        assert_eq!(
            ast.get_children_ast(parent),
            vec![kids[0], kids[1], kids[2]]
        );
    }

    #[test]
    fn a_move_within_a_split_list_does_not_duplicate_the_child() {
        let (mut ast, parent, kids) = parent_with_two_child_lists();
        ast.move_to_parent_at(kids[3], Some(parent), Some(parent), 0);
        assert_eq!(
            ast.get_children_ast(parent),
            vec![kids[3], kids[0], kids[1], kids[2]],
            "the child moved to the front and is there exactly once"
        );
    }

    #[test]
    fn an_index_past_the_first_list_lands_in_the_second() {
        let (mut ast, parent, kids) = parent_with_two_child_lists();
        let extra = ast.create_entity_node(vec![BsnPatch::Name("Extra".to_string())]);
        ast.insert_child_in_ast(parent, extra, 3);
        assert_eq!(
            ast.get_children_ast(parent),
            vec![kids[0], kids[1], kids[2], extra, kids[3]],
            "the index counts over the concatenated list, not over one patch"
        );
    }

    /// A cycle cannot come out of the parser, but it can come off a clipboard.
    #[test]
    fn cloning_a_cyclic_document_ends_at_the_depth_cap() {
        let mut src = SceneBsnAst::default();
        let child = src.create_entity_node(vec![BsnPatch::Name("Child".to_string())]);
        let root = src.create_entity_node(vec![
            BsnPatch::Name("Root".to_string()),
            BsnPatch::Children(vec![child]),
        ]);
        src.add_to_roots(root);
        src.add_child_to_ast(child, root);

        let mut dst = SceneBsnAst::default();
        clone_subtree_into(&mut dst, &src, root, None);

        let mut depth = 0;
        let mut node = dst.roots[0];
        while let Some(&next) = dst.get_children_ast(node).first() {
            depth += 1;
            node = next;
            assert!(depth <= MAX_AST_DEPTH, "the clone did not stop at the cap");
        }
        assert_eq!(depth, MAX_AST_DEPTH, "the clone stopped exactly at the cap");
    }
}
