use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_terrain::GridRect;

use super::{
    CHUNK_SIZE, TerrainBrushSettings, TerrainDataStore, TerrainDirtyChunks, TerrainEditMode,
    TerrainSculptState,
};
use crate::commands::{CommandHistory, EditorCommand};
use crate::default_style;
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            update_terrain_brush_position,
            sculpt_invoke_trigger,
            handle_brush_resize_scroll,
            draw_terrain_brush_gizmo,
        )
            .chain()
            .run_if(in_state(crate::AppState::Editor)),
    );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainSculptOp>();
}

/// The cells a [`SetTerrainHeights`] entry restores, before and after.
///
/// A stroke brushes a disc a few dozen cells across and keeps that
/// rectangle, so a hillside's worth of strokes costs the brush footprint
/// each rather than the whole terrain each. Generation, erosion and a
/// quantize-all rewrite every cell and have nothing smaller to keep.
enum HeightPatch {
    /// Every cell of the terrain.
    Whole { old: Vec<f32>, new: Vec<f32> },
    /// One rectangle of it, row-major, as [`GridRect::read`] produces.
    Rect {
        rect: GridRect,
        old: Vec<f32>,
        new: Vec<f32>,
    },
}

impl HeightPatch {
    fn heap_bytes(&self) -> usize {
        let (old, new) = match self {
            Self::Whole { old, new } | Self::Rect { old, new, .. } => (old, new),
        };
        (old.capacity() + new.capacity()) * size_of::<f32>()
    }
}

/// Undo command for terrain height changes.
///
/// Heights are written to [`TerrainDataStore`], not to the component and
/// not to the scene document, which would push 262,144 floats through the
/// BSN AST on every stroke.
pub struct SetTerrainHeights {
    pub entity: Entity,
    patch: HeightPatch,
    pub label: String,
}

impl SetTerrainHeights {
    /// An entry covering every cell: generation, erosion, a quantize-all.
    pub fn whole(
        entity: Entity,
        old_heights: Vec<f32>,
        new_heights: Vec<f32>,
        label: String,
    ) -> Self {
        Self {
            entity,
            patch: HeightPatch::Whole {
                old: old_heights,
                new: new_heights,
            },
            label,
        }
    }

    /// An entry covering only the cells one stroke brushed.
    ///
    /// `old` and `new` are that rectangle read out of the terrain before
    /// and after the stroke ([`GridRect::read`]).
    pub fn stroke(
        entity: Entity,
        rect: GridRect,
        old: Vec<f32>,
        new: Vec<f32>,
        label: String,
    ) -> Self {
        Self {
            entity,
            patch: HeightPatch::Rect { rect, old, new },
            label,
        }
    }

    fn restore(&self, world: &mut World, old: bool) {
        let Some(terrain) = world.get::<jackdaw_scene_types::Terrain>(self.entity) else {
            return;
        };
        let terrain = terrain.clone();
        // Both arms go through `entry_for`, which retires the shared
        // heightmap, so the brush target and the ring gizmo read the
        // heights this entry left.
        if let Some(mut data) = world.resource_mut::<TerrainDataStore>().entry_for(&terrain) {
            match &self.patch {
                HeightPatch::Whole { old: o, new: n } => {
                    data.set_heights(if old { o } else { n });
                }
                HeightPatch::Rect {
                    rect,
                    old: o,
                    new: n,
                } => {
                    data.set_heights_rect(*rect, if old { o } else { n });
                }
            }
        }
        if let Some(mut dirty) = world.get_mut::<TerrainDirtyChunks>(self.entity) {
            dirty.rebuild_all = true;
        }
    }
}

impl EditorCommand for SetTerrainHeights {
    fn execute(&mut self, world: &mut World) {
        self.restore(world, false);
    }

    fn undo(&mut self, world: &mut World) {
        self.restore(world, true);
    }

    fn description(&self) -> &str {
        &self.label
    }

    fn heap_bytes(&self) -> usize {
        self.patch.heap_bytes()
    }
}

