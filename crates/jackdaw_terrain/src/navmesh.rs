//! Navmesh bake input and the baked artifact's binary format.
//!
//! [`surface_mesh`] builds the full-resolution triangle mesh a baker
//! rasterizes; [`decode`] reads back the polygons a baker produced.
//! Neither links a baker: the artifact carries plain vertices and polygon
//! indices.
//!
//! # Bake input
//!
//! [`surface_mesh`] walks present regions at one vertex per cell, with no
//! decimation, and emits a quad only where all four of its corners are
//! owned. An absent region contributes nothing, and no cliff is drawn down
//! to zero at its border. Positions come out terrain-local; a caller
//! applies the terrain's transform.
//!
//! # Artifact
//!
//! ```text
//! offset  size          field
//! 0       8             magic, b"JDNAVMS\0"
//! 8       2             format version, u16
//! 10      2             flags, u16 (reserved, must be 0)
//! 12      4             agent radius, f32
//! 16      4             agent height, f32
//! 20      4             max slope in degrees, f32
//! 24      4             cell size, f32
//! 28      8             source hash, u64
//! 36      4             terrain path length in bytes, u32
//! 40      4             vertex count, u32
//! 44      4             polygon count, u32
//! 48      4             surface vertex count, u32
//! 52      4             surface triangle count, u32
//! 56      len           terrain path, UTF-8
//! then    12*vertices   polygon vertices, 3 x f32
//! then, per polygon:
//!         1             area, u8
//!         1             corner count, u8 (at least 3)
//!         2             padding, must be 0
//!         4*corners     vertex indices, u32
//!         4*corners     edge neighbors, u32 ([`NO_NEIGHBOR`] = border)
//! then
//!         12*surface vertices    surface vertices, 3 x f32
//!         12*surface triangles   surface triangles, 3 x u32
//! ```
//!
//! Every integer and float is little-endian and every vertex is in world
//! space, so nothing has to be re-derived from the terrain to walk the
//! artifact.
//!
//! The polygons are what a pathfinder walks; the surface triangles are the
//! same walkable ground at the baker's detail resolution, for height
//! queries and for drawing. A consumer that only pathfinds can stop after
//! the polygons.
//!
//! [`decode`] refuses a file it does not fully understand rather than
//! returning part of one: bad magic, a newer version, a reserved field
//! set, a bake parameter no bake could have run at, an index that does
//! not address a vertex, a polygon with fewer than three corners, a
//! terrain path that is not text, and a file that ends early or runs long
//! are all errors. There is no checksum; structural corruption is caught,
//! bit rot within well-formed bytes is not.
//!
//! # Queries
//!
//! [`NavmeshArtifact::contains_point`] answers whether a world XZ point
//! is on walkable ground, and [`NavmeshArtifact::height_at`] gives the
//! ground height there, which is what validating a move needs. The module
//! does not pathfind; the polygons and their edge neighbors are a
//! pathfinder's input, and a game brings its own.
//!
//! Both queries scan every polygon or triangle. A caller asking thousands
//! of times a frame wants an index of its own over the same data.
//!
//! # Staleness
//!
//! [`SourceHasher`] builds the artifact's `source_hash`, so an editor can
//! tell whether the world moved under a bake. It covers the heights, the
//! terrain's footprint and sample grid, and every triangle of scene
//! geometry the bake rasterized, and nothing else: a repaint or a recolor
//! does not invalidate a bake, while a sculpt, a resize, and a moved prop
//! all do.

use std::collections::HashMap;

use bevy_math::{Affine3A, UVec2, Vec2, Vec3};

use crate::region::TerrainRegions;

/// Magic bytes at the head of every baked navmesh.
pub const MAGIC: [u8; 8] = *b"JDNAVMS\0";

/// Format version [`encode`] writes and [`decode`] reads.
pub const VERSION: u16 = 1;

/// Conventional file extension for a baked navmesh, beside the scene.
pub const EXTENSION: &str = "jdnav";

/// Edge neighbor value meaning "this edge is a border, not a crossing".
pub const NO_NEIGHBOR: u32 = u32::MAX;

/// Fixed-size part of the header, before the terrain path.
const HEADER_LEN: usize = 56;

/// A triangle mesh in terrain-local space, indexed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceMesh {
    pub vertices: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

impl SurfaceMesh {
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }
}

/// What a bake was asked for. Persisted with its result so a later
/// staleness check can say what the artifact on disk was built from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakeParams {
    /// Agent radius in world units. Walkable ground is eroded by this.
    pub agent_radius: f32,
    /// Agent height in world units. Ground with less clearance is dropped.
    pub agent_height: f32,
    /// Steepest ground an agent may stand on, in degrees.
    pub max_slope_degrees: f32,
    /// Voxel size on the horizontal plane, in world units. Derived from
    /// the terrain's own cell size by [`Self::with_terrain_cell`].
    pub cell_size: f32,
}

impl Default for BakeParams {
    fn default() -> Self {
        Self {
            agent_radius: 0.4,
            agent_height: 1.8,
            max_slope_degrees: 45.0,
            cell_size: 0.25,
        }
    }
}

impl BakeParams {
    /// Smallest voxel a bake may use. Below this a terrain-sized bake
    /// runs out of the u16 grid the baker addresses cells with.
    pub const MIN_CELL_SIZE: f32 = 0.05;

    /// Most voxels a bake spends on one axis.
    ///
    /// The grid is squared, so a voxel per quarter-cell that costs a
    /// second on a small terrain costs minutes on a kilometre of one. Past
    /// this width the voxel grows instead, and a wide terrain gets a
    /// coarser navmesh. At a kilometre of ground that lands on half a
    /// terrain cell; at one voxel per cell the baker cannot triangulate
    /// the contours it gets back.
    ///
    /// A budget, not a guarantee: it bounds the *widest* axis of the
    /// extent it is given, and an extent that is not a finite positive
    /// number leaves the voxel alone. What the baker can address is
    /// [`Self::ADDRESSABLE_GRID`], checked separately, since a grid past
    /// it is truncated rather than refused.
    pub const MAX_GRID: f32 = 2048.0;

    /// Widest grid the baker can address on one axis.
    ///
    /// The heightfield indexes columns by `u16` and derives its width
    /// with a saturating cast, so a wider grid is not an error there: it
    /// wraps to 65535 columns while the span buffer is still allocated
    /// for the untruncated count. The stride is then wrong for every row
    /// and the rasterization is wrong with it. A bake that would land past
    /// this refuses instead.
    pub const ADDRESSABLE_GRID: f32 = u16::MAX as f32;

    /// Fraction of a terrain cell one voxel spans.
    ///
    /// Four voxels per cell resolves the ground the terrain stores without
    /// paying for detail no height sample carries.
    const CELL_FRACTION: f32 = 0.25;

