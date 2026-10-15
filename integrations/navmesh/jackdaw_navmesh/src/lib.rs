//! Navigation mesh generation: the [Recast](https://github.com/recastnavigation/recastnavigation)
//! pipeline from triangles to a polygon mesh and a detail mesh.
//!
//! This is [rerecast](https://github.com/janhohenheim/rerecast) 0.4.0, Jan
//! Hohenheim's Rust port of Recast, kept here under its MIT or Apache-2.0
//! license (see `LICENSE-MIT` and `LICENSE-APACHE`), trimmed to what the
//! terrain navmesh bake uses.

#[macro_use]
extern crate alloc;

mod compact_cell;
mod compact_heightfield;
mod compact_span;
mod config;
mod contours;
mod detail_mesh;
mod erosion;
mod heightfield;
pub(crate) mod math;
pub(crate) mod ops;
mod poly_mesh;
mod pre_filter;
mod rasterize;
mod region;
mod span;
mod trimesh;
mod watershed_build_regions;
mod watershed_distance_field;

pub use compact_cell::CompactCell;
pub use compact_heightfield::CompactHeightfield;
pub use compact_span::CompactSpan;
pub use config::{Config, ConfigBuilder};
pub use contours::{BuildContoursFlags, Contour, ContourSet, RegionVertexId};
pub use detail_mesh::{DetailNavmesh, SubMesh};
pub use heightfield::{Heightfield, HeightfieldBuilder, HeightfieldBuilderError};
pub use math::Aabb3d;
pub use poly_mesh::PolygonNavmesh;
pub use region::RegionId;
pub use span::{AreaType, Span, SpanKey, Spans};
pub use trimesh::TriMesh;
