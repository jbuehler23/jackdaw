//! Editor document model for the `.bsn` scene format.
//!
//! This is a flat, string-typed AST that the editor mutates and later emits
//! back to `.bsn` text. The parser in [`crate::parse`] builds it directly
//! from source text; there is no separate parse-time representation.
//!
//! Nodes live as entities in a private [`World`] held by [`SceneBsnAst`].

use bevy::ecs::entity::Entity;
use bevy::ecs::prelude::{Component, Resource};
use bevy::ecs::world::World;
use bevy::platform::collections::HashMap;

mod from_reflect;
mod mutate;
mod query;

pub use from_reflect::{
    BsnAssetContext, component_to_bsn_patch, component_to_bsn_patch_with_assets,
};
pub use mutate::{clone_node_into, clone_subtree_into};
pub use query::{bsn_value_as_int, is_enum_variant_of, type_paths_include};

/// A list of patches that together define one BSN entity.
/// Each child entity has a [`BsnPatch`] component.
#[derive(Component, Debug, Clone)]
pub struct BsnPatches(pub Vec<Entity>);

/// A single patch within a [`BsnPatches`] list.
#[derive(Component, Debug, Clone)]
pub enum BsnPatch {
    /// `#Name` entity name reference.
    Name(String),
    /// `:"path.bsn"` base scene inheritance.
    Base(String),
    /// `type::Path` a bare type (unit struct or enum variant, all defaults).
    Type(String),
    /// `type::Path { field: value, ... }` struct with field overrides.
    Struct(BsnStructData),
    /// `type::Path(value, ...)` tuple struct with positional values.
    TupleStruct(BsnTupleStructData),
    /// `@type::Path { ... }` template type.
    Template(String, Option<BsnStructFields>),
    /// `Children [...]` child entity relation.
    Children(Vec<Entity>),
}

/// Fields of a BSN struct patch: `TypePath { field: expr, ... }`.
#[derive(Debug, Clone)]
pub struct BsnStructData {
    pub type_path: String,
    pub fields: BsnStructFields,
}

/// Ordered list of named fields.
#[derive(Debug, Clone, Default)]
pub struct BsnStructFields(pub Vec<BsnField>);

/// A single `name: value` field.
#[derive(Debug, Clone)]
pub struct BsnField {
    pub name: String,
    pub value: BsnValue,
}

/// Tuple struct data: `TypePath(value, ...)`.
#[derive(Debug, Clone)]
pub struct BsnTupleStructData {
    pub type_path: String,
    pub values: Vec<BsnValue>,
}

/// A BSN expression value (the right-hand side of a field or tuple element).
#[derive(Debug, Clone)]
pub enum BsnValue {
    /// `1.0`
    Float(f64),
    /// `42`
    Int(i128),
    /// `true`
    Bool(bool),
    /// `"string"`
    String(String),
    /// `type::Path` unit type or enum variant.
    Type(String),
    /// `type::Path { field: value }` nested struct.
    Struct(BsnStructData),
    /// `type::Path(value)` nested tuple struct.
    TupleStruct(BsnTupleStructData),
    /// `[value, value, ...]` list/vec.
    List(Vec<BsnValue>),
    /// `map[(key, value), ...]` map/HashMap.
    Map(Vec<(BsnValue, BsnValue)>),
}

/// Resource holding the BSN AST for the currently loaded scene.
/// The AST is stored as entities in a separate [`World`].
#[derive(Resource, Default)]
pub struct SceneBsnAst {
    /// The AST world; each entity is an AST node.
    pub world: World,
    /// Root-level entity patches (top-level entities in the scene).
    pub roots: Vec<Entity>,
    /// Maps ECS scene entities to AST patches entities.
    pub ecs_to_ast: HashMap<Entity, Entity>,
    /// Maps AST patches entities to ECS scene entities (reverse of above).
    pub ast_to_ecs: HashMap<Entity, Entity>,
}

/// Component on every ECS entity that was spawned from (or synced to) BSN.
/// Points back to the AST node entity in [`SceneBsnAst::world`].
#[derive(Component)]
pub struct AstNodeRef {
    pub patches_entity: Entity,
}