    /// The same params with [`Self::cell_size`] derived from a terrain's
    /// cell, so a coarse terrain does not get voxelized as if it were
    /// fine. The narrower axis wins: it is the one that would alias.
    pub fn with_terrain_cell(self, terrain_cell: Vec2) -> Self {
        let narrow = terrain_cell.x.min(terrain_cell.y);
        let derived = narrow * Self::CELL_FRACTION;
        Self {
            cell_size: if derived.is_finite() && derived >= Self::MIN_CELL_SIZE {
                derived
            } else {
                Self::MIN_CELL_SIZE
            },
            ..self
        }
    }

    /// The same params coarsened until a bake over `extent` world units
    /// fits the grid budget. A terrain wide enough to need this gets a
    /// coarser navmesh rather than a wrapped one.
    ///
    /// `extent` is the bake input's own span, not the terrain's: an
    /// obstacle past the terrain's edge, a region outside it, or a scaled
    /// terrain transform all widen what is rasterized, and it is that
    /// width the voxel has to pay for.
    pub fn fit_to_extent(self, extent: Vec2) -> Self {
        let widest = extent.x.max(extent.y);
        if !widest.is_finite() || widest <= 0.0 {
            return self;
        }
        let floor = widest / Self::MAX_GRID;
        Self {
            cell_size: self.cell_size.max(floor).max(Self::MIN_CELL_SIZE),
            ..self
        }
    }

    /// Whether these are numbers a bake could have been run at.
    ///
    /// An agent may be any size, so this rules out only the values that
    /// mean the file is wrong rather than unusual: infinities, NaN, a size
    /// at or below zero, a slope past vertical.
    pub fn is_plausible(self) -> bool {
        self.agent_radius.is_finite()
            && self.agent_radius > 0.0
            && self.agent_height.is_finite()
            && self.agent_height > 0.0
            && self.cell_size.is_finite()
            && self.cell_size > 0.0
            && self.max_slope_degrees.is_finite()
            && (0.0..=90.0).contains(&self.max_slope_degrees)
    }

    /// Voxel columns a bake over `extent` would span, or `None` when
    /// that is past [`Self::ADDRESSABLE_GRID`] on either axis.
    ///
    /// Rounds the way the baker's own heightfield does, so the count
    /// matches the one the baker will build.
    pub fn grid_span(self, extent: Vec2) -> Option<UVec2> {
        let span = |length: f32| -> Option<u32> {
            let columns = length / self.cell_size + 0.5;
            (columns.is_finite() && (0.0..=Self::ADDRESSABLE_GRID).contains(&columns))
                .then_some(columns as u32)
        };
        Some(UVec2::new(span(extent.x)?, span(extent.y)?))
    }

    /// The same params at half the voxel, or `None` when halving would
    /// spend more than the bake over `extent` is budgeted.
    ///
    /// A voxel too coarse for the ground under it is one reason a bake
    /// comes back empty. Halving costs four times the work, so it is
    /// offered only where the budget has room for it.
    pub fn halved_within_budget(self, extent: Vec2) -> Option<Self> {
        let finer = Self {
            cell_size: self.cell_size / 2.0,
            ..self
        };
        let widest = extent.x.max(extent.y);
        let over_budget = widest.is_finite() && widest / finer.cell_size > Self::MAX_GRID;
        (finer.cell_size >= Self::MIN_CELL_SIZE && !over_budget).then_some(finer)
    }
}

/// One walkable convex polygon of a baked navmesh.
#[derive(Clone, Debug, PartialEq)]
pub struct NavPolygon {
    /// Baker area id. Nonzero is walkable.
    pub area: u8,
    /// Indices into [`NavmeshArtifact::vertices`], in winding order.
    pub corners: Vec<u32>,
    /// Polygon reached across the edge starting at the corner of the same
    /// index, or [`NO_NEIGHBOR`].
    pub neighbors: Vec<u32>,
}

/// A baked navmesh, in world space.
#[derive(Clone, Debug, PartialEq)]
pub struct NavmeshArtifact {
    pub params: BakeParams,
    /// [`SourceHasher`] digest of the world this was baked from.
    pub source_hash: u64,
    /// Scene-relative path of the terrain data this was baked from, the
    /// same string the terrain carries. A staleness check needs to know
    /// which ground an artifact describes, and a scene with two terrains
    /// has two artifacts to tell apart.
    pub terrain: String,
    pub vertices: Vec<Vec3>,
    pub polygons: Vec<NavPolygon>,
    /// Walkable ground at the baker's detail resolution.
    pub surface_vertices: Vec<Vec3>,
    pub surface_triangles: Vec<[u32; 3]>,
}

impl NavmeshArtifact {
    pub fn is_empty(&self) -> bool {
        self.polygons.is_empty()
    }

    /// Whether the world XZ point `point` stands on walkable ground.
    ///
    /// Tested against the coarse polygons, which outline the walkable
    /// surface: a point outside every polygon is off the mesh. Polygons
    /// with area 0 are unwalkable and contain nothing.
    ///
    /// The test is a crossing count, so it holds for either winding and
    /// for a polygon that is not convex. A point within [`ON_EDGE`] of any
    /// edge is on the mesh outright; see that constant for what the
    /// crossing count alone would do to a seam.
    pub fn contains_point(&self, point: Vec2) -> bool {
        self.polygons
            .iter()
            .any(|polygon| polygon.area != 0 && self.polygon_contains(polygon, point))
    }

    /// Height of the walkable surface at the world XZ point `point`, or
    /// `None` where no surface triangle covers it.
    ///
    /// Read off the detail triangles rather than the coarse polygons: a
    /// polygon spans ground the terrain rolls across, and its corners say
    /// nothing about the height in the middle of it.
    ///
    /// One XZ point can have ground at several heights (a bridge, a ramp
    /// doubling back) and this returns the highest of them. Nothing in
    /// `point` says which deck the caller is on, so a mover walking under
    /// a bridge is told the height of the deck above it. Stacked levels
    /// need a query given a height to start from, which this is not.
    pub fn height_at(&self, point: Vec2) -> Option<f32> {
        let mut best: Option<f32> = None;
        for triangle in &self.surface_triangles {
            let Some(height) = self.triangle_height(*triangle, point) else {
                continue;
            };
            if best.is_none_or(|current| height > current) {
                best = Some(height);
            }
        }
        best
    }

    fn polygon_contains(&self, polygon: &NavPolygon, point: Vec2) -> bool {
        let corner = |index: usize| -> Option<Vec2> {
            polygon
                .corners
                .get(index)
                .and_then(|vertex| self.vertices.get(*vertex as usize))
                .map(flatten)
        };
        let count = polygon.corners.len();
        let mut inside = false;
        for index in 0..count {
            let (Some(a), Some(b)) = (corner(index), corner((index + count - 1) % count)) else {
                return false;
            };
            // Claimed before anything is counted. Widening the crossing
            // comparison by the same tolerance instead would flip the
            // parity of the whole polygon, counting an interior point that
            // close to a border edge out of the polygon it is inside.
            if on_edge(point, a, b) {
                return true;
            }
            // The edge crosses the ray cast along +x from the point when
            // its ends straddle the point's z. Half-open in z, so a
            // corner sitting on the ray is counted once rather than twice.
            if (a.y > point.y) != (b.y > point.y) {
                let along = (point.y - a.y) / (b.y - a.y);
                if point.x < a.x + along * (b.x - a.x) {
                    inside = !inside;
                }
            }
        }
        inside
    }

