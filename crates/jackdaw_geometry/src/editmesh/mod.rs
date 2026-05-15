//! Half-edge mesh edit layer. Verts / edges / loops / polys with disk + radial cycles
//! for O(1) adjacency. Lifted from `BrushTopology` on enter-edit-mode and flattened
//! back on operator commit.
//!
//! `EditMesh` is in-memory only, lifted from `BrushTopology` on enter-edit-mode
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
