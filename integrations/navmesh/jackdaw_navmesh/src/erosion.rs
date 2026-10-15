use crate::{
    AreaType, CompactHeightfield,
    math::{dir_offset_x, dir_offset_z},
};

impl CompactHeightfield {
    /// Erode the walkable area by agent radius.
    pub fn erode_walkable_area(&mut self, erosion_radius: u16) {
        let mut distance_to_boundary = vec![u8::MAX; self.spans.len()];

        // Mark boundary cells.
        for z in 0..self.height {
            for x in 0..self.width {
                let cell = self.cell_at(x, z);
                let max_span_index = cell.index() as usize + cell.count() as usize;
                #[expect(
                    clippy::needless_range_loop,
                    reason = "lol the alternative suggestion is really unreadable"
                )]
                for span_index in cell.index() as usize..max_span_index {
                    if !self.areas[span_index].is_walkable() {
                        distance_to_boundary[span_index] = 0;
                        continue;
                    }
                    let span = &self.spans[span_index];
                    // Check that there is a non-null adjacent span in each of the 4 cardinal directions.
                    let mut neighbor_count = 0;
                    for direction in 0..4 {
                        let Some(neighbor_connection) = span.con(direction) else {
                            break;
                        };
                        let neighbor_x = x as i32 + dir_offset_x(direction) as i32;
                        let neighbor_z = z as i32 + dir_offset_z(direction) as i32;
                        let neighbor_span_index =
                            self.cell_at(neighbor_x as u16, neighbor_z as u16).index() as usize
                                + neighbor_connection as usize;

                        if !self.areas[neighbor_span_index].is_walkable() {
                            break;
                        }
                        neighbor_count += 1;
                    }

                    // At least one missing neighbour, so this is a boundary cell.
                    if neighbor_count != 4 {
                        distance_to_boundary[span_index] = 0;
                    }
                }
            }
        }

        let mut new_distance: u8;

        // Pass 1
        for z in 0..self.height {
            for x in 0..self.width {
                let cell = self.cell_at(x, z);
                let max_span_index = cell.index() as usize + cell.count() as usize;
                for span_index in cell.index() as usize..max_span_index {
                    let span = &self.spans[span_index];

                    if let Some(con) = span.con(0) {
                        // (-1,0)
                        let a_x = (x as i32 + dir_offset_x(0) as i32) as u16;
                        let a_z = (z as i32 + dir_offset_z(0) as i32) as u16;
                        let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                        let a_span = &self.spans[a_index];
                        new_distance = distance_to_boundary[a_index].saturating_add(2);
                        if new_distance < distance_to_boundary[span_index] {
                            distance_to_boundary[span_index] = new_distance;
                        };

                        // (-1,-1)
                        if let Some(con) = a_span.con(3) {
                            let b_x = (a_x as i32 + dir_offset_x(3) as i32) as u16;
                            let b_z = (a_z as i32 + dir_offset_z(3) as i32) as u16;
                            let b_index = self.cell_at(b_x, b_z).index() as usize + con as usize;
                            new_distance = distance_to_boundary[b_index].saturating_add(3);
                            if new_distance < distance_to_boundary[span_index] {
                                distance_to_boundary[span_index] = new_distance;
                            };
                        }
                    }
                    if let Some(con) = span.con(3) {
                        // (0,-1)
                        let a_x = (x as i32 + dir_offset_x(3) as i32) as u16;
                        let a_z = (z as i32 + dir_offset_z(3) as i32) as u16;
                        let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                        let a_span = &self.spans[a_index];
                        new_distance = distance_to_boundary[a_index].saturating_add(2);
                        if new_distance < distance_to_boundary[span_index] {
                            distance_to_boundary[span_index] = new_distance;
                        };

                        // (1,-1)
                        if let Some(con) = a_span.con(2) {
                            let b_x = (a_x as i32 + dir_offset_x(2) as i32) as u16;
                            let b_z = (a_z as i32 + dir_offset_z(2) as i32) as u16;
                            let b_index = self.cell_at(b_x, b_z).index() as usize + con as usize;
                            new_distance = distance_to_boundary[b_index].saturating_add(3);
                            if new_distance < distance_to_boundary[span_index] {
                                distance_to_boundary[span_index] = new_distance;
                            };
                        }
                    }
                }
            }
        }

        // Pass 2
        for z in (0..self.height).rev() {
            for x in (0..self.width).rev() {
                let cell = self.cell_at(x, z);
                let max_span_index = cell.index() as usize + cell.count() as usize;
                for span_index in cell.index() as usize..max_span_index {
                    let span = &self.spans[span_index];

                    if let Some(con) = span.con(2) {
                        // (1,0)
                        let a_x = (x as i32 + dir_offset_x(2) as i32) as u16;
                        let a_z = (z as i32 + dir_offset_z(2) as i32) as u16;
                        let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                        let a_span = &self.spans[a_index];
                        new_distance = distance_to_boundary[a_index].saturating_add(2);
                        if new_distance < distance_to_boundary[span_index] {
                            distance_to_boundary[span_index] = new_distance;
                        };

                        // (1,1)
                        if let Some(con) = a_span.con(1) {
                            let b_x = (a_x as i32 + dir_offset_x(1) as i32) as u16;
                            let b_z = (a_z as i32 + dir_offset_z(1) as i32) as u16;
                            let b_index = self.cell_at(b_x, b_z).index() as usize + con as usize;
                            new_distance = distance_to_boundary[b_index].saturating_add(3);
                            if new_distance < distance_to_boundary[span_index] {
                                distance_to_boundary[span_index] = new_distance;
                            };
                        }
                    }
                    if let Some(con) = span.con(1) {
                        // (0,1)
                        let a_x = (x as i32 + dir_offset_x(1) as i32) as u16;
                        let a_z = (z as i32 + dir_offset_z(1) as i32) as u16;
                        let a_index = self.cell_at(a_x, a_z).index() as usize + con as usize;
                        let a_span = &self.spans[a_index];
                        new_distance = distance_to_boundary[a_index].saturating_add(2);
                        if new_distance < distance_to_boundary[span_index] {
                            distance_to_boundary[span_index] = new_distance;
                        };

                        // (1,1)
                        if let Some(con) = a_span.con(0) {
                            let b_x = (a_x as i32 + dir_offset_x(0) as i32) as u16;
                            let b_z = (a_z as i32 + dir_offset_z(0) as i32) as u16;
                            let b_index = self.cell_at(b_x, b_z).index() as usize + con as usize;
                            new_distance = distance_to_boundary[b_index].saturating_add(3);
                            if new_distance < distance_to_boundary[span_index] {
                                distance_to_boundary[span_index] = new_distance;
                            };
                        }
                    }
                }
            }
        }

        // Jan: This just wraps on overflow. Is that intentional???
        let min_boundary_distance = (erosion_radius * 2) as u8;
        #[expect(
            clippy::needless_range_loop,
            reason = "lol the alternative suggestion is really unreadable"
        )]
        for span_index in 0..self.spans.len() {
            if distance_to_boundary[span_index] < min_boundary_distance {
                self.areas[span_index] = AreaType::NOT_WALKABLE;
            }
        }
    }
}
