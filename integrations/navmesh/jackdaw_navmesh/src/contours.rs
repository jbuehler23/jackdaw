use alloc::vec::Vec;

use glam::{U16Vec3, Vec3Swizzles};

use crate::{
    Aabb3d, AreaType, CompactHeightfield, RegionId,
    math::{dir_offset_x, dir_offset_z, distance_squared_between_point_and_line_u16vec2},
};

impl CompactHeightfield {
    /// The raw contours will match the region outlines exactly. The `max_error` and `max_edge_len`
    /// parameters control how closely the simplified contours will match the raw contours.
    ///
    /// Simplified contours are generated such that the vertices for portals between areas match up.
    /// (They are considered mandatory vertices.)
    ///
    /// Setting `max_edge_length` to zero will disabled the edge length feature.
    pub fn build_contours(
        &self,
        max_error: f32,
        max_edge_len: u16,
        build_flags: BuildContoursFlags,
    ) -> ContourSet {
        let mut cset = ContourSet {
            contours: Vec::new(),
            aabb: self.aabb,
            cell_size: self.cell_size,
            cell_height: self.cell_height,
            width: self.width - self.border_size * 2,
            height: self.height - self.border_size * 2,
            border_size: self.border_size,
            max_error,
        };
        if self.border_size > 0 {
            // If the heightfield was built with border_size, remove the offset
            let pad = self.border_size as f32 * self.cell_size;
            cset.aabb.min.x += pad;
            cset.aabb.min.z += pad;
            cset.aabb.max.x -= pad;
            cset.aabb.max.z -= pad;
        }

        let mut max_contours = self.max_region.bits().max(8);

        cset.contours = vec![Contour::default(); max_contours as usize];
        // We will shrink contours to this value later
        let mut contour_count = 0;
        let mut flags = vec![0_u8; self.spans.len()];

        // Mark boundaries
        for z in 0..self.height {
            for x in 0..self.width {
                let cell = &self.cell_at(x, z);
                for i in cell.index_range() {
                    let mut res = 0;
                    let span = &self.spans[i];
                    if span.region == RegionId::NONE
                        || span.region.contains(RegionId::BORDER_REGION)
                    {
                        flags[i] = 0;
                        continue;
                    }
                    for dir in 0..4 {
                        let mut r = RegionId::NONE;
                        if let Some(con) = span.con(dir) {
                            let a_x = x as i32 + dir_offset_x(dir) as i32;
                            let a_z = z as i32 + dir_offset_z(dir) as i32;
                            let cell_index = (a_x + a_z * self.width as i32) as usize;
                            let a_i = self.cells[cell_index].index() as usize + con as usize;
                            r = self.spans[a_i].region;
                        }
                        if r == self.spans[i].region {
                            res |= 1 << dir;
                        }
                    }
                    // Inverse, mark non connected edges.
                    flags[i] = res ^ 0xf;
                }
            }
        }

        let mut verts = Vec::with_capacity(256);
        let mut simplified = Vec::with_capacity(64);

        for z in 0..self.height {
            for x in 0..self.width {
                let c = self.cell_at(x, z);
                for i in c.index_range() {
                    if flags[i] == 0 || flags[i] == 0xf {
                        flags[i] = 0;
                        continue;
                    }
                    let reg = self.spans[i].region;
                    if reg == RegionId::NONE || reg.contains(RegionId::BORDER_REGION) {
                        continue;
                    }
                    let area = self.areas[i];

                    verts.clear();
                    simplified.clear();

                    self.walk_contour_build(x, z, i, &mut flags, &mut verts);

                    simplify_contour(
                        &verts,
                        &mut simplified,
                        max_error,
                        max_edge_len,
                        build_flags,
                    );
                    remove_degenerate_segments(&mut simplified);

                    // Store region->contour remap info.
                    // Create contour.
                    if simplified.len() >= 3 {
                        if contour_count >= max_contours as usize {
                            // Allocate more contours.
                            // This happens when a region has holes.
                            let old_max = max_contours;
                            max_contours *= 2;
                            cset.contours
                                .resize(max_contours as usize, Contour::default());

                            tracing::warn!(
                                "Region has holes. Expanding contour set from max {old_max} to max {max_contours}"
                            );
                        }
                        let cont = &mut cset.contours[contour_count];
                        contour_count += 1;

                        cont.vertices = simplified.clone();
                        if self.border_size > 0 {
                            // If the heightfield was build with bordersize, remove the offset.
                            for (vert, _) in &mut cont.vertices {
                                vert.x = vert.x.saturating_sub(self.border_size);
                                vert.z = vert.z.saturating_sub(self.border_size);
                            }
                        }
                        cont.raw_vertices = verts.clone();
                        if self.border_size > 0 {
                            // If the heightfield was build with bordersize, remove the offset.
                            for (vert, _) in &mut cont.raw_vertices {
                                vert.x = vert.x.saturating_sub(self.border_size);
                                vert.z = vert.z.saturating_sub(self.border_size);
                            }
                        }
                        cont.region = reg;
                        cont.area = area;
                    }
                }
            }
        }
        cset.contours.resize_with(contour_count, Contour::default);
        cset
    }