    fn triangle_height(&self, triangle: [u32; 3], point: Vec2) -> Option<f32> {
        let vertex = |index: u32| self.surface_vertices.get(index as usize).copied();
        let (a, b, c) = (
            vertex(triangle[0])?,
            vertex(triangle[1])?,
            vertex(triangle[2])?,
        );
        let (flat_a, flat_b, flat_c) = (flatten(&a), flatten(&b), flatten(&c));
        let ab = flat_b - flat_a;
        let ac = flat_c - flat_a;
        let ap = point - flat_a;
        // Twice the triangle's signed area on the XZ plane. Zero is a
        // triangle seen edge-on, which covers no ground to sample.
        let area = ab.x * ac.y - ac.x * ab.y;
        if area == 0.0 {
            return None;
        }
        let at_b = (ap.x * ac.y - ac.x * ap.y) / area;
        let at_c = (ab.x * ap.y - ap.x * ab.y) / area;
        let at_a = 1.0 - at_b - at_c;
        // A hair of tolerance, so a point on an edge two triangles share
        // samples from one of them rather than falling between them.
        // Unitless: these are weights, not distances.
        const SLACK: f32 = -1e-5;
        if at_a >= SLACK && at_b >= SLACK && at_c >= SLACK {
            Some(at_a * a.y + at_b * b.y + at_c * c.y)
        } else {
            None
        }
    }
}

/// How near an edge counts as standing on it, in world units.
///
/// Two polygons that share an edge compute that edge from opposite ends.
/// Along a diagonal the two answers are the same number in algebra and
/// not always the same float, so a point exactly on the seam can fall
/// outside the crossing count of both polygons and be dropped. Being on
/// an edge is therefore answered directly, before any crossing is
/// counted. Both polygons may claim such a point: the question is whether
/// *some* polygon holds it, not how many do.
pub const ON_EDGE: f32 = 1e-5;

/// Whether `point` stands on the segment `a`..`b`, within [`ON_EDGE`].
fn on_edge(point: Vec2, a: Vec2, b: Vec2) -> bool {
    let edge = b - a;
    let length = edge.length();
    if length == 0.0 {
        return point.distance(a) <= ON_EDGE;
    }
    if edge.perp_dot(point - a).abs() > ON_EDGE * length {
        return false;
    }
    let along = (point - a).dot(edge) / length;
    (-ON_EDGE..=length + ON_EDGE).contains(&along)
}

/// A world-space vertex as the XZ point the queries work on.
fn flatten(vertex: &Vec3) -> Vec2 {
    Vec2::new(vertex.x, vertex.z)
}

/// Why a baked navmesh could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavmeshError {
    /// The file does not start with [`MAGIC`]. Not a baked navmesh.
    BadMagic,
    /// Written by a newer jackdaw than this one.
    UnsupportedVersion(u16),
    /// A reserved field was non-zero, so the file means something this
    /// build does not understand.
    ReservedFieldSet,
    /// The file ended before the declared data did, or had bytes left
    /// over once every declared field was read.
    Truncated,
    /// The declared counts do not fit in this platform's address space.
    TooLarge,
    /// A polygon declared fewer than three corners.
    DegeneratePolygon,
    /// A text field was not valid UTF-8.
    BadText,
    /// A recorded bake parameter is not a number a bake could have run at.
    BadParams,
    /// An index does not address a vertex or a polygon that exists.
    IndexOutOfRange,
}

impl core::fmt::Display for NavmeshError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "not a baked navmesh (bad magic)"),
            Self::UnsupportedVersion(v) => {
                write!(f, "baked navmesh version {v} is newer than this build")
            }
            Self::ReservedFieldSet => write!(f, "baked navmesh sets a reserved field"),
            Self::Truncated => write!(f, "baked navmesh is truncated"),
            Self::TooLarge => write!(f, "baked navmesh declares more data than fits in memory"),
            Self::DegeneratePolygon => write!(f, "baked navmesh polygon has fewer than 3 corners"),
            Self::BadText => write!(
                f,
                "baked navmesh names its terrain in something that is not text"
            ),
            Self::BadParams => write!(f, "baked navmesh records params no bake could have run at"),
            Self::IndexOutOfRange => write!(f, "baked navmesh index addresses nothing"),
        }
    }
}

impl core::error::Error for NavmeshError {}

/// Full-resolution triangle mesh of every present region.
///
/// `cell_size` is the terrain's world size per cell and `origin` is the
/// terrain-local XZ of grid vertex `(0, 0)`, wherever the terrain's grid
/// places it.
pub fn surface_mesh(regions: &TerrainRegions, cell_size: Vec2, origin: Vec2) -> SurfaceMesh {
    let side = regions.region_size().get() as i32;
    let mut mesh = SurfaceMesh::default();
    let mut seen: HashMap<(i32, i32), u32> = HashMap::new();

    for (coord, _) in regions.iter_sorted() {
        let base_x = coord.x * side;
        let base_z = coord.z * side;
        for lz in 0..side {
            for lx in 0..side {
                let (x, z) = (base_x + lx, base_z + lz);
                let corners = [(x, z), (x + 1, z), (x, z + 1), (x + 1, z + 1)];
                if !corners.iter().all(|(cx, cz)| regions.covers(*cx, *cz)) {
                    continue;
                }
                let mut index = |cx: i32, cz: i32| -> u32 {
                    *seen.entry((cx, cz)).or_insert_with(|| {
                        let next = mesh.vertices.len() as u32;
                        mesh.vertices.push(Vec3::new(
                            cx as f32 * cell_size.x + origin.x,
                            regions.height_at(cx, cz),
                            cz as f32 * cell_size.y + origin.y,
                        ));
                        next
                    })
                };
                // Wound so the face normal points up, which is what a
                // slope filter tests against.
                let tl = index(x, z);
                let tr = index(x + 1, z);
                let bl = index(x, z + 1);
                let br = index(x + 1, z + 1);
                mesh.triangles.push([tl, bl, tr]);
                mesh.triangles.push([tr, bl, br]);
            }
        }
    }

    mesh
}

/// Rolling content hash of everything a bake's result depends on.
///
/// FNV-1a, fed in a fixed order by whoever assembles a bake: the stored
/// ground, the shape it is laid over, then every triangle of scene
/// geometry that joined it. Paint, materials, and names are not ground
/// and are not fed, so a recolor does not invalidate a bake.
///
/// The result is written into the artifact and compared against a fresh
/// one to answer whether the world moved under the bake. That comparison
/// spans separate runs of the editor, so nothing assigned at load goes
/// in: geometry enters as its own placed vertices, never as an asset
/// handle.
#[derive(Clone, Copy, Debug)]
pub struct SourceHasher {
    hash: u64,
}

