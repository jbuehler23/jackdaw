//! The inspector: value editing, panel width, the preview guard and the
//! bindings card.
//!
//! Each module below was its own test binary. Merged, the editor
//! links once for the theme rather than once per file.

#[path = "../util/mod.rs"]
mod util;

mod bindings_card;
mod bindings_link;
mod inspector_panel_width;
mod inspector_preview_guard;
mod inspector_val;
