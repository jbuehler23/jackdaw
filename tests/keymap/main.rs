//! The keymap: the shipped presets and the user's own overrides layered
//! over them.
//!
//! One binary per theme: each module below was its own test
//! binary, and linking the editor once instead of 2 times is
//! what the split cost.

#[path = "util/mod.rs"]
mod util;

mod keymap_presets;
mod keymap_user_overrides;
