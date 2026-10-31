//! Operator registration and dispatch: availability, parameters, modals,
//! tooltips, undo and the id space.
//!
//! Each module below was its own test binary. Merged, the editor
//! links once for the theme rather than once per file.

#[path = "../util/mod.rs"]
mod util;

mod authoring_ops;
mod dialog_answer;
mod field_edits;
mod light_defaults;
mod operator_availability;
mod operator_entity_params;
mod operator_modals;
mod operator_params;
mod operator_smoke;
mod operator_tooltip;
mod operator_undo;
mod param_declarations;
mod pixel_canvas_ops;
mod prefab_ops;
mod remote_coverage;
mod scatter_ops;
mod scene_op_ids;
mod sprite_ops;
mod tileset_bits;
mod tileset_browser;
mod tileset_dock;
mod tileset_edit_ops;
mod tileset_layout_ops;
mod tileset_ops;
mod view_camera_ops;
