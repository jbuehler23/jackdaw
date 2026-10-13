//! Parser for `.bsn` source text.
//!
//! [`crate::syntax`] reads the text into a syntax tree; this module turns
//! that tree into the editor document, a [`SceneBsnAst`].
//!
//! A tuple is written as a list, and a list of pairs as a list of
//! two-element lists: `[["class", "Mage"]]`, never `[("class", "Mage")]`,
//! which fails at the `(`. The emitter writes that shape (see
//! `document::from_reflect`), so this constrains hand-authored text.

use bevy::ecs::entity::Entity;
use thiserror::Error;

use crate::document::{
    BsnField, BsnPatch, BsnPatches, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue,
    SceneBsnAst,
};
use crate::syntax::{
    self,
    parse_stream::ParseStream,
    span::Span,
    types::{Bsn, BsnEntry, BsnFields, BsnListRoot, BsnScene, BsnType},
};

/// An error produced while parsing `.bsn` source text.
#[derive(Error, Debug)]
pub enum ParseError {
    /// The lexer hit a character it could not turn into a token.
    #[error("lex error: unexpected character {0:?}")]
    Lex(char),
    /// The text does not follow the BSN syntax.
    #[error("parse error: {0}")]
    Parse(String),
    /// A struct gave the same field name twice.
    #[error("duplicate field '{field}' in '{type_name}'")]
    DuplicateField { type_name: String, field: String },
}

impl From<syntax::parse_stream::ParseError> for ParseError {
    fn from(err: syntax::parse_stream::ParseError) -> Self {
        match err {
            syntax::parse_stream::ParseError::LexError(syntax::lex::LexError::UnexpectedChar(
                c,
            )) => ParseError::Lex(c),
            other => ParseError::Parse(other.to_string()),
        }
    }
}

/// Parse `.bsn` source text into a document, returning the document and the
/// entity of the top-level patch group (which may be a multi-root `Children`
/// wrapper; the loader unwraps it).
pub fn parse_bsn(text: &str) -> Result<(SceneBsnAst, Entity), ParseError> {
    let root = ParseStream::from(Span::from(text)).parse::<BsnListRoot>()?;
    let mut ast = SceneBsnAst::default();
    let mut entities = root.0.0;
    let top = match entities.len() {
        0 => ast.world.spawn(BsnPatches(Vec::new())).id(),
        1 => spawn_bsn(&mut ast, entities.remove(0)),
        _ => {
            // Several roots take the shape a multi-root document has on
            // disk, an anonymous node holding only `Children`, which the
            // loader unwraps back into roots.
            let roots = entities
                .into_iter()
                .map(|bsn| spawn_bsn(&mut ast, bsn))
                .collect();
            let children = ast.world.spawn(BsnPatch::Children(roots)).id();
            ast.world.spawn(BsnPatches(vec![children])).id()
        }
    };
    check_duplicate_fields(&mut ast)?;
    Ok((ast, top))
}

/// Spawn one entity's patches into the document, children first.
fn spawn_bsn(ast: &mut SceneBsnAst, bsn: Bsn) -> Entity {
    let patches = bsn
        .entries
        .into_iter()
        .map(|entry| {
            let patch = patch_from_entry(ast, entry);
            ast.world.spawn(patch).id()
        })
        .collect();
    ast.world.spawn(BsnPatches(patches)).id()
}

fn patch_from_entry(ast: &mut SceneBsnAst, entry: BsnEntry) -> BsnPatch {
    match entry {
        BsnEntry::Name(name) => BsnPatch::Name(name.0),
        BsnEntry::CachedScene(BsnScene::Asset(path))
        | BsnEntry::UncachedScene(BsnScene::Asset(path)) => BsnPatch::Base(path.0),
        BsnEntry::CachedScene(BsnScene::SceneComponent(scene))
        | BsnEntry::UncachedScene(BsnScene::SceneComponent(scene)) => {
            let type_path = scene.path.to_type_path();
            match scene.fields {
                BsnFields::Unit => BsnPatch::Template(type_path, None),
                BsnFields::NamedFields(fields) => {
                    BsnPatch::Template(type_path, Some(struct_fields(fields)))
                }
                BsnFields::UnnamedFields(values) => BsnPatch::TupleStruct(BsnTupleStructData {
                    type_path,
                    values: values.into_iter().map(value_from_syntax).collect(),
                }),
            }
        }
        BsnEntry::FromTemplatePatch(patch) => {
            let type_path = patch.path.to_type_path();
            match patch.fields {
                BsnFields::Unit => BsnPatch::Type(type_path),
                BsnFields::NamedFields(fields) => BsnPatch::Struct(BsnStructData {
                    type_path,
                    fields: struct_fields(fields),
                }),
                BsnFields::UnnamedFields(values) => BsnPatch::TupleStruct(BsnTupleStructData {
                    type_path,
                    values: values.into_iter().map(value_from_syntax).collect(),
                }),
            }
        }
        // Every relationship reads as `Children`, the only one jackdaw writes.
        BsnEntry::RelatedSceneList { scene_list, .. } => BsnPatch::Children(
            scene_list
                .0
                .0
                .into_iter()
                .map(|bsn| spawn_bsn(ast, bsn))
                .collect(),
        ),
    }
}

