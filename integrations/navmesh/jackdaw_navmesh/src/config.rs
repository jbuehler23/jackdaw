use crate::ops::*;
use crate::{Aabb3d, BuildContoursFlags};

/// Specifies a configuration to use when performing Recast builds. Usually built using [`ConfigBuilder`].
///
/// This is a convenience structure that represents an aggregation of parameters used at different stages in the Recast build process.
/// Some values are derived during the build process. Not all parameters are used for all build processes.
///
/// Units are usually in voxels (vx) or world units (wu). The units for voxels, grid size,
/// and cell size are all based on the values of cs and ch.
///
/// In this documentation, the term 'field' refers to heightfield and contour data structures that define spacial information
///  using an integer grid.
///
/// The upper and lower limits for the various parameters often depend on the platform's floating point accuracy as
/// well as interdependencies between the values of multiple parameters. See the individual parameter documentation for details.
///
/// > Note:
/// >
/// > First you should decide the size of your agent's logical cylinder.
/// > If your game world uses meters as units, a reasonable starting point for a human-sized agent
/// > might be a radius of 0.4 and a height of 2.0.

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The width of the field along the x-axis. `[Limit: >= 0] [Units: vx]`
    pub width: u16,

    /// The height of the field along the z-axis. `[Limit: >= 0] [Units: vx]`
    pub height: u16,

    /// The width/height size of tiles on the xz-plane. `[Limit: >= 0] [Units: vx]`
    ///
    /// This field is only used when building multi-tile meshes.
    pub tile_size: u16,

    /// The size of the non-navigable border around the heightfield. `[Limit: >=0] [Units: vx]`
    ///
    /// This value represents the the closest the walkable area of the heightfield should come to the xz-plane AABB of the field.
    /// It does not have any impact on the borders around internal obstructions.
    pub border_size: u16,

    /// The xz-plane cell size to use for fields. `[Limit: > 0] [Units: wu]`.
    ///
    /// The voxelization cell size defines the voxel size along both axes of the ground plane: x and z in Recast.
    /// This value is usually derived from the character radius r. A recommended starting value for cell_size is either r/2 or r/3.
    /// Smaller values of cell_size will increase rasterization resolution and navmesh detail, but total generation time will increase exponentially.
    /// In outdoor environments, r/2 is often good enough. For indoor scenes with tight spaces you might want the extra precision,
    /// so a value of r/3 or smaller may give better results.
    ///
    /// The initial instinct is to reduce this value to something very close to zero to maximize the detail of the generated navmesh.
    /// This quickly becomes a case of diminishing returns, however. Beyond a certain point there's usually not much perceptable difference
    /// in the generated navmesh, but huge increases in generation time.
    /// This hinders your ability to quickly iterate on level designs and provides little benefit.
    /// The general recommendation here is to use as large a value for cell_size as you can get away with.
    ///
    /// cell_size and cell_height define voxel/grid/cell size. So their values have significant side effects on all parameters defined in voxel units.
    ///
    /// The minimum value for this parameter depends on the platform's floating point accuracy,
    /// with the practical minimum usually around 0.05.
    pub cell_size: f32,

    /// The y-axis cell size to use for fields. `[Limit: > 0] [Units: wu]`
    ///
    /// The voxelization cell height is defined separately in order to allow for greater precision in height tests.
    /// A good starting point for cell_height is half the cell_size value.
    /// Smaller cell_height values ensure that the navmesh properly connects areas that are only separated by a small curb or ditch.
    /// If small holes are generated in your navmesh around where there are discontinuities in height (for example, stairs or curbs),
    /// you may want to decrease the cell height value to increase the vertical rasterization precision of rerecast.
    ///
    /// cell_size and cell_height define voxel/grid/cell size. So their values have significant side effects on all parameters defined in voxel units.
    ///
    /// The minimum value for this parameter depends on the platform's floating point accuracy, with the practical minimum usually around 0.05.
    pub cell_height: f32,

    /// The field's AABB [Units: wu]
    pub aabb: Aabb3d,

    /// The maximum slope that is considered walkable. `[Limits: 0 <= value < 0.5*π] [Units: Radians]`
    ///
    /// The parameter walkable_slope_angle is to filter out areas of the world where the ground slope
    /// would be too steep for an agent to traverse.
    /// This value is defined as a maximum angle in degrees that the surface normal of a polygon can differ from the world's up vector.
    /// This value must be within the range `[0, 90.0.to_radians()]`.
    ///
    /// The practical upper limit for this parameter is usually around `85.0.to_radians()`.
    pub walkable_slope_angle: f32,

    /// Minimum floor to 'ceiling' height that will still allow the floor area to
    /// be considered walkable. `[Limit: >= 3] [Units: vx]`
    ///
    /// This value defines the worldspace height h of the agent in voxels.
    /// The value of walkable_height should be calculated as `(h / cell_height).ceil()`.
    /// Note this is based on cell_height and not cell_size since it's a height value.
    ///
    /// Permits detection of overhangs in the source geometry that make the geometry below un-walkable.
    /// The value is usually set to the maximum agent height
    pub walkable_height: u16,

    /// Maximum ledge height that is considered to still be traversable. `[Limit: >=0] [Units: vx]`
    ///
    /// The walkable_climb value defines the maximum height of ledges and steps that the agent can walk up.
    /// Given a designer-defined `max_climb` distance in world units,
    /// the value of walkable_climb should be calculated as `(max_climb / cell_height).ceil()`.
    /// Note that this is using ch not cs because it's a height-based value.
    ///
    /// Allows the mesh to flow over low lying obstructions such as curbs and up/down stairways.
    /// The value is usually set to how far up/down an agent can step.
    pub walkable_climb: u16,

    /// The distance to erode/shrink the walkable area of the heightfield away from
    /// obstructions.  `[Limit: >=0] [Units: vx]`
    ///
    /// The parameter walkable_radius defines the worldspace agent radius r in voxels.
    /// Most often, this value of walkable_radius should be calculated as `(r / cell_size).ceil()`.
    /// Note this is based on cs since the agent radius is always parallel to the ground plane.
    ///
    /// If the walkable_radius value is greater than zero, the edges of the navmesh will be pushed away from all obstacles by this amount.
    ///
    /// A non-zero walkable_radius allows for much simpler runtime navmesh collision checks.
    /// The game only needs to check that the center point of the agent is contained within a navmesh polygon.
    /// Without this erosion, runtime navigation checks need to collide the geometric projection of the agent's
    /// logical cylinder onto the navmesh with the boundary edges of the navmesh polygons.
    ///
    /// In general, this is the closest any part of the final mesh should get to an obstruction in the source geometry.
    /// It is usually set to the maximum agent radius.
    ///
    /// If you want to have tight-fitting navmesh, or want to reuse the same navmesh for multiple agents with differing radii,
    /// you can use a walkable_radius value of zero. Be advised though that you will need to perform your own
    /// collisions with the navmesh edges, and odd edge cases issues in the mesh generation can potentially occur.
    /// For these reasons, specifying a radius of zero is allowed but is not recommended.
    pub walkable_radius: u16,

    /// The maximum allowed length for contour edges along the border of the mesh. `[Limit: >=0] [Units: vx]`
    ///
    /// In certain cases, long outer edges may decrease the quality of the resulting triangulation, creating very long thin triangles.
    /// This can sometimes be remedied by limiting the maximum edge length, causing the problematic long edges to be broken up into smaller segments.
    ///
    /// The parameter max_edge_len defines the maximum edge length and is defined in terms of voxels.
    /// A good value for max_edge_len is something like `walkable_radius * 8`.
    /// A good way to adjust this value is to first set it really high and see if your data creates long edges.
    /// If it does, decrease max_edge_len until you find the largest value which improves the resulting tesselation.
    ///
    /// Extra vertices will be inserted as needed to keep contour edges below this length.
    /// A value of zero effectively disables this feature.
    pub max_edge_len: u16,

    /// The maximum distance a simplified contour's border edges should deviate
    /// the original raw contour. `[Limit: >=0] [Units: vx]`
    ///
    /// When the rasterized areas are converted back to a vectorized representation,
    /// the max_simplification_error describes how loosely the simplification is done.
    /// The simplification process uses the Ramer–Douglas-Peucker algorithm, and this value describes the max deviation in voxels.
    ///
    /// Good values for max_simplification_error are in the range `[1.1, 1.5]`.
    /// A value of 1.3 is a good starting point and usually yields good results.
    /// If the value is less than 1.1, some sawtoothing starts to appear at the generated edges.
    /// If the value is more than 1.5, the mesh simplification starts to cut some corners it shouldn't.
    ///
    /// The effect of this parameter only applies to the xz-plane.
    pub max_simplification_error: f32,

    /// The minimum number of cells allowed to form isolated island areas. `[Limit: >=0] [Units: vx]`
    ///
    /// Watershed partitioning is really prone to noise in the input distance field.
    /// In order to get nicer areas, the areas are merged and small disconnected areas are removed after the water shed partitioning.
    /// The parameter min_region_area describes the minimum isolated region size that is still kept.
    /// A region is removed if the number of voxels in the region is less than min_region_area.
    ///
    /// Any regions that are smaller than this area will be marked as unwalkable.
    /// This is useful in removing useless regions that can sometimes form on geometry such as table tops, box tops, etc.
    pub min_region_area: u16,

    /// Any regions with a span count smaller than this value will, if possible,
    /// be merged with larger regions. `[Limit: >=0] [Units: vx]`
    ///
    /// The triangulation process works best with small, localized voxel regions.
    /// The parameter merge_region_area controls the maximum voxel area of a region that is allowed to be merged with another region.
    /// If you see small patches missing here and there, you could lower the [`Self::min_region_area`] value.
    pub merge_region_area: u16,

    /// The maximum number of vertices allowed for polygons generated during the
    /// contour to polygon conversion process. `[Limit: >= 3]`
    pub max_vertices_per_polygon: u16,

    /// Sets the sampling distance to use when generating the detail mesh.
    /// (For height detail only.) `[Limits: 0 or >= 0.9] [Units: wu]`
    pub detail_sample_dist: f32,

    /// The maximum distance the detail mesh surface should deviate from heightfield
    /// data. (For height detail only.) `[Limit: >=0] [Units: wu]`
    pub detail_sample_max_error: f32,

    /// Flags controlling the [`ContourSet`](crate::ContourSet) generation process.
    pub contour_flags: BuildContoursFlags,
}

