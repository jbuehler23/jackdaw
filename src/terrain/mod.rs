pub mod autoterrain_ops;
pub mod channel_ops;
pub mod export;
pub mod inspector;
pub mod mesh;
pub mod navmesh_bake;
pub mod ops;
pub mod options_bar;
pub mod paint;
pub mod palette;
pub mod panel;
pub mod quantize_ops;
pub mod regions;
pub mod scatter;
pub mod sculpt;
pub mod shape_ops;
pub mod splat;
pub mod store;
pub mod texture_ops;
pub(crate) mod ui_fields;

use std::collections::HashSet;

use bevy::prelude::*;

pub use options_bar::TerrainOptionsBar;
pub use paint::{PaintDomain, TerrainPaintState};
pub use palette::TerrainPalette;
pub use regions::{RegionVisibility, TerrainRegionView};
pub use store::TerrainDataStore;

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
                (
                    ensure_terrain_dirty_chunks,
                    ensure_terrain_data_path,
                    sync_terrain_bounds,
                    prune_terrain_heightmaps,
                )
                    .chain()
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_plugins((
                mesh::plugin,
                sculpt::plugin,
                paint::plugin,
                scatter::plugin,
                palette::plugin,
                regions::plugin,
                navmesh_bake::plugin,
                splat::plugin,
                options_bar::plugin,
                panel::plugin,
                inspector::plugin,
                ui_fields::plugin,
                texture_ops::plugin,
            ));
    }
}

/// Ensures every `Terrain` entity has a `TerrainDirtyChunks` component.
///
/// Scene load deserializes only reflected components, so runtime-only types like
/// `TerrainDirtyChunks` are missing on loaded entities.
pub fn ensure_terrain_dirty_chunks(
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

/// Gives every terrain a sidecar path and a store entry.
///
/// Terrains that already have a path are skipped. A duplicated terrain arrives
/// carrying the original's `data_path`, which would alias two terrains onto one
/// heightmap, so the copy is re-keyed and the original's data cloned into it.
pub fn ensure_terrain_data_path(world: &mut World) {
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
        // A duplicate starts from the original's data, not flat ground.
        let cloned =
            aliased.and_then(|from| world.resource::<TerrainDataStore>().get(&from).cloned());
        {
            let mut store = world.resource_mut::<TerrainDataStore>();
            match cloned {
                Some(data) => store.insert(minted.clone(), data),
                None => {
                    if !store.contains(&minted) {
                        store.insert(
                            minted.clone(),
                            jackdaw_terrain::RegionTerrainData::default(),
                        );
                    }
                }
            }
        }
        let Some(mut terrain) = world.get_mut::<jackdaw_scene_types::Terrain>(entity) else {
            continue;
        };
        terrain.data_path = minted;
        // Inline heights migrate into the sidecar.
        let legacy = std::mem::take(&mut terrain.heights);
        let terrain = terrain.clone();
        {
            let mut store = world.resource_mut::<TerrainDataStore>();
            if let Some(mut data) = store.entry_for(&terrain)
                && !legacy.is_empty()
            {
                data.set_heights(&legacy);
            }
        }
        // The document is the save-time source of truth, so the emptied
        // heights and the minted path have to reach it explicitly.
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

/// Keeps an `Aabb` on every terrain describing the ground it authors.
///
/// The drawn geometry cannot answer this: the clipmap rings are laid out around
/// the camera and reach as far as it stands, which is why they carry
/// [`crate::ViewDependentBounds`]. The authored extent is the component's own
/// `size` and the range its heights span. Written only when it changes.
fn sync_terrain_bounds(
    mut commands: Commands,
    store: Res<TerrainDataStore>,
    terrains: Query<(
        Entity,
        &jackdaw_scene_types::Terrain,
        Option<&bevy::camera::primitives::Aabb>,
    )>,
) {
    for (entity, terrain, current) in &terrains {
        let (low, high) = store.heightmap(terrain).bounds();
        // The cells a terrain holds are not centred on the entity: a migrated
        // grid starts at its `-size/2`, and any grid grows as regions are
        // allocated. The centre is the middle of that ground, not the origin.
        let shape = store.grid_shape(terrain);
        let middle = shape.origin + shape.size * 0.5;
        let half = Vec3::new(
            shape.size.x * 0.5,
            ((high - low) * 0.5).max(0.0),
            shape.size.y * 0.5,
        );
        let wanted = bevy::camera::primitives::Aabb {
            center: Vec3::new(middle.x, (low + high) * 0.5, middle.y).into(),
            half_extents: half.into(),
        };
        if current != Some(&wanted) {
            commands.entity(entity).insert(wanted);
        }
    }
}

/// Frees the sampling view of every terrain absent from the scene.
///
/// The store keeps a deleted terrain's document, since an undo can bring the
/// terrain back, but its cached heightmap is derived and costs about a megabyte
/// per 512-resolution terrain. Guarded on the counts so a frame where every
/// cached map still has its terrain costs one comparison.
fn prune_terrain_heightmaps(
    store: Res<TerrainDataStore>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
) {
    if store.cached_heightmap_count() <= terrains.iter().count() {
        return;
    }
    store.retain_heightmaps(|path| terrains.iter().any(|terrain| terrain.data_path == path));
}

// --- Components ---

/// Marks a child entity as one LOD level of a terrain's surface.
///
/// Levels are rebuilt from the parent terrain's heightmap as the camera moves,
/// so they are hidden from the outliner and excluded from the saved scene.
#[derive(Component)]
#[require(
    crate::EditorHidden,
    crate::NonSerializable,
    crate::ViewDependentBounds
)]
pub struct TerrainSurface {
    pub terrain_entity: Entity,
    /// 0 is the finest ring.
    pub level: u32,
}

