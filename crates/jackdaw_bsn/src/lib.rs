//! Reader for the `.bsn` scene format.
//!
//! This crate provides the parser front-end (turning `.bsn` source text into a
//! self-contained parser AST), the editor document model, the loader that
//! adapts a parsed AST into it, the apply path that resolves the document AST
//! to ECS components, and the emitter that writes the document AST back to
//! `.bsn` text.
//!
//! The parser and document layers both name some types `BsnPatch`, `BsnField`,
//! and `BsnPatches`. The document (editor-facing) versions are re-exported at
//! the crate root; the parser versions stay under [`crate::parse`].

pub mod apply;
pub mod catalog;
pub mod delta;
pub mod document;
pub mod emitter;
pub mod loader;
pub mod parse;
pub mod sync;

pub use catalog::{
    CatalogAssetRef, CatalogEntry, LoadedBsnScene, append_assets_to_ast, load_bsn_assets,
    load_bsn_scene, serialize_assets_to_bsn,
};

pub use parse::{
    BsnAst, BsnExpr, BsnNameStore, BsnNamedTuple, BsnRelation, BsnRoot, BsnStruct, BsnSymbol,
    BsnVar, ParseError, parse_bsn,
};

pub use delta::{apply_deltas, bsn_value_eq, shallow_diff};

pub use document::{
    AstNodeRef, BsnAssetContext, BsnField, BsnPatch, BsnPatches, BsnStructData, BsnStructFields,
    BsnTupleStructData, BsnValue, DerivedComponents, SceneBsnAst, clone_node_into,
    component_to_bsn_patch, component_to_bsn_patch_with_assets,
};
pub use emitter::{emit_entities, emit_entity, emit_scene};
pub use loader::{BsnLoadError, parse_bsn_text};

pub use apply::{
    AstDirty, BsnApplyAssets, BsnSceneAssets, apply_ast_to_ecs, apply_component_patch,
    apply_dirty_ast_patches, bsn_value_to_reflect, get_bsn_field, parse_string_to_bsn_value,
    set_bsn_field, spawn_from_ast,
};

pub use sync::{
    add_component_to_ast, create_entity_in_ast, delete_entity_from_ast, ensure_ast_node,
    remove_component_from_ast, sync_hierarchy_to_ast, sync_to_ast,
};

use bevy::prelude::*;

/// Registers the BSN scene AST resource for the editor.
///
/// The apply path ([`apply_dirty_ast_patches`]) is called explicitly during
/// scene load, so it is deliberately not registered as a per-frame system.
pub struct JackdawBsnPlugin;

impl Plugin for JackdawBsnPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SceneBsnAst>();
    }
}