/// A builder for [`Config`]. The config has lots of interdependent configurations,
/// so this builder provides a convenient way to set all the necessary parameters.
///
/// The most important parameters are:
/// - [`Self::agent_radius`]
/// - [`Self::agent_height`]
///
/// The default values are chosen to be reasonable for an agent resembling and adult human.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigBuilder {
    /// How many cells should fit in the [`Self::agent_radius`] on the xz-plane to use for fields. `[Limit: > 0]`.
    ///
    /// The voxelization cell size defines the voxel size along both axes of the ground plane: x and z in Recast.
    /// The resulting value is derived from the character radius r. For example, setting `cell_size_fraction` to 2 will result in the
    /// cell size being r/2, where r is [`Self::agent_radius`].
    ///
    /// A recommended starting value for cell_size is either 2 or 3.
    /// Larger values of cell_size will increase rasterization resolution and navmesh detail, but total generation time will increase exponentially.
    /// In outdoor environments, 2 is often good enough. For indoor scenes with tight spaces you might want the extra precision,
    /// so a value of 3 or higher may give better results.
    ///
    /// The initial instinct is to reduce this value to something very high to maximize the detail of the generated navmesh.
    /// This quickly becomes a case of diminishing returns, however. Beyond a certain point there's usually not much perceptable difference
    /// in the generated navmesh, but huge increases in generation time.
    /// This hinders your ability to quickly iterate on level designs and provides little benefit.
    /// The general recommendation here is to use as small a value for cell_size as you can get away with.
    ///
    /// [`Self::cell_size_fraction`] and [`Self::cell_height_fraction`] define voxel/grid/cell size. So their values have significant side effects on all parameters defined in voxel units.
    ///
    /// The maximum value for this parameter depends on the platform's floating point accuracy,
    /// with the practical maximum usually such that [`Self::agent_radius`] / [`Self::cell_size_fraction`] = 0.05.
    pub cell_size_fraction: f32,
    /// How many cells should fit in the [`Self::agent_height`] on the y-axis to use for fields. `[Limit: > 0]`
    ///
    /// The voxelization cell height is defined separately in order to allow for greater precision in height tests.
    /// A good starting point for [`Self::cell_height_fraction`] is twice the size of [`Self::cell_size_fraction`].
    /// Higher [`Self::cell_height_fraction`] values ensure that the navmesh properly connects areas that are only separated by a small curb or ditch.
    /// If small holes are generated in your navmesh around where there are discontinuities in height (for example, stairs or curbs),
    /// you may want to increase the cell height value to increase the vertical rasterization precision of rerecast.
    ///
    /// [`Self::cell_size_fraction`] and [`Self::cell_height_fraction`] define voxel/grid/cell size. So their values have significant side effects on all parameters defined in voxel units.
    ///
    /// The minimum value for this parameter depends on the platform's floating point accuracy, with the practical minimum usually usually such that [`Self::agent_radius`] / [`Self::cell_height_fraction`] = 0.05.
    pub cell_height_fraction: f32,
    /// The height of the agent. `[Limit: > 0] [Units: wu]`
    ///
    /// It's often a good idea to add a little bit of padding to the height. For example,
    /// an agent that is 1.8 world units tall might want to set this value to 2.0 units.
    pub agent_height: f32,
    /// The radius of the agent. `[Limit: > 0] [Units: wu]`
    pub agent_radius: f32,
    /// Maximum ledge height that is considered to still be traversable. `[Limit: >=0] [Units: wu]`
    ///
    /// The walkable_climb value defines the maximum height of ledges and steps that the agent can walk up.
    ///
    /// Allows the mesh to flow over low lying obstructions such as curbs and up/down stairways.
    /// The value is usually set to how far up/down an agent can step.
    pub walkable_climb: f32,
    /// The maximum slope that is considered walkable. `[Limits: 0 <= value < 0.5*π] [Units: Radians]`
    ///
    /// The parameter walkable_slope_angle is to filter out areas of the world where the ground slope
    /// would be too steep for an agent to traverse.
    /// This value is defined as a maximum angle in degrees that the surface normal of a polygon can differ from the world's up vector.
    /// This value must be within the range `[0, 90.0.to_radians()]`.
    ///
    /// The practical upper limit for this parameter is usually around `85.0.to_radians()`.
    pub walkable_slope_angle: f32,
    /// The minimum number of cells allowed to form isolated island areas along one horizontal axis. `[Limit: >=0] [Units: vx]`
    ///
    /// Watershed partitioning is really prone to noise in the input distance field.
    /// In order to get nicer areas, the areas are merged and small disconnected areas are removed after the water shed partitioning.
    /// The parameter [`Self::min_region_size`] describes the minimum isolated region size that is still kept.
    /// A region is removed if the number of voxels in the region is less than the square of [`Self::min_region_size`].
    ///
    /// Any regions that are smaller than this area will be marked as unwalkable.
    /// This is useful in removing useless regions that can sometimes form on geometry such as table tops, box tops, etc.
    pub min_region_size: u16,
    /// Any regions with a span count smaller than the square of this value will, if possible,
    /// be merged with larger regions. `[Limit: >=0] [Units: vx]`
    ///
    /// The triangulation process works best with small, localized voxel regions.
    /// The parameter [`Self::merge_region_size`] controls the maximum voxel area of a region that is allowed to be merged with another region.
    /// If you see small patches missing here and there, you could lower the [`Self::min_region_size`] value.
    pub merge_region_size: u16,
    /// The maximum allowed length for contour edges along the border of the mesh in terms of [`Self::agent_radius`]. `[Limit: >=0]`
    ///
    /// In certain cases, long outer edges may decrease the quality of the resulting triangulation, creating very long thin triangles.
    /// This can sometimes be remedied by limiting the maximum edge length, causing the problematic long edges to be broken up into smaller segments.
    ///
    /// The parameter [`Self::edge_max_len_factor`] defines the maximum edge length and is defined in terms of world units.
    /// A good value for [`Self::edge_max_len_factor`] is something like 8.
    /// A good way to adjust this value is to first set it really high and see if your data creates long edges.
    /// If it does, decrease [`Self::edge_max_len_factor`] until you find the largest value which improves the resulting tesselation.
    ///
    /// Extra vertices will be inserted as needed to keep contour edges below this length.
    /// A value of zero effectively disables this feature.
    pub edge_max_len_factor: u16,
    /// The maximum distance a simplified contour's border edges should deviate
    /// the original raw contour. `[Limit: >=0] [Units: vx]`
    ///
    /// When the rasterized areas are converted back to a vectorized representation,
    /// the max_simplification_error describes how loosely the simplification is done.
    /// The simplification process uses the Ramer–Douglas-Peucker algorithm, and this value describes the max deviation in voxels.
    ///
    /// Good values for max_simplification_error are in the range `[1.1, 1.5]`.
    /// A value of 1.3 is a good starting point and usually yields good results.
    /// If the value is less than 1.1, some sawtoothing starts to appear at the generated edges.
    /// If the value is more than 1.5, the mesh simplification starts to cut some corners it shouldn't.
    ///
    /// The effect of this parameter only applies to the xz-plane.
    pub max_simplification_error: f32,
    /// The maximum number of vertices allowed for polygons generated during the
    /// contour to polygon conversion process. `[Limit: >= 3]`
    pub max_vertices_per_polygon: u16,
    /// Sets the sampling distance to use when generating the detail mesh.
    /// (For height detail only.) `[Limits: 0 or >= 0.9] [Units: wu]`
    ///
    /// When this value is below 0.9, it will be clamped to 0.0.
    pub detail_sample_dist: f32,
    /// The maximum distance the detail mesh surface should deviate from heightfield
    /// data. (For height detail only.) `[Limit: >=0] [Units: wu]`
    pub detail_sample_max_error: f32,
    /// The width/height size of tiles on the xz-plane. `[Limit: >= 0] [Units: vx]`
    ///
    /// This field is only used when building multi-tile meshes, i.e. when [`Self::tiling`] is `true`.
    pub tile_size: u16,
    /// The navmesh's AABB [Units: wu]
    pub aabb: Aabb3d,
    /// Flags controlling the [`ContourSet`](crate::ContourSet) generation process.
    pub contour_flags: BuildContoursFlags,
    /// Whether the navmesh should be tiled or not.
    pub tiling: bool,
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self {
            cell_size_fraction: 3.0,
            cell_height_fraction: 4.0,
            agent_height: 1.75,
            agent_radius: 0.3,
            walkable_climb: 0.9,
            walkable_slope_angle: 45.0_f32.to_radians(),
            min_region_size: 8,
            merge_region_size: 20,
            edge_max_len_factor: 8,
            max_simplification_error: 1.3,
            max_vertices_per_polygon: 6,
            detail_sample_dist: 6.0,
            detail_sample_max_error: 1.0,
            tile_size: 32,
            aabb: Aabb3d::default(),
            contour_flags: BuildContoursFlags::default(),
            tiling: false,
        }
    }
}

