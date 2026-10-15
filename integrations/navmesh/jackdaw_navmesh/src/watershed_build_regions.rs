use alloc::vec::Vec;

use crate::{
    AreaType, CompactHeightfield, RegionId,
    math::{dir_offset_x, dir_offset_z},
};

impl CompactHeightfield {
    /// Non-null regions will consist of connected, non-overlapping walkable spans that form a single contour.
    /// Contours will form simple polygons.
    ///
    /// If multiple regions form an area that is smaller than `min_region_area`, then all spans will be
    /// re-assigned to [`AreaType::NOT_WALKABLE`].
    ///
    /// Watershed partitioning can result in smaller than necessary regions, especially in diagonal corridors.
    /// `merge_region_area` helps reduce unnecessarily small regions.
    ///
    /// See the #rcConfig documentation for more information on the configuration parameters.
    ///
    /// The region data will be available via the [`CompactHeightfield::max_region`]
    /// and [`CompactSpan::region`](crate::CompactSpan::region) fields.
    ///
    /// Warning: The distance field must be created using [`CompactHeightfield::build_distance_field`] before attempting to build regions.
    pub fn build_regions(
        &mut self,
        border_size: u16,
        min_region_area: u16,
        merge_region_area: u16,
    ) -> Result<(), BuildRegionsError> {
        const LOG_NB_STACKS: usize = 3;
        const NB_STACKS: usize = 1 << LOG_NB_STACKS;
        let mut level_stacks: [Vec<LevelStackEntry>; NB_STACKS] = [const { Vec::new() }; NB_STACKS];
        for stack in &mut level_stacks {
            stack.reserve(256);
        }

        let mut stack: Vec<LevelStackEntry> = Vec::with_capacity(256);

        let mut src_reg = vec![RegionId::NONE; self.spans.len()];
        let mut src_dist = vec![0_u16; self.spans.len()];

        let mut region_id = RegionId::from(1);
        let mut level = (self.max_distance + 1) & !1;

        // Jan: The following comment is taken from the original implementation.
        // TODO: Figure better formula, expandIters defines how much the
        // watershed "overflows" and simplifies the regions. Tying it to
        // agent radius was usually good indication how greedy it could be.
        //	const int expandIters = 4 + walkableRadius * 2;
        let expand_iters = 8;

        if border_size > 0 {
            // Make sure border will not overflow.
            let border_width = border_size.min(self.width);
            let border_height = border_size.min(self.height);

            // Paint regions
            self.paint_rect_region(
                0,
                border_width,
                0,
                self.height,
                region_id | RegionId::BORDER_REGION,
                &mut src_reg,
            );
            region_id += 1;
            self.paint_rect_region(
                self.width - border_width,
                self.width,
                0,
                self.height,
                region_id | RegionId::BORDER_REGION,
                &mut src_reg,
            );
            region_id += 1;
            self.paint_rect_region(
                0,
                self.width,
                0,
                border_height,
                region_id | RegionId::BORDER_REGION,
                &mut src_reg,
            );
            region_id += 1;
            self.paint_rect_region(
                0,
                self.width,
                self.height - border_height,
                self.height,
                region_id | RegionId::BORDER_REGION,
                &mut src_reg,
            );
            region_id += 1;
        }
        self.border_size = border_size;

        let mut s_id = -1_i32;
        while level > 0 {
            level = level.saturating_sub(2);
            s_id = (s_id + 1) & (NB_STACKS as i32 - 1);

            if s_id == 0 {
                self.sort_cells_by_level(level, &mut src_reg, NB_STACKS, &mut level_stacks, 1);
            } else {
                // copy left overs from last level
                let (src, dst) = level_stacks.split_at_mut(s_id as usize);
                append_stacks(&src[s_id as usize - 1], &mut dst[0], &src_reg);
            }

            self.expand_regions(
                expand_iters,
                level,
                &mut src_reg,
                &mut src_dist,
                &mut level_stacks[s_id as usize],
                false,
            );

            // Mark new regions with IDs.
            for current in level_stacks[s_id as usize].iter() {
                let Some(i) = current.index else {
                    continue;
                };
                if src_reg[i] == RegionId::NONE
                    && self.flood_region(
                        current,
                        level,
                        region_id,
                        &mut src_reg,
                        &mut src_dist,
                        &mut stack,
                    )
                {
                    if region_id == RegionId::MAX {
                        return Err(BuildRegionsError::RegionIdOverflow);
                    }
                    region_id += 1;
                }
            }
        }

        // Expand current regions until no empty connected cells found.
        self.expand_regions(
            expand_iters * 8,
            0,
            &mut src_reg,
            &mut src_dist,
            &mut stack,
            true,
        );

        // Merge regions and filter out small regions.
        self.max_region = region_id;
        // Jan: the early return is just triggered for OOM, so let's skip that :P
        let overlaps =
            self.merge_and_filter_regions(min_region_area, merge_region_area, &mut src_reg);

        // If overlapping regions were found during merging, split those regions.
        if !overlaps.is_empty() {
            // Jan: Contrary to the comment above, we don't actually split anything here. This probably happens during the next iteration? idk
            tracing::error!(
                "{len} overlapping regions found during merging.",
                len = overlaps.len()
            );
        }

        // Write the result out
        #[expect(clippy::needless_range_loop)]
        for i in 0..self.spans.len() {
            self.spans[i].region = src_reg[i];
        }

        Ok(())
    }

