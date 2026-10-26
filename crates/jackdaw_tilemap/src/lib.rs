//! Engine-agnostic tilemap kernel: square grid math, sparse 32x32 chunk
//! storage, the per-cell value types stored in tile layers and tileset
//! slicing over RGBA8 bytes.
//!
//! Conventions: the world is y-up, cell (0,0) has its min corner at the
//! layer origin, and cells are half-open `[min, max)` so a point exactly on
//! an edge belongs to the cell whose min corner it is.

mod grid;
mod layer;
mod path;
mod region_text;
mod sidecar;
mod slice;
mod solver;
mod strip;
mod template;
mod tile;

pub use grid::{CHUNK_AREA, CHUNK_SIZE, CellCoord, CellRect, ChunkCoord, Grid, WorldRect};
pub use layer::{CellValue, ChunkedLayer};
pub use path::{Links, StrokeError, apply_connect_stroke, apply_path_stroke, orthogonal_line};
pub use region_text::{
    Legend, LegendEntry, ReadRegionError, Region, RegionTextError, RegionTextErrorKind, emit_rows,
    parse_rows, read_region, write_region,
};
pub use sidecar::{
    LayerData, LayerKind, MAGIC, SidecarError, TerrainAuthoring, TilemapData, VERSION, decode,
    encode,
};
pub use slice::{
    COMMON_TILE_SIZES, GridDetection, GridLayout, RgbaImage, detect_grid, empty_tiles, uniform_tile,
};
pub use solver::{
    MatchMode, Peer, RequiredPeers, Requirement, Resolved, TerrainSet, TerrainTile, TileTerrains,
    match_score, required_peers, resolve_cell, resolve_region,
};
pub use strip::{AnimationStrip, FrameRun, LOOSE_FRAME_MATCH, detect_animation, pair_animations};
pub use template::LayoutTemplate;
pub use tile::{TerrainId, TileRef};