impl Default for SourceHasher {
    fn default() -> Self {
        Self {
            hash: 0xcbf2_9ce4_8422_2325,
        }
    }
}

impl SourceHasher {
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    pub fn eat(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(Self::PRIME);
        }
    }

    pub fn eat_u32(&mut self, value: u32) {
        self.eat(&value.to_le_bytes());
    }

    /// A float, canonicalized so a NaN payload or the sign on a zero does
    /// not read as different ground.
    pub fn eat_f32(&mut self, value: f32) {
        let bits = if value.is_nan() {
            f32::NAN.to_bits()
        } else if value == 0.0 {
            0
        } else {
            value.to_bits()
        };
        self.eat(&bits.to_le_bytes());
    }

    pub fn eat_vec3(&mut self, value: Vec3) {
        self.eat_f32(value.x);
        self.eat_f32(value.y);
        self.eat_f32(value.z);
    }

    /// Every present region's coordinate and heights. Order-independent
    /// of the map's iteration: regions are visited sorted. The raw height
    /// bits go in, so a height that round-trips through the sidecar
    /// hashes the same on the way back.
    pub fn eat_regions(&mut self, regions: &TerrainRegions) {
        self.eat_u32(regions.region_size().get());
        for (coord, region) in regions.iter_sorted() {
            self.eat(&coord.x.to_le_bytes());
            self.eat(&coord.z.to_le_bytes());
            for height in region.heights() {
                self.eat_f32(*height);
            }
        }
    }

    /// The terrain's world footprint and sample grid.
    ///
    /// `cell` is how far apart the terrain's cells are, and `extent` how
    /// many of them it stores per axis. Changing either moves every stored
    /// height somewhere new, so a terrain whose heights are byte-identical
    /// after a respacing is different ground under the same numbers.
    pub fn eat_shape(&mut self, cell: Vec2, extent: u32) {
        self.eat_f32(cell.x);
        self.eat_f32(cell.y);
        self.eat_u32(extent);
    }

    /// One piece of scene geometry that joined the bake: where it stands
    /// and how much of it there is.
    ///
    /// The affine goes in whole, shear included, so any move, rotation,
    /// or scale invalidates the bake. The mesh enters as its own size
    /// rather than as an asset handle, whose id is handed out at load and
    /// would make every artifact on disk read as out of date when its
    /// scene was reopened. A mesh re-authored to the same vertex and
    /// triangle count therefore reads as unchanged.
    pub fn eat_placement(&mut self, placement: Affine3A, vertices: u32, triangles: u32) {
        self.eat_affine(placement);
        self.eat_u32(vertices);
        self.eat_u32(triangles);
    }

    /// Where something stands, on its own.
    ///
    /// The terrain's own transform goes in this way. Its shape is already
    /// hashed through [`Self::eat_regions`] and [`Self::eat_shape`], so it
    /// has no vertex count to add, but a bake is taken in world space and
    /// moving, turning, or scaling the ground moves every polygon off it.
    pub fn eat_affine(&mut self, placement: Affine3A) {
        for column in [
            placement.matrix3.x_axis,
            placement.matrix3.y_axis,
            placement.matrix3.z_axis,
            placement.translation,
        ] {
            self.eat_vec3(column.into());
        }
    }

    /// Fold a set of geometry into the hash without depending on the
    /// order it was collected in: each piece is hashed on its own and the
    /// results are sorted before they go in. Query iteration order is not
    /// a property of a scene, so two props swapping places in it must not
    /// make a bake read as stale.
    pub fn eat_unordered(&mut self, mut hashes: Vec<u64>) {
        hashes.sort_unstable();
        self.eat_u32(hashes.len() as u32);
        for hash in hashes {
            self.eat(&hash.to_le_bytes());
        }
    }

    pub fn finish(&self) -> u64 {
        self.hash
    }
}

/// Serialize a baked navmesh.
pub fn encode(artifact: &NavmeshArtifact) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&artifact.params.agent_radius.to_le_bytes());
    out.extend_from_slice(&artifact.params.agent_height.to_le_bytes());
    out.extend_from_slice(&artifact.params.max_slope_degrees.to_le_bytes());
    out.extend_from_slice(&artifact.params.cell_size.to_le_bytes());
    out.extend_from_slice(&artifact.source_hash.to_le_bytes());
    out.extend_from_slice(&(artifact.terrain.len() as u32).to_le_bytes());
    out.extend_from_slice(&(artifact.vertices.len() as u32).to_le_bytes());
    out.extend_from_slice(&(artifact.polygons.len() as u32).to_le_bytes());
    out.extend_from_slice(&(artifact.surface_vertices.len() as u32).to_le_bytes());
    out.extend_from_slice(&(artifact.surface_triangles.len() as u32).to_le_bytes());
    out.extend_from_slice(artifact.terrain.as_bytes());

    for vertex in &artifact.vertices {
        write_vec3(&mut out, *vertex);
    }
    for polygon in &artifact.polygons {
        let corners = polygon.corners.len().min(u8::MAX as usize);
        out.push(polygon.area);
        out.push(corners as u8);
        out.extend_from_slice(&[0u8; 2]);
        for index in &polygon.corners[..corners] {
            out.extend_from_slice(&index.to_le_bytes());
        }
        for i in 0..corners {
            let neighbor = polygon.neighbors.get(i).copied().unwrap_or(NO_NEIGHBOR);
            out.extend_from_slice(&neighbor.to_le_bytes());
        }
    }
    for vertex in &artifact.surface_vertices {
        write_vec3(&mut out, *vertex);
    }
    for triangle in &artifact.surface_triangles {
        for index in triangle {
            out.extend_from_slice(&index.to_le_bytes());
        }
    }
    out
}

