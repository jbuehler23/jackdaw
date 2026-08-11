use rand::Rng;

/// Parameters for hydraulic erosion simulation.
#[derive(Clone, Debug)]
pub struct ErosionParams {
    /// Number of water droplets to simulate.
    pub iterations: u32,
    /// Radius of erosion effect per step.
    pub erosion_radius: u32,
    /// How much a droplet keeps its previous direction (0-1).
    pub inertia: f32,
    /// How much sediment water can carry.
    pub capacity: f32,
    /// Rate sediment is deposited when water slows.
    pub deposition: f32,
    /// Rate terrain is dissolved by flowing water.
    pub erosion: f32,
    /// How quickly water evaporates (0-1).
    pub evaporation: f32,
    /// Gravity constant.
    pub gravity: f32,
    /// Initial speed of each droplet.
    pub initial_speed: f32,
    /// Initial water volume of each droplet.
    pub initial_water: f32,
    /// Maximum steps per droplet before it dies.
    pub max_lifetime: u32,
    /// Minimum slope used in capacity calculation.
    pub min_slope: f32,
}

/// Iterations above this are clamped down to it rather than run.
///
/// Each iteration costs up to `max_lifetime` steps, so an unbounded
/// count -- typed into the UI, or passed by a script or a headless
/// caller -- can hang the editor for an unbounded amount of time. This
/// is well above the default (70,000) and any legitimate heavy use, and
/// is enforced here regardless of which widget or caller set
/// `iterations`, not just whatever limit an inspector field happens to
/// impose.
pub const MAX_ITERATIONS: u32 = 1_000_000;

/// Clamp a requested iteration count to [`MAX_ITERATIONS`].
fn clamp_iterations(requested: u32) -> u32 {
    requested.min(MAX_ITERATIONS)
}

impl Default for ErosionParams {
    fn default() -> Self {
        Self {
            iterations: 70_000,
            erosion_radius: 3,
            inertia: 0.05,
            capacity: 4.0,
            deposition: 0.03,
            erosion: 0.3,
            evaporation: 0.01,
            gravity: 4.0,
            initial_speed: 1.0,
            initial_water: 1.0,
            max_lifetime: 30,
            min_slope: 0.01,
        }
    }
}

/// Run hydraulic erosion on a heightmap.
///
/// `heights`: mutable row-major height data of length `resolution * resolution`.
/// `resolution`: vertices per edge (same in X and Z).
/// `params`: erosion parameters.
pub fn hydraulic_erosion(heights: &mut [f32], resolution: u32, params: &ErosionParams) {
    let res = resolution as i32;
    let mut rng = rand::rng();
    let iterations = clamp_iterations(params.iterations);

    // Precompute erosion brush weights
    let brush = compute_erosion_brush(params.erosion_radius as i32);

    for _ in 0..iterations {
        // Random start position (avoid edges)
        let mut pos_x = rng.random_range(1.0..(res - 2) as f32);
        let mut pos_z = rng.random_range(1.0..(res - 2) as f32);
        let mut dir_x = 0.0_f32;
        let mut dir_z = 0.0_f32;
        let mut speed = params.initial_speed;
        let mut water = params.initial_water;
        let mut sediment = 0.0_f32;

        for _ in 0..params.max_lifetime {
            let node_x = pos_x as i32;
            let node_z = pos_z as i32;

            // Offset within cell
            let cell_offset_x = pos_x - node_x as f32;
            let cell_offset_z = pos_z - node_z as f32;

            // Compute gradient via bilinear interpolation
            let (grad_x, grad_z, height) =
                compute_gradient(heights, res, node_x, node_z, cell_offset_x, cell_offset_z);

            // Update direction with inertia
            dir_x = dir_x * params.inertia - grad_x * (1.0 - params.inertia);
            dir_z = dir_z * params.inertia - grad_z * (1.0 - params.inertia);

            // Normalize direction
            let len = (dir_x * dir_x + dir_z * dir_z).sqrt();
            if len < 1e-6 {
                // Random direction if flat
                let angle = rng.random_range(0.0..std::f32::consts::TAU);
                dir_x = angle.cos();
                dir_z = angle.sin();
            } else {
                dir_x /= len;
                dir_z /= len;
            }

            // Move droplet
            let new_x = pos_x + dir_x;
            let new_z = pos_z + dir_z;

            // Check bounds
            if new_x < 1.0 || new_x >= (res - 2) as f32 || new_z < 1.0 || new_z >= (res - 2) as f32
            {
                break;
            }

            // Height at new position
            let new_node_x = new_x as i32;
            let new_node_z = new_z as i32;
            let new_cell_x = new_x - new_node_x as f32;
            let new_cell_z = new_z - new_node_z as f32;
            let (_, _, new_height) =
                compute_gradient(heights, res, new_node_x, new_node_z, new_cell_x, new_cell_z);

            let height_diff = new_height - height;

            // Sediment capacity
            let c = (-height_diff).max(params.min_slope) * speed * water * params.capacity;

            if sediment > c || height_diff > 0.0 {
                // Deposit sediment
                let deposit = if height_diff > 0.0 {
                    // Uphill: deposit proportional to height diff, but not more than sediment
                    height_diff.min(sediment)
                } else {
                    (sediment - c) * params.deposition
                };

                sediment -= deposit;
                deposit_at(
                    heights,
                    res,
                    node_x,
                    node_z,
                    cell_offset_x,
                    cell_offset_z,
                    deposit,
                );
            } else {
                // Erode terrain
                let erode_amount = ((c - sediment) * params.erosion).min(-height_diff);

                erode_at(heights, res, node_x, node_z, erode_amount, &brush);
                sediment += erode_amount;
            }

            // Update droplet state
            speed = ((speed * speed + height_diff * params.gravity).max(0.0)).sqrt();
            water *= 1.0 - params.evaporation;

            pos_x = new_x;
            pos_z = new_z;
        }
    }
}

