//! The outliner and the entity operations reached from it: reordering, the
//! clipboard, the palette and the add-entity flows.
//!
//! Each module below was its own test binary. Merged, the editor
//! links once for the theme rather than once per file.

#[path = "../util/mod.rs"]
mod util;

mod component_picker;
mod entity_clipboard;
mod entity_reorder;
mod multi_outliner;
mod new_scene_kinds;
mod outliner_drag_cleanup;
mod outliner_range_select;
mod outliner_rename;
mod outliner_row_icons;
mod tree_row_labels;
mod ui_palette;