/// Read a baked navmesh, or refuse the whole file.
pub fn decode(bytes: &[u8]) -> Result<NavmeshArtifact, NavmeshError> {
    if bytes.len() < HEADER_LEN {
        return Err(NavmeshError::Truncated);
    }
    if bytes[..8] != MAGIC {
        return Err(NavmeshError::BadMagic);
    }

    let mut r = Reader { bytes, at: 8 };
    let version = r.u16()?;
    if version == 0 || version > VERSION {
        return Err(NavmeshError::UnsupportedVersion(version));
    }
    if r.u16()? != 0 {
        return Err(NavmeshError::ReservedFieldSet);
    }

    let params = BakeParams {
        agent_radius: r.f32()?,
        agent_height: r.f32()?,
        max_slope_degrees: r.f32()?,
        cell_size: r.f32()?,
    };
    // A reader compares these against the params it would ask for, and a
    // NaN compares equal to nothing. A file claiming a bake at an
    // impossible agent is refused whole rather than read and never
    // matched.
    if !params.is_plausible() {
        return Err(NavmeshError::BadParams);
    }
    let source_hash = r.u64()?;
    let terrain_len = r.u32()? as usize;
    let vertex_count = r.u32()? as usize;
    let polygon_count = r.u32()? as usize;
    let surface_vertex_count = r.u32()? as usize;
    let surface_triangle_count = r.u32()? as usize;
    let terrain = core::str::from_utf8(r.take(terrain_len)?)
        .map_err(|_| NavmeshError::BadText)?
        .to_string();

    // Counts are read from the file, so nothing is reserved up front: the
    // reader refuses to run past the end and the loop stops there.
    let mut vertices = Vec::new();
    for _ in 0..vertex_count {
        vertices.push(r.vec3()?);
    }

    let mut polygons = Vec::new();
    for _ in 0..polygon_count {
        let area = r.u8()?;
        let corners = r.u8()? as usize;
        if corners < 3 {
            return Err(NavmeshError::DegeneratePolygon);
        }
        if r.take(2)? != [0u8; 2] {
            return Err(NavmeshError::ReservedFieldSet);
        }
        let mut corner_indices = Vec::with_capacity(corners);
        for _ in 0..corners {
            let index = r.u32()?;
            if index as usize >= vertex_count {
                return Err(NavmeshError::IndexOutOfRange);
            }
            corner_indices.push(index);
        }
        let mut neighbors = Vec::with_capacity(corners);
        for _ in 0..corners {
            let index = r.u32()?;
            if index != NO_NEIGHBOR && index as usize >= polygon_count {
                return Err(NavmeshError::IndexOutOfRange);
            }
            neighbors.push(index);
        }
        polygons.push(NavPolygon {
            area,
            corners: corner_indices,
            neighbors,
        });
    }

    let mut surface_vertices = Vec::new();
    surface_vertices
        .try_reserve(surface_vertex_count.min(bytes.len() / 12))
        .map_err(|_| NavmeshError::TooLarge)?;
    for _ in 0..surface_vertex_count {
        surface_vertices.push(r.vec3()?);
    }

    let mut surface_triangles = Vec::new();
    for _ in 0..surface_triangle_count {
        let mut triangle = [0u32; 3];
        for index in &mut triangle {
            let value = r.u32()?;
            if value as usize >= surface_vertex_count {
                return Err(NavmeshError::IndexOutOfRange);
            }
            *index = value;
        }
        surface_triangles.push(triangle);
    }

    if r.at != bytes.len() {
        return Err(NavmeshError::Truncated);
    }

    Ok(NavmeshArtifact {
        params,
        source_hash,
        terrain,
        vertices,
        polygons,
        surface_vertices,
        surface_triangles,
    })
}

fn write_vec3(out: &mut Vec<u8>, value: Vec3) {
    out.extend_from_slice(&value.x.to_le_bytes());
    out.extend_from_slice(&value.y.to_le_bytes());
    out.extend_from_slice(&value.z.to_le_bytes());
}