/// Raycast the cursor against the selected terrain's sculpted surface
/// and return the (entity, grid coordinate) the brush should target.
///
/// The one entry point for the ring preview, paint, sculpt and region
/// picking, so the ring and the cells a stroke writes agree about where the
/// cursor is.
///
/// Reads the cursor through
/// [`crate::viewport::ViewportCursor::viewport_pointer`], so a press on the
/// tool palette floating over the viewport targets no terrain: it starts no
/// stroke and picks no region. A running stroke stalls while the pointer
/// crosses that UI and resumes once it is back over open viewport, as it
/// does when the pointer wanders past the window edge.
pub(super) fn terrain_brush_hit(
    vp: &crate::viewport::ViewportCursor,
    terrain_query: &Query<(Entity, &jackdaw_scene_types::Terrain, &GlobalTransform)>,
    selection: &Selection,
    store: &TerrainDataStore,
) -> Option<(Entity, Vec2)> {
    let selected = selection.primary()?;
    let (terrain_entity, terrain, terrain_tf) = terrain_query.get(selected).ok()?;

    let cursor_pos = vp.viewport_pointer()?;

    let camera_entity = vp.camera_entity()?;
    let viewport_entity = vp.viewport_entity()?;
    let (camera, cam_tf) = vp.camera_for(camera_entity)?;
    let local_cursor = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos)?;
    let ray = camera.viewport_to_world(cam_tf, local_cursor).ok()?;

    let heightmap = store.heightmap(terrain);
    let origin = ray.origin - terrain_tf.translation();
    let hit = heightmap
        .map
        .raycast_within(origin, *ray.direction, heightmap.bounds())?;

    Some((terrain_entity, hit.grid))
}

/// Track the brush-target grid position so the overlay gizmo follows
/// the cursor even when no stroke is in progress.
fn update_terrain_brush_position(
    edit_mode: Res<TerrainEditMode>,
    vp: crate::viewport::ViewportCursor,
    terrain_query: Query<(Entity, &jackdaw_scene_types::Terrain, &GlobalTransform)>,
    selection: Res<Selection>,
    store: Res<TerrainDataStore>,
    mut sculpt_state: ResMut<TerrainSculptState>,
) {
    if !matches!(*edit_mode, TerrainEditMode::Sculpt(_)) {
        if sculpt_state.brush_position.is_some() || sculpt_state.target.is_some() {
            sculpt_state.brush_position = None;
            sculpt_state.target = None;
        }
        return;
    }
    match terrain_brush_hit(&vp, &terrain_query, &selection, &store) {
        Some((entity, grid)) => {
            sculpt_state.target = Some(entity);
            sculpt_state.brush_position = Some(grid);
        }
        None => sculpt_state.brush_position = None,
    }
}

/// LMB in sculpt mode, with the brush over the terrain, dispatches
/// `terrain.sculpt`. Mouse-button gestures are not expressible as BEI key
/// bindings.
fn sculpt_invoke_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<TerrainEditMode>,
    sculpt_state: Res<TerrainSculptState>,
    mut commands: Commands,
) {
    if sculpt_state.active
        || !mouse.just_pressed(MouseButton::Left)
        || !matches!(*edit_mode, TerrainEditMode::Sculpt(_))
        || sculpt_state.brush_position.is_none()
        || sculpt_state.target.is_none()
    {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(TerrainSculptOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: false,
            })
            .call();
    });
}

