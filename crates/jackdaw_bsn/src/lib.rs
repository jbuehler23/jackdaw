//! Reader, editor document, and writer for the `.bsn` scene format.
//!
//! The parser builds the editor document ([`SceneBsnAst`]) directly from
//! `.bsn` source text; there is no separate parse-time representation. The
//! apply path resolves the document to ECS components, and the emitter
//! writes the document back to `.bsn` text. The grammar rules track the
//! dynamic-BSN work in bevyengine/bevy#23576.

pub mod apply;
pub mod binary;
pub mod catalog;
pub mod delta;
pub mod document;
pub mod emitter;
pub mod file;
pub mod header;
pub mod loader;
pub mod parse;
pub mod retired;
pub mod sync;
pub mod writer;

pub use catalog::{
    CatalogAssetRef, CatalogEntry, LoadedBsnScene, adopt_asset_roots, append_assets_to_ast,
    asset_roots, asset_value_from_root, entity_roots, is_asset_root, load_asset_root,
    load_bsn_assets, load_bsn_scene, serialize_assets_to_bsn, serialize_assets_to_bsn_reporting,
};

pub use header::{
    ASSET_HEADER, AssetFileError, PREFAB_TYPE, StemIndex, asset_file_type, asset_stem,
    asset_text_type, document_type_path, path_stem, read_asset_file, read_asset_header,
    root_type_path, walk_asset_files, walk_document_files, walk_files_with_extensions,
    with_asset_header,
};

pub use binary::{BinaryError, DecodedDocument, is_binary};

pub use file::{
    BINARY_EXTENSION, Document, DocumentError, DocumentForm, TEXT_EXTENSION, binary_twin,
    convert_to_binary, convert_to_text, document_as_text, document_bytes, document_from_bytes,
    document_text_from_bytes, existing_form, export_binary, is_binary_path, is_document_extension,
    is_document_path, leading_comments, read_document, read_document_text, text_as_binary,
    text_twin, write_document_text,
};

pub use parse::{ParseError, parse_bsn};

pub use delta::{apply_deltas, bsn_value_eq, shallow_diff};

pub use document::{
    AstNodeRef, BsnAssetContext, BsnField, BsnPatch, BsnPatches, BsnStructData, BsnStructFields,
    BsnTupleStructData, BsnValue, MAX_AST_DEPTH, SceneBsnAst, bsn_value_as_int, clone_node_into,
    clone_subtree_into, component_to_bsn_patch, component_to_bsn_patch_with_assets,
    is_enum_variant_of, patch_type_path, type_paths_include,
};
pub use emitter::{emit_entities, emit_entity, emit_scene};
pub use loader::{BsnLoadError, parse_bsn_text};
pub use retired::{RETIRED_UI_PREFIX, RetiredUiComponents, reject_retired_ui_components};

pub use apply::{
    AstDirty, BsnApplyAssets, BsnAssetPaths, BsnProjectAssets, BsnSceneAssets, DocumentOnlyTypes,
    UnresolvedTypes, apply_ast_to_ecs, apply_component_patch, apply_dirty_ast_patches,
    apply_reference_map, bsn_value_to_reflect, get_bsn_field, remove_bsn_field, set_bsn_field,
    spawn_ast_node, spawn_from_ast,
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
        app.init_resource::<SceneBsnAst>()
            .init_resource::<DocumentOnlyTypes>()
            .init_resource::<UnresolvedTypes>();
    }
}