    fn walk_contour_build(
        &self,
        mut x: u16,
        mut z: u16,
        mut i: usize,
        flags: &mut [u8],
        points: &mut Vec<(U16Vec3, RegionVertexId)>,
    ) {
        // Choose the first non-connected edge
        let mut dir = 0;
        while (flags[i] & (1 << dir)) == 0 {
            dir += 1;
        }

        let start_dir = dir;
        let start_i = i;
        let area = self.areas[i];

        for _ in 0..40_000 {
            if (flags[i] & (1 << dir)) != 0 {
                // Choose the edge corner
                let mut is_area_border = false;
                let mut p_x = x;
                let (p_y, is_border_vertex) = self.get_corner_height(x, z, i, dir);
                let mut p_z = z;
                match dir {
                    0 => {
                        p_z += 1;
                    }
                    1 => {
                        p_x += 1;
                        p_z += 1;
                    }
                    2 => {
                        p_x += 1;
                    }
                    _ => {}
                }
                let mut r = RegionVertexId::NONE;
                let s = &self.spans[i];
                if let Some(con) = s.con(dir) {
                    let (_a_x, _a_z, a_i) = self.con_indices(x as i32, z as i32, dir, con);
                    r = RegionVertexId::from(self.spans[a_i].region);
                    if area != self.areas[a_i] {
                        is_area_border = true;
                    }
                }
                if is_border_vertex {
                    r |= RegionVertexId::BORDER_VERTEX;
                }
                if is_area_border {
                    r |= RegionVertexId::AREA_BORDER;
                }
                points.push((U16Vec3::new(p_x, p_y, p_z), r));

                flags[i] &= !(1 << dir);
                dir = (dir + 1) & 0x3;
            } else {
                let mut n_i = None;
                let n_x = (x as i32 + dir_offset_x(dir) as i32) as u16;
                let n_z = (z as i32 + dir_offset_z(dir) as i32) as u16;
                let s = &self.spans[i];
                if let Some(con) = s.con(dir) {
                    let cell_index = n_x as usize + n_z as usize * self.width as usize;
                    let n_c = &self.cells[cell_index];
                    n_i = Some(n_c.index() + con as u32);
                }
                let Some(n_i) = n_i else {
                    // Should not happen.
                    // Jan: Should this not be an error?
                    return;
                };
                x = n_x;
                z = n_z;
                i = n_i as usize;
                // Rotate counterclockwise
                dir = (dir + 3) & 0x3;
            }
            if start_i == i && start_dir == dir {
                break;
            }
        }
    }

