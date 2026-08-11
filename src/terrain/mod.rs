pub mod channel_ops;
pub mod export;
pub mod inspector;
pub mod mesh;
pub mod ops;
pub mod paint;
pub mod quantize_ops;
pub mod scatter;
pub mod sculpt;
pub mod store;
pub mod toolbar;

use std::collections::HashSet;

use bevy::prelude::*;

pub use paint::TerrainPaintState;
pub use store::TerrainDataStore;
pub use toolbar::TerrainToolbar;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        // Picker category lives on the `Terrain` struct via
        // `#[reflect(@EditorCategory("Terrain"))]`.
        app.init_resource::<TerrainEditMode>()
            .init_resource::<TerrainBrushSettings>()
            .init_resource::<TerrainSculptState>()
            .init_resource::<TerrainDataStore>()
            .add_systems(
                Update,
                (ensure_terrain_dirty_chunks, ensure_terrain_data_path)
                    .chain()
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_plugins((
                mesh::plugin,
                sculpt::plugin,
                paint::plugin,
                scatter::plugin,
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
    terrains: Query<
        Entity,
        (
            With<jackdaw_scene_types::Terrain>,
            Without<TerrainDirtyChunks>,
        ),
    >,
) {
    for entity in &terrains {
        commands.entity(entity).insert(TerrainDirtyChunks {
            rebuild_all: true,
            ..default()
        });
    }
}

/// Give every terrain a sidecar path and a store entry.
///
/// One system covers every way a terrain can appear -- the add-entity
/// operator, a paste, a duplicate, a legacy scene with no `data_path` --
/// so no spawn site has to remember to do it. Terrains that already carry
/// a path are left alone, which is what makes this cheap enough to run
/// every frame.
///
/// A duplicated terrain arrives carrying the original's `data_path`. That
/// would silently alias two terrains onto one heightmap, so the copy is
/// re-keyed here and the original's data is cloned into the new key.
fn ensure_terrain_data_path(world: &mut World) {
    let mut query = world.query::<(Entity, &jackdaw_scene_types::Terrain)>();
    let mut seen: HashSet<String> = HashSet::new();
    let mut needs_path: Vec<(Entity, Option<String>)> = Vec::new();
    for (entity, terrain) in query.iter(world) {
        if terrain.data_path.is_empty() {
            needs_path.push((entity, None));
        } else if !seen.insert(terrain.data_path.clone()) {
            needs_path.push((entity, Some(terrain.data_path.clone())));
        }
    }
    if needs_path.is_empty() {
        return;
    }

    let stem = store::active_scene_stem(world);
    for (entity, aliased) in needs_path {
        let minted = {
            let store = world.resource::<TerrainDataStore>();
            store::mint_data_path(store, &stem)
        };
        // A duplicate starts from the original's data rather than flat
        // ground: the user copied a shape, not an empty terrain.
        let cloned =
            aliased.and_then(|from| world.resource::<TerrainDataStore>().get(&from).cloned());
        {
            let mut store = world.resource_mut::<TerrainDataStore>();
            match cloned {
                Some(data) => store.insert(minted.clone(), data),
                None => {
                    if !store.contains(&minted) {
                        store.insert(minted.clone(), jackdaw_terrain::TerrainData::default());
                    }
                }
            }
        }
        let Some(mut terrain) = world.get_mut::<jackdaw_scene_types::Terrain>(entity) else {
            continue;
        };
        terrain.data_path = minted;
        // Drain any legacy inline heights the same pass, so a scene
        // authored before the sidecar existed keeps its shape.
        let legacy = std::mem::take(&mut terrain.heights);
        let terrain = terrain.clone();
        {
            let mut store = world.resource_mut::<TerrainDataStore>();
            if let Some(data) = store.entry_for(&terrain)
                && !legacy.is_empty()
            {
                data.heights = legacy;
                data.normalize();
            }
        }
        // The document is the save-time source of truth, so the emptied
        // heights and the new path have to reach it explicitly.
        crate::commands::sync_component_to_ast(
            world,
            entity,
            "jackdaw_scene_types::types::Terrain",
            &terrain,
        );
        if let Some(mut dirty) = world.get_mut::<TerrainDirtyChunks>(entity) {
            dirty.rebuild_all = true;
        }
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
    /// Stamping palette values into the active paint channel.
    Paint,
    Generate,
}

impl TerrainEditMode {
    /// Whether the brush (sculpt or paint) is the active tool and should
    /// own viewport input: entity click-select, gizmo drag, and
    /// Shift+Scroll grid resize all defer to it. `Generate` and `None`
    /// do not claim the viewport this way.
    ///
    /// One predicate for every mode guard that used to check `Sculpt(_)`
    /// alone: each left `Paint` unguarded, so painting could select
    /// entities out from under the brush, drag the gizmo, or resize the
    /// snap grid mid-stroke.
    pub fn brush_active(&self) -> bool {
        matches!(self, Self::Sculpt(_) | Self::Paint)
    }
}

#[cfg(test)]
mod terrain_edit_mode_tests {
    use super::*;

    /// I4 pinning test, the exact scenario from the review finding: every
    /// mode guard that special-cased `Sculpt(_)` alone left `Paint`
    /// unguarded.
    #[test]
    fn brush_active_covers_both_sculpt_and_paint() {
        assert!(TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Raise).brush_active());
        assert!(TerrainEditMode::Paint.brush_active());
    }

    #[test]
    fn brush_active_excludes_generate_and_none() {
        assert!(!TerrainEditMode::None.brush_active());
        assert!(!TerrainEditMode::Generate.brush_active());
    }
}

/// Whether a brush-modal stroke (`terrain.sculpt`, `terrain.paint`)
/// should end this frame.
///
/// Level state (`pressed`), not a `just_released` edge -- and meant to
/// be checked unconditionally every frame the modal runs, including its
/// first, when `ActiveModalOperator` has not been attached yet. A press
/// and release that land within the same input frame set `just_released`
/// on that very first frame; a modal that only checks the edge once
/// `ActiveModalOperator` exists (i.e. on later frames, gated behind an
/// `else` on the initialization branch) never observes it, and the
/// brush keeps applying every frame with no way to stop.
pub(crate) fn stroke_should_end(mouse: &bevy::input::ButtonInput<MouseButton>) -> bool {
    !mouse.pressed(MouseButton::Left)
}

#[cfg(test)]
mod stroke_should_end_tests {
    use bevy::input::ButtonInput;

    use super::*;

    /// I5 pinning test, the exact scenario from the review finding: a
    /// press and release that both land within one input frame (before
    /// `ButtonInput::clear` runs) must still read as "should end" --
    /// `pressed()` is already false, unlike `just_released()`, which
    /// depends on which frame happens to check it.
    #[test]
    fn a_press_and_release_within_one_frame_still_ends_the_stroke() {
        let mut mouse = ButtonInput::<MouseButton>::default();
        mouse.press(MouseButton::Left);
        mouse.release(MouseButton::Left);

        assert!(mouse.just_released(MouseButton::Left));
        assert!(stroke_should_end(&mouse));
    }

    #[test]
    fn a_held_button_does_not_end_the_stroke() {
        let mut mouse = ButtonInput::<MouseButton>::default();
        mouse.press(MouseButton::Left);
        assert!(!stroke_should_end(&mouse));
    }

    #[test]
    fn a_button_that_was_never_pressed_ends_the_stroke() {
        let mouse = ButtonInput::<MouseButton>::default();
        assert!(stroke_should_end(&mouse));
    }
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
            jackdaw_scene_types::Terrain::default(),
            TerrainDirtyChunks {
                rebuild_all: true,
                ..default()
            },
        ))
        .id()
}
