use crate::heightmap::Heightmap;

/// Raw mesh data for a terrain chunk, ready to be turned into a Bevy `Mesh`.
pub struct ChunkMeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Grid coordinate each vertex was sampled from, parallel to
    /// `positions`.
    ///
    /// The smooth and flat meshers emit vertices in different orders and
    /// different counts, so anything that wants a per-vertex attribute of
    /// its own -- the paint-channel vertex colours, above all -- cannot
    /// re-derive the order by walking the grid itself. It reads this
    /// instead and stays in lockstep with whichever mesher ran.
    pub grid: Vec<[u32; 2]>,
}

/// Build smooth-shaded mesh data for a single chunk of the terrain.
///
/// One shared vertex per grid point, normals from central differences, so
/// the surface reads as a continuous landscape.
///
/// `chunk_x` / `chunk_z`: chunk indices (0-based).
/// `chunk_size`: number of cells per chunk edge.
///
/// Positions are in terrain-local space (terrain centered at origin).
pub fn build_chunk_mesh_data(
    heightmap: &Heightmap,
    chunk_x: u32,
    chunk_z: u32,
    chunk_size: u32,
) -> ChunkMeshData {
    let res = heightmap.resolution;
    let cell = heightmap.cell_size();
    let half_size = heightmap.size / 2.0;

    // Grid range for this chunk (in vertex indices)
    let x_start = chunk_x * chunk_size;
    let z_start = chunk_z * chunk_size;
    let x_end = ((chunk_x + 1) * chunk_size + 1).min(res);
    let z_end = ((chunk_z + 1) * chunk_size + 1).min(res);

    let cols = x_end - x_start;
    let rows = z_end - z_start;

    let mut positions = Vec::with_capacity((cols * rows) as usize);
    let mut normals = Vec::with_capacity((cols * rows) as usize);
    let mut uvs = Vec::with_capacity((cols * rows) as usize);
    let mut grid = Vec::with_capacity((cols * rows) as usize);

    for lz in 0..rows {
        for lx in 0..cols {
            let gx = x_start + lx;
            let gz = z_start + lz;
            let h = heightmap.get_height(gx, gz);

            let world_x = gx as f32 * cell.x - half_size.x;
            let world_z = gz as f32 * cell.y - half_size.y;

            positions.push([world_x, h, world_z]);
            grid.push([gx, gz]);

            // UV normalized across full terrain
            let u = gx as f32 / (res - 1) as f32;
            let v = gz as f32 / (res - 1) as f32;
            uvs.push([u, v]);

            // Normal via central differences
            let h_left = if gx > 0 {
                heightmap.get_height(gx - 1, gz)
            } else {
                h
            };
            let h_right = if gx < res - 1 {
                heightmap.get_height(gx + 1, gz)
            } else {
                h
            };
            let h_down = if gz > 0 {
                heightmap.get_height(gx, gz - 1)
            } else {
                h
            };
            let h_up = if gz < res - 1 {
                heightmap.get_height(gx, gz + 1)
            } else {
                h
            };

            let dx = (h_right - h_left) / (2.0 * cell.x);
            let dz = (h_up - h_down) / (2.0 * cell.y);
            let len = (dx * dx + 1.0 + dz * dz).sqrt();
            normals.push([-dx / len, 1.0 / len, -dz / len]);
        }
    }

    // Triangle indices
    let cells_x = cols - 1;
    let cells_z = rows - 1;
    let mut indices = Vec::with_capacity((cells_x * cells_z * 6) as usize);

    for lz in 0..cells_z {
        for lx in 0..cells_x {
            let tl = lz * cols + lx;
            let tr = tl + 1;
            let bl = (lz + 1) * cols + lx;
            let br = bl + 1;

            indices.push(tl);
            indices.push(bl);
            indices.push(tr);

            indices.push(tr);
            indices.push(bl);
            indices.push(br);
        }
    }

    ChunkMeshData {
        positions,
        normals,
        uvs,
        indices,
        grid,
    }
}

