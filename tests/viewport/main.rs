//! Viewports: 2D and 3D modes, chrome, several at once, and the preview
//! context a panel renders through.
//!
//! Each module below was its own test binary. Merged, the editor
//! links once for the theme rather than once per file.

#[path = "../util/mod.rs"]
mod util;

mod avian_picker_visibility;
mod multi_viewport;
mod preview_context;
mod viewport_2d;
mod viewport_chrome;
mod viewport_mode;