    fn merge_and_filter_regions(
        &mut self,
        min_region_area: u16,
        merge_region_size: u16,
        src_reg: &mut [RegionId],
    ) -> Vec<RegionId> {
        let mut overlaps = Vec::new();
        let w = self.width;
        let h = self.height;

        let nreg = self.max_region.bits() + 1;

        // Construct regions
        let mut regions = (0..nreg)
            .map(|i| Region::new(RegionId::from(i)))
            .collect::<Vec<_>>();

        // Find edge of a region and find connections around the contour.
        for z in 0..h {
            for x in 0..w {
                let cell = self.cell_at(x, z);
                let max_index = cell.index() as usize + cell.count() as usize;
                #[expect(clippy::needless_range_loop)]
                for i in cell.index() as usize..max_index {
                    let r = src_reg[i];
                    if r == RegionId::NONE || r >= RegionId::from(nreg) {
                        continue;
                    }
                    let reg = &mut regions[r.bits() as usize];
                    reg.span_count += 1;

                    // Update floors
                    for j in cell.index() as usize..max_index {
                        if i == j {
                            continue;
                        }
                        let floor_id = src_reg[j];
                        if floor_id == RegionId::NONE || floor_id >= RegionId::from(nreg) {
                            continue;
                        }
                        if floor_id == r {
                            reg.overlap = true;
                        }
                        reg.add_unique_floor_region(floor_id);
                    }

                    // Have found contour
                    if !reg.connections.is_empty() {
                        continue;
                    }

                    reg.area = self.areas[i];

                    // Check if this cell is next to a border
                    let ndir = (0..4).find(|&dir| self.is_solid_edge(src_reg, x, z, i, dir));
                    if let Some(ndir) = ndir {
                        // The cell is at border.
                        // Walk around the contour to find all the neighbours.
                        self.walk_contour(x, z, i, ndir, src_reg, &mut reg.connections);
                    }
                }
            }
        }

        // Remove too small regions.
        let mut stack = Vec::with_capacity(32);
        let mut trace = Vec::with_capacity(32);
        for i in 0..nreg as usize {
            let reg: &mut Region = &mut regions[i];
            if reg.id == RegionId::NONE || reg.id.intersects(RegionId::BORDER_REGION) {
                continue;
            }
            if reg.span_count == 0 {
                continue;
            }
            if reg.visited {
                continue;
            }

            // Count the total size of all the connected regions.
            // Also keep track of the regions connects to a tile border.
            let mut connects_to_border = false;
            let mut span_count = 0;
            stack.clear();
            trace.clear();

            reg.visited = true;
            stack.push(i);

            while let Some(ri) = stack.pop() {
                let c_reg = regions[ri].clone();
                span_count += c_reg.span_count;
                trace.push(ri);

                for connection in c_reg.connections.iter() {
                    if connection.intersects(RegionId::BORDER_REGION) {
                        connects_to_border = true;
                        continue;
                    }
                    let nei_reg = &mut regions[connection.bits() as usize];
                    if nei_reg.visited {
                        continue;
                    }
                    if nei_reg.id == RegionId::NONE
                        || nei_reg.id.intersects(RegionId::BORDER_REGION)
                    {
                        continue;
                    }
                    // Visit
                    stack.push(nei_reg.id.bits() as usize);
                    nei_reg.visited = true;
                }
            }

            // If the accumulated regions size is too small, remove it.
            // Do not remove areas which connect to tile borders
            // as their size cannot be estimated correctly and removing them
            // can potentially remove necessary areas.
            if span_count < min_region_area as usize && !connects_to_border {
                // Kill all visited regions.
                for &trace in trace.iter() {
                    regions[trace].span_count = 0;
                    regions[trace].id = RegionId::NONE;
                }
            }
        }

        // Merge too small regions to neighbour regions.
        loop {
            let mut merge_count = 0;
            for i in 0..nreg as usize {
                let reg = regions[i].clone();
                if reg.id == RegionId::NONE || reg.id.intersects(RegionId::BORDER_REGION) {
                    continue;
                }
                if reg.overlap {
                    continue;
                }
                if reg.span_count == 0 {
                    continue;
                }

                // Check to see if the region should be merged
                if reg.span_count > merge_region_size as usize
                    && reg.is_region_connected_to_border()
                {
                    continue;
                }
                // Small region with more than 1 connection.
                // Or region which is not connected to a border at all.
                // Find smallest neighbour region that connects to this one.
                let mut smallest = usize::MAX;
                let mut merge_id = reg.id;
                for connection in reg.connections.iter() {
                    if connection.intersects(RegionId::BORDER_REGION) {
                        continue;
                    }
                    let mreg = regions[connection.bits() as usize].clone();
                    if mreg.id == RegionId::NONE
                        || mreg.id.intersects(RegionId::BORDER_REGION)
                        || mreg.overlap
                    {
                        continue;
                    }
                    if mreg.span_count < smallest
                        && reg.can_merge_with(&mreg)
                        && mreg.can_merge_with(&reg)
                    {
                        smallest = mreg.span_count;
                        merge_id = mreg.id;
                    }
                }
                // Found new id.
                if merge_id != reg.id {
                    let old_id = reg.id;
                    // Merge neighbours.
                    if regions[merge_id.bits() as usize].merge_regions(&reg) {
                        regions[i].span_count = 0;
                        regions[i].connections.clear();
                        // Fixup regions pointing to current region.
                        #[expect(clippy::needless_range_loop)]
                        for j in 0..nreg as usize {
                            let reg = &mut regions[j];
                            if reg.id == RegionId::NONE
                                || reg.id.intersects(RegionId::BORDER_REGION)
                            {
                                continue;
                            }
                            // If another region was already merged into current region
                            // change the nid of the previous region too.
                            if reg.id == old_id {
                                reg.id = merge_id;
                            }
                            // Replace the current region with the new one if the
                            // current regions is neighbour.
                            reg.replace_neighbour(old_id, merge_id);
                        }
                        merge_count += 1;
                    }
                }
            }

            if merge_count == 0 {
                break;
            }
        }

        // Compress region IDs
        #[expect(clippy::needless_range_loop)]
        for i in 0..nreg as usize {
            let reg = &mut regions[i];
            // Skip nil regions and external regions.
            reg.remap = !(reg.id == RegionId::NONE || reg.id.intersects(RegionId::BORDER_REGION));
        }

        let mut reg_id_gen = 0;
        for i in 0..nreg as usize {
            if !regions[i].remap {
                continue;
            }
            let old_id = regions[i].id;
            reg_id_gen += 1;
            let new_id = RegionId::from(reg_id_gen);
            #[expect(clippy::needless_range_loop)]
            for j in i..nreg as usize {
                if regions[j].id == old_id {
                    regions[j].id = new_id;
                    regions[j].remap = false;
                }
            }
        }
        self.max_region = RegionId::from(reg_id_gen);

        // Remap regions
        for reg in src_reg.iter_mut() {
            if !reg.intersects(RegionId::BORDER_REGION) {
                *reg = regions[reg.bits() as usize].id;
            }
        }

        // Return regions that we found to be overlapping.
        #[expect(clippy::needless_range_loop)]
        for i in 0..nreg as usize {
            if regions[i].overlap {
                overlaps.push(regions[i].id);
            }
        }

        overlaps
    }

