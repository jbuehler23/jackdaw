use crate::{
    heightfield::Heightfield,
    math::{dir_offset_x, dir_offset_z},
    span::{AreaType, Span},
};

impl Heightfield {
    /// Adds the walkable flag to spans which are adjacent to a walkable span and the height difference is small enough for the agent to walk over.
    pub fn filter_low_hanging_walkable_obstacles(&mut self, walkable_climb: u16) {
        for z in 0..self.height {
            for x in 0..self.width {
                let mut previous_span: Option<Span> = None;
                let mut previous_was_walkable = false;
                let mut previous_area_id = AreaType::NOT_WALKABLE;
                let mut span = self.span_at_mut(x, z);

                // For each span in the column...
                while let Some(current_span) = span {
                    let walkable = current_span.area.is_walkable();

                    // If current span is not walkable, but there is walkable span just below it and the height difference
                    // is small enough for the agent to walk over, mark the current span as walkable too.
                    if let Some(previous_span) = previous_span.as_ref()
                        && !walkable
                        && previous_was_walkable
                        && (current_span.max as i32 - previous_span.max as i32)
                            <= walkable_climb as i32
                    {
                        current_span.area = previous_area_id;
                    }

                    // Copy the original walkable value regardless of whether we changed it.
                    // This prevents multiple consecutive non-walkable spans from being erroneously marked as walkable.
                    previous_was_walkable = walkable;
                    previous_area_id = current_span.area;
                    previous_span.replace(current_span.clone());
                    span = current_span.next.map(|key| self.span_mut(key));
                }
            }
        }
    }

