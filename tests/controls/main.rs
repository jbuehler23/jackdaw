//! The editor's controls: pickers, combo boxes, segmented controls, text
//! fields, lists, popovers, context menus and operator buttons.
//!
//! Each module below was its own test binary. Merged, the editor
//! links once for the theme rather than once per file.

#[path = "../util/mod.rs"]
mod util;

mod button_operator_dispatch;
mod color_picker;
mod comboboxes;
mod context_menus;
mod lists_and_scroll;
mod popovers;
mod segmented_controls;
mod synthetic_click;
mod text_fields;