    fn get_corner_height(&self, x: u16, z: u16, i: usize, dir: u8) -> (u16, bool) {
        let s = &self.spans[i];
        let mut ch = s.y;
        let dir_p = (dir + 1) & 0x3;

        let mut regs = [RegionVertexId::NONE; 4];

        // Combine region and area codes in order to prevent
        // border vertices which are in between two areas to be removed.
        // Jan: `RegionVertexId` is not *quite* the correct thing semantically,
        // rather this is a combination of region and area codes in a single u32.
        // But eh, this was fast to implement.
        let get_reg = |i: usize| {
            RegionVertexId::from(
                self.spans[i].region.bits() as u32 | ((self.areas[i].0 as u32) << 16),
            )
        };
        regs[0] = get_reg(i);

        if let Some(con) = s.con(dir) {
            let (a_x, a_z, a_i) = self.con_indices(x as i32, z as i32, dir, con);
            let a_s = &self.spans[a_i];
            ch = ch.max(a_s.y);
            regs[1] = get_reg(a_i);
            if let Some(con) = a_s.con(dir_p) {
                let (_b_x, _b_z, b_i) = self.con_indices(a_x, a_z, dir_p, con);
                let b_s = &self.spans[b_i];
                ch = ch.max(b_s.y);
                regs[2] = get_reg(b_i);
            }
        }
        if let Some(con) = s.con(dir_p) {
            let (a_x, a_z, a_i) = self.con_indices(x as i32, z as i32, dir_p, con);
            let a_s = &self.spans[a_i];
            ch = ch.max(a_s.y);
            regs[3] = get_reg(a_i);
            if let Some(con) = a_s.con(dir) {
                let (_b_x, _b_z, b_i) = self.con_indices(a_x, a_z, dir, con);
                let b_s = &self.spans[b_i];
                ch = ch.max(b_s.y);
                regs[2] = get_reg(b_i);
            }
        }

        // Check if the vertex is special edge vertex, these vertices will be removed later.
        let mut is_border_vertex = false;
        for dir in 0..4 {
            let a = dir;
            let b = (dir + 1) & 0x3;
            let c = (dir + 2) & 0x3;
            let d = (dir + 3) & 0x3;

            // The vertex is a border vertex there are two same exterior cells in a row,
            // followed by two interior cells and none of the regions are out of bounds.
            let two_same_exts =
                regs[a] == regs[b] && regs[a].contains(RegionId::BORDER_REGION.into());
            let two_ints = !(regs[c] | regs[d]).contains(RegionId::BORDER_REGION.into());
            let ints_same_area = (regs[c].bits() >> 16) == (regs[d].bits() >> 16);
            let no_zeros = regs[a] != RegionVertexId::NONE
                && regs[b] != RegionVertexId::NONE
                && regs[c] != RegionVertexId::NONE
                && regs[d] != RegionVertexId::NONE;
            if two_same_exts && two_ints && no_zeros && ints_same_area {
                is_border_vertex = true;
                break;
            }
        }
        (ch, is_border_vertex)
    }
}

