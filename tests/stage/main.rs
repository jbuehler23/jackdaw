//! Authoring on the canvas and the stage: placement, snapping, guides,
//! grouping, layout presets and the modelling tools.
//!
//! Each module below was its own test binary. Merged, the editor
//! links once for the theme rather than once per file.

#[path = "../util/mod.rs"]
mod util;

mod brush_ops;
mod canvas_guides;
mod canvas_snap;
mod gltf_authoring;
mod mesh_quick_menu;
mod modeling_essentials;
mod positionable_mirror_plane;
mod ui_grouping;
mod ui_layout_presets;
mod ui_stage_manipulation;
