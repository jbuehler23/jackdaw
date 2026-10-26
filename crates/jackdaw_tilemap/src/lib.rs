//! Engine-agnostic tilemap kernel: square grid math, sparse 32x32 chunk
//! storage and the per-cell value types stored in tile layers.
//!
//! Conventions: the world is y-up, cell (0,0) has its min corner at the
//! layer origin, and cells are half-open `[min, max)` so a point exactly on
//! an edge belongs to the cell whose min corner it is.

mod grid;
mod layer;
mod sidecar;
mod tile;

pub use grid::{CHUNK_AREA, CHUNK_SIZE, CellCoord, CellRect, ChunkCoord, Grid, WorldRect};
pub use layer::{CellValue, ChunkedLayer};
pub use sidecar::{
    LayerData, LayerKind, MAGIC, SidecarError, TerrainAuthoring, TilemapData, VERSION, decode,
    encode,
};
pub use tile::{TerrainId, TileRef};
