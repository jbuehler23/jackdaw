//! Document mutation: node creation, hierarchy edits, patch writes, and
//! deep cloning.

use bevy::ecs::entity::Entity;

use super::{BsnPatch, BsnPatches, SceneBsnAst};

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

    /// Remove a child from a parent's Children patch.
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
                return;
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
            self.despawn_recursive(child);
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

    /// Insert a child into a parent's ordered Children patch.
    pub fn insert_child_in_ast(&mut self, parent_ast: Entity, child_ast: Entity, index: usize) {
        let Some(patches) = self.get_patches(parent_ast) else {
            return;
        };
        let patch_ids: Vec<Entity> = patches.0.clone();

        for &patch_entity in &patch_ids {
            if let Some(patch) = self.world.get_mut::<BsnPatch>(patch_entity)
                && let BsnPatch::Children(children) = patch.into_inner()
            {
                let index = index.min(children.len());
                children.insert(index, child_ast);
                return;
            }
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
pub fn clone_subtree_into(
    dst: &mut SceneBsnAst,
    src: &SceneBsnAst,
    src_node: Entity,
    dst_parent: Option<Entity>,
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
    for child in src.get_children_ast(src_node) {
        clone_subtree_into(dst, src, child, Some(new_node));
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