#[operator(
    id = "terrain.sculpt",
    label = "Sculpt Terrain",
    description = "Sculpt the terrain under the brush while the mouse button is held.",
    modal = true,
    allows_undo = false,
    cancel = cancel_terrain_sculpt,
)]
pub fn terrain_sculpt(
    _: In<OperatorParameters>,
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<TerrainEditMode>,
    brush_settings: Res<TerrainBrushSettings>,
    mut sculpt_state: ResMut<TerrainSculptState>,
    mut terrain_query: Query<(&jackdaw_scene_types::Terrain, &mut TerrainDirtyChunks)>,
    mut store: ResMut<TerrainDataStore>,
    mut history: ResMut<CommandHistory>,
    time: Res<Time>,
    active: ActiveModalQuery,
) -> OperatorResult {
    let TerrainEditMode::Sculpt(tool) = *edit_mode else {
        return OperatorResult::Cancelled;
    };
    let target = sculpt_state.target?;
    let (terrain, mut dirty) = terrain_query.get_mut(target)?;
    // The stroke lands on the cells the terrain holds, so the brush reaches
    // wherever ground has been allocated.
    let resolution = store.grid_shape(terrain).resolution;
    let radius = brush_settings.radius;

    if !active.is_modal_running() {
        // The only whole-terrain read a stroke does, and where a terrain
        // the store refuses turns the stroke away before it starts.
        sculpt_state.stroke_snapshot = store.entry_for(terrain)?.heights().to_vec();
        sculpt_state.active = true;
        sculpt_state.stroke_rect = None;
    }

    // See `super::stroke_should_end`: checked every frame, including the
    // modal's first.
    if super::stroke_should_end(&mouse) {
        sculpt_state.active = false;
        let before = std::mem::take(&mut sculpt_state.stroke_snapshot);
        // A stroke pressed and released off the terrain changed nothing,
        // so it leaves no entry.
        if let Some(rect) = sculpt_state.stroke_rect.take() {
            let after = rect.read(&store.heights(&terrain.data_path), resolution);
            history.push_executed(Box::new(SetTerrainHeights::stroke(
                target,
                rect,
                rect.read(&before, resolution),
                after,
                format!("Terrain {tool:?}"),
            )));
        }
        return OperatorResult::Finished;
    }

    if let Some(grid_pos) = sculpt_state.brush_position
        // The cells this frame writes, and what the stroke's history entry
        // grows from, so the entry names every cell the brush touched.
        && let Some(rect) = GridRect::brush(resolution, grid_pos, radius)
    {
        let step = terrain.quantization.active_height_step();
        let strength = brush_settings.strength;
        let falloff = brush_settings.falloff;
        let dt = time.delta_secs();
        let wrote = store.brush_heights(terrain, rect, |heights| {
            jackdaw_terrain::apply_brush_at(
                heights, resolution, tool, grid_pos, radius, strength, falloff, dt, None,
            );
            // Snap inside the stroke rather than on release, so the
            // terraces form while dragging. Only the cells the brush
            // touched are rewritten.
            if let Some(step) = step {
                jackdaw_terrain::quantize_region(heights, resolution, grid_pos, radius, step);
            }
        });
        if wrote {
            for chunk in
                jackdaw_terrain::affected_chunks_at(resolution, grid_pos, radius, CHUNK_SIZE)
            {
                dirty.dirty.insert(chunk);
            }
            sculpt_state.stroke_rect = Some(match sculpt_state.stroke_rect {
                Some(grown) => grown.union(rect),
                None => rect,
            });
        }
    }
    OperatorResult::Running
}

fn cancel_terrain_sculpt(
    mut sculpt_state: ResMut<TerrainSculptState>,
    mut terrain_query: Query<(&jackdaw_scene_types::Terrain, &mut TerrainDirtyChunks)>,
    mut store: ResMut<TerrainDataStore>,
) {
    if !sculpt_state.active {
        return;
    }
    sculpt_state.active = false;
    let snapshot = std::mem::take(&mut sculpt_state.stroke_snapshot);
    if let Some(target) = sculpt_state.target
        && let Ok((terrain, mut dirty)) = terrain_query.get_mut(target)
        && let Some(mut data) = store.entry_for(terrain)
    {
        data.set_heights(&snapshot);
        dirty.rebuild_all = true;
    }
}

