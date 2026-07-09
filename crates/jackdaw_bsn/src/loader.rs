//! BSN text loader: parse `.bsn` text into a [`SceneBsnAst`].
//!
//! Parsing is delegated to [`crate::parse::parse_bsn`]; this module adapts the
//! parser AST ([`crate::parse`] types) into the editor document AST
//! ([`crate::document`] types).

use bevy::ecs::entity::Entity;

use crate::document::{
    BsnField, BsnPatch, BsnPatches, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue,
    SceneBsnAst,
};
use crate::parse::{
    BsnAst, BsnExpr, BsnField as ParserBsnField, BsnPatch as ParserBsnPatch,
    BsnPatches as ParserBsnPatches, BsnRoot, BsnSymbol, ParseError,
};

/// Errors that can occur when loading BSN text.
#[derive(Debug)]
pub enum BsnLoadError {
    /// The source text could not be parsed into a parser AST.
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
    let bevy_ast = crate::parse::parse_bsn(text).map_err(BsnLoadError::Parse)?;
    let patches_id = bevy_ast.0.resource::<BsnRoot>().0;

    // Adapt parser AST into document AST.
    let mut jackdaw_ast = SceneBsnAst::default();
    let root = adapt_patches(&bevy_ast, patches_id, &mut jackdaw_ast)?;

    // If the top-level is a single entity with only a Children relation, unwrap
    // it to get the real roots (multi-root format).
    let root_patches = jackdaw_ast.get_patches(root);
    let is_children_wrapper = root_patches.is_some_and(|p| {
        p.0.len() == 1
            && jackdaw_ast
                .get_patch(p.0[0])
                .is_some_and(|patch| matches!(patch, BsnPatch::Children(_)))
    });

    if is_children_wrapper
        && let Some(patches) = jackdaw_ast.get_patches(root)
        && let Some(patch) = jackdaw_ast.get_patch(patches.0[0])
        && let BsnPatch::Children(children) = patch
    {
        let children = children.clone();
        for child in children {
            jackdaw_ast.add_to_roots(child);
        }
        return Ok(jackdaw_ast);
    }

    jackdaw_ast.add_to_roots(root);
    Ok(jackdaw_ast)
}

fn adapt_patches(
    bevy_ast: &BsnAst,
    patches_id: Entity,
    jd_ast: &mut SceneBsnAst,
) -> Result<Entity, BsnLoadError> {
    let Some(bevy_patches) = bevy_ast.0.get::<ParserBsnPatches>(patches_id) else {
        return Err(BsnLoadError::NoAstNode);
    };

    let mut jd_patch_entities = Vec::new();
    for &patch_id in &bevy_patches.0 {
        let Some(bevy_patch) = bevy_ast.0.get::<ParserBsnPatch>(patch_id) else {
            continue;
        };
        match bevy_patch {
            ParserBsnPatch::Name(name, _index) => {
                let pe = jd_ast.world.spawn(BsnPatch::Name(name.clone())).id();
                jd_patch_entities.push(pe);
            }
            ParserBsnPatch::Base(path) => {
                let pe = jd_ast.world.spawn(BsnPatch::Base(path.clone())).id();
                jd_patch_entities.push(pe);
            }
            ParserBsnPatch::Var(var) => {
                let type_path = symbol_to_path(&var.0);
                let is_template = var.1;
                let pe = if is_template {
                    jd_ast.world.spawn(BsnPatch::Template(type_path, None)).id()
                } else {
                    jd_ast.world.spawn(BsnPatch::Type(type_path)).id()
                };
                jd_patch_entities.push(pe);
            }
            ParserBsnPatch::Struct(bsn_struct) => {
                let type_path = symbol_to_path(&bsn_struct.0);
                let is_template = bsn_struct.2;
                let fields = adapt_struct_fields(bevy_ast, &bsn_struct.1);
                let pe = if is_template {
                    jd_ast
                        .world
                        .spawn(BsnPatch::Template(type_path, Some(fields)))
                        .id()
                } else {
                    jd_ast
                        .world
                        .spawn(BsnPatch::Struct(BsnStructData { type_path, fields }))
                        .id()
                };
                jd_patch_entities.push(pe);
            }
            ParserBsnPatch::NamedTuple(tuple) => {
                let type_path = symbol_to_path(&tuple.0);
                let values = adapt_tuple_values(bevy_ast, &tuple.1);
                let pe = jd_ast
                    .world
                    .spawn(BsnPatch::TupleStruct(BsnTupleStructData {
                        type_path,
                        values,
                    }))
                    .id();
                jd_patch_entities.push(pe);
            }
            ParserBsnPatch::Relation(relation) => {
                // Only Children relations are supported.
                let mut child_entities = Vec::new();
                for &child_patches_id in &relation.1 {
                    if let Ok(child) = adapt_patches(bevy_ast, child_patches_id, jd_ast) {
                        child_entities.push(child);
                    }
                }
                let pe = jd_ast.world.spawn(BsnPatch::Children(child_entities)).id();
                jd_patch_entities.push(pe);
            }
        }
    }

    Ok(jd_ast.world.spawn(BsnPatches(jd_patch_entities)).id())
}

fn adapt_struct_fields(bevy_ast: &BsnAst, fields: &[ParserBsnField]) -> BsnStructFields {
    let mut jd_fields = Vec::new();
    for field in fields {
        let value = adapt_expr(bevy_ast, field.1);
        jd_fields.push(BsnField {
            name: field.0.clone(),
            value,
        });
    }
    BsnStructFields(jd_fields)
}

fn adapt_tuple_values(bevy_ast: &BsnAst, expr_ids: &[Entity]) -> Vec<BsnValue> {
    expr_ids
        .iter()
        .map(|&id| adapt_expr(bevy_ast, id))
        .collect()
}

fn adapt_expr(bevy_ast: &BsnAst, expr_id: Entity) -> BsnValue {
    let Some(expr) = bevy_ast.0.get::<BsnExpr>(expr_id) else {
        return BsnValue::String("<error>".into());
    };
    match expr {
        BsnExpr::Var(var) => {
            let path = symbol_to_path(&var.0);
            BsnValue::Type(path)
        }
        BsnExpr::Struct(bsn_struct) => {
            let type_path = symbol_to_path(&bsn_struct.0);
            let fields = adapt_struct_fields(bevy_ast, &bsn_struct.1);
            BsnValue::Struct(BsnStructData { type_path, fields })
        }
        BsnExpr::NamedTuple(tuple) => {
            let type_path = symbol_to_path(&tuple.0);
            let values = adapt_tuple_values(bevy_ast, &tuple.1);
            BsnValue::TupleStruct(BsnTupleStructData { type_path, values })
        }
        BsnExpr::StringLit(s) => BsnValue::String(s.clone()),
        BsnExpr::FloatLit(f) => BsnValue::Float(*f),
        BsnExpr::BoolLit(b) => BsnValue::Bool(*b),
        BsnExpr::IntLit(i) => BsnValue::Int(*i),
        BsnExpr::List(expr_ids) => {
            let values = adapt_tuple_values(bevy_ast, expr_ids);
            BsnValue::List(values)
        }
        BsnExpr::Map(pairs) => {
            let entries = pairs
                .iter()
                .map(|&(k, v)| (adapt_expr(bevy_ast, k), adapt_expr(bevy_ast, v)))
                .collect();
            BsnValue::Map(entries)
        }
    }
}

fn symbol_to_path(sym: &BsnSymbol) -> String {
    let mut path = String::new();
    for segment in &sym.0 {
        path.push_str(segment);
        path.push_str("::");
    }
    path.push_str(&sym.1);
    path
}
