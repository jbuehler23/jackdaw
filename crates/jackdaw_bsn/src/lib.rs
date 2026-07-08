//! Reader for the `.bsn` scene format.
//!
//! This crate currently provides the parser front-end only: it turns `.bsn`
//! source text into a self-contained parser AST. Resolving that AST to ECS,
//! emitting `.bsn`, and the editor document model are added by later work.

pub mod parse;

pub use parse::{
    BsnAst, BsnExpr, BsnField, BsnNameStore, BsnNamedTuple, BsnPatch, BsnPatches, BsnRelation,
    BsnRoot, BsnStruct, BsnSymbol, BsnVar, ParseError, parse_bsn,
};