/// Tracks which chunks need mesh rebuilds.
#[derive(Component, Default)]
pub struct TerrainDirtyChunks {
    pub(crate) dirty: HashSet<(u32, u32)>,
    pub(crate) rebuild_all: bool,
}

// --- Resources ---

/// Current terrain editing mode. `None` is no tool active; `Sculpt` carries the
/// specific sculpt tool (Raise, Lower, Flatten, Smooth, Noise).
#[derive(Resource, Default, PartialEq, Eq, Clone, Debug)]
pub enum TerrainEditMode {
    #[default]
    None,
    Sculpt(jackdaw_terrain::SculptTool),
    /// Stamping palette values into the active paint channel.
    Paint,
    /// Showing the grid-quantization options (cell size, height step, on/off,
    /// Apply). Claims no viewport input.
    Quantize,
    /// Choosing which region is active and how the rest of them draw
    /// (`regions.rs`). A click picks a region; the terrain is not edited, so
    /// this claims no brush.
    Regions,
    /// Showing the navmesh bake params, the Bake action and the overlay toggle
    /// (`navmesh_bake.rs`). Claims no viewport input.
    Navmesh,
}

impl TerrainEditMode {
    /// Whether the brush (sculpt or paint) is the active tool and owns viewport
    /// input: entity click-select, gizmo drag, and Shift+Scroll grid resize all
    /// defer to it.
    pub fn brush_active(&self) -> bool {
        matches!(self, Self::Sculpt(_) | Self::Paint)
    }
}

#[cfg(test)]
mod terrain_edit_mode_tests {
    use super::*;

    #[test]
    fn brush_active_covers_both_sculpt_and_paint() {
        assert!(TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Raise).brush_active());
        assert!(TerrainEditMode::Paint.brush_active());
    }

    #[test]
    fn brush_active_excludes_quantize_and_none() {
        assert!(!TerrainEditMode::None.brush_active());
        assert!(!TerrainEditMode::Quantize.brush_active());
    }
}

/// Whether a brush-modal stroke (`terrain.sculpt`, `terrain.paint`)
/// should end this frame.
///
/// Level state (`pressed`), not a `just_released` edge: a press and release
/// landing within one input frame set `just_released` on that frame, before
/// `ActiveModalOperator` exists to check it, so the edge would be missed and
/// the brush would never stop.
pub(crate) fn stroke_should_end(mouse: &bevy::input::ButtonInput<MouseButton>) -> bool {
    !mouse.pressed(MouseButton::Left)
}

#[cfg(test)]
mod stroke_should_end_tests {
    use bevy::input::ButtonInput;