    fn walk_contour(
        &self,
        mut x: u16,
        mut z: u16,
        mut i: usize,
        mut dir: u8,
        src_reg: &[RegionId],
        connections: &mut Vec<RegionId>,
    ) {
        let start_dir = dir;
        let start_i = i;

        let ss = &self.spans[i];
        let mut current_region = RegionId::NONE;
        if let Some(con) = ss.con(dir) {
            let a_x = (x as i32 + dir_offset_x(dir) as i32) as u16;
            let a_z = (z as i32 + dir_offset_z(dir) as i32) as u16;
            let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
            current_region = src_reg[a_index];
        }
        connections.push(current_region);

        // Jan: cool magic number lol
        for _ in 0..40_000 {
            let s = &self.spans[i];
            if self.is_solid_edge(src_reg, x, z, i, dir) {
                // Choose the edge corner
                let mut r = RegionId::NONE;
                if let Some(con) = s.con(dir) {
                    let a_x = (x as i32 + dir_offset_x(dir) as i32) as u16;
                    let a_z = (z as i32 + dir_offset_z(dir) as i32) as u16;
                    let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                    r = src_reg[a_index];
                }
                if r != current_region {
                    current_region = r;
                    connections.push(current_region);
                }
                // Rotate clockwise
                dir = (dir + 1) & 0x3;
            } else {
                let mut ni = None;
                let n_x = (x as i32 + dir_offset_x(dir) as i32) as u16;
                let n_z = (z as i32 + dir_offset_z(dir) as i32) as u16;
                if let Some(con) = s.con(dir) {
                    let n_c = self.cell_at(n_x, n_z);
                    let n_index = n_c.index() as usize + con as usize;
                    ni = Some(n_index);
                }
                if let Some(ni) = ni {
                    x = n_x;
                    z = n_z;
                    i = ni;
                    // Rotate counter-clockwise
                    dir = (dir + 3) & 0x3;
                } else {
                    // Should not happen
                    // Jan: Why not an error then??
                    return;
                }
            }
            if start_i == i && start_dir == dir {
                break;
            }
        }

        // Remove adjacent duplicates.
        if connections.len() > 1 {
            let mut j = 0;
            while j < connections.len() {
                let nj = (j + 1) % connections.len();
                if connections[j] == connections[nj] {
                    for k in j..connections.len() - 1 {
                        connections[k] = connections[k + 1];
                    }
                    connections.pop();
                } else {
                    j += 1;
                }
            }
        }
    }

