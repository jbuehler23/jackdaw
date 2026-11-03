//! Seeded random offsets for the vertices of a closed polygon mesh, each
//! vertex moved once so the faces that share it stay joined.

use std::collections::HashMap;

use glam::Vec3;

use crate::splitmix64 as mix;

/// How far and which way [`jitter_offsets`] moves vertices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Jitter {
    /// Longest offset any vertex gets.
    pub amount: f32,
    pub seed: u32,
    /// Factor per axis, each 0 to 1, applied to the offset after it is drawn.
    pub axis_mask: Vec3,
    /// Move along the vertex normal, either way, rather than in any direction.
    pub normal_only: bool,
}

/// One offset per entry of `positions`, zero where `picked` is false.
///
/// `rings` are the faces as vertex indices, used for the vertex normals
/// `normal_only` follows. `stream` is a stable id of the mesh, so identical
/// meshes get different offsets and one mesh gets the same offsets every
/// time. Vertices at bit-identical
/// positions share one offset, and they all move when any of them is picked.
pub fn jitter_offsets(
    positions: &[Vec3],
    rings: &[Vec<usize>],
    picked: &[bool],
    jitter: &Jitter,
    stream: u64,
) -> Vec<Vec3> {
    let leaders = coincident_leaders(positions);
    let mut group_picked = vec![false; positions.len()];
    for (vert, &leader) in leaders.iter().enumerate() {
        if picked.get(vert).copied().unwrap_or(false) {
            group_picked[leader] = true;
        }
    }
    let normals = jitter
        .normal_only
        .then(|| group_normals(positions, rings, &leaders));
    let state = mix(mix(u64::from(jitter.seed)) ^ stream);
    leaders
        .iter()
        .map(|&leader| {
            if !group_picked[leader] {
                return Vec3::ZERO;
            }
            let key = mix(state ^ leader as u64);
            let raw = match &normals {
                Some(normals) => normals[leader] * jitter.amount * (2.0 * unit(key, 0) - 1.0),
                None => random_direction(key) * jitter.amount * unit(key, 3),
            };
            raw * jitter.axis_mask
        })
        .collect()
}

/// For each vertex, the lowest index at the same position.
fn coincident_leaders(positions: &[Vec3]) -> Vec<usize> {
    let mut first: HashMap<[u32; 3], usize> = HashMap::with_capacity(positions.len());
    positions
        .iter()
        .enumerate()
        .map(|(vert, point)| {
            let key = (*point + Vec3::ZERO).to_array().map(f32::to_bits);
            *first.entry(key).or_insert(vert)
        })
        .collect()
}

/// Area-weighted normal per coincident group, stored at its leader.
fn group_normals(positions: &[Vec3], rings: &[Vec<usize>], leaders: &[usize]) -> Vec<Vec3> {
    let mut sums = vec![Vec3::ZERO; positions.len()];
    for ring in rings {
        let points: Vec<Vec3> = ring
            .iter()
            .filter_map(|&vert| positions.get(vert).copied())
            .collect();
        if points.len() != ring.len() {
            continue;
        }
        let area: Vec3 = points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .map(|(a, b)| a.cross(*b))
            .sum();
        for &vert in ring {
            sums[leaders[vert]] += area;
        }
    }
    sums.into_iter().map(Vec3::normalize_or_zero).collect()
}

/// A direction spread evenly over the unit sphere.
fn random_direction(key: u64) -> Vec3 {
    let z = 2.0 * unit(key, 1) - 1.0;
    let angle = std::f32::consts::TAU * unit(key, 2);
    let ring = (1.0 - z * z).max(0.0).sqrt();
    Vec3::new(ring * angle.cos(), ring * angle.sin(), z)
}