/// Cursor over an artifact's bytes that refuses to read past the end.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], NavmeshError> {
        let end = self.at.checked_add(n).ok_or(NavmeshError::TooLarge)?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or(NavmeshError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, NavmeshError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, NavmeshError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, NavmeshError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, NavmeshError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn f32(&mut self) -> Result<f32, NavmeshError> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn vec3(&mut self) -> Result<Vec3, NavmeshError> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Control;
    use crate::region::{RegionCoord, RegionSize};

    fn regions_with(side: u32, coords: &[RegionCoord]) -> TerrainRegions {
        let mut regions = TerrainRegions::new(RegionSize::new(side).expect("power of two"));
        for coord in coords {
            regions.ensure_region(*coord);
        }
        regions
    }

    fn sample_artifact() -> NavmeshArtifact {
        NavmeshArtifact {
            params: BakeParams {
                agent_radius: 0.4,
                agent_height: 1.8,
                max_slope_degrees: 45.0,
                cell_size: 0.25,
            },
            source_hash: 0x0123_4567_89ab_cdef,
            terrain: "zone.terrain-0.jdterrain".to_string(),
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.5, 0.0),
                Vec3::new(1.0, 0.5, 1.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
            polygons: vec![
                NavPolygon {
                    area: 63,
                    corners: vec![0, 1, 2],
                    neighbors: vec![NO_NEIGHBOR, 1, NO_NEIGHBOR],
                },
                NavPolygon {
                    area: 63,
                    corners: vec![0, 2, 3],
                    neighbors: vec![0, NO_NEIGHBOR, NO_NEIGHBOR],
                },
            ],
            surface_vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.5, 0.0),
                Vec3::new(1.0, 0.5, 1.0),
            ],
            surface_triangles: vec![[0, 1, 2]],
        }
    }

    #[test]
    fn an_artifact_round_trips_through_its_bytes() {
        let artifact = sample_artifact();
        let bytes = encode(&artifact);
        assert_eq!(decode(&bytes), Ok(artifact));
    }

    #[test]
    fn an_empty_artifact_round_trips() {
        let artifact = NavmeshArtifact {
            params: BakeParams::default(),
            source_hash: 0,
            terrain: String::new(),
            vertices: Vec::new(),
            polygons: Vec::new(),
            surface_vertices: Vec::new(),
            surface_triangles: Vec::new(),
        };
        let bytes = encode(&artifact);
        assert_eq!(bytes.len(), HEADER_LEN);
        let decoded = decode(&bytes).expect("an empty artifact is still an artifact");
        assert!(decoded.is_empty());
    }

    #[test]
    fn every_prefix_of_an_artifact_is_rejected() {
        let bytes = encode(&sample_artifact());
        for cut in 1..bytes.len() {
            assert_eq!(
                decode(&bytes[..cut]),
                Err(NavmeshError::Truncated),
                "a {cut}-byte prefix must be refused, not read"
            );
        }
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut bytes = encode(&sample_artifact());
        bytes.push(0);
        assert_eq!(decode(&bytes), Err(NavmeshError::Truncated));
    }

    #[test]
    fn other_bytes_are_not_mistaken_for_an_artifact() {
        assert_eq!(decode(&[0u8; 64]), Err(NavmeshError::BadMagic));
        assert_eq!(decode(b"JDTERRN\0"), Err(NavmeshError::Truncated));
    }

    #[test]
    fn a_newer_version_is_quarantined_rather_than_read() {
        let mut bytes = encode(&sample_artifact());
        bytes[8..10].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert_eq!(
            decode(&bytes),
            Err(NavmeshError::UnsupportedVersion(VERSION + 1))
        );
        bytes[8..10].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(decode(&bytes), Err(NavmeshError::UnsupportedVersion(0)));
    }

    #[test]
    fn a_set_reserved_field_is_refused() {
        let mut bytes = encode(&sample_artifact());
        bytes[10] = 1;
        assert_eq!(decode(&bytes), Err(NavmeshError::ReservedFieldSet));
    }

    #[test]
    fn params_no_bake_could_have_run_at_are_refused() {
        for broken in [
            BakeParams {
                agent_radius: 0.0,
                ..BakeParams::default()
            },
            BakeParams {
                agent_height: f32::NAN,
                ..BakeParams::default()
            },
            BakeParams {
                max_slope_degrees: 91.0,
                ..BakeParams::default()
            },
            BakeParams {
                max_slope_degrees: -1.0,
                ..BakeParams::default()
            },
            BakeParams {
                cell_size: f32::INFINITY,
                ..BakeParams::default()
            },
        ] {
            assert!(!broken.is_plausible(), "{broken:?}");
            let mut artifact = sample_artifact();
            artifact.params = broken;
            assert_eq!(decode(&encode(&artifact)), Err(NavmeshError::BadParams));
        }
        assert!(BakeParams::default().is_plausible());
    }

    #[test]
    fn an_artifact_names_the_terrain_it_describes() {
        let decoded = decode(&encode(&sample_artifact())).expect("round trips");
        assert_eq!(decoded.terrain, "zone.terrain-0.jdterrain");
    }

    #[test]
    fn a_terrain_path_that_is_not_text_is_refused() {
        let mut artifact = sample_artifact();
        artifact.terrain = "ab".to_string();
        let mut bytes = encode(&artifact);
        let at = bytes
            .windows(2)
            .position(|w| w == b"ab")
            .expect("the path is in there");
        bytes[at] = 0xff;
        assert_eq!(decode(&bytes), Err(NavmeshError::BadText));
    }

    #[test]
    fn a_polygon_with_too_few_corners_is_refused() {
        let mut artifact = sample_artifact();
        artifact.polygons[0].corners.pop();
        artifact.polygons[0].neighbors.pop();
        let bytes = encode(&artifact);
        assert_eq!(decode(&bytes), Err(NavmeshError::DegeneratePolygon));
    }

    #[test]
    fn an_index_that_addresses_nothing_is_refused() {
        let mut artifact = sample_artifact();
        artifact.polygons[0].corners[0] = 99;
        assert_eq!(
            decode(&encode(&artifact)),
            Err(NavmeshError::IndexOutOfRange)
        );

        let mut artifact = sample_artifact();
        artifact.polygons[0].neighbors[1] = 7;
        assert_eq!(
            decode(&encode(&artifact)),
            Err(NavmeshError::IndexOutOfRange)
        );

        let mut artifact = sample_artifact();
        artifact.surface_triangles[0][2] = 5;
        assert_eq!(
            decode(&encode(&artifact)),
            Err(NavmeshError::IndexOutOfRange)
        );
    }

    #[test]
    fn one_region_meshes_its_own_cells_and_stops_at_its_edge() {
        let regions = regions_with(4, &[RegionCoord::ORIGIN]);
        let mesh = surface_mesh(&regions, Vec2::ONE, Vec2::ZERO);
        // 4 cells per side gives a 4x4 sample grid, so 3x3 quads.
        assert_eq!(mesh.triangles.len(), 3 * 3 * 2);
        assert_eq!(mesh.vertices.len(), 4 * 4);
    }

    #[test]
    fn an_absent_region_contributes_nothing() {
        let lone = surface_mesh(
            &regions_with(4, &[RegionCoord::ORIGIN]),
            Vec2::ONE,
            Vec2::ZERO,
        );
        let with_gap = surface_mesh(
            &regions_with(4, &[RegionCoord::ORIGIN, RegionCoord::new(5, 5)]),
            Vec2::ONE,
            Vec2::ZERO,
        );
        // The far region meshes its own cells and nothing bridges the gap.
        assert_eq!(with_gap.triangles.len(), lone.triangles.len() * 2);
        assert_eq!(with_gap.vertices.len(), lone.vertices.len() * 2);
    }

    #[test]
    fn no_regions_at_all_mesh_to_nothing() {
        let mesh = surface_mesh(&regions_with(4, &[]), Vec2::ONE, Vec2::ZERO);
        assert!(mesh.is_empty());
        assert!(mesh.vertices.is_empty());
    }

    #[test]
    fn adjacent_regions_share_their_seam_vertices() {
        let regions = regions_with(4, &[RegionCoord::ORIGIN, RegionCoord::new(1, 0)]);
        let mesh = surface_mesh(&regions, Vec2::ONE, Vec2::ZERO);
        // 8x4 samples, so 7x3 quads, and the seam column is meshed once.
        assert_eq!(mesh.triangles.len(), 7 * 3 * 2);
        assert_eq!(mesh.vertices.len(), 8 * 4);
    }

    #[test]
    fn vertices_land_where_the_terrain_puts_them() {
        let mut regions = regions_with(4, &[RegionCoord::ORIGIN]);
        regions.set_height(2, 1, 3.5);
        let mesh = surface_mesh(&regions, Vec2::new(10.0, 10.0), Vec2::new(-15.0, -15.0));
        let peak = mesh
            .vertices
            .iter()
            .find(|v| v.y == 3.5)
            .expect("the sculpted vertex is in the mesh");
        assert_eq!(*peak, Vec3::new(5.0, 3.5, -5.0));
    }

    #[test]
    fn faces_point_up() {
        let regions = regions_with(4, &[RegionCoord::ORIGIN]);
        let mesh = surface_mesh(&regions, Vec2::ONE, Vec2::ZERO);
        for triangle in &mesh.triangles {
            let [a, b, c] = triangle.map(|i| mesh.vertices[i as usize]);
            assert!((b - a).cross(c - a).y > 0.0, "wound the wrong way");
        }
    }

    fn ground_hash(regions: &TerrainRegions) -> u64 {
        let mut hasher = SourceHasher::default();
        hasher.eat_regions(regions);
        hasher.finish()
    }

    #[test]
    fn a_sculpt_changes_the_source_hash_and_a_repaint_does_not() {
        let mut regions = regions_with(4, &[RegionCoord::ORIGIN]);
        let before = ground_hash(&regions);
        regions.set_control(1, 1, Control::from_raw(7));
        assert_eq!(ground_hash(&regions), before, "paint is not ground");
        regions.set_height(1, 1, 2.0);
        assert_ne!(ground_hash(&regions), before);
    }

    #[test]
    fn the_source_hash_ignores_the_sign_on_a_zero() {
        let mut regions = regions_with(4, &[RegionCoord::ORIGIN]);
        let flat = ground_hash(&regions);
        regions
            .region_mut(RegionCoord::ORIGIN)
            .expect("present")
            .heights_mut()[3] = -0.0;
        assert_eq!(ground_hash(&regions), flat);
    }

    #[test]
    fn a_new_region_changes_the_source_hash() {
        let one = ground_hash(&regions_with(4, &[RegionCoord::ORIGIN]));
        let two = ground_hash(&regions_with(
            4,
            &[RegionCoord::ORIGIN, RegionCoord::new(1, 0)],
        ));
        assert_ne!(one, two);
    }

    /// A quantize-apply can resize a terrain without touching one stored
    /// height. The ground it describes is different all the same, so the
    /// shape is hashed alongside the numbers.
    #[test]
    fn a_resize_with_the_same_heights_changes_the_source_hash() {
        let regions = regions_with(4, &[RegionCoord::ORIGIN]);
        let hash_at = |size: Vec2, resolution: u32| {
            let mut hasher = SourceHasher::default();
            hasher.eat_regions(&regions);
            hasher.eat_shape(size, resolution);
            hasher.finish()
        };
        let before = hash_at(Vec2::splat(100.0), 256);
        assert_ne!(before, hash_at(Vec2::splat(128.0), 256), "resized");
        assert_ne!(before, hash_at(Vec2::splat(100.0), 512), "resampled");
        assert_eq!(before, hash_at(Vec2::splat(100.0), 256), "unchanged");
    }

    fn placement_hash(placement: Affine3A) -> u64 {
        let mut hasher = SourceHasher::default();
        hasher.eat_placement(placement, 8, 12);
        hasher.finish()
    }

    #[test]
    fn moving_scene_geometry_changes_the_source_hash() {
        let here = placement_hash(Affine3A::IDENTITY);
        assert_eq!(here, placement_hash(Affine3A::IDENTITY));
        assert_ne!(here, placement_hash(Affine3A::from_translation(Vec3::Y)));
        assert_ne!(here, placement_hash(Affine3A::from_scale(Vec3::splat(2.0))));

        let mut sheared = Affine3A::IDENTITY;
        sheared.matrix3.x_axis.z = 0.5;
        assert_ne!(here, placement_hash(sheared), "shear is placement too");
    }

    #[test]
    fn the_order_geometry_was_collected_in_is_not_part_of_the_scene() {
        let hash_of = |order: &[u64]| {
            let mut hasher = SourceHasher::default();
            hasher.eat_unordered(order.to_vec());
            hasher.finish()
        };
        assert_eq!(hash_of(&[1, 2, 3]), hash_of(&[3, 1, 2]));
        assert_ne!(hash_of(&[1, 2, 3]), hash_of(&[1, 2, 4]));
        assert_ne!(hash_of(&[1, 2, 3]), hash_of(&[1, 2]), "one prop fewer");
    }

    #[test]
    fn nothing_at_all_still_hashes_to_something_repeatable() {
        assert_eq!(
            SourceHasher::default().finish(),
            SourceHasher::default().finish()
        );
    }

    #[test]
    fn the_voxel_follows_the_terrain_cell() {
        let params = BakeParams::default().with_terrain_cell(Vec2::new(2.0, 4.0));
        assert_eq!(params.cell_size, 0.5);
        let coarse = BakeParams::default().with_terrain_cell(Vec2::splat(0.01));
        assert_eq!(coarse.cell_size, BakeParams::MIN_CELL_SIZE);
    }

    #[test]
    fn a_terrain_too_wide_for_the_grid_bakes_coarser_instead_of_unbounded() {
        let params = BakeParams::default()
            .with_terrain_cell(Vec2::splat(0.4))
            .fit_to_extent(Vec2::splat(120_000.0));
        assert!(params.cell_size >= 120_000.0 / BakeParams::MAX_GRID);
        let small = BakeParams::default().fit_to_extent(Vec2::splat(100.0));
        assert_eq!(small.cell_size, BakeParams::default().cell_size);
    }

    #[test]
    fn the_grid_span_is_the_one_the_baker_would_build() {
        let params = BakeParams {
            cell_size: 0.5,
            ..BakeParams::default()
        };
        assert_eq!(
            params.grid_span(Vec2::new(100.0, 50.0)),
            Some(UVec2::new(200, 100))
        );
    }

    #[test]
    fn a_grid_the_baker_cannot_address_has_no_span() {
        let fine = BakeParams {
            cell_size: BakeParams::MIN_CELL_SIZE,
            ..BakeParams::default()
        };
        // One distant prop is enough: the extent is the input's, not the
        // terrain's, and at the finest voxel this is past the u16 the
        // baker indexes columns with.
        assert_eq!(fine.grid_span(Vec2::splat(1_000_000.0)), None);
        assert_eq!(fine.grid_span(Vec2::new(10.0, f32::INFINITY)), None);
        assert_eq!(fine.grid_span(Vec2::splat(f32::NAN)), None);
    }

    #[test]
    fn a_fitted_bake_always_has_a_span_the_baker_can_address() {
        for extent in [1.0f32, 100.0, 4096.0, 250_000.0] {
            let extent = Vec2::splat(extent);
            let params = BakeParams::default().fit_to_extent(extent);
            let span = params.grid_span(extent).expect("fitted, so addressable");
            assert!(span.x as f32 <= BakeParams::MAX_GRID + 1.0, "{span}");
        }
    }

    #[test]
    fn halving_the_voxel_stops_at_the_budget() {
        // A terrain the budget already had to coarsen has nothing left to
        // spend: halving would take the grid past the limit the
        // coarsening holds it to.
        let kilometre = Vec2::splat(1024.0);
        let spent = BakeParams::default()
            .with_terrain_cell(Vec2::splat(1.0))
            .fit_to_extent(kilometre);
        assert_eq!(spent.halved_within_budget(kilometre), None);

        // A terrain well inside it can afford the retry.
        let small = Vec2::splat(100.0);
        let params = BakeParams::default().fit_to_extent(small);
        let finer = params
            .halved_within_budget(small)
            .expect("a small terrain has room for a finer bake");
        assert_eq!(finer.cell_size, params.cell_size / 2.0);
    }

    #[test]
    fn the_voxel_never_halves_below_its_floor() {
        let floor = BakeParams {
            cell_size: BakeParams::MIN_CELL_SIZE,
            ..BakeParams::default()
        };
        assert_eq!(floor.halved_within_budget(Vec2::splat(1.0)), None);
    }

    #[test]
    fn a_kilometre_of_ground_bakes_half_a_voxel_per_cell() {
        let cell = Vec2::splat(1024.0 / 1023.0);
        let params = BakeParams::default()
            .with_terrain_cell(cell)
            .fit_to_extent(Vec2::splat(1024.0));
        assert!(params.cell_size >= 0.5, "{}", params.cell_size);
        assert!(params.cell_size <= cell.x * 0.51, "{}", params.cell_size);
    }
}

