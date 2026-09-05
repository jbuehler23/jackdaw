//! Operator registration and dispatch: availability, parameters, modals,
//! tooltips, undo and the id space.
//!
//! Each module below was its own test binary. Merged, the editor
//! links once for the theme rather than once per file.

#[path = "../util/mod.rs"]
mod util;

mod authoring_ops;
mod dialog_answer;
mod operator_availability;
mod operator_entity_params;
mod operator_modals;
mod operator_params;
mod operator_smoke;
mod operator_tooltip;
mod operator_undo;
mod param_declarations;
mod prefab_ops;
mod remote_coverage;
mod scatter_ops;
mod scene_op_ids;
mod view_camera_ops;
