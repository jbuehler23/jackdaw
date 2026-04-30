//! Main launcher crate for the Jackdaw editor.
//!
//! Phase 1 of the launcher architecture extracted the editor library
//! into the `jackdaw_editor` crate. This top-level package now
//! re-exports the editor's public surface so existing `jackdaw::...`
//! call sites (`main.rs`, integration tests, examples) keep
//! compiling. Subsequent phases trim the launcher down to launcher
//! specific concerns; for now the goal is purely "don't break
//! anything; just relocate."
//!
//! See `docs/superpowers/specs/2026-04-30-jackdaw-launcher-architecture-design.md`.

// Glob re-export the editor library so `jackdaw::brush::...`,
// `jackdaw::project::...`, `jackdaw::project_select::...`, etc. resolve
// to the relocated modules.
pub use jackdaw_editor::*;
