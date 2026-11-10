//! Engine-agnostic tilemap kernel: square grid math, sparse 32x32 chunk
//! storage, the per-cell value types stored in tile layers and tileset
//! slicing over RGBA8 bytes.
//!
//! Conventions: the world is y-up, cell (0,0) has its min corner at the
//! layer origin, and cells are half-open `[min, max)` so a point exactly on
//! an edge belongs to the cell whose min corner it is.

mod block_fit;
mod collision_trace;
mod data_path;
mod grid;
mod layer;
mod layout_fit;
mod path;
mod region_text;
mod report;
mod setup;
mod setup_edit;
mod setup_layout;
mod sidecar;
mod slice;
mod solver;
mod stamp;
mod strip;
mod suggest;
mod template;
mod terrain_find;
mod tile;
mod tile_collision;

pub use block_fit::{BlockEvidence, BlockFit, TerrainLook, fit_block};
pub use collision_trace::{
    SolidMask, TileShape, solid_mask, trace_art, trace_mask, trace_proposal,
};
pub use data_path::{DataPathError, EXTENSION, mint_data_path, resolve_path};
pub use grid::{
    CHUNK_AREA, CHUNK_SIZE, CellCoord, CellRect, ChunkCoord, Grid, Placement, WorldRect,
};
pub use layer::{CellValue, ChunkedLayer};
pub use layout_fit::{
    FitError, FittedTile, ProposedSet, ProposedTerrain, TilesetProposal, fit_layouts,
};
pub use path::{Links, StrokeError, apply_connect_stroke, apply_path_stroke, orthogonal_line};
pub use region_text::{
    Legend, LegendEntry, ReadRegionError, Region, RegionTextError, RegionTextErrorKind, emit_rows,
    parse_rows, read_region, write_region,
};
pub use report::{
    AutoSetup, CollisionReport, SetReport, SetupError, SetupReport, SheetReport, StampReport,
    StripReport, TerrainReport, auto_setup, auto_setup_on_grid,
};
pub use setup::{
    MARKER_COLORS, Pair, PairGroup, Provenance, Scope, SetupSet, SetupTerrain, SetupTile,
    TilesetSetup, Tracked, fits, free_set_name, group_pairs, next_marker_color, rerun,
    reset_to_auto, set_name, solver_takes, stacked_pairs,
};
pub use setup_layout::{Coverage, LayoutTerrains};
pub use sidecar::{
    LayerData, LayerKind, MAGIC, SidecarError, TerrainAuthoring, TilemapData, VERSION, decode,
    encode,
};
pub use slice::{
    COMMON_TILE_SIZES, GridDetection, GridLayout, RgbaImage, SharedDetection, detect_grid,
    detect_grid_shared, empty_tiles, uniform_tile,
};
pub use solver::{
    MatchMode, Peer, RequiredPeers, Requirement, Resolved, TerrainSet, TerrainTile, TileTerrains,
    match_score, required_peers, resolve_cell, resolve_region, resolve_region_within,
};
pub use stamp::{Stamp, find_stamps, reconcile};
pub use strip::{AnimationStrip, FrameRun, LOOSE_FRAME_MATCH, detect_animation, pair_animations};
pub use suggest::{
    LayoutGuess, MIN_OBJECT, Suggestion, SuggestionKind, suggest_blocks, taken_tiles,
};
pub use template::{LayoutTemplate, PaintPreview};
pub use terrain_find::{
    FoundTerrain, SheetInput, TerrainAnalysis, TileAddress, TileKind, TileLabels, find_terrains,
};
pub use tile::{TerrainId, TileRef};
pub use tile_collision::{ShapePreset, TileCollision};
