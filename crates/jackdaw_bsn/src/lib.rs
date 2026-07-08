//! Reader for the `.bsn` scene format.
//!
//! This crate provides the parser front-end (turning `.bsn` source text into a
//! self-contained parser AST) plus the editor document model and the loader
//! that adapts a parsed AST into it. Resolving the document AST to ECS and
//! emitting `.bsn` are added by later work.
//!
//! The parser and document layers both name some types `BsnPatch`, `BsnField`,
//! and `BsnPatches`. The document (editor-facing) versions are re-exported at
//! the crate root; the parser versions stay under [`crate::parse`].

pub mod apply;
pub mod document;
pub mod loader;
pub mod parse;

pub use parse::{
    BsnAst, BsnExpr, BsnNameStore, BsnNamedTuple, BsnRelation, BsnRoot, BsnStruct, BsnSymbol,
    BsnVar, ParseError, parse_bsn,
};

pub use document::{
    AstNodeRef, BsnField, BsnPatch, BsnPatches, BsnStructData, BsnStructFields, BsnTupleStructData,
    BsnValue, SceneBsnAst, component_to_bsn_patch,
};
pub use loader::{BsnLoadError, parse_bsn_text};

pub use apply::{
    AstDirty, apply_ast_to_ecs, apply_dirty_ast_patches, bsn_value_to_reflect, get_bsn_field,
    parse_string_to_bsn_value, set_bsn_field, spawn_from_ast,
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
