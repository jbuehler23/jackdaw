//! Levels of detail generated from a model's own meshes by meshoptimizer, as
//! Godot, Unreal and Unity 6 generate them at import.
//!
//! Each primitive of the model is simplified towards a share of its
//! triangles, keeping the vertices on its borders, and then carries only the
//! vertices its triangles still use. The result is deterministic, so it is
//! cached by the model's bytes and settings.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use bevy::mesh::{Indices, Mesh, VertexAttributeValues};

/// A simplified primitive: its triangles, and for each vertex it keeps the
/// vertex of the source it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimplifiedPrimitive {
    pub indices: Vec<u32>,
    pub source_vertices: Vec<u32>,
}

/// Below this share of its triangles a primitive is simplified without
/// keeping its topology, which reaches any target.
const SLOPPY_BELOW: f32 = 0.05;

/// Simplify a primitive with `positions` and triangle `indices` towards
/// `ratio` of its triangles, moving no surface further than `error` of the
/// primitive's extent.
pub fn simplify_primitive(
    positions: &[[f32; 3]],
    indices: &[u32],
    ratio: f32,
    error: f32,
) -> SimplifiedPrimitive {
    let ratio = ratio.clamp(0.0, 1.0);
    let target = ((indices.len() / 3) as f32 * ratio).round() as usize * 3;
    let Ok(vertices) = meshopt::VertexDataAdapter::new(meshopt::typed_to_bytes(positions), 12, 0)
    else {
        return SimplifiedPrimitive {
            indices: indices.to_vec(),
            source_vertices: (0..positions.len() as u32).collect(),
        };
    };
    let mut simplified = if ratio < SLOPPY_BELOW {
        meshopt::simplify_sloppy(indices, &vertices, target, 1.0, None)
    } else {
        meshopt::simplify(
            indices,
            &vertices,
            target,
            error,
            meshopt::SimplifyOptions::LockBorder,
            None,
        )
    };
    let mut remap = vec![u32::MAX; positions.len()];
    let mut source_vertices = Vec::new();
    for index in &mut simplified {
        let new = &mut remap[*index as usize];
        if *new == u32::MAX {
            *new = source_vertices.len() as u32;
            source_vertices.push(*index);
        }
        *index = *new;
    }
    SimplifiedPrimitive {
        indices: simplified,
        source_vertices,
    }
}

/// The triangles of `mesh` as indices, numbering them when it has none.
pub fn triangle_indices(mesh: &Mesh) -> Vec<u32> {
    match mesh.indices() {
        Some(Indices::U16(indices)) => indices.iter().map(|index| u32::from(*index)).collect(),
        Some(Indices::U32(indices)) => indices.clone(),
        None => (0..mesh.count_vertices() as u32).collect(),
    }
}

/// The positions of `mesh`, when it has three-component float positions.
pub fn positions(mesh: &Mesh) -> Option<Vec<[f32; 3]>> {
    match mesh.attribute(Mesh::ATTRIBUTE_POSITION)? {
        VertexAttributeValues::Float32x3(positions) => Some(positions.clone()),
        _ => None,
    }
}

fn pick<T: Clone>(values: &[T], order: &[u32]) -> Vec<T> {
    order
        .iter()
        .filter_map(|index| values.get(*index as usize).cloned())
        .collect()
}

fn pick_values(values: &VertexAttributeValues, order: &[u32]) -> VertexAttributeValues {
    macro_rules! picked {
        ($($variant:ident),* $(,)?) => {
            match values {
                $(VertexAttributeValues::$variant(v) => VertexAttributeValues::$variant(pick(v, order)),)*
            }
        };
    }
    picked! {
        Uint8, Uint8x2, Uint8x4, Sint8, Sint8x2, Sint8x4, Unorm8, Unorm8x2, Unorm8x4, Snorm8,
        Snorm8x2, Snorm8x4, Uint16, Uint16x2, Uint16x4, Sint16, Sint16x2, Sint16x4, Unorm16,
        Unorm16x2, Unorm16x4, Snorm16, Snorm16x2, Snorm16x4, Float16, Float16x2, Float16x4,
        Float32, Float32x2, Float32x3, Float32x4, Uint32, Uint32x2, Uint32x3, Uint32x4, Sint32,
        Sint32x2, Sint32x3, Sint32x4, Float64, Float64x2, Float64x3, Float64x4, Unorm8x4Bgra,
        Unorm10_10_10_2,
    }
}

/// `mesh` drawn with a simplified primitive's triangles and only the vertices
/// they use.
pub fn simplified_mesh(mesh: &Mesh, simplified: &SimplifiedPrimitive) -> Mesh {
    let mut level = Mesh::new(mesh.primitive_topology(), mesh.asset_usage);
    for (attribute, values) in mesh.attributes() {
        level.insert_attribute(*attribute, pick_values(values, &simplified.source_vertices));
    }
    level.insert_indices(Indices::U32(simplified.indices.clone()));
    level
}

