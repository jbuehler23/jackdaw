//! Repository-wide guards and smoke checks that scan the tree or the
//! running editor rather than exercising one feature.
//!
//! Each module below was its own test binary. Merged, the editor
//! links once for the theme rather than once per file.

#[path = "../util/mod.rs"]
mod util;

mod remote_debug_smoke;
mod scaffolded_component_flow;
mod widget_purity;