fn simplify_contour(
    points: &[(U16Vec3, RegionVertexId)],
    simplified: &mut Vec<(U16Vec3, u32)>,
    max_error: f32,
    max_edge_len: u16,
    flags: BuildContoursFlags,
) {
    // Add initial points.
    let has_connections = points
        .iter()
        .any(|(_p, r)| r.intersects(RegionVertexId::REGION_MASK));

    if has_connections {
        // The contour has some portals to other regions.
        // Add a new point to every location where the region changes.
        let ni = points.len();
        for (i, (point, region)) in points.iter().enumerate() {
            let ii = (i + 1) % ni;
            let region = *region;
            let next_region = points[ii].1;
            let different_regs =
                region & RegionVertexId::REGION_MASK != next_region & RegionVertexId::REGION_MASK;
            let area_borders =
                region & RegionVertexId::AREA_BORDER != next_region & RegionVertexId::AREA_BORDER;
            if different_regs || area_borders {
                simplified.push((*point, i as u32));
            };
        }
    }
    if simplified.is_empty() {
        // If there is no connections at all,
        // create some initial points for the simplification process.
        // Find lower-left and upper-right vertices of the contour.
        let mut ll = &points[0].0;
        let mut lli = 0;
        let mut ur = &points[0].0;
        let mut uri = 0;
        for (i, point) in points.iter().map(|(p, _)| p).enumerate() {
            if point.x < ll.x || (point.x == ll.x && point.z < ll.z) {
                ll = point;
                lli = i;
            }
            if point.x > ur.x || (point.x == ur.x && point.z > ur.z) {
                ur = point;
                uri = i;
            }
        }
        simplified.push((*ll, lli as u32));
        simplified.push((*ur, uri as u32));
    }
    // Add points until all raw points are within
    // error tolerance to the simplified shape.
    let mut i = 0;
    while i < simplified.len() {
        let ii = (i + 1) % simplified.len();
        let (mut a, ai) = simplified[i];
        let (mut b, bi) = simplified[ii];

        // Find maximum deviation from the segment.
        let mut maxd = 0.0;
        let mut maxi = None;
        let mut ci: usize;
        let cinc: usize;
        let endi: usize;

        // Traverse the segment in lexilogical order so that the
        // max deviation is calculated similarly when traversing
        // opposite segments.
        if b.x > a.x || b.x == a.x && b.z > a.z {
            cinc = 1;
            ci = (ai as usize + cinc) % points.len();
            endi = bi as usize;
        } else {
            cinc = points.len() - 1;
            ci = (bi as usize + cinc) % points.len();
            endi = ai as usize;
            core::mem::swap(&mut a.x, &mut b.x);
            core::mem::swap(&mut a.z, &mut b.z);
        }
        // Tessellate only outer edges or edges between areas.
        let region = points[ci].1;
        if !region.intersects(RegionVertexId::REGION_MASK)
            || region.intersects(RegionVertexId::AREA_BORDER)
        {
            while ci != endi {
                let point = points[ci].0;
                let d =
                    distance_squared_between_point_and_line_u16vec2(point.xz(), (a.xz(), b.xz()));
                if d > maxd {
                    maxd = d;
                    maxi = Some(ci);
                }
                ci = (ci + cinc) % points.len();
            }
        }

        // If the max deviation is larger than accepted error,
        // add new point, else continue to next segment.
        if let Some(maxi) = maxi
            && maxd > max_error * max_error
        {
            // Add space for the new point.
            simplified.resize(simplified.len() + 1, Default::default());
            for j in ((i + 1)..simplified.len()).rev() {
                simplified[j] = simplified[j - 1];
            }
            // Add the point.
            simplified[i + 1].0 = points[maxi].0;
            simplified[i + 1].1 = maxi as u32;
        } else {
            i += 1;
        }
    }

    // Split too long edges.
    if max_edge_len > 0
        && flags.intersects(
            BuildContoursFlags::TESSELLATE_SOLID_WALL_EDGES
                | BuildContoursFlags::TESSELLATE_AREA_EDGES,
        )
    {
        let mut i = 0;
        while i < simplified.len() {
            let ii = (i + 1) % simplified.len();
            let (a, ai) = simplified[i];
            let (b, bi) = simplified[ii];
            // Find maximum deviation from the segment.
            let mut maxi = None;
            let ci = (ai as usize + 1) % points.len();

            // Tessellate only outer edges or edges between areas.
            let area = points[ci].1;
            let is_wall_edge = flags.intersects(BuildContoursFlags::TESSELLATE_SOLID_WALL_EDGES)
                && !area.intersects(RegionVertexId::REGION_MASK);
            let is_edge_between_areas = flags.intersects(BuildContoursFlags::TESSELLATE_AREA_EDGES)
                && area.intersects(RegionVertexId::AREA_BORDER);
            let should_tesselate = is_wall_edge || is_edge_between_areas;
            if should_tesselate {
                let d = b.xz().as_ivec2() - a.xz().as_ivec2();
                if d.length_squared() > (max_edge_len * max_edge_len) as i32 {
                    // Round based on the segments in lexilogical order so that the
                    // max tesselation is consistent regardless in which direction
                    // segments are traversed.
                    let n = if bi < ai {
                        bi + points.len() as u32 - ai
                    } else {
                        bi - ai
                    };
                    if n > 1 {
                        maxi = if b.x > a.x || (b.x == a.x && b.z > a.z) {
                            Some((ai + n / 2) % points.len() as u32)
                        } else {
                            Some((ai + n.div_ceil(2)) % points.len() as u32)
                        };
                    }
                }
            }
            // If the max deviation is larger than accepted error,
            // add new point, else continue to next segment.
            if let Some(maxi) = maxi {
                // Add space for the new point.
                simplified.resize(simplified.len() + 1, Default::default());
                for j in ((i + 1)..simplified.len()).rev() {
                    simplified[j] = simplified[j - 1];
                }
                // Add the point.
                simplified[i + 1].0 = points[maxi as usize].0;
                simplified[i + 1].1 = maxi;
            } else {
                i += 1;
            }
        }
    }
    for (_point, index) in simplified {
        // The edge vertex flag is taken from the current raw point,
        // and the neighbour region is take from the next raw point.
        let ai = (*index as usize + 1) % points.len();
        let bi = *index as usize;
        let a = points[ai].1;
        let b = points[bi].1;
        *index = (a.bits() & (RegionVertexId::REGION_MASK | RegionVertexId::AREA_BORDER).bits())
            | (b.bits() & RegionVertexId::BORDER_VERTEX.bits());
    }
}

