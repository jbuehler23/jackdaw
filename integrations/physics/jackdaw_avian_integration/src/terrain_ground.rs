//! A heightfield collider under every terrain a scene loads, built from the
//! heights the terrain is drawn from, so the two cannot disagree about where
//! the ground is.

use avian3d::prelude::*;
use bevy::prelude::*;
use jackdaw_runtime::TerrainHeights;
use jackdaw_terrain::Heightmap;

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Update, build_ground_colliders);
}

/// Marks the entity carrying one terrain's ground collider, and names the
/// terrain it stands under.
///
/// A child rather than the terrain itself, because the collider has to be
/// offset: parry's heightfield is centred on its own origin while a terrain's
/// cells start at its grid anchor. The child's `Transform` holds the
/// difference and follows the terrain.
#[derive(Component)]
struct TerrainGround(Entity);

/// The collider that stands under a terrain's drawn ground, and where it
/// sits in the terrain's own space.
///
/// Parry's heightfield is a unit grid stretched by a scale vector, which
/// requires:
///
/// - an XZ scale of the grid's whole span, `(resolution - 1) * cell`, which is
///   the heightmap's `size`;
/// - a Y scale of `1`, because the stored heights are already metres, the same
///   numbers the mesher writes into vertex positions;
/// - and a grid centred on the shape's origin, which is not where the cells
///   are. Cells start at the grid anchor: the terrain's own origin for a
///   terrain authored against this format, and `-size/2` for one migrated from
///   a declared rectangle. The returned offset is that difference, and applying
///   it puts grid vertex `(x, z)` at `anchor + (x, z) * cell`, where the mesher
///   puts it.
///
/// Avian takes the heights as rows of a square array and hands them to parry
/// column-major, leaving the outer index advancing X and the inner one Z.
///
/// `None` for a terrain with no cells: parry needs two rows and two columns,
/// and a terrain holding no regions is the unsculpted case rather than a fault.
fn ground_collider(heightmap: &Heightmap) -> Option<(Collider, Vec3)> {
    let resolution = heightmap.resolution;
    if resolution < 2 {
        return None;
    }
    let heights: Vec<Vec<f32>> = (0..resolution)
        .map(|x| {
            (0..resolution)
                .map(|z| heightmap.get_height(x, z))
                .collect()
        })
        .collect();
    let centre = heightmap.origin + heightmap.size / 2.0;
    Some((
        Collider::heightfield(heights, Vec3::new(heightmap.size.x, 1.0, heightmap.size.y)),
        Vec3::new(centre.x, 0.0, centre.y),
    ))
}

/// Put ground under every terrain whose heights arrived, and take it away from
/// one left with no cells.
fn build_ground_colliders(
    mut commands: Commands,
    terrains: Query<(Entity, &TerrainHeights), Changed<TerrainHeights>>,
    grounds: Query<(Entity, &TerrainGround)>,
) {
    for (terrain, heights) in &terrains {
        let standing = grounds
            .iter()
            .find(|(_, ground)| ground.0 == terrain)
            .map(|(entity, _)| entity);
        match ground_collider(&heights.0) {
            Some((collider, offset)) => {
                // Avian only simulates a collider that some rigid body owns,
                // and the ground belongs to no other.
                let ground = (
                    collider,
                    RigidBody::Static,
                    Transform::from_translation(offset),
                );
                match standing {
                    Some(entity) => {
                        commands.entity(entity).insert(ground);
                    }
                    None => {
                        commands.spawn((TerrainGround(terrain), ground, ChildOf(terrain)));
                    }
                }
            }
            None => {
                if let Some(entity) = standing {
                    commands.entity(entity).despawn();
                }
            }
        }
    }
}