struct ErosionBrush {
    offsets: Vec<(i32, i32)>,
    weights: Vec<f32>,
}

fn compute_erosion_brush(radius: i32) -> ErosionBrush {
    let mut offsets = Vec::new();
    let mut weights = Vec::new();
    let mut weight_sum = 0.0;

    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let dist = ((dx * dx + dz * dz) as f32).sqrt();
            if dist <= radius as f32 {
                let w = (1.0 - dist / radius as f32).max(0.0);
                offsets.push((dx, dz));
                weights.push(w);
                weight_sum += w;
            }
        }
    }

    // Normalize
    if weight_sum > 0.0 {
        for w in &mut weights {
            *w /= weight_sum;
        }
    }

    ErosionBrush { offsets, weights }
}

fn compute_gradient(
    heights: &[f32],
    res: i32,
    node_x: i32,
    node_z: i32,
    offset_x: f32,
    offset_z: f32,
) -> (f32, f32, f32) {
    let idx = |x: i32, z: i32| -> f32 { heights[(z * res + x) as usize] };

    let h_nw = idx(node_x, node_z);
    let h_ne = idx(node_x + 1, node_z);
    let h_sw = idx(node_x, node_z + 1);
    let h_se = idx(node_x + 1, node_z + 1);

    let grad_x = (h_ne - h_nw) * (1.0 - offset_z) + (h_se - h_sw) * offset_z;
    let grad_z = (h_sw - h_nw) * (1.0 - offset_x) + (h_se - h_ne) * offset_x;

    let height = h_nw * (1.0 - offset_x) * (1.0 - offset_z)
        + h_ne * offset_x * (1.0 - offset_z)
        + h_sw * (1.0 - offset_x) * offset_z
        + h_se * offset_x * offset_z;

    (grad_x, grad_z, height)
}

fn deposit_at(
    heights: &mut [f32],
    res: i32,
    node_x: i32,
    node_z: i32,
    offset_x: f32,
    offset_z: f32,
    amount: f32,
) {
    // Distribute deposit via bilinear weights to the four surrounding vertices
    let w_nw = (1.0 - offset_x) * (1.0 - offset_z);
    let w_ne = offset_x * (1.0 - offset_z);
    let w_sw = (1.0 - offset_x) * offset_z;
    let w_se = offset_x * offset_z;

    heights[(node_z * res + node_x) as usize] += amount * w_nw;
    heights[(node_z * res + node_x + 1) as usize] += amount * w_ne;
    heights[((node_z + 1) * res + node_x) as usize] += amount * w_sw;
    heights[((node_z + 1) * res + node_x + 1) as usize] += amount * w_se;
}

fn erode_at(
    heights: &mut [f32],
    res: i32,
    node_x: i32,
    node_z: i32,
    amount: f32,
    brush: &ErosionBrush,
) {
    for (i, &(dx, dz)) in brush.offsets.iter().enumerate() {
        let x = node_x + dx;
        let z = node_z + dz;
        if x >= 0 && x < res && z >= 0 && z < res {
            heights[(z * res + x) as usize] -= amount * brush.weights[i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// I11 pinning test: an iteration count typed into the UI (or set by
    /// any other caller) is capped regardless of what requested it, since
    /// each iteration costs real, unbounded wall-clock time.
    #[test]
    fn a_pathological_iteration_count_is_clamped() {
        assert_eq!(clamp_iterations(u32::MAX), MAX_ITERATIONS);
        assert_eq!(clamp_iterations(MAX_ITERATIONS + 1), MAX_ITERATIONS);
    }

    #[test]
    fn ordinary_iteration_counts_are_left_alone() {
        assert_eq!(clamp_iterations(70_000), 70_000);
        assert_eq!(clamp_iterations(0), 0);
        assert_eq!(clamp_iterations(MAX_ITERATIONS), MAX_ITERATIONS);
    }
}
