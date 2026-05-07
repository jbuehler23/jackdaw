pub mod inspector;
pub mod mesh;
pub mod ops;
pub mod sculpt;
pub mod toolbar;

use std::collections::HashSet;

use bevy::prelude::*;

pub use toolbar::TerrainToolbar;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        // Picker category lives on the `Terrain` struct via
        // `#[reflect(@EditorCategory("Terrain"))]`.
        app.init_resource::<TerrainEditMode>()
            .init_resource::<TerrainBrushSettings>()
            .init_resource::<TerrainSculptState>()
            .add_systems(
                Update,
                ensure_terrain_dirty_chunks.run_if(in_state(crate::AppState::Editor)),
            )
            .add_plugins((
                mesh::plugin,
                sculpt::plugin,
                toolbar::plugin,
                inspector::plugin,
            ));
    }
}

/// Ensures every `Terrain` entity has a `TerrainDirtyChunks` component.
/// This handles entities spawned via JSN scene load, where only reflected
/// components are deserialized and runtime-only types like `TerrainDirtyChunks`
/// are missing.
fn ensure_terrain_dirty_chunks(
    mut commands: Commands,
    terrains: Query<Entity, (With<jackdaw_jsn::Terrain>, Without<TerrainDirtyChunks>)>,
) {
    for entity in &terrains {
        commands.entity(entity).insert(TerrainDirtyChunks {
            rebuild_all: true,
            ..default()
        });
    }
}

// --- Components ---

/// Marks a child entity as a terrain chunk mesh. Chunks are
/// rebuilt from the parent terrain's heightmap, so they're always
/// hidden from the outliner and excluded from the saved scene.
#[derive(Component)]
#[require(crate::EditorHidden, crate::NonSerializable)]
pub struct TerrainChunk {
    pub terrain_entity: Entity,
    pub chunk_x: u32,
    pub chunk_z: u32,
}

/// Tracks which chunks need mesh rebuilds.
#[derive(Component, Default)]
pub(crate) struct TerrainDirtyChunks {
    pub(crate) dirty: HashSet<(u32, u32)>,
    pub(crate) rebuild_all: bool,
}

// --- Resources ---

/// Current terrain editing mode.
#[derive(Resource, Default, PartialEq, Eq, Clone, Debug)]
pub enum TerrainEditMode {
    #[default]
    None,
    Sculpt(jackdaw_terrain::SculptTool),
    Generate,
}

/// Brush settings for terrain sculpting.
#[derive(Resource)]
pub struct TerrainBrushSettings {
    pub radius: f32,
    pub strength: f32,
    pub falloff: f32,
}

impl Default for TerrainBrushSettings {
    fn default() -> Self {
        Self {
            radius: 5.0,
            strength: 10.0,
            falloff: 2.0,
        }
    }
}

/// State for an active sculpt stroke.
#[derive(Resource, Default)]
pub(crate) struct TerrainSculptState {
    /// The terrain entity being sculpted.
    pub target: Option<Entity>,
    /// Whether a stroke is currently active (LMB held).
    pub active: bool,
    /// Snapshot of heights at stroke start, for undo.
    pub stroke_snapshot: Vec<f32>,
    /// Current brush position in grid space.
    pub brush_position: Option<Vec2>,
}

// --- Constants ---

/// Number of cells per chunk edge.
pub const CHUNK_SIZE: u32 = 32;

// --- Spawn ---

pub fn spawn_terrain_entity(commands: &mut Commands) -> Entity {
    commands
        .spawn((
            Name::new("Terrain"),
            Transform::default(),
            Visibility::default(),
            jackdaw_jsn::Terrain::default(),
            TerrainDirtyChunks {
                rebuild_all: true,
                ..default()
            },
        ))
        .id()
}
