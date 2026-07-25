//! Reader, editor document, and writer for the `.bsn` scene format.
//!
//! The parser builds the editor document ([`SceneBsnAst`]) directly from
//! `.bsn` source text; there is no separate parse-time representation. The
//! apply path resolves the document to ECS components, and the emitter
//! writes the document back to `.bsn` text. The grammar rules track the
//! dynamic-BSN work in bevyengine/bevy#23576.

pub mod apply;
pub mod catalog;
pub mod delta;
pub mod document;
pub mod emitter;
pub mod loader;
pub mod parse;
pub mod sync;
pub mod writer;

pub use catalog::{
    CatalogAssetRef, CatalogEntry, LoadedBsnScene, append_assets_to_ast, load_bsn_assets,
    load_bsn_scene, serialize_assets_to_bsn,
};

pub use parse::{ParseError, parse_bsn};

pub use delta::{apply_deltas, bsn_value_eq, shallow_diff};

pub use document::{
    AstNodeRef, BsnAssetContext, BsnField, BsnPatch, BsnPatches, BsnStructData, BsnStructFields,
    BsnTupleStructData, BsnValue, DerivedComponents, SceneBsnAst, bsn_value_as_int,
    clone_node_into, component_to_bsn_patch, component_to_bsn_patch_with_assets,
};
pub use emitter::{emit_entities, emit_entity, emit_scene};
pub use loader::{BsnLoadError, parse_bsn_text};

pub use apply::{
    AstDirty, BsnApplyAssets, BsnSceneAssets, apply_ast_to_ecs, apply_component_patch,
    apply_dirty_ast_patches, bsn_value_to_reflect, get_bsn_field, remove_bsn_field, set_bsn_field,
    spawn_from_ast,
};

pub use sync::{
    create_entity_in_ast, delete_entity_from_ast, sync_hierarchy_to_ast, sync_hierarchy_to_ast_at,
    sync_to_ast,
};

pub use writer::{
    BsnWriterConfig, append_world_to_ast, serialize_to_bsn, serialize_to_bsn_with_config,
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