impl ConfigBuilder {
    /// Builds a [`Config`] from the current configuration.
    pub fn build(self) -> Config {
        let cell_size = self.agent_radius / self.cell_size_fraction;
        let cell_height = self.agent_radius / self.cell_height_fraction;
        let walkable_radius = ceil(self.agent_radius / cell_size) as u16;
        // Reserve enough padding.
        let border_size = walkable_radius + 3;
        Config {
            width: if self.tiling {
                self.tile_size + border_size * 2
            } else {
                ((self.aabb.max.x - self.aabb.min.x) / cell_size + 0.5) as u16
            },
            height: if self.tiling {
                self.tile_size + border_size * 2
            } else {
                ((self.aabb.max.z - self.aabb.min.z) / cell_size + 0.5) as u16
            },
            tile_size: self.tile_size,
            border_size,
            cell_size,
            cell_height,
            aabb: self.aabb,
            walkable_slope_angle: self.walkable_slope_angle,
            walkable_height: ceil(self.agent_height / cell_height) as u16,
            walkable_climb: floor(self.walkable_climb / cell_height) as u16,
            walkable_radius,
            max_edge_len: walkable_radius * self.edge_max_len_factor,
            max_simplification_error: self.max_simplification_error,
            min_region_area: (self.min_region_size * self.min_region_size),
            merge_region_area: (self.merge_region_size * self.merge_region_size),
            max_vertices_per_polygon: self.max_vertices_per_polygon,
            detail_sample_dist: if self.detail_sample_dist < 0.9 {
                0.0
            } else {
                cell_size * self.detail_sample_dist
            },
            detail_sample_max_error: cell_height * self.detail_sample_max_error,
            contour_flags: self.contour_flags,
        }
    }
}
