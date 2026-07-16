//! BSN text loader: parse `.bsn` text into a [`SceneBsnAst`].
//!
//! Parsing is delegated to [`crate::parse::parse_bsn`], which builds the
//! document directly; this module only unwraps the multi-root form and
//! installs the document roots.

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

/// Parse BSN text into a document [`SceneBsnAst`].
pub fn parse_bsn_text(text: &str) -> Result<SceneBsnAst, BsnLoadError> {
    let (mut ast, root) = crate::parse::parse_bsn(text).map_err(BsnLoadError::Parse)?;

    // If the top-level is a single entity with only a Children relation,
    // unwrap it to get the real roots (multi-root format).
    let root_patches = ast.get_patches(root);
    let is_children_wrapper = root_patches.is_some_and(|p| {
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

    ast.add_to_roots(root);
    Ok(ast)
}