fn remove_degenerate_segments(simplified: &mut Vec<(U16Vec3, u32)>) {
    // Remove adjacent vertices which are equal on xz-plane,
    // or else the triangulator will get confused.

    // Jan: for using a range / for loop because we are changing the collection's length in the loop
    let mut i = 0;
    while i < simplified.len() {
        let ni = (i + 1) % simplified.len();
        if simplified[i].0.xz() == simplified[ni].0.xz() {
            // Degenerate segment, remove.
            for j in i..simplified.len() - 1 {
                simplified[j] = simplified[j + 1];
            }
            simplified.pop();
        }
        i += 1;
    }
}

/// Represents a group of related contours.
/// All contours within the set share the minimum bounds and cell sizes of the set.
///
/// The standard process for building a contour set is to use [`CompactHeightfield::build_contours`].
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ContourSet {
    /// An array of the contours in the set.
    pub contours: Vec<Contour>,
    /// The AABB in world space
    pub aabb: Aabb3d,
    /// The size of each cell. (On the xz-plane.)
    pub cell_size: f32,
    /// The height of each cell. (The minimum increment along the y-axis.)
    pub cell_height: f32,
    /// The width of the set. (Along the x-axis in cell units.)
    pub width: u16,
    /// The height of the set. (Along the z-axis in cell units.)
    pub height: u16,
    /// The AABB border size used to generate the source data from which the contours were derived.
    pub border_size: u16,
    /// The max edge error that this contour set was simplified with. See [`Config::max_simplification_error`](crate::Config::max_simplification_error).
    pub max_error: f32,
}

bitflags::bitflags! {
    /// Flags used by [`Contour::vertices`]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct RegionVertexId: u32 {
        ///No flags
        const NONE = 0;

        /// Applied to the region id field of contour vertices in order to extract the region id.
        /// The region id field of a vertex may have several flags applied to it.  So the
        /// fields value can't be used directly.
        const REGION_MASK = RegionId::MAX.bits() as u32;

        /// Border vertex flag.
        /// If a region ID has this bit set, then the associated element lies on
        /// a tile border. If a contour vertex's region ID has this bit set, the
        /// vertex will later be removed in order to match the segments and vertices
        /// at tile boundaries.
        /// (Used during the build process.)
        const BORDER_VERTEX = 0x10_000;

        /// Area border flag.
        /// If a region ID has this bit set, then the associated element lies on
        /// the border of an area.
        /// (Used during the region and contour build process.)
        const AREA_BORDER = 0x20_000;
    }
}