    /// Removes the walkable flag from spans which are adjacent to a ledge.
    pub fn filter_ledge_spans(&mut self, walkable_height: u16, walkable_climb: u16) {
        // Mark spans that are adjacent to a ledge as unwalkable..
        for z in 0..self.height {
            for x in 0..self.width {
                let mut span_key = self.span_key_at(x, z);
                while let Some(current_span_key) = span_key {
                    let filtered = {
                        let span = self.span(current_span_key);
                        span_key = span.next;

                        // Skip non-walkable spans.
                        if !self.span(current_span_key).area.is_walkable() {
                            continue;
                        }

                        let floor = span.max as i32;
                        let ceiling = span
                            .next
                            .map(|key| self.span(key).min as i32)
                            .unwrap_or(Self::MAX_HEIGHTFIELD_HEIGHT as i32);

                        // The difference between this walkable area and the lowest neighbor walkable area.
                        // This is the difference between the current span and all neighbor spans that have
                        // enough space for an agent to move between, but not accounting at all for surface slope.
                        let mut lowest_neighbor_floor_difference =
                            Self::MAX_HEIGHTFIELD_HEIGHT as i32;

                        // Min and max height of accessible neighbours.
                        let mut lowest_traversable_neighbor_floor = span.max as i32;
                        let mut highest_traversable_neighbor_floor = span.max as i32;

                        for direction in 0..4 {
                            let neighbor_x = x as i32 + dir_offset_x(direction) as i32;
                            let neighbor_z = z as i32 + dir_offset_z(direction) as i32;

                            // Skip neighbours which are out of bounds.
                            if !self.contains(neighbor_x, neighbor_z) {
                                lowest_neighbor_floor_difference = -(walkable_climb as i32) - 1;
                                break;
                            }
                            let neighbor_x = neighbor_x as u16;
                            let neighbor_z = neighbor_z as u16;

                            let mut neighbor_span = self.span_at(neighbor_x, neighbor_z);

                            // The most we can step down to the neighbor is the walkableClimb distance.
                            // Start with the area under the neighbor span
                            let mut neighbor_ceiling = neighbor_span
                                .map(|span| span.min as i32)
                                .unwrap_or(Self::MAX_HEIGHTFIELD_HEIGHT as i32);

                            // Skip neighbour if the gap between the spans is too small.
                            if ceiling.min(neighbor_ceiling) - floor >= walkable_height as i32 {
                                lowest_neighbor_floor_difference = -(walkable_climb as i32) - 1;
                                break;
                            }

                            // For each span in the neighboring column...
                            while let Some(current_neighbor_span) = neighbor_span {
                                let neighbor_floor = current_neighbor_span.max as i32;
                                neighbor_ceiling = current_neighbor_span
                                    .next
                                    .map(|key| self.span(key).min as i32)
                                    .unwrap_or(Self::MAX_HEIGHTFIELD_HEIGHT as i32);

                                // Only consider neighboring areas that have enough overlap to be potentially traversable.
                                if ceiling.min(neighbor_ceiling) - floor.max(neighbor_floor)
                                    < walkable_height as i32
                                {
                                    // No space to travese between them.
                                    neighbor_span =
                                        current_neighbor_span.next.map(|key| self.span(key));
                                    continue;
                                }

                                let neighbor_floor_difference = neighbor_floor - floor;
                                lowest_neighbor_floor_difference =
                                    lowest_neighbor_floor_difference.min(neighbor_floor_difference);

                                // Find min/max accessible neighbor height.
                                // Only consider neighbors that are at most walkableClimb away.
                                if neighbor_floor_difference.abs() <= walkable_climb as i32 {
                                    // There is space to move to the neighbor cell and the slope isn't too much.
                                    lowest_traversable_neighbor_floor =
                                        lowest_traversable_neighbor_floor.min(neighbor_floor);
                                    highest_traversable_neighbor_floor =
                                        highest_traversable_neighbor_floor.max(neighbor_floor);
                                } else if neighbor_floor_difference < -(walkable_climb as i32) {
                                    // We already know this will be considered a ledge span so we can early-out
                                    break;
                                }

                                neighbor_span =
                                    current_neighbor_span.next.map(|key| self.span(key));
                            }
                        }

                        // The current span is close to a ledge if the magnitude of the drop to any neighbour span is greater than the walkableClimb distance.
                        // That is, there is a gap that is large enough to let an agent move between them, but the drop (surface slope) is too large to allow it.
                        // (If this is the case, then biggestNeighborStepDown will be negative, so compare against the negative walkableClimb as a means of checking
                        // the magnitude of the delta)
                        if lowest_neighbor_floor_difference < -(walkable_climb as i32) {
                            true
                        } else {
                            // If the difference between all neighbor floors is too large, this is a steep slope, so mark the span as an unwalkable ledge.
                            highest_traversable_neighbor_floor - lowest_traversable_neighbor_floor
                                > walkable_climb as i32
                        }
                    };
                    let span = self.span_mut(current_span_key);
                    if filtered {
                        span.area = AreaType::NOT_WALKABLE;
                    }
                }
            }
        }
    }

    /// Taken 1:1 from the original implementation.
    const MAX_HEIGHTFIELD_HEIGHT: u16 = u16::MAX;

    /// Removes the walkable flag from spans which do not have enough space above them for the agent to stand there.
    pub fn filter_walkable_low_height_spans(&mut self, walkable_height: u16) {
        // Remove walkable flag from spans which do not have enough
        // space above them for the agent to stand there.
        for z in 0..self.height {
            for x in 0..self.width {
                let mut span_key = self.span_key_at(x, z);
                while let Some(current_span_key) = span_key {
                    let filtered: bool = {
                        let span = self.span(current_span_key);
                        let floor = span.max as i32;
                        let ceiling = span
                            .next
                            .map(|key| self.span(key).min as i32)
                            .unwrap_or(Self::MAX_HEIGHTFIELD_HEIGHT as i32);
                        ceiling - floor < walkable_height as i32
                    };

                    let span = self.span_mut(current_span_key);
                    if filtered {
                        span.area = AreaType::NOT_WALKABLE;
                    }

                    span_key = span.next;
                }
            }
        }
    }
}
