//! The `.bsn` file format: lexing, parsing, and the syntax tree.
//!
//! This module mirrors `bevy_bsn` from bevyengine/bevy#26043 (Bevy, MIT OR
//! Apache-2.0): the same modules, type names, and parse functions, so that
//! jackdaw can depend on `bevy_bsn` once it ships by swapping this module
//! out. Everything jackdaw-specific (the version stamp, migration, the editor
//! document, the binary form) lives outside it.
//!
//! Where jackdaw reads more than that PR does, the item says so. The
//! additions are: comments; `#"quoted names"`; `@"path"` and `@Type { ... }`
//! entries ([`types::BsnEntry::UncachedScene`]); generic type arguments
//! ([`types::PathSegment::arguments`]); list and `map[...]` values; signed
//! numbers and the `inf`/`NaN` floats; and a multi-entity root
//! ([`types::BsnListRoot`]).

/// Lexing and Tokens
pub mod lex;

/// BSN Parsing logic
pub mod parse;

/// A parseable stream of tokens.
pub mod parse_stream;

/// Span functionality, for referencing "spans" of BSN source strings.
pub mod span;

/// BSN Types. This is the Abstract Syntax Tree (AST).
pub mod types;