    use super::*;

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
pub struct TerrainSculptState {
    /// The terrain entity being sculpted.
    pub target: Option<Entity>,
    /// Whether a stroke is currently active (LMB held).
    pub active: bool,
    /// Snapshot of every height at stroke start, held only while the button is
    /// down. The history entry the stroke leaves behind keeps just the cells in
    /// [`Self::stroke_rect`].
    pub stroke_snapshot: Vec<f32>,
    /// Every cell this stroke has brushed, grown frame by frame.
    pub stroke_rect: Option<jackdaw_terrain::GridRect>,
    /// Current brush position in grid space.
    pub brush_position: Option<Vec2>,
}

// --- Constants ---

/// Number of cells per chunk edge.
///
/// Chunks record where an edit landed: the brush marks the ones it touched and
/// the surface rebuilds the LOD levels over them. Nothing is drawn per chunk.
pub const CHUNK_SIZE: u32 = 32;

/// The material slot an unpainted cell draws:
/// `jackdaw_terrain::Control::default().base_id()`. Painting sets a cell's base
/// id, and Ctrl blends from this coat toward the active texture, so the brush
/// never has to lay this slot down.
pub const BASE_TEXTURE_SLOT: usize = 0;

/// Segments the brush preview ring is drawn with.
pub const BRUSH_RING_SEGMENTS: usize = 32;

/// Terrain-local points tracing a brush ring of `radius` cells around
/// `grid_pos`, one per segment boundary and closing back on the first.
///
/// Read from the map's own origin rather than assuming a grid centred on the
/// entity: cells sit where the terrain's regions are.
pub fn brush_ring_points(
    map: &jackdaw_terrain::Heightmap,
    grid_pos: Vec2,
    radius: f32,
) -> Vec<Vec3> {
    let cell = map.cell_size();
    let at = map.origin;
    (0..=BRUSH_RING_SEGMENTS)
        .map(|i| {
            let angle = (i as f32 / BRUSH_RING_SEGMENTS as f32) * std::f32::consts::TAU;
            let gx = grid_pos.x + angle.cos() * radius;
            let gz = grid_pos.y + angle.sin() * radius;
            Vec3::new(
                gx * cell.x + at.x,
                map.sample_bilinear(gx, gz) + 0.1,
                gz * cell.y + at.y,
            )
        })
        .collect()
}

/// Puts a terrain notice on screen as well as in the log.
///
/// Carries the cases where the editor declines to do, or silently changes, what
/// the user asked for: a stroke refused at the region cap, and a migrated
/// terrain whose non-square rectangle is respaced onto one square cell. The log
/// keeps the text after the toast expires.
///
/// A no-op without the editor's fonts, which is the headless case tests run in.
pub fn toast_terrain_notice(world: &mut World, message: &str) {
    let (Some(editor_font), Some(icon_font)) = (
        world
            .get_resource::<jackdaw_feathers::icons::EditorFont>()
            .map(|f| f.0.clone()),
        world
            .get_resource::<jackdaw_feathers::icons::IconFont>()
            .map(|f| f.0.clone()),
    ) else {
        return;
    };
    world.spawn(jackdaw_feathers::toast::toast(
        jackdaw_feathers::toast::ToastVariant::Error,
        message,
        jackdaw_feathers::toast::DEFAULT_TOAST_DURATION,
        &editor_font,
        &icon_font,
    ));
}

// --- Spawn ---

/// Spawns a terrain that declares no extent: it starts with no ground, and how
/// far it reaches is whatever its regions come to hold. The component carries
/// only cell spacing, one world unit, which generating a fresh footprint
/// assumes.
pub fn spawn_terrain_entity(commands: &mut Commands) -> Entity {
    commands
        .spawn((
            Name::new("Terrain"),
            Transform::default(),
            Visibility::default(),
            jackdaw_scene_types::Terrain {
                cell_size: 1.0,
                ..default()
            },
            TerrainDirtyChunks {
                rebuild_all: true,
                ..default()
            },
        ))
        .id()
}

#[cfg(test)]
mod brush_ring_tests {
    use super::*;

    #[test]
    fn the_ring_rides_the_grid_where_it_actually_sits() {
        let at = Vec2::new(400.0, -700.0);
        let map = jackdaw_terrain::Heightmap::new_at(65, Vec2::splat(128.0), 32.0, at);
        let centre = Vec2::new(32.0, 32.0);

        let ring = brush_ring_points(&map, centre, 4.0);

        assert_eq!(ring.len(), BRUSH_RING_SEGMENTS + 1);
        let cell = map.cell_size();
        for point in &ring {
            let expected_centre =
                Vec3::new(centre.x * cell.x + at.x, point.y, centre.y * cell.y + at.y);
            let offset = *point - expected_centre;
            assert!(
                (offset.length() - 4.0 * cell.x).abs() < 1.0e-3,
                "ring point {point:?} sits {} from the brush centre, not one radius",
                offset.length(),
            );
        }
    }

