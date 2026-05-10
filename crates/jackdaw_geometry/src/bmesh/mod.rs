//! Half-edge mesh edit layer. Inspired by Blender's BMesh (architectural
//! reference); implementation re-derived from standard half-edge mesh
//! literature and original to this project.
//!
//! `BMesh` is in-memory only, lifted from `BrushTopology` on enter-edit-mode
//! and flattened back to topology on operator commit. All topology operations
//! (loop cut, knife, bevel, slide) mutate this structure directly.

pub mod cycles;
pub mod flatten;
pub mod lift;
pub mod ops;
pub mod select;
pub mod types;
pub mod validate;

pub use types::*;