fn struct_fields(fields: Vec<syntax::types::BsnNamedField>) -> BsnStructFields {
    BsnStructFields(
        fields
            .into_iter()
            .map(|field| BsnField {
                name: field.name,
                value: value_from_syntax(field.value),
            })
            .collect(),
    )
}

fn value_from_type(value: BsnType) -> BsnValue {
    let type_path = value.path.to_type_path();
    match value.fields {
        BsnFields::Unit => BsnValue::Type(type_path),
        BsnFields::NamedFields(fields) => BsnValue::Struct(BsnStructData {
            type_path,
            fields: struct_fields(fields),
        }),
        BsnFields::UnnamedFields(values) => BsnValue::TupleStruct(BsnTupleStructData {
            type_path,
            values: values.into_iter().map(value_from_syntax).collect(),
        }),
    }
}

fn value_from_syntax(value: syntax::types::BsnValue) -> BsnValue {
    use syntax::types::BsnValue as Syntax;
    match value {
        Syntax::Float(f) => BsnValue::Float(f),
        Syntax::Int(i) => BsnValue::Int(i),
        Syntax::Bool(b) => BsnValue::Bool(b),
        Syntax::String(s) => BsnValue::String(s),
        Syntax::Type(t) => value_from_type(t),
        Syntax::List(items) => BsnValue::List(items.into_iter().map(value_from_syntax).collect()),
        Syntax::Map(entries) => BsnValue::Map(
            entries
                .into_iter()
                .map(|(k, v)| (value_from_syntax(k), value_from_syntax(v)))
                .collect(),
        ),
    }
}

/// Reject structs that name the same field twice: applying such a patch
/// would silently let the last occurrence win, hiding hand-edit mistakes.
fn check_duplicate_fields(ast: &mut SceneBsnAst) -> Result<(), ParseError> {
    fn find_duplicate(data: &BsnStructData) -> Option<ParseError> {
        let mut seen = std::collections::HashSet::new();
        for BsnField { name, value } in &data.fields.0 {
            if !seen.insert(name.as_str()) {
                return Some(ParseError::DuplicateField {
                    type_name: data.type_path.clone(),
                    field: name.clone(),
                });
            }
            if let Some(err) = find_duplicate_in_value(value) {
                return Some(err);
            }
        }
        None
    }

    fn find_duplicate_in_value(value: &BsnValue) -> Option<ParseError> {
        match value {
            BsnValue::Struct(data) => find_duplicate(data),
            BsnValue::TupleStruct(data) => data.values.iter().find_map(find_duplicate_in_value),
            BsnValue::List(items) => items.iter().find_map(find_duplicate_in_value),
            BsnValue::Map(entries) => entries.iter().find_map(|(k, v)| {
                find_duplicate_in_value(k).or_else(|| find_duplicate_in_value(v))
            }),
            _ => None,
        }
    }

    let mut patches = ast.world.query::<&BsnPatch>();
    for patch in patches.iter(&ast.world) {
        let found = match patch {
            BsnPatch::Struct(data) => find_duplicate(data),
            BsnPatch::Template(_, Some(fields)) => fields
                .0
                .iter()
                .find_map(|f| find_duplicate_in_value(&f.value)),
            BsnPatch::TupleStruct(data) => data.values.iter().find_map(find_duplicate_in_value),
            _ => None,
        };
        if let Some(err) = found {
            return Err(err);
        }
    }
    Ok(())
}