    #[test]
    fn the_ring_closes_on_itself() {
        let map = jackdaw_terrain::Heightmap::new(65, Vec2::splat(64.0), 8.0);
        let ring = brush_ring_points(&map, Vec2::splat(10.0), 3.0);
        let first = ring.first().expect("a ring has points");
        let last = ring.last().expect("a ring has points");
        assert!((*first - *last).length() < 1.0e-4);
    }
}

#[cfg(test)]
mod new_terrain_tests {
    use super::*;

    /// A BSN scene falls back to `Default` for an elided field, and the extent
    /// fields are read by the load-time migration, so changing either default
    /// moves the ground under a migrated scene.
    #[test]
    fn the_components_default_stays_the_shape_older_scenes_elided() {
        let terrain = jackdaw_scene_types::Terrain::default();
        assert_eq!(terrain.resolution, 256);
        assert_eq!(terrain.size, Vec2::new(100.0, 100.0));
    }

    #[test]
    fn a_new_terrain_declares_spacing_not_extent() {
        let mut world = World::new();
        let mut commands_queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut commands_queue, &world);
        let spawned = spawn_terrain_entity(&mut commands);
        commands_queue.apply(&mut world);

        let terrain = world
            .get::<jackdaw_scene_types::Terrain>(spawned)
            .expect("the operator spawns a Terrain");
        let defaults = jackdaw_scene_types::Terrain::default();
        assert_eq!(terrain.cell_size, 1.0);
        assert_eq!(terrain.resolution, defaults.resolution);
        assert_eq!(terrain.size, defaults.size);
    }
}

/// A world where a pointer press over the viewport can be simulated: one
/// selected terrain, a camera aimed straight down at the middle of it, a
/// viewport UI node under the cursor, and a hover map the test points wherever
/// the press should land. Shared by the tool modules, which all read the cursor
/// through the same helper.
#[cfg(test)]
pub(crate) mod pointer_harness {
    use bevy::picking::{backend::HitData, hover::HoverMap, pointer::PointerId};
    use bevy::prelude::*;
    use bevy::ui::widget::ViewportNode;
    use bevy::ui::{ComputedNode, UiGlobalTransform};

    use super::{TerrainDataStore, TerrainDirtyChunks};
    use crate::selection::Selection;
    use crate::viewport::{ActiveViewport, MainViewportCamera, SceneViewport};

    const VIEWPORT_SIZE: Vec2 = Vec2::new(800.0, 600.0);

    /// The terrain the harness lays down and selects.
    pub(crate) fn terrain_of(resolution: u32) -> jackdaw_scene_types::Terrain {
        jackdaw_scene_types::Terrain {
            resolution,
            size: Vec2::splat((resolution - 1) as f32),
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        }
    }