    fn is_solid_edge(&self, src_reg: &[RegionId], x: u16, z: u16, i: usize, dir: u8) -> bool {
        let span = &self.spans[i];
        let mut r = RegionId::NONE;
        if let Some(con) = span.con(dir) {
            let a_x = (x as i32 + dir_offset_x(dir) as i32) as u16;
            let a_z = (z as i32 + dir_offset_z(dir) as i32) as u16;
            let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
            r = src_reg[a_index];
        }
        r != src_reg[i]
    }

    fn flood_region(
        &self,
        entry: &LevelStackEntry,
        level: u16,
        region: RegionId,
        src_reg: &mut [RegionId],
        src_dist: &mut [u16],
        stack: &mut Vec<LevelStackEntry>,
    ) -> bool {
        // Safety: entry.index has to be verified before calling this function.
        let i = entry.index.unwrap();
        let area = self.areas[i];

        // Flood fill mark region
        stack.clear();
        stack.push(entry.clone());
        src_reg[i] = region;
        src_dist[i] = 0;

        let level = level.saturating_sub(2);
        let mut count = 0;

        while let Some(back) = stack.pop() {
            let cx = back.x;
            let cz = back.z;
            let Some(ci) = back.index else {
                // Jan: The original just accesses invalid memory here lol.
                continue;
            };

            let cs = &self.spans[ci];

            // Check if any of the neighbours already have a valid region set.
            let mut ar = RegionId::NONE;
            for dir in 0..4 {
                // 8 connected
                if let Some(con) = cs.con(dir) {
                    let a_x = (cx as i32 + dir_offset_x(dir) as i32) as u16;
                    let a_z = (cz as i32 + dir_offset_z(dir) as i32) as u16;
                    let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                    if self.areas[a_index] != area {
                        continue;
                    }
                    let nr = src_reg[a_index];
                    if nr.intersects(RegionId::BORDER_REGION) {
                        // Do not take borders into account.
                        break;
                    }
                    if nr != RegionId::NONE && nr != region {
                        ar = nr;
                        break;
                    }

                    let a_span = &self.spans[a_index];

                    let dir2 = (dir + 1) & 0x3;
                    if let Some(con) = a_span.con(dir2) {
                        let a_x = (a_x as i32 + dir_offset_x(dir2) as i32) as u16;
                        let a_z = (a_z as i32 + dir_offset_z(dir2) as i32) as u16;
                        let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                        if self.areas[a_index] != area {
                            continue;
                        }
                        let nr = src_reg[a_index];
                        if nr != RegionId::NONE && nr != region {
                            ar = nr;
                            break;
                        }
                    }
                }
            }

            if ar != RegionId::NONE {
                src_reg[ci] = RegionId::NONE;
                continue;
            }

            count += 1;

            // Expand neighbours.
            for dir in 0..4 {
                let Some(con) = cs.con(dir) else {
                    continue;
                };
                let a_x = (cx as i32 + dir_offset_x(dir) as i32) as u16;
                let a_z = (cz as i32 + dir_offset_z(dir) as i32) as u16;
                let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                if self.areas[a_index] != area {
                    continue;
                }
                if self.dist[a_index] >= level && src_reg[a_index] == RegionId::NONE {
                    src_reg[a_index] = region;
                    src_dist[a_index] = 0;
                    stack.push(LevelStackEntry {
                        x: a_x,
                        z: a_z,
                        index: Some(a_index),
                    });
                }
            }
        }
        count > 0
    }

