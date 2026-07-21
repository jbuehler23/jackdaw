//! BSN text loader: parse `.bsn` text into a [`SceneBsnAst`].
//!
//! Parsing is delegated to [`crate::parse::parse_bsn`], which builds the
//! document directly; this module only unwraps the multi-root form and
//! installs the document roots.

use bevy::ecs::entity::Entity;

use crate::document::{BsnPatch, SceneBsnAst};
use crate::parse::ParseError;

/// Errors that can occur when loading BSN text.
#[derive(Debug)]
pub enum BsnLoadError {
    /// The source text could not be parsed.
    Parse(ParseError),
    /// A referenced AST node was missing from the parsed world.
    NoAstNode,
}

impl std::fmt::Display for BsnLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BsnLoadError::Parse(err) => write!(f, "BSN parse error: {err}"),
            BsnLoadError::NoAstNode => write!(f, "No AST node found"),
        }
    }
}

impl std::error::Error for BsnLoadError {}

/// Marker component the emitter puts on a single scene root whose only patch is
/// `Children`. Without it that entity is byte-identical to the synthetic
/// multi-root wrapper and would be unwrapped, dropping a real grouping entity.
/// The loader strips it before installing the root, so it never reaches apply.
pub(crate) const SCENE_ROOT_GROUP_MARKER: &str = "jackdaw_bsn::__SceneRootGroup";

/// Parse BSN text into a document [`SceneBsnAst`].
pub fn parse_bsn_text(text: &str) -> Result<SceneBsnAst, BsnLoadError> {
    let (mut ast, root) = crate::parse::parse_bsn(text).map_err(BsnLoadError::Parse)?;

    // Multiple roots emit as a single anonymous entity holding only a Children
    // relation; unwrap it to recover the real roots. A genuine single root that
    // has only children carries `SCENE_ROOT_GROUP_MARKER`, giving it a second
    // patch, so it fails this length check and is preserved below.
    let is_children_wrapper = ast.get_patches(root).is_some_and(|p| {
        p.0.len() == 1
            && ast
                .get_patch(p.0[0])
                .is_some_and(|patch| matches!(patch, BsnPatch::Children(_)))
    });

    if is_children_wrapper
        && let Some(patches) = ast.get_patches(root)
        && let Some(patch) = ast.get_patch(patches.0[0])
        && let BsnPatch::Children(children) = patch
    {
        let children = children.clone();
        for child in children {
            ast.add_to_roots(child);
        }
        return Ok(ast);
    }

    strip_scene_root_marker(&mut ast, root);
    ast.add_to_roots(root);
    Ok(ast)
}

/// Remove the internal [`SCENE_ROOT_GROUP_MARKER`] patch from `root`, if present.
fn strip_scene_root_marker(ast: &mut SceneBsnAst, root: Entity) {
    let marker = ast.get_patches(root).and_then(|p| {
        p.0.iter().copied().find(|&pe| {
            matches!(ast.get_patch(pe), Some(BsnPatch::Type(tp)) if tp == SCENE_ROOT_GROUP_MARKER)
        })
    });
    if let Some(marker) = marker
        && let Some(patches) = ast.get_patches_mut(root)
    {
        patches.0.retain(|&pe| pe != marker);
    }
}
