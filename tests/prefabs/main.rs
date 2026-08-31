//! Prefabs: their lifecycle, the paths they record and the UI import path.
//!
//! Each module below was its own test binary. Merged, the editor
//! links once for the theme rather than once per file.

#[path = "../util/mod.rs"]
mod util;

mod prefab_lifecycle;
mod prefab_source_paths;
mod prefab_ui_import;