    fn paint_rect_region(
        &self,
        min_x: u16,
        max_x: u16,
        min_z: u16,
        max_z: u16,
        region: RegionId,
        src_reg: &mut [RegionId],
    ) {
        for z in min_z..max_z {
            for x in min_x..max_x {
                let cell = self.cell_at(x, z);
                let max_index = cell.index() as usize + cell.count() as usize;
                #[expect(clippy::needless_range_loop)]
                for i in cell.index() as usize..max_index {
                    if self.areas[i].is_walkable() {
                        src_reg[i] = region;
                    }
                }
            }
        }
    }

    fn sort_cells_by_level(
        &self,
        start_level: u16,
        src_reg: &mut [RegionId],
        nb_stacks: usize,
        stacks: &mut [Vec<LevelStackEntry>],
        log_levels_per_stack: u16,
    ) {
        let start_level = start_level >> log_levels_per_stack;
        for stack in stacks.iter_mut().take(nb_stacks) {
            stack.clear();
        }

        // put all cells in the level range into the appropriate stacks
        for z in 0..self.height {
            for x in 0..self.width {
                let cell = self.cell_at(x, z);
                let max_index = cell.index() as usize + cell.count() as usize;
                #[expect(clippy::needless_range_loop)]
                for i in cell.index() as usize..max_index {
                    if !self.areas[i].is_walkable() || src_reg[i] != RegionId::NONE {
                        continue;
                    }
                    let level = self.dist[i] >> log_levels_per_stack;
                    // Jan: The original can underflow here FYI
                    let s_id = start_level.saturating_sub(level);
                    if s_id >= nb_stacks as u16 {
                        continue;
                    }
                    stacks[s_id as usize].push(LevelStackEntry {
                        x,
                        z,
                        index: Some(i),
                    });
                }
            }
        }
    }