    /// Builds the world. The hover map starts on the viewport's own node, which
    /// is what a press on open ground looks like; call [`hover`] to put an
    /// overlay under the cursor instead.
    pub(crate) fn app(resolution: u32) -> App {
        let mut app = App::new();
        app.init_resource::<TerrainDataStore>()
            .init_resource::<Selection>()
            .init_resource::<ActiveViewport>()
            .init_resource::<UiScale>()
            .init_resource::<HoverMap>()
            .init_resource::<ButtonInput<MouseButton>>();

        let terrain = terrain_of(resolution);
        let mut regions = jackdaw_terrain::TerrainRegions::new(
            jackdaw_terrain::RegionSize::new(resolution).expect("a power of two"),
        );
        regions
            .ensure_grid(resolution)
            .expect("inside the region cap");
        app.world_mut().resource_mut::<TerrainDataStore>().insert(
            terrain.data_path.clone(),
            jackdaw_terrain::RegionTerrainData {
                regions,
                ..default()
            },
        );

        // The middle of the ground the store laid down, in the terrain's own
        // space; the terrain entity sits at the origin.
        let middle = {
            let store = app.world().resource::<TerrainDataStore>();
            let heightmap = store.heightmap(&terrain);
            heightmap.map.origin + heightmap.map.size / 2.0
        };

        let terrain_entity = app
            .world_mut()
            .spawn((
                terrain,
                TerrainDirtyChunks::default(),
                GlobalTransform::IDENTITY,
            ))
            .id();
        app.world_mut()
            .resource_mut::<Selection>()
            .entities
            .push(terrain_entity);

        let mut camera = Camera::default();
        camera.computed.clip_from_view =
            bevy::camera::CameraProjection::get_clip_from_view(&PerspectiveProjection {
                aspect_ratio: VIEWPORT_SIZE.x / VIEWPORT_SIZE.y,
                ..default()
            });
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: VIEWPORT_SIZE.as_uvec2(),
            scale_factor: 1.0,
        });
        let camera_entity = app
            .world_mut()
            .spawn((
                camera,
                GlobalTransform::from(
                    Transform::from_xyz(middle.x, 50.0, middle.y)
                        .looking_at(Vec3::new(middle.x, 0.0, middle.y), Vec3::Z),
                ),
                MainViewportCamera,
            ))
            .id();

        let viewport_entity = app
            .world_mut()
            .spawn((
                ComputedNode {
                    size: VIEWPORT_SIZE,
                    inverse_scale_factor: 1.0,
                    ..default()
                },
                // A UI node's transform sits at its centre.
                UiGlobalTransform::from(bevy::math::Affine2::from_translation(VIEWPORT_SIZE / 2.0)),
                SceneViewport,
                ViewportNode::new(camera_entity),
            ))
            .id();

        let mut window = Window::default();
        window.set_physical_cursor_position(Some(
            bevy::math::DVec2::new(f64::from(VIEWPORT_SIZE.x), f64::from(VIEWPORT_SIZE.y)) / 2.0,
        ));
        app.world_mut().spawn(window);

        *app.world_mut().resource_mut::<ActiveViewport>() = ActiveViewport {
            camera: Some(camera_entity),
            ui_node: Some(viewport_entity),
        };
        hover(&mut app, viewport_entity);
        app
    }

    /// Puts `entity` under the cursor, the way the picking pass would.
    pub(crate) fn hover(app: &mut App, entity: Entity) {
        let hit = HitData {
            camera: Entity::PLACEHOLDER,
            depth: 0.0,
            position: None,
            normal: None,
            extra: None,
        };
        let mut hits = bevy::ecs::entity::EntityHashMap::default();
        hits.insert(entity, hit);
        let mut map = HoverMap::default();
        map.insert(PointerId::Mouse, hits);
        *app.world_mut().resource_mut::<HoverMap>() = map;
    }

    /// Adds a second viewport panel and moves the pointer into it, the way
    /// `update_active_viewport` retargets when the cursor crosses from one
    /// panel of a split view into the other. Returns its UI node.
    pub(crate) fn hover_second_viewport(app: &mut App) -> Entity {
        let first_camera = app
            .world()
            .resource::<ActiveViewport>()
            .camera
            .expect("the harness aims a camera at the terrain");
        let (camera, camera_tf) = {
            let entity = app.world().entity(first_camera);
            (
                entity.get::<Camera>().expect("a camera").clone(),
                *entity.get::<GlobalTransform>().expect("a transform"),
            )
        };
        let camera_entity = app
            .world_mut()
            .spawn((camera, camera_tf, MainViewportCamera))
            .id();
        let ui_node = app
            .world_mut()
            .spawn((
                ComputedNode {
                    size: VIEWPORT_SIZE,
                    inverse_scale_factor: 1.0,
                    ..default()
                },
                UiGlobalTransform::from(bevy::math::Affine2::from_translation(VIEWPORT_SIZE / 2.0)),
                SceneViewport,
                ViewportNode::new(camera_entity),
            ))
            .id();
        *app.world_mut().resource_mut::<ActiveViewport>() = ActiveViewport {
            camera: Some(camera_entity),
            ui_node: Some(ui_node),
        };
        hover(app, ui_node);
        ui_node
    }

    /// Puts a fresh overlay node, such as a tool-palette button, under the
    /// cursor inside the viewport's rectangle.
    pub(crate) fn hover_overlay(app: &mut App) {
        let overlay = app.world_mut().spawn_empty().id();
        hover(app, overlay);
    }
}
