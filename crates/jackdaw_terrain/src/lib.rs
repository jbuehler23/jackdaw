pub mod brush;
pub mod channel;
pub mod clipmap;
pub mod control;
pub mod erosion;
pub mod generate;
pub mod heightmap;
pub mod navmesh;
pub mod quantize;
pub mod rect;
pub mod region;
#[cfg(feature = "render")]
pub mod render;
pub mod scatter;
pub mod sidecar;
pub mod splat;
pub mod texture_set;

pub use brush::{SculptTool, affected_chunks, affected_chunks_at, apply_brush, apply_brush_at};
pub use channel::{ChannelData, ChannelDescriptor, ChannelElement, apply_channel_brush};
pub use clipmap::{
    ClipmapLevel, GridSquare, REBUILD_BUDGET, SurfaceMeshData, build_clipmap_indices,
    build_clipmap_mesh_data, clipmap_levels, flat_shaded, plan_rebuilds,
};
pub use control::{
    Control, MANUAL_BIT, MAX_BLEND, MAX_TEXTURE_ID, apply_control_brush, apply_restore_brush,
};
pub use erosion::{ErosionParams, hydraulic_erosion};
pub use generate::{GenerateSettings, NoiseType, generate_heightmap};
pub use heightmap::{Heightmap, SurfaceHit};
pub use navmesh::{BakeParams, NavPolygon, NavmeshArtifact, NavmeshError, SurfaceMesh};
pub use quantize::{quantize_height, quantize_heights, quantize_region};
pub use rect::GridRect;
pub use region::{Region, RegionCoord, RegionSize, RegionSizeError, TerrainRegions};
pub use scatter::{Placement, ScatterMask, ScatterParams, scatter};
pub use sidecar::{
    AutoTerrainSettings, GridGeometry, GridShape, RegionTerrainData, SidecarError, TerrainData,
};
pub use splat::{
    ControlTexels, control_cell, control_corner, grid_uv, layers_covered, write_control_rect,
};
pub use texture_set::{TextureSet, TextureSetEntry, TextureSetError};