/// Build flat-shaded mesh data for a single chunk of the terrain.
///
/// Same grid, same triangles, same winding as [`build_chunk_mesh_data`];
/// the only difference is that no vertex is shared. Each triangle gets
/// three of its own and all three carry that triangle's face normal, so
/// the shading breaks at every cell edge instead of blending across it.
///
/// This is what a quantized terrain is meshed with. Snapped heights drawn
/// with smoothed vertex normals read as a soft ramp -- the terracing is in
/// the data and invisible on screen -- which defeats the point of turning
/// quantization on.
pub fn build_chunk_mesh_data_flat(
    heightmap: &Heightmap,
    chunk_x: u32,
    chunk_z: u32,
    chunk_size: u32,
) -> ChunkMeshData {
    let res = heightmap.resolution;
    let cell = heightmap.cell_size();
    let half_size = heightmap.size / 2.0;

    let x_start = chunk_x * chunk_size;
    let z_start = chunk_z * chunk_size;
    let x_end = ((chunk_x + 1) * chunk_size + 1).min(res);
    let z_end = ((chunk_z + 1) * chunk_size + 1).min(res);

    let cols = x_end.saturating_sub(x_start);
    let rows = z_end.saturating_sub(z_start);
    let cells_x = cols.saturating_sub(1);
    let cells_z = rows.saturating_sub(1);

    let vertex_count = (cells_x * cells_z * 6) as usize;
    let mut positions = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    let mut uvs = Vec::with_capacity(vertex_count);
    let mut grid = Vec::with_capacity(vertex_count);
    let mut indices = Vec::with_capacity(vertex_count);

    let vertex_at = |gx: u32, gz: u32| -> ([f32; 3], [f32; 2]) {
        let h = heightmap.get_height(gx, gz);
        let world_x = gx as f32 * cell.x - half_size.x;
        let world_z = gz as f32 * cell.y - half_size.y;
        let u = gx as f32 / (res - 1) as f32;
        let v = gz as f32 / (res - 1) as f32;
        ([world_x, h, world_z], [u, v])
    };

    for lz in 0..cells_z {
        for lx in 0..cells_x {
            let gx = x_start + lx;
            let gz = z_start + lz;

            let tl = (gx, gz);
            let tr = (gx + 1, gz);
            let bl = (gx, gz + 1);
            let br = (gx + 1, gz + 1);

            for [a, b, c] in [[tl, bl, tr], [tr, bl, br]] {
                let (pa, ua) = vertex_at(a.0, a.1);
                let (pb, ub) = vertex_at(b.0, b.1);
                let (pc, uc) = vertex_at(c.0, c.1);
                let normal = face_normal(pa, pb, pc);

                for ((p, u), g) in [(pa, ua), (pb, ub), (pc, uc)].into_iter().zip([a, b, c]) {
                    indices.push(positions.len() as u32);
                    positions.push(p);
                    normals.push(normal);
                    uvs.push(u);
                    grid.push([g.0, g.1]);
                }
            }
        }
    }

    ChunkMeshData {
        positions,
        normals,
        uvs,
        indices,
        grid,
    }
}

/// Normal of the triangle `a, b, c`, wound counter-clockwise when seen
/// from the front. A degenerate triangle -- two coincident corners on a
/// zero-size terrain -- reports straight up rather than a NaN that would
/// blacken the chunk.
fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len <= f32::EPSILON {
        return [0.0, 1.0, 0.0];
    }
    [n[0] / len, n[1] / len, n[2] / len]
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::Vec2;

    fn flat_ground(resolution: u32) -> Heightmap {
        Heightmap::new(resolution, Vec2::splat((resolution - 1) as f32), 10.0)
    }

    #[test]
    fn the_smooth_mesher_records_a_grid_coordinate_per_vertex() {
        let data = build_chunk_mesh_data(&flat_ground(9), 0, 0, 4);
        assert_eq!(data.grid.len(), data.positions.len());
        // Row-major over the 5x5 vertex window this chunk owns.
        assert_eq!(data.grid[0], [0, 0]);
        assert_eq!(data.grid[4], [4, 0]);
        assert_eq!(data.grid[5], [0, 1]);
    }

    #[test]
    fn the_flat_mesher_emits_three_unshared_vertices_per_triangle() {
        let data = build_chunk_mesh_data_flat(&flat_ground(9), 0, 0, 4);
        // 4x4 cells, two triangles each, three vertices each.
        assert_eq!(data.positions.len(), 4 * 4 * 6);
        assert_eq!(data.normals.len(), data.positions.len());
        assert_eq!(data.uvs.len(), data.positions.len());
        assert_eq!(data.grid.len(), data.positions.len());
        // Indices are sequential: nothing is reused.
        let expected: Vec<u32> = (0..data.positions.len() as u32).collect();
        assert_eq!(data.indices, expected);
    }

    #[test]
    fn flat_ground_meshes_with_upward_face_normals() {
        let data = build_chunk_mesh_data_flat(&flat_ground(9), 0, 0, 4);
        for n in &data.normals {
            assert!((n[1] - 1.0).abs() < 1e-5, "normal {n:?} should point up");
        }
    }

    #[test]
    fn a_terrace_edge_gets_two_different_normals_rather_than_one_blended_one() {
        // A single step: everything at x >= 2 is one unit higher.
        let mut hm = flat_ground(5);
        for gz in 0..5 {
            for gx in 2..5 {
                hm.set_height(gx, gz, 1.0);
            }
        }
        let data = build_chunk_mesh_data_flat(&hm, 0, 0, 4);
        let mut distinct: Vec<[f32; 3]> = Vec::new();
        for n in &data.normals {
            if !distinct
                .iter()
                .any(|d| (d[0] - n[0]).abs() < 1e-4 && (d[2] - n[2]).abs() < 1e-4)
            {
                distinct.push(*n);
            }
        }
        assert!(
            distinct.len() > 1,
            "the riser and the flats should not share a normal, got {distinct:?}"
        );
        // The flats are still exactly level: no smoothing bled the step
        // into the cells beside it.
        assert!(
            data.normals.iter().any(|n| (n[1] - 1.0).abs() < 1e-5),
            "flat cells should stay flat"
        );
    }

    #[test]
    fn both_meshers_cover_the_same_grid_window() {
        let hm = flat_ground(9);
        let smooth = build_chunk_mesh_data(&hm, 1, 1, 4);
        let flat = build_chunk_mesh_data_flat(&hm, 1, 1, 4);

        let bounds = |grid: &[[u32; 2]]| {
            let xs = grid.iter().map(|g| g[0]);
            let zs = grid.iter().map(|g| g[1]);
            (
                xs.clone().min().unwrap(),
                xs.max().unwrap(),
                zs.clone().min().unwrap(),
                zs.max().unwrap(),
            )
        };
        assert_eq!(bounds(&smooth.grid), bounds(&flat.grid));
    }
}