fn handle_brush_resize_scroll(
    keyboard: Res<ButtonInput<KeyCode>>,
    nav_scroll: crate::modal_inputs::NavScrollInputs,
    edit_mode: Res<TerrainEditMode>,
    mut brush_settings: ResMut<TerrainBrushSettings>,
) {
    if !matches!(*edit_mode, TerrainEditMode::Sculpt(_)) {
        return;
    }

    // Resize fires only while Shift is held; the camera system skips zoom
    // when Shift is down, so resize and zoom stay mutually exclusive.
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if !shift {
        return;
    }

    if nav_scroll.resize_up() {
        brush_settings.radius = f32::min(brush_settings.radius * 1.15, 50.0);
    } else if nav_scroll.resize_down() {
        brush_settings.radius = f32::max(brush_settings.radius * 0.87, 1.0);
    }
}

fn draw_terrain_brush_gizmo(
    sculpt_state: Res<TerrainSculptState>,
    brush_settings: Res<TerrainBrushSettings>,
    edit_mode: Res<TerrainEditMode>,
    terrains: Query<(&jackdaw_scene_types::Terrain, &GlobalTransform)>,
    store: Res<TerrainDataStore>,
    mut gizmos: Gizmos,
) {
    if !matches!(*edit_mode, TerrainEditMode::Sculpt(_)) {
        return;
    }

    let Some(target) = sculpt_state.target else {
        return;
    };
    let Some(grid_pos) = sculpt_state.brush_position else {
        return;
    };

    let Ok((terrain, terrain_tf)) = terrains.get(target) else {
        return;
    };

    let heightmap = store.heightmap(terrain);
    let origin = terrain_tf.translation();
    let ring = super::brush_ring_points(&heightmap.map, grid_pos, brush_settings.radius);

    for pair in ring.windows(2) {
        gizmos.line(
            origin + pair[0],
            origin + pair[1],
            default_style::TERRAIN_SCULPT_GIZMO,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(resolution: u32) -> jackdaw_scene_types::Terrain {
        jackdaw_scene_types::Terrain {
            resolution,
            size: Vec2::splat((resolution - 1) as f32),
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        }
    }

    fn stroke_world(resolution: u32) -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<TerrainDataStore>();
        let terrain = subject(resolution);
        // Ground for the stroke to land on: nothing allocates implicitly,
        // and the region size keeps this fixture a few hundred cells rather
        // than a million.
        let mut regions = jackdaw_terrain::TerrainRegions::new(
            jackdaw_terrain::RegionSize::new(resolution).expect("a power of two"),
        );
        regions
            .ensure_grid(resolution)
            .expect("inside the region cap");
        world.resource_mut::<TerrainDataStore>().insert(
            terrain.data_path.clone(),
            jackdaw_terrain::RegionTerrainData {
                regions,
                ..default()
            },
        );
        let entity = world.spawn((terrain, TerrainDirtyChunks::default())).id();
        (world, entity)
    }

    fn heights(world: &World) -> Vec<f32> {
        world
            .resource::<TerrainDataStore>()
            .heights("zone.terrain-0.jdterrain")
            .to_vec()
    }

    /// Drag a brush across a terrain the way the operator does, and hand
    /// back the entry the stroke would leave behind.
    fn brushed_stroke(world: &mut World, entity: Entity, path: &[Vec2]) -> SetTerrainHeights {
        let terrain = subject(16);
        let before = heights(world);
        let mut stroke_rect: Option<GridRect> = None;
        for grid_pos in path {
            let Some(rect) = GridRect::brush(terrain.resolution, *grid_pos, 3.0) else {
                continue;
            };
            let mut store = world.resource_mut::<TerrainDataStore>();
            store.brush_heights(&terrain, rect, |heights| {
                jackdaw_terrain::apply_brush_at(
                    heights,
                    terrain.resolution,
                    jackdaw_terrain::SculptTool::Raise,
                    *grid_pos,
                    3.0,
                    10.0,
                    2.0,
                    1.0 / 60.0,
                    None,
                );
            });
            stroke_rect = Some(match stroke_rect {
                Some(grown) => grown.union(rect),
                None => rect,
            });
        }
        let rect = stroke_rect.expect("the path brushed the terrain");
        let after = heights(world);
        SetTerrainHeights::stroke(
            entity,
            rect,
            rect.read(&before, terrain.resolution),
            rect.read(&after, terrain.resolution),
            "Terrain Raise".to_string(),
        )
    }

    /// Undo of a rect-scoped entry restores the terrain cell for cell,
    /// including the cells outside the entry's rect.
    #[test]
    fn undoing_a_rect_scoped_stroke_restores_the_whole_terrain_bit_for_bit() {
        let (mut world, entity) = stroke_world(16);
        // Sculpt first, so undo has a non-flat state to restore rather than
        // the zeroes a fresh terrain reads back anyway.
        let mut seed = brushed_stroke(&mut world, entity, &[Vec2::new(11.0, 11.0)]);
        seed.execute(&mut world);
        let before: Vec<u32> = heights(&world).iter().map(|h| h.to_bits()).collect();

        let mut command = brushed_stroke(
            &mut world,
            entity,
            &[
                Vec2::new(4.0, 4.0),
                Vec2::new(5.5, 4.5),
                Vec2::new(7.0, 6.0),
            ],
        );
        let after: Vec<u32> = heights(&world).iter().map(|h| h.to_bits()).collect();
        assert_ne!(before, after, "the stroke has to have changed something");

        command.undo(&mut world);
        assert_eq!(
            heights(&world)
                .iter()
                .map(|h| h.to_bits())
                .collect::<Vec<_>>(),
            before,
            "undo must restore every cell exactly, not only the brushed ones",
        );

        command.execute(&mut world);
        assert_eq!(
            heights(&world)
                .iter()
                .map(|h| h.to_bits())
                .collect::<Vec<_>>(),
            after,
            "redo must put the stroke back exactly",
        );
    }

    /// Writing heights back retires the shared heightmap; otherwise the
    /// next brush target would raycast against the pre-undo terrain.
    #[test]
    fn undo_retires_the_shared_heightmap() {
        let (mut world, entity) = stroke_world(16);
        let terrain = subject(16);
        let mut command = brushed_stroke(&mut world, entity, &[Vec2::new(8.0, 8.0)]);
        let sculpted = world
            .resource_mut::<TerrainDataStore>()
            .heightmap(&terrain)
            .map
            .heights
            .clone();
        assert!(sculpted.iter().any(|h| *h != 0.0));

        command.undo(&mut world);
        let restored = world
            .resource_mut::<TerrainDataStore>()
            .heightmap(&terrain)
            .map
            .heights
            .clone();
        assert!(
            restored.iter().all(|h| *h == 0.0),
            "the shared heightmap still holds the undone stroke",
        );
    }

    /// Brushing the store's heights in place lands the same terrain the
    /// copy-out, brush, copy-back path lands, frame for frame across a
    /// drag.
    #[test]
    fn brushing_in_place_lands_the_same_terrain_as_copying_out_and_back() {
        let terrain = subject(16);
        let path = [
            Vec2::new(4.0, 4.0),
            Vec2::new(5.5, 4.5),
            Vec2::new(7.0, 6.0),
            Vec2::new(9.25, 8.75),
        ];

        let mut reference =
            jackdaw_terrain::Heightmap::new(terrain.resolution, terrain.size, terrain.max_height);
        for grid_pos in path {
            jackdaw_terrain::apply_brush(
                &mut reference,
                jackdaw_terrain::SculptTool::Raise,
                grid_pos,
                3.0,
                10.0,
                2.0,
                1.0 / 60.0,
                None,
            );
        }

        let (mut world, entity) = stroke_world(16);
        brushed_stroke(&mut world, entity, &path);

        assert_eq!(
            heights(&world)
                .iter()
                .map(|h| h.to_bits())
                .collect::<Vec<_>>(),
            reference
                .heights
                .iter()
                .map(|h| h.to_bits())
                .collect::<Vec<_>>(),
        );
    }

    /// A stroke keeps its brushed rectangle, not the terrain. At 512 that
    /// is the difference between kilobytes and the two megabytes a
    /// whole-terrain entry costs.
    #[test]
    fn a_stroke_entry_holds_its_rect_and_a_global_edit_holds_the_terrain() {
        let (mut world, entity) = stroke_world(16);
        let stroke = brushed_stroke(&mut world, entity, &[Vec2::new(8.0, 8.0)]);
        let whole = SetTerrainHeights::whole(
            entity,
            vec![0.0; 256],
            vec![1.0; 256],
            "Generate Terrain".to_string(),
        );
        assert!(
            stroke.heap_bytes() < whole.heap_bytes() / 4,
            "stroke {} bytes vs whole {} bytes",
            stroke.heap_bytes(),
            whole.heap_bytes(),
        );
    }
}

/// What a pointer press over the viewport reaches, and what a press on the
/// tool palette floating on top of it does not.
#[cfg(test)]
mod pointer_tests {
    use super::*;
    use crate::terrain::pointer_harness;

    /// Where the last run of [`probe_brush_hit`] landed.
    #[derive(Resource, Default)]
    struct BrushHitProbe(Option<(Entity, Vec2)>);

    fn probe_brush_hit(
        vp: crate::viewport::ViewportCursor,
        terrains: Query<(Entity, &jackdaw_scene_types::Terrain, &GlobalTransform)>,
        selection: Res<Selection>,
        store: Res<TerrainDataStore>,
        mut probe: ResMut<BrushHitProbe>,
    ) {
        probe.0 = terrain_brush_hit(&vp, &terrains, &selection, &store);
    }

    fn sculpt_app() -> App {
        let mut app = pointer_harness::app(16);
        app.init_resource::<BrushHitProbe>()
            .init_resource::<TerrainSculptState>()
            .insert_resource(TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Raise))
            .add_systems(Update, (probe_brush_hit, update_terrain_brush_position));
        app
    }

    /// A press on open ground finds the terrain under the cursor.
    #[test]
    fn a_press_on_open_ground_targets_the_terrain() {
        let mut app = sculpt_app();
        app.update();
        assert!(
            app.world().resource::<BrushHitProbe>().0.is_some(),
            "a press over the viewport's own node must reach the terrain",
        );
    }

    /// The palette floats over the viewport, so a press on one of its
    /// buttons is inside the viewport's rectangle and targets no terrain;
    /// otherwise picking a tool would also act on the ground behind it.
    #[test]
    fn a_press_on_the_tool_palette_targets_no_terrain() {
        let mut app = sculpt_app();
        pointer_harness::hover_overlay(&mut app);
        app.update();
        assert!(
            app.world().resource::<BrushHitProbe>().0.is_none(),
            "a press on UI drawn over the viewport must not reach the terrain behind it",
        );
    }

    /// A split view holds several `SceneViewport` nodes, and the guard
    /// blocks on any hovered node that is not the active viewport's own.
    /// The active viewport follows the pointer, so the panel the pointer
    /// moved into keeps its gestures instead of reading its own node as
    /// UI in the way.
    #[test]
    fn a_press_in_another_viewport_targets_the_terrain() {
        let mut app = sculpt_app();
        pointer_harness::hover_second_viewport(&mut app);
        app.update();
        assert!(
            app.world().resource::<BrushHitProbe>().0.is_some(),
            "a second viewport panel must not block presses inside itself",
        );
    }

    /// A stroke starts only where the brush has a target, so the same guard
    /// keeps a palette press from sculpting.
    #[test]
    fn a_press_on_the_tool_palette_leaves_the_brush_no_target() {
        let mut app = sculpt_app();
        app.update();
        assert!(
            app.world()
                .resource::<TerrainSculptState>()
                .brush_position
                .is_some(),
            "the brush follows the cursor over open ground",
        );

        pointer_harness::hover_overlay(&mut app);
        app.update();
        assert!(
            app.world()
                .resource::<TerrainSculptState>()
                .brush_position
                .is_none(),
            "no brush target under the palette means no stroke can start there",
        );
    }
}