/// A number in [0, 1) drawn from `key`; each `lane` is independent.
fn unit(key: u64, lane: u64) -> f32 {
    (mix(key ^ mix(lane)) >> 40) as f32 / (1u64 << 24) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> (Vec<Vec3>, Vec<Vec<usize>>) {
        let positions = (0..8)
            .map(|i| {
                Vec3::new(
                    if i & 1 == 0 { -0.5 } else { 0.5 },
                    if i & 2 == 0 { -0.5 } else { 0.5 },
                    if i & 4 == 0 { -0.5 } else { 0.5 },
                )
            })
            .collect();
        let rings = vec![
            vec![0, 2, 3, 1],
            vec![4, 5, 7, 6],
            vec![0, 1, 5, 4],
            vec![2, 6, 7, 3],
            vec![0, 4, 6, 2],
            vec![1, 3, 7, 5],
        ];
        (positions, rings)
    }

    fn jitter(amount: f32, seed: u32) -> Jitter {
        Jitter {
            amount,
            seed,
            axis_mask: Vec3::ONE,
            normal_only: false,
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_offsets_and_another_seed_differs() {
        let (positions, rings) = cube();
        let all = vec![true; positions.len()];
        let first = jitter_offsets(&positions, &rings, &all, &jitter(0.1, 7), 0);
        let again = jitter_offsets(&positions, &rings, &all, &jitter(0.1, 7), 0);
        let other = jitter_offsets(&positions, &rings, &all, &jitter(0.1, 8), 0);
        let stream = jitter_offsets(&positions, &rings, &all, &jitter(0.1, 7), 1);
        assert_eq!(first, again);
        assert_ne!(first, other);
        assert_ne!(first, stream);
    }

    #[test]
    fn no_offset_is_longer_than_the_amount() {
        let (positions, rings) = cube();
        let all = vec![true; positions.len()];
        for seed in 0..200 {
            let offsets = jitter_offsets(&positions, &rings, &all, &jitter(0.25, seed), 0);
            assert!(offsets.iter().all(|offset| offset.length() <= 0.25 + 1e-6));
            assert!(offsets.iter().any(|offset| offset.length() > 0.0));
        }
    }

    #[test]
    fn unpicked_vertices_stay_and_picked_ones_keep_their_offsets() {
        let (positions, rings) = cube();
        let all = vec![true; positions.len()];
        let mut some = vec![false; positions.len()];
        some[3] = true;
        let everything = jitter_offsets(&positions, &rings, &all, &jitter(0.1, 3), 0);
        let one = jitter_offsets(&positions, &rings, &some, &jitter(0.1, 3), 0);
        for (vert, offset) in one.iter().enumerate() {
            if vert == 3 {
                assert_eq!(*offset, everything[3]);
            } else {
                assert_eq!(*offset, Vec3::ZERO);
            }
        }
    }

    #[test]
    fn coincident_vertices_share_one_offset() {
        let (mut positions, mut rings) = cube();
        positions.push(positions[7]);
        rings[1] = vec![4, 5, 8, 6];
        let mut picked = vec![false; positions.len()];
        picked[8] = true;
        let offsets = jitter_offsets(&positions, &rings, &picked, &jitter(0.1, 5), 0);
        assert_ne!(offsets[7], Vec3::ZERO);
        assert_eq!(offsets[7], offsets[8]);
    }

    #[test]
    fn a_zero_axis_never_moves() {
        let (positions, rings) = cube();
        let all = vec![true; positions.len()];
        let mut flat = jitter(0.2, 11);
        flat.axis_mask = Vec3::new(1.0, 0.0, 0.5);
        let offsets = jitter_offsets(&positions, &rings, &all, &flat, 0);
        assert!(offsets.iter().all(|offset| offset.y == 0.0));
        assert!(offsets.iter().any(|offset| offset.x != 0.0));
    }

    #[test]
    fn normal_only_moves_along_the_vertex_normal() {
        let (positions, rings) = cube();
        let all = vec![true; positions.len()];
        let mut along = jitter(0.2, 2);
        along.normal_only = true;
        let offsets = jitter_offsets(&positions, &rings, &all, &along, 0);
        for (point, offset) in positions.iter().zip(&offsets) {
            assert!(offset.length() <= 0.2 + 1e-6);
            assert!(
                offset.cross(point.normalize()).length() < 1e-5,
                "{offset} is along {point}"
            );
        }
    }

    #[test]
    fn a_zero_amount_moves_nothing() {
        let (positions, rings) = cube();
        let all = vec![true; positions.len()];
        let offsets = jitter_offsets(&positions, &rings, &all, &jitter(0.0, 9), 0);
        assert!(offsets.iter().all(|offset| *offset == Vec3::ZERO));
    }
}