    fn expand_regions(
        &self,
        max_iter: u16,
        level: u16,
        src_reg: &mut [RegionId],
        src_dist: &mut [u16],
        stack: &mut Vec<LevelStackEntry>,
        fill_stack: bool,
    ) {
        if fill_stack {
            // Find cells revealed by the raised level.
            stack.clear();
            for z in 0..self.height {
                for x in 0..self.width {
                    let cell = self.cell_at(x, z);
                    let max_index = cell.index() as usize + cell.count() as usize;
                    #[expect(clippy::needless_range_loop)]
                    for i in cell.index() as usize..max_index {
                        if self.dist[i] >= level
                            && src_reg[i] == RegionId::NONE
                            && self.areas[i].is_walkable()
                        {
                            stack.push(LevelStackEntry {
                                x,
                                z,
                                index: Some(i),
                            });
                        }
                    }
                }
            }
        } else {
            // use cells in the input stack
            // mark all cells which already have a region
            for entry in stack.iter_mut() {
                let Some(i) = entry.index else {
                    continue;
                };
                if src_reg[i] != RegionId::NONE {
                    entry.index = None;
                }
            }
        }

        let mut dirty_entries = Vec::new();
        let mut iter = 0;
        // Jan: I don't think stack is ever made smaller? Is this just an `if` in disguise?
        while !stack.is_empty() {
            let mut failed = 0;
            dirty_entries.clear();

            for entry in stack.iter_mut() {
                let x = entry.x;
                let z = entry.z;
                let Some(i) = entry.index else {
                    failed += 1;
                    continue;
                };

                let mut r = src_reg[i];
                let mut d2 = u16::MAX;
                let area = self.areas[i];
                let span = &self.spans[i];
                for dir in 0..4 {
                    let Some(con) = span.con(dir) else {
                        continue;
                    };
                    let a_x = (x as i32 + dir_offset_x(dir) as i32) as u16;
                    let a_z = (z as i32 + dir_offset_z(dir) as i32) as u16;
                    let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                    if self.areas[a_index] != area {
                        continue;
                    }
                    let a_region = src_reg[a_index];
                    let a_dist = src_dist[a_index] + 2;
                    if a_region != RegionId::NONE
                        && !a_region.intersects(RegionId::BORDER_REGION)
                        && a_dist < d2
                    {
                        r = a_region;
                        d2 = a_dist;
                    }
                }
                if r != RegionId::NONE {
                    // Mark as used
                    entry.index = None;
                    dirty_entries.push(DirtyEntry {
                        index: i,
                        region: r,
                        distance2: d2,
                    });
                } else {
                    failed += 1;
                }
            }
            // Copy entries that differ between src and dst to keep them in sync.
            for dirty_entry in dirty_entries.iter() {
                let index = dirty_entry.index;
                src_reg[index] = dirty_entry.region;
                src_dist[index] = dirty_entry.distance2;
            }

            if failed == stack.len() {
                break;
            }

            if level > 0 {
                iter += 1;
                if iter >= max_iter {
                    break;
                }
            }
        }
    }
}

fn append_stacks(
    src_stack: &[LevelStackEntry],
    dst_stack: &mut Vec<LevelStackEntry>,
    src_region: &[RegionId],
) {
    for stack in src_stack.iter() {
        let Some(i) = stack.index else {
            continue;
        };
        if src_region[i] != RegionId::NONE {
            continue;
        }
        dst_stack.push(stack.clone());
    }
}

#[derive(Clone, Debug)]
struct LevelStackEntry {
    x: u16,
    z: u16,
    index: Option<usize>,
}

#[derive(Clone, Debug)]
struct DirtyEntry {
    index: usize,
    region: RegionId,
    distance2: u16,
}

