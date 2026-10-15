//! Watershed partitioning
//!   - the classic Recast partitioning
//!   - creates the nicest tessellation
//!   - usually slowest
//!   - partitions the heightfield into nice regions without holes or overlaps
//!   - the are some corner cases where this method creates produces holes and overlaps
//!      - holes may appear when a small obstacles is close to large open area (triangulation can handle this)
//!      - overlaps may occur if you have narrow spiral corridors (i.e stairs), this make triangulation to fail
//!   * generally the best choice if you precompute the navmesh, use this if you have large open areas

use alloc::vec::Vec;

use crate::{
    CompactHeightfield,
    math::{dir_offset_x, dir_offset_z},
};

impl CompactHeightfield {
    /// Prepare for region partitioning, by calculating distance field along the walkable surface.
    pub fn build_distance_field(&mut self) {
        let distance_field = self.calculate_distance_field();
        self.max_distance = distance_field.iter().max().copied().unwrap_or_default();
        let distance_field = self.box_blur(1, &distance_field);
        self.dist = distance_field;
    }

    fn calculate_distance_field(&self) -> Vec<u16> {
        // Init distance and points.
        let mut distance_field = vec![u16::MAX; self.spans.len()];

        // Mark boundary cells.
        for z in 0..self.height {
            for x in 0..self.width {
                let cell = self.cell_at(x, z);
                let max_index = cell.index() as usize + cell.count() as usize;
                #[expect(
                    clippy::needless_range_loop,
                    reason = "lol the alternative suggestion is really unreadable"
                )]
                for i in cell.index() as usize..max_index {
                    let span = &self.spans[i];
                    let area = self.areas[i];

                    let mut connection_count = 0;
                    for dir in 0..4 {
                        if let Some(con) = span.con(dir) {
                            let a_x = (x as i32 + dir_offset_x(dir) as i32) as u16;
                            let a_z = (z as i32 + dir_offset_z(dir) as i32) as u16;
                            let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                            if area == self.areas[a_index] {
                                connection_count += 1;
                            }
                        }
                    }
                    if connection_count < 4 {
                        distance_field[i] = 0;
                    }
                }
            }
        }

        // Pass 1
        for z in 0..self.height {
            for x in 0..self.width {
                let cell = self.cell_at(x, z);
                let max_index = cell.index() as usize + cell.count() as usize;
                for i in cell.index() as usize..max_index {
                    let span = &self.spans[i];
                    if let Some(con) = span.con(0) {
                        // (-1,0)
                        let a_x = (x as i32 + dir_offset_x(0) as i32) as u16;
                        let a_z = (z as i32 + dir_offset_z(0) as i32) as u16;
                        let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                        let a_span = &self.spans[a_index];
                        distance_field[i] = distance_field[i].min(distance_field[a_index] + 2);

                        // (-1,-1)
                        if let Some(con) = a_span.con(3) {
                            let b_x = (a_x as i32 + dir_offset_x(3) as i32) as u16;
                            let b_z = (a_z as i32 + dir_offset_z(3) as i32) as u16;
                            let b_index = self.cell_at(b_x, b_z).index() as usize + con as usize;
                            distance_field[i] = distance_field[i].min(distance_field[b_index] + 3);
                        }
                    }

                    if let Some(con) = span.con(3) {
                        // (0, -1)
                        let a_x = (x as i32 + dir_offset_x(3) as i32) as u16;
                        let a_z = (z as i32 + dir_offset_z(3) as i32) as u16;
                        let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                        let a_span = &self.spans[a_index];
                        distance_field[i] = distance_field[i].min(distance_field[a_index] + 2);

                        // (1, -1)
                        if let Some(con) = a_span.con(2) {
                            let b_x = (a_x as i32 + dir_offset_x(2) as i32) as u16;
                            let b_z = (a_z as i32 + dir_offset_z(2) as i32) as u16;
                            let b_index = self.cell_at(b_x, b_z).index() as usize + con as usize;
                            distance_field[i] = distance_field[i].min(distance_field[b_index] + 3);
                        }
                    }
                }
            }
        }

        // Pass 2
        for z in (0..self.height).rev() {
            for x in (0..self.width).rev() {
                let cell = self.cell_at(x, z);
                let max_index = cell.index() as usize + cell.count() as usize;
                for i in cell.index() as usize..max_index {
                    let span = &self.spans[i];
                    if let Some(con) = span.con(2) {
                        // (1, 0)
                        let a_x = (x as i32 + dir_offset_x(2) as i32) as u16;
                        let a_z = (z as i32 + dir_offset_z(2) as i32) as u16;
                        let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                        let a_span = &self.spans[a_index];
                        distance_field[i] = distance_field[i].min(distance_field[a_index] + 2);

                        // (1, 1)
                        if let Some(con) = a_span.con(1) {
                            let b_x = (a_x as i32 + dir_offset_x(1) as i32) as u16;
                            let b_z = (a_z as i32 + dir_offset_z(1) as i32) as u16;
                            let b_index = self.cell_at(b_x, b_z).index() as usize + con as usize;
                            distance_field[i] = distance_field[i].min(distance_field[b_index] + 3);
                        }
                    }

                    if let Some(con) = span.con(1) {
                        // (0, 1)
                        let a_x = (x as i32 + dir_offset_x(1) as i32) as u16;
                        let a_z = (z as i32 + dir_offset_z(1) as i32) as u16;
                        let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                        let a_span = &self.spans[a_index];
                        distance_field[i] = distance_field[i].min(distance_field[a_index] + 2);

                        // (-1, 1)
                        if let Some(con) = a_span.con(0) {
                            let b_x = (a_x as i32 + dir_offset_x(0) as i32) as u16;
                            let b_z = (a_z as i32 + dir_offset_z(0) as i32) as u16;
                            let b_index = self.cell_at(b_x, b_z).index() as usize + con as usize;
                            distance_field[i] = distance_field[i].min(distance_field[b_index] + 3);
                        }
                    }
                }
            }
        }

        distance_field
    }

    fn box_blur(&self, threshold: u16, distance_field: &[u16]) -> Vec<u16> {
        let threshold = threshold.saturating_mul(2);
        let mut result = vec![0; distance_field.len()];

        for z in 0..self.height {
            for x in 0..self.width {
                let cell = self.cell_at(x, z);
                let max_index = cell.index() as usize + cell.count() as usize;
                for i in cell.index() as usize..max_index {
                    let span = &self.spans[i];
                    let cd = distance_field[i];
                    if cd <= threshold {
                        result[i] = cd;
                        continue;
                    }
                    let mut d = cd as u32;
                    for dir in 0..4 {
                        if let Some(con) = span.con(dir) {
                            let a_x = (x as i32 + dir_offset_x(dir) as i32) as u16;
                            let a_z = (z as i32 + dir_offset_z(dir) as i32) as u16;
                            let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                            d += distance_field[a_index] as u32;

                            let a_span = &self.spans[a_index];
                            let dir2 = (dir + 1) & 0x3;
                            if let Some(con) = a_span.con(dir2) {
                                let b_x = (a_x as i32 + dir_offset_x(dir2) as i32) as u16;
                                let b_z = (a_z as i32 + dir_offset_z(dir2) as i32) as u16;
                                let b_index =
                                    self.cell_at(b_x, b_z).index() as usize + con as usize;
                                d += distance_field[b_index] as u32;
                            } else {
                                d += cd as u32;
                            }
                        } else {
                            d += cd as u32 * 2;
                        }
                    }
                    result[i] = ((d + 5) / 9) as u16;
                }
            }
        }
        result
    }
}