/// Where the simplified primitives of a model with these bytes and settings
/// are cached under `cache`.
pub fn cache_file(cache: &Path, model: &str, source: &[u8], settings: &[u8]) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    settings.hash(&mut hasher);
    cache.join(format!("{model}.{:016x}.lods", hasher.finish()))
}

/// The bytes of cached primitives, each under its label.
pub fn encode(primitives: &[(String, SimplifiedPrimitive)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut put = |value: u32| bytes.extend_from_slice(&value.to_le_bytes());
    put(primitives.len() as u32);
    for (label, primitive) in primitives {
        put(label.len() as u32);
        for byte in label.as_bytes() {
            put(u32::from(*byte));
        }
        put(primitive.indices.len() as u32);
        for index in &primitive.indices {
            put(*index);
        }
        put(primitive.source_vertices.len() as u32);
        for vertex in &primitive.source_vertices {
            put(*vertex);
        }
    }
    bytes
}

/// Read cached primitives back, or `None` for bytes that are not a cache.
pub fn decode(bytes: &[u8]) -> Option<Vec<(String, SimplifiedPrimitive)>> {
    let mut words = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    let mut take = |count: u32| -> Option<Vec<u32>> { (0..count).map(|_| words.next()).collect() };
    let count = take(1)?[0];
    let mut primitives = Vec::new();
    for _ in 0..count {
        let length = take(1)?[0];
        let label: String = take(length)?
            .into_iter()
            .map(|byte| char::from(u8::try_from(byte).ok()?).into())
            .collect::<Option<Vec<char>>>()?
            .into_iter()
            .collect();
        let indices_length = take(1)?[0];
        let indices = take(indices_length)?;
        let vertices_length = take(1)?[0];
        let source_vertices = take(vertices_length)?;
        primitives.push((
            label,
            SimplifiedPrimitive {
                indices,
                source_vertices,
            },
        ));
    }
    Some(primitives)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat grid of `cells` by `cells` squares, its vertices shared.
    fn grid(cells: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
        let side = cells + 1;
        let positions = (0..side * side)
            .map(|index| {
                [
                    (index % side) as f32,
                    ((index / side) as f32 * 0.37).sin(),
                    (index / side) as f32,
                ]
            })
            .collect();
        let mut indices = Vec::new();
        for row in 0..cells {
            for column in 0..cells {
                let corner = row * side + column;
                indices.extend([corner, corner + side, corner + 1]);
                indices.extend([corner + 1, corner + side, corner + side + 1]);
            }
        }
        (positions, indices)
    }

    #[test]
    fn a_generated_level_lands_near_its_target_triangle_count() {
        let (positions, indices) = grid(40);
        let triangles = indices.len() / 3;
        for ratio in [0.5, 0.25] {
            let level = simplify_primitive(&positions, &indices, ratio, 1.0);
            let wanted = triangles as f32 * ratio;
            let got = (level.indices.len() / 3) as f32;
            assert!(
                (got - wanted).abs() <= wanted * 0.1,
                "{ratio}: {got} of {wanted}"
            );
        }
    }

    #[test]
    fn generating_twice_gives_the_same_level() {
        let (positions, indices) = grid(30);
        assert_eq!(
            simplify_primitive(&positions, &indices, 0.3, 0.05),
            simplify_primitive(&positions, &indices, 0.3, 0.05)
        );
    }

    #[test]
    fn a_level_keeps_only_the_vertices_its_triangles_use() {
        let (positions, indices) = grid(20);
        let level = simplify_primitive(&positions, &indices, 0.2, 1.0);
        let used: std::collections::BTreeSet<u32> = level.indices.iter().copied().collect();
        assert_eq!(used.len(), level.source_vertices.len());
        assert!(level.source_vertices.len() < positions.len());
    }

    #[test]
    fn the_vertices_along_a_uv_seam_stay_where_they_are() {
        let (mut positions, mut indices) = grid(20);
        let side = 21u32;
        let seam_column = 10u32;
        let mut seam = Vec::new();
        for row in 0..side {
            let original = row * side + seam_column;
            let copy = positions.len() as u32;
            positions.push(positions[original as usize]);
            seam.push((original, copy));
        }
        for triangle in indices.as_chunks_mut::<3>().0 {
            let right_of_seam = triangle.iter().all(|index| index % side >= seam_column);
            if right_of_seam {
                for index in triangle.iter_mut() {
                    if *index % side == seam_column && (*index as usize) < (side * side) as usize {
                        *index = seam[(*index / side) as usize].1;
                    }
                }
            }
        }
        let level = simplify_primitive(&positions, &indices, 0.25, 1.0);
        for (original, copy) in &seam {
            for vertex in [original, copy] {
                assert!(
                    level.source_vertices.contains(vertex),
                    "the seam vertex {vertex} went"
                );
            }
        }
    }

    #[test]
    fn cached_levels_read_back_as_written() {
        let (positions, indices) = grid(8);
        let primitives = vec![(
            "Mesh0/Primitive0".to_string(),
            simplify_primitive(&positions, &indices, 0.5, 0.1),
        )];
        assert_eq!(decode(&encode(&primitives)), Some(primitives));
    }
}