#[derive(Debug, Clone)]
struct Region {
    span_count: usize,
    id: RegionId,
    area: AreaType,
    remap: bool,
    visited: bool,
    overlap: bool,
    #[expect(dead_code, reason = "Used by the layer API")]
    connects_to_border: bool,
    #[expect(dead_code, reason = "Used by the layer API")]
    y_min: u16,
    #[expect(dead_code, reason = "Used by the layer API")]
    y_max: u16,
    connections: Vec<RegionId>,
    floors: Vec<RegionId>,
}
impl Region {
    fn new(id: RegionId) -> Self {
        Self {
            id,
            span_count: 0,
            area: AreaType::NOT_WALKABLE,
            remap: false,
            visited: false,
            overlap: false,
            connects_to_border: false,
            y_min: u16::MAX,
            y_max: u16::MIN,
            connections: Vec::new(),
            floors: Vec::new(),
        }
    }

    fn add_unique_floor_region(&mut self, floor_id: RegionId) {
        if self.floors.contains(&floor_id) {
            return;
        }
        self.floors.push(floor_id);
    }

    fn is_region_connected_to_border(&self) -> bool {
        // Region is connected to border if
        // one of the neighbours is null id.
        self.connections.contains(&RegionId::NONE)
    }

    fn can_merge_with(&self, other: &Self) -> bool {
        if self.area != other.area {
            return false;
        }
        let mut n = 0;
        for connection in self.connections.iter() {
            if *connection == other.id {
                n += 1;
            }
        }
        if n > 1 {
            return false;
        }
        for floor in self.floors.iter() {
            if floor == &other.id {
                return false;
            }
        }
        true
    }

    fn replace_neighbour(&mut self, old_id: RegionId, new_id: RegionId) {
        let mut nei_changed = false;
        for connection in self.connections.iter_mut() {
            if *connection == old_id {
                *connection = new_id;
                nei_changed = true;
            }
        }
        for floor in self.floors.iter_mut() {
            if *floor == old_id {
                *floor = new_id;
            }
        }
        if nei_changed {
            self.remove_adjacent_neighbours();
        }
    }

    fn remove_adjacent_neighbours(&mut self) {
        // Remove adjacent duplicates.
        let mut i = 0;
        while i < self.connections.len() && self.connections.len() > 1 {
            let ni = (i + 1) % self.connections.len();
            if self.connections[i] == self.connections[ni] {
                // Remove duplicate
                for j in i..self.connections.len() - 1 {
                    self.connections[j] = self.connections[j + 1];
                }
                self.connections.pop();
            } else {
                i += 1;
            }
        }
    }

    /// Implementation note: due to mutability rules, do the following outside the function when it returns `true`:
    /// ```ignore
    /// other.span_count = 0;
    /// other.connections.clear();
    /// ```
    fn merge_regions(&mut self, other: &Self) -> bool {
        let a_id = self.id;
        let b_id = other.id;

        // Duplicate current neighbourhood.
        let a_con = self.connections.clone();
        let b_con = &other.connections;

        // Find insertion point on A.
        let ins_a = a_con.iter().position(|con| *con == b_id);

        let Some(ins_a) = ins_a else {
            return false;
        };

        // Find insertion point on B.
        let ins_b = b_con.iter().position(|con| *con == a_id);

        let Some(ins_b) = ins_b else {
            return false;
        };

        // Merge neighbours.
        self.connections.clear();
        let ni = a_con.len();
        for i in 0..ni - 1 {
            let connection = a_con[(ins_a + 1 + i) % ni];
            self.connections.push(connection);
        }

        let ni = b_con.len();
        for i in 0..ni - 1 {
            let connection = b_con[(ins_b + 1 + i) % ni];
            self.connections.push(connection);
        }

        self.remove_adjacent_neighbours();
        for floor in other.floors.iter() {
            self.add_unique_floor_region(*floor);
        }
        self.span_count += other.span_count;

        true
    }
}
/// Error type for [`CompactHeightfield::build_regions`].
#[derive(Debug, thiserror::Error)]
pub enum BuildRegionsError {
    /// The region ID overflowed.
    #[error("Region ID overflow")]
    RegionIdOverflow,
}
