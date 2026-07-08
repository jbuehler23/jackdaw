//! Parser AST for the BSN scene format.
//!
//! The grammar builds these nodes into a private `World` held by [`BsnAst`],
//! spawning each node as an entity and referring to children by [`Entity`].
//! The AST is self-contained: it holds the parsed structure and nothing about
//! how it later resolves to concrete component values.
//!
//! Vendored from Bevy's dynamic BSN work.

use core::mem;
use std::collections::HashMap;

use bevy::ecs::{
    entity::Entity,
    prelude::{Component, Resource},
    world::World,
};

/// The parsed contents of a `.bsn` document.
///
/// Nodes are stored as entities in an internal `World`; the [`Entity`]
/// returned by the top-level parse identifies the root [`BsnPatches`] node.
#[derive(Default)]
pub struct BsnAst(pub World);

/// Assigns a stable index to every distinct name seen while parsing.
#[derive(Resource, Default)]
pub struct BsnNameStore {
    pub name_indices: HashMap<String, usize>,
    pub next_name_index: usize,
}

/// A group of sibling patches (the root, or the contents of a relation).
#[derive(Component)]
pub struct BsnPatches(pub Vec<Entity>);

/// A single top-level patch node.
#[derive(Component)]
pub enum BsnPatch {
    Name(String, usize),
    Base(String),
    Var(BsnVar),
    Struct(BsnStruct),
    NamedTuple(BsnNamedTuple),
    Relation(BsnRelation),
}

/// A symbol reference; the trailing `bool` marks a template (`@`) reference.
#[derive(Clone)]
pub struct BsnVar(pub BsnSymbol, pub bool);

/// A path symbol: the leading path segments plus the final identifier.
#[derive(Clone)]
pub struct BsnSymbol(pub Vec<String>, pub String);

/// A struct patch: symbol, named fields, and a template marker.
pub struct BsnStruct(pub BsnSymbol, pub Vec<BsnField>, pub bool);

/// A named struct field: field name and the expression node it holds.
pub struct BsnField(pub String, pub Entity);

/// A tuple-style patch: symbol, positional argument nodes, and a template marker.
pub struct BsnNamedTuple(pub BsnSymbol, pub Vec<Entity>, pub bool);

/// A relation: the relation symbol and its list of related patch groups.
pub struct BsnRelation(pub BsnSymbol, pub Vec<Entity>);

/// An expression node (a field value or tuple argument).
#[derive(Component)]
pub enum BsnExpr {
    Var(BsnVar),
    Struct(BsnStruct),
    NamedTuple(BsnNamedTuple),
    StringLit(String),
    FloatLit(f64),
    BoolLit(bool),
    IntLit(i128),
    List(Vec<Entity>),
}

impl BsnSymbol {
    pub fn from_ident(ident: String) -> BsnSymbol {
        BsnSymbol(vec![], ident)
    }

    pub fn append(mut self, ident: String) -> BsnSymbol {
        self.0.push(mem::replace(&mut self.1, ident));
        self
    }
}

impl BsnAst {
    pub fn create_patches(&mut self, patches: Vec<Entity>) -> Entity {
        self.0.spawn(BsnPatches(patches)).id()
    }

    pub fn create_patch(&mut self, patch: BsnPatch) -> Entity {
        self.0.spawn(patch).id()
    }

    pub fn create_expr(&mut self, expr: BsnExpr) -> Entity {
        self.0.spawn(expr).id()
    }

    pub fn create_name_patch(&mut self, name: String) -> Entity {
        let mut name_store = self.0.resource_mut::<BsnNameStore>();
        let index = match name_store.name_indices.get(&*name) {
            Some(index) => *index,
            None => {
                let index = name_store.next_name_index;
                name_store.next_name_index += 1;
                name_store.name_indices.insert(name.clone(), index);
                index
            }
        };
        self.create_patch(BsnPatch::Name(name, index))
    }
}