/// The two move-validation queries, over a decoded artifact: a game holds
/// a decoded file, not a struct built in memory.
#[cfg(test)]
mod query_tests {
    use super::*;

    /// Walkable ground over `x` and `z` in 0..2, and a second square of
    /// unwalkable ground at `x` in 4..6. Its detail surface is a ramp
    /// with a different height at each corner, so an interpolation that
    /// weighted the corners wrongly could not land on the same answer.
    fn ramp() -> NavmeshArtifact {
        let artifact = NavmeshArtifact {
            params: BakeParams::default(),
            source_hash: 7,
            terrain: "zone.terrain-0.jdterrain".to_string(),
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(2.0, 0.0, 2.0),
                Vec3::new(0.0, 0.0, 2.0),
                Vec3::new(4.0, 0.0, 0.0),
                Vec3::new(6.0, 0.0, 0.0),
                Vec3::new(6.0, 0.0, 2.0),
                Vec3::new(4.0, 0.0, 2.0),
            ],
            polygons: vec![
                NavPolygon {
                    area: 63,
                    corners: vec![0, 1, 2, 3],
                    neighbors: vec![NO_NEIGHBOR; 4],
                },
                NavPolygon {
                    area: 0,
                    corners: vec![4, 5, 6, 7],
                    neighbors: vec![NO_NEIGHBOR; 4],
                },
            ],
            surface_vertices: vec![
                Vec3::new(0.0, 0.5, 0.0),
                Vec3::new(2.0, 1.0, 0.0),
                Vec3::new(2.0, 4.0, 2.0),
                Vec3::new(0.0, 3.0, 2.0),
            ],
            surface_triangles: vec![[0, 1, 2], [0, 2, 3]],
        };
        decode(&encode(&artifact)).expect("a fresh artifact decodes")
    }

    #[test]
    fn a_point_over_walkable_ground_is_on_the_mesh() {
        let mesh = ramp();

        assert!(mesh.contains_point(Vec2::new(1.0, 1.0)));
        assert!(mesh.contains_point(Vec2::new(0.1, 1.9)));
    }

    #[test]
    fn a_point_off_every_polygon_is_off_the_mesh() {
        let mesh = ramp();

        assert!(!mesh.contains_point(Vec2::new(3.0, 1.0)));
        assert!(!mesh.contains_point(Vec2::new(1.0, -0.5)));
        assert!(!mesh.contains_point(Vec2::new(2.5, 2.5)));
    }

    /// Area 0 is ground the baker rejected. It is in the artifact so a
    /// consumer can see it, not so an agent can stand on it.
    #[test]
    fn unwalkable_ground_holds_nobody() {
        let mesh = ramp();

        assert!(!mesh.contains_point(Vec2::new(5.0, 1.0)));
    }

    /// A baker may wind its polygons either way, so a query that answered
    /// for one winding would fail on half the artifacts it is handed.
    #[test]
    fn a_polygon_wound_the_other_way_holds_the_same_points() {
        let mut reversed = ramp();
        for polygon in &mut reversed.polygons {
            polygon.corners.reverse();
            polygon.neighbors.reverse();
        }
        let reversed = decode(&encode(&reversed)).expect("a rewound artifact decodes");

        for point in [Vec2::new(1.0, 1.0), Vec2::new(3.0, 1.0)] {
            assert_eq!(reversed.contains_point(point), ramp().contains_point(point));
        }
    }

    /// The ramp's plane, sampled where all three corners of a triangle
    /// carry weight: at (1.5, 0.5) they weigh 0.25, 0.5 and 0.25.
    #[test]
    fn the_height_over_a_ramp_is_interpolated_across_its_corners() {
        let mesh = ramp();

        let height = mesh.height_at(Vec2::new(1.5, 0.5)).expect("on the surface");

        assert!((height - 1.625).abs() < 1e-5, "{height}");
    }

    #[test]
    fn the_corners_of_the_surface_sample_their_own_heights() {
        let mesh = ramp();

        for vertex in &mesh.surface_vertices.clone() {
            let height = mesh
                .height_at(Vec2::new(vertex.x, vertex.z))
                .expect("a corner is on the surface");
            assert!((height - vertex.y).abs() < 1e-5, "{height} at {vertex}");
        }
    }

    #[test]
    fn a_point_off_the_surface_has_no_height() {
        let mesh = ramp();

        assert_eq!(mesh.height_at(Vec2::new(5.0, 1.0)), None);
        assert_eq!(mesh.height_at(Vec2::new(-0.1, 1.0)), None);
    }

    /// Ground stacked over ground: the top deck comes back, which is the
    /// limit [`NavmeshArtifact::height_at`] documents. Pinned so the limit
    /// stays where the doc says it is.
    #[test]
    fn stacked_surfaces_sample_the_higher_one() {
        let mut stacked = ramp();
        let base = stacked.surface_vertices.len() as u32;
        stacked.surface_vertices.extend([
            Vec3::new(0.0, 9.0, 0.0),
            Vec3::new(2.0, 9.0, 0.0),
            Vec3::new(2.0, 9.0, 2.0),
        ]);
        stacked.surface_triangles.push([base, base + 1, base + 2]);
        let stacked = decode(&encode(&stacked)).expect("a stacked artifact decodes");

        assert_eq!(stacked.height_at(Vec2::new(1.5, 0.5)), Some(9.0));
    }

    /// Two polygons sharing a diagonal seam, which is where a crossing
    /// count alone loses points: each polygon computes the seam from its
    /// own end, and the two answers are the same number in algebra and
    /// not always the same float.
    fn seam() -> NavmeshArtifact {
        let artifact = NavmeshArtifact {
            params: BakeParams::default(),
            source_hash: 0,
            terrain: String::new(),
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(3.0, 0.0, 0.0),
                Vec3::new(3.0, 0.0, 7.0),
                Vec3::new(0.0, 0.0, 7.0),
            ],
            polygons: vec![
                NavPolygon {
                    area: 63,
                    corners: vec![0, 1, 2],
                    neighbors: vec![NO_NEIGHBOR, NO_NEIGHBOR, 1],
                },
                NavPolygon {
                    area: 63,
                    corners: vec![0, 2, 3],
                    neighbors: vec![1, NO_NEIGHBOR, NO_NEIGHBOR],
                },
            ],
            surface_vertices: Vec::new(),
            surface_triangles: Vec::new(),
        };
        decode(&encode(&artifact)).expect("a seam artifact decodes")
    }

    #[test]
    fn a_point_on_a_diagonal_seam_is_on_the_mesh() {
        let mesh = seam();

        for step in 0..=64 {
            let along = step as f32 / 64.0;
            let point = Vec2::new(3.0 * along, 7.0 * along);
            assert!(mesh.contains_point(point), "{point} is on the seam");
        }
    }

    /// The seam is claimed; the ground past the square is not.
    #[test]
    fn claiming_the_seam_does_not_claim_the_ground_around_it() {
        let mesh = seam();

        assert!(!mesh.contains_point(Vec2::new(-0.5, 3.5)));
        assert!(!mesh.contains_point(Vec2::new(3.5, 3.5)));
        assert!(!mesh.contains_point(Vec2::new(1.5, 7.5)));
    }
}