impl From<u32> for RegionVertexId {
    fn from(bits: u32) -> Self {
        RegionVertexId::from_bits_retain(bits)
    }
}

impl From<RegionId> for RegionVertexId {
    fn from(region_id: RegionId) -> Self {
        RegionVertexId::from_bits_retain(region_id.bits() as u32)
    }
}

impl From<RegionVertexId> for RegionId {
    fn from(region_vertex_id: RegionVertexId) -> Self {
        let bits = region_vertex_id.bits() & RegionVertexId::REGION_MASK.bits();
        assert!(bits <= Self::MAX.bits() as u32);
        RegionId::from_bits_retain(bits as u16)
    }
}

/// Represents a simple, non-overlapping contour in field space.
///
/// A contour only exists within the context of a [`ContourSet`] object.
///
/// While the height of the contour's border may vary, the contour will always form a simple polygon when projected onto the xz-plane.
///
/// Example of converting vertices into world space:
///
/// ```rust
/// // Where cset is the ContourSet object to which the contour belongs.
/// # use rerecast::*;
/// # use glam::Vec3;
/// # let cset = ContourSet::default();
/// # let vert = Vec3::new(1.0, 2.0, 3.0);
/// let world_vertex = Vec3 {
///     x: cset.aabb.min.x + vert.x * cset.cell_size,
///     y: cset.aabb.min.y + vert.y * cset.cell_height,
///     z: cset.aabb.min.z + vert.z * cset.cell_size,
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Contour {
    /// Simplified contour vertex and connection data.
    ///
    /// The simplified contour is a version of the raw contour with all 'unnecessary' vertices removed.
    /// Whether a vertex is considered unnecessary depends on the contour build process.
    ///
    /// The data format is as follows: ((x, y, z), r)
    ///
    /// A contour edge is formed by the current and next vertex. The r-value represents region and connection information for the edge.
    /// For example:
    /// ```rust
    /// # use rerecast::*;
    /// # use glam::U16Vec3;
    /// # let mut contour = Contour::default();
    /// # contour.vertices = vec![(U16Vec3::new(1, 2, 3), 4)];
    /// # let i = 0;
    /// let r = contour.vertices[i * 4].1;
    ///
    /// let region_id = r & RegionVertexId::REGION_MASK.bits();
    /// println!("Region ID: {region_id}");
    ///
    /// if (r & RegionVertexId::BORDER_VERTEX.bits()) != 0 {
    ///     // The edge represents a solid border.
    /// }
    ///
    /// if (r & RegionVertexId::AREA_BORDER.bits()) != 0 {
    ///     // The edge represents a transition between different areas.
    /// }
    /// ```
    pub vertices: Vec<(U16Vec3, u32)>,
    /// Raw contour vertex and connection data.
    pub raw_vertices: Vec<(U16Vec3, RegionVertexId)>,
    /// Region ID of the contour.
    pub region: RegionId,
    /// Area type of the contour.
    pub area: AreaType,
}

/// Contour build flags used in [`CompactHeightfield::build_contours`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct BuildContoursFlags(u8);

bitflags::bitflags! {
    impl BuildContoursFlags: u8 {
        /// Tessellate solid (impassable) edges during contour simplification.
        const TESSELLATE_SOLID_WALL_EDGES = 1;
        /// Tessellate edges between areas during contour simplification.
        const TESSELLATE_AREA_EDGES = 2;

        /// Default flags for building contours.
        const DEFAULT = Self::TESSELLATE_SOLID_WALL_EDGES.bits();
    }
}

impl Default for BuildContoursFlags {
    fn default() -> Self {
        Self::DEFAULT
    }
}
