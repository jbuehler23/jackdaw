//! Terrain state the core reads without the terrain editor: which terrain tool
//! is in use, the ground height under a point, the plain terrain node a scene
//! adds, and the names other areas refer to the terrain editor's windows by.

use bevy::prelude::*;

pub use jackdaw_api_internal::terrain::TerrainEditMode;
pub use jackdaw_terrain::store::TerrainDataStore;

/// Catalog name of the terrain editor extension.
pub const TERRAIN_EDITOR_EXTENSION_ID: &str = "jackdaw.terrain";

/// The Terrain window in the right sidebar.
pub const TERRAIN_WINDOW_ID: &str = "jackdaw.inspector.terrain";

/// Terrains as a ground query reads them.
pub type TerrainSurfaces<'w, 's> = Query<
    'w,
    's,
    (
        &'static jackdaw_scene_types::Terrain,
        &'static GlobalTransform,
    ),
>;

/// Height of the terrain surface under a world-space XZ point, or `None`
/// when no terrain covers it.
///
/// The point is read in each terrain's own space and the height answered back
/// in world space, so a terrain that stands somewhere other than the origin,
/// or at a scale of its own, reads correctly. The highest surface wins, so a
/// point over two overlapping terrains lands on the one a falling object would
/// meet first.
pub fn height_under(
    terrains: &TerrainSurfaces,
    store: &TerrainDataStore,
    point: Vec2,
) -> Option<f32> {
    terrains
        .iter()
        .filter_map(|(terrain, transform)| height_on(terrain, transform, store, point))
        .max_by(f32::total_cmp)
}

/// Height of one terrain's surface under a world-space XZ point, or `None`
/// when that terrain does not cover it.
pub fn height_on(
    terrain: &jackdaw_scene_types::Terrain,
    transform: &GlobalTransform,
    store: &TerrainDataStore,
    point: Vec2,
) -> Option<f32> {
    let to_local = transform.affine().inverse();
    let local = to_local.transform_point3(Vec3::new(point.x, 0.0, point.y));
    let height = store.heightmap(terrain).map.height_at_local(local.xz())?;
    Some(
        transform
            .transform_point(Vec3::new(local.x, height, local.z))
            .y,
    )
}

/// Spawns a terrain that declares no extent: it starts with no ground, and how
/// far it reaches is whatever its regions come to hold. The component carries
/// only cell spacing, one world unit, which generating a fresh footprint
/// assumes.
pub fn spawn_terrain_node(commands: &mut Commands) -> Entity {
    commands
        .spawn((
            Name::new("Terrain"),
            Transform::default(),
            Visibility::default(),
            jackdaw_scene_types::Terrain {
                cell_size: 1.0,
                ..default()
            },
        ))
        .id()
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TerrainEditMode>();
}
