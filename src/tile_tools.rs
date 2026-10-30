//! Tile edit mode on a selected tilemap: its own input context for the tool
//! keys, the cell under the cursor, and the Brush and Eraser strokes, each one
//! history entry through [`TilemapStrokeBuilder`].

use bevy::{
    picking::events::{PointerDrag, PointerPress},
    prelude::*,
    ui::{ComputedNode, UiGlobalTransform},
};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::operator::warn_caller;
use jackdaw_input::prelude::{ContextActivity, ContextPriority, InputContext};
use jackdaw_scene_types::{Tilemap, TilemapLayerKind};
use jackdaw_tilemap::{CellCoord, Grid, TerrainId, TileRef, orthogonal_line};
use jackdaw_tileset::Tileset;

use crate::core_extension::CoreExtensionInputContext;
use crate::selection::Selection;
use crate::tilemap_stroke::{TileWrite, TilemapStrokeBuilder, packed_tile};
use crate::viewport_2d::{Scene2dViewport, Viewport2dMode, Viewport2dPanelHost};

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TileEditMode>()
        .init_resource::<TileBrush>()
        .init_resource::<TileStrokeState>()
        .add_observer(on_tile_press)
        .add_observer(on_tile_drag)
        .add_observer(on_tilemap_removed)
        .add_systems(
            Update,
            leave_when_unselected.run_if(resource_changed::<Selection>),
        );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TilemapEditModeOp>()
        .register_operator::<TilemapEditModeExitOp>()
        .register_operator::<TilemapBrushOp>()
        .register_operator::<TilemapEraserOp>();
    ctx.bind_operator::<CoreExtensionInputContext, TilemapEditModeOp>([PresetInput::key("Digit1")]);
    let context = ctx
        .spawn((
            TileEditInputContext,
            ContextPriority(5),
            ContextActivity::INACTIVE,
            Name::new("TileEditInputContext"),
            crate::EditorEntity,
        ))
        .id();
    ctx.bind_operator_in::<TilemapBrushOp>(context, [PresetInput::key("KeyB")])
        .bind_operator_in::<TilemapEraserOp>(context, [PresetInput::key("KeyD")])
        .bind_operator_in::<TilemapEditModeExitOp>(context, [PresetInput::key("Escape")]);
}

/// The input context of the tile tools, active only in tile edit mode.
#[derive(Component, Default)]
#[require(InputContext)]
pub struct TileEditInputContext;

/// The tool a press on the map uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TileTool {
    #[default]
    Brush,
    Eraser,
}

/// Tile edit mode: on while it names a map.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileEditMode {
    pub map: Option<Entity>,
    /// The index of the layer strokes paint.
    pub layer: usize,
    pub tool: TileTool,
}

/// What the Brush paints: terrain on a terrain layer, the tile on a tile
/// layer.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileBrush {
    pub terrain: TerrainId,
    pub tile: TileRef,
}

impl Default for TileBrush {
    fn default() -> Self {
        Self {
            terrain: 1,
            tile: TileRef::new(0),
        }
    }
}

/// The map cell under the pointer while a button is held on the map, that
/// button, and the stroke it is drawing.
#[derive(Resource, Default)]
pub struct TileStrokeState {
    pub cell: Option<CellCoord>,
    pub button: Option<MouseButton>,
    stroke: Option<(TilemapStrokeBuilder, TileWrite)>,
    last: Option<CellCoord>,
}

/// The cell of a map holding `point`, a world point, under the map's
/// transform.
pub fn cell_at(point: Vec2, map: &GlobalTransform, cell_size: UVec2) -> CellCoord {
    let local = map.affine().inverse().transform_point3(point.extend(0.0));
    Grid::new(cell_size).world_to_cell(local.truncate())
}

/// Enter tile edit mode on the selected tilemap.
#[operator(
    id = "tilemap.edit_mode",
    label = "Tile Edit Mode",
    description = "Paint the selected tilemap's cells with the tile tools.",
    allows_undo = false,
    is_available = can_enter_edit_mode,
    params(layer(i64, doc = "Index of the layer to paint; defaults to 0.")),
)]
pub(crate) fn tilemap_edit_mode(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let map = world.resource::<Selection>().primary();
    let layers = map
        .and_then(|m| world.get::<Tilemap>(m))
        .map(|t| t.layers.len());
    let layer = params.as_int("layer").unwrap_or(0);
    match usize::try_from(layer)
        .ok()
        .filter(|l| *l < layers.unwrap_or(0))
    {
        Some(layer) => {
            *world.resource_mut::<TileEditMode>() = TileEditMode {
                map,
                layer,
                tool: TileTool::Brush,
            };
            set_context_active(world, true);
            OperatorResult::Finished
        }
        None => {
            let id = TilemapEditModeOp::ID;
            warn_caller(world, format!("{id}: the map has no layer {layer}"));
            OperatorResult::Cancelled
        }
    }
}

fn can_enter_edit_mode(
    mode: Res<TileEditMode>,
    selection: Res<Selection>,
    maps: Query<(), With<Tilemap>>,
    active: ActiveModalQuery,
) -> bool {
    mode.map.is_none()
        && !active.is_modal_running()
        && selection.primary().is_some_and(|e| maps.contains(e))
}

/// Leave tile edit mode.
#[operator(
    id = "tilemap.edit_mode.exit",
    label = "Leave Tile Edit Mode",
    description = "Stop painting the tilemap's cells.",
    allows_undo = false,
    is_available = can_leave_edit_mode,
)]
pub(crate) fn tilemap_edit_mode_exit(
    _: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    leave_edit_mode(world);
    OperatorResult::Finished
}

fn can_leave_edit_mode(mode: Res<TileEditMode>, active: ActiveModalQuery) -> bool {
    mode.map.is_some() && !active.is_modal_running()
}

fn has_tile_map(mode: Res<TileEditMode>, maps: Query<(), With<Tilemap>>) -> bool {
    mode.map.is_some_and(|map| maps.contains(map))
}

fn leave_edit_mode(world: &mut World) {
    cancel_tile_stroke(world);
    world.resource_mut::<TileEditMode>().map = None;
    set_context_active(world, false);
}

fn set_context_active(world: &mut World, active: bool) {
    let mut contexts = world.query_filtered::<&mut ContextActivity, With<TileEditInputContext>>();
    for mut activity in contexts.iter_mut(world) {
        activity.set_if_neq(ContextActivity(active));
    }
}

fn on_tilemap_removed(event: On<Remove<Tilemap>>, mode: Res<TileEditMode>, mut commands: Commands) {
    if mode.map == Some(event.entity) {
        commands.queue(leave_edit_mode);
    }
}

/// Tile edit mode follows the selection: it ends once its map is no longer
/// the primary selection.
fn leave_when_unselected(world: &mut World) {
    let map = world.resource::<TileEditMode>().map;
    if map.is_some() && world.resource::<Selection>().primary() != map {
        leave_edit_mode(world);
    }
}

/// Paint with the Brush.
#[operator(
    id = "tilemap.brush",
    label = "Tile Brush",
    description = "Choose the Brush, or paint the active layer while the mouse button is held: \
                   terrain on a terrain layer, the brush tile on a tile layer.",
    modal = true,
    allows_undo = false,
    is_available = has_tile_map,
    cancel = cancel_tile_stroke,
    params(
        terrain(String, doc = "Terrain id or name the Brush paints on terrain layers."),
        tile(String, doc = "Tile as sheet:tile or packed layer the Brush places on tile layers."),
    )
)]
pub(crate) fn tilemap_brush(
    params: In<OperatorParameters>,
    world: &mut World,
) -> Option<OperatorResult> {
    if world.resource::<TileStrokeState>().stroke.is_none()
        && let Err(err) = set_brush(world, &params)
    {
        warn_caller(world, format!("{}: {err}", TilemapBrushOp::ID));
        return Some(OperatorResult::Cancelled);
    }
    tile_stroke(world, TileTool::Brush)
}

/// Erase with the Eraser.
#[operator(
    id = "tilemap.eraser",
    label = "Tile Eraser",
    description = "Choose the Eraser, or erase the active layer's cells while the mouse button \
                   is held.",
    modal = true,
    allows_undo = false,
    is_available = has_tile_map,
    cancel = cancel_tile_stroke
)]
pub(crate) fn tilemap_eraser(
    _: In<OperatorParameters>,
    world: &mut World,
) -> Option<OperatorResult> {
    tile_stroke(world, TileTool::Eraser)
}

fn cancel_tile_stroke(world: &mut World) {
    let state = std::mem::take(&mut *world.resource_mut::<TileStrokeState>());
    if let Some((builder, _)) = state.stroke {
        builder.cancel(world);
    }
}

/// The tileset of the active layer in tile edit mode.
fn active_tileset(world: &mut World) -> Result<AssetId<Tileset>, String> {
    let mode = *world.resource::<TileEditMode>();
    let path = (mode.map)
        .and_then(|map| world.get::<Tilemap>(map))
        .and_then(|map| map.layers.get(mode.layer))
        .map(|layer| layer.tileset.clone())
        .ok_or("no tilemap is in tile edit mode")?;
    crate::tileset_dock::tileset_id(world, &path)
}

/// Sets the brush source from the `terrain` and `tile` params, read against
/// the active layer's tileset.
fn set_brush(world: &mut World, params: &OperatorParameters) -> Result<(), String> {
    let text = |name: &str| params.as_str(name).unwrap_or_default().trim().to_string();
    let (terrain, tile) = (text("terrain"), text("tile"));
    if terrain.is_empty() && tile.is_empty() {
        return Ok(());
    }
    let id = active_tileset(world)?;
    let tileset =
        (world.resource::<Assets<Tileset>>().get(id)).ok_or("the layer's tileset is not loaded")?;
    let mut brush = *world.resource::<TileBrush>();
    if !terrain.is_empty() {
        brush.terrain = crate::tileset_edit::terrain(tileset, &terrain)?;
    }
    if !tile.is_empty() {
        brush.tile = packed_tile(tileset, &tile)?;
    }
    *world.resource_mut::<TileBrush>() = brush;
    Ok(())
}

/// What a stroke with `button` writes on the active layer, or why the brush
/// cannot paint it: its terrain or tile is not in the layer's tileset.
fn stroke_write(
    world: &mut World,
    tool: TileTool,
    button: MouseButton,
) -> Result<TileWrite, String> {
    if tool == TileTool::Eraser || button == MouseButton::Right {
        return Ok(TileWrite::Erase);
    }
    let mode = *world.resource::<TileEditMode>();
    let terrain_layer = (mode.map)
        .and_then(|map| world.get::<Tilemap>(map))
        .and_then(|map| map.layers.get(mode.layer))
        .is_some_and(|layer| layer.kind == TilemapLayerKind::Terrain);
    let brush = *world.resource::<TileBrush>();
    let id = active_tileset(world)?;
    let tileset =
        (world.resource::<Assets<Tileset>>().get(id)).ok_or("the layer's tileset is not loaded")?;
    if terrain_layer {
        (tileset.terrains.iter().any(|t| t.id == brush.terrain))
            .then_some(TileWrite::Terrain(brush.terrain))
            .ok_or_else(|| {
                format!(
                    "the brush terrain {} is not in the layer's tileset",
                    brush.terrain
                )
            })
    } else {
        (tileset.tile_of_layer(u32::from(brush.tile.index())))
            .map(|_| TileWrite::Tile(brush.tile))
            .ok_or_else(|| {
                format!(
                    "the brush tile {} is not in the layer's tileset",
                    brush.tile.index()
                )
            })
    }
}

/// One frame of a Brush or Eraser stroke. Invoked with no press on the map,
/// it only makes `tool` the one a press uses. The secondary button erases
/// with either tool. The first frame paints the pressed cell even when the
/// button is already up again.
fn tile_stroke(world: &mut World, tool: TileTool) -> Option<OperatorResult> {
    let mut state = std::mem::take(&mut *world.resource_mut::<TileStrokeState>());
    let mouse = world.resource::<ButtonInput<MouseButton>>();
    let held = state.button.filter(|b| mouse.pressed(*b));
    if state.stroke.is_none() {
        let Some(button) = state.button else {
            world.resource_mut::<TileEditMode>().tool = tool;
            return Some(OperatorResult::Finished);
        };
        let mode = *world.resource::<TileEditMode>();
        let begun = stroke_write(world, tool, button).and_then(|write| {
            let label = if write == TileWrite::Erase {
                "Erase tiles"
            } else {
                "Paint tiles"
            };
            TilemapStrokeBuilder::begin(
                world,
                mode.map.ok_or("no tilemap is in tile edit mode")?,
                mode.layer,
                label,
            )
            .map(|b| (b, write))
        });
        match begun {
            Ok(stroke) => state.stroke = Some(stroke),
            Err(err) => {
                crate::status_bar::notify_warn(world, format!("Tile stroke refused: {err}"));
                warn_caller(world, format!("tile stroke: {err}"));
                return Some(OperatorResult::Cancelled);
            }
        }
    }
    if let Some(cell) = state.cell
        && state.last != Some(cell)
        && let Some((builder, write)) = &mut state.stroke
    {
        let cells = state
            .last
            .map_or(vec![cell], |last| orthogonal_line(last, cell));
        builder.apply(world, &cells, *write);
        state.last = Some(cell);
    }
    if held.is_none() {
        if let Some((builder, _)) = state.stroke {
            builder.end(world);
        }
        return Some(OperatorResult::Finished);
    }
    *world.resource_mut::<TileStrokeState>() = state;
    Some(OperatorResult::Running)
}

/// The map cell under a pointer on a 2D panel's stage in Edit mode, and
/// whether the pointer is on the stage.
fn stage_cell(
    target: Entity,
    position: Vec2,
    mode: &TileEditMode,
    hosts: &Query<&Viewport2dPanelHost>,
    stages: &Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dViewport>>,
    maps: &Query<(&GlobalTransform, &Tilemap)>,
) -> Option<(CellCoord, bool)> {
    let host = hosts.iter().find(|host| host.stage == target)?;
    if host.mode != Viewport2dMode::Edit {
        return None;
    }
    let (computed, transform) = stages.get(host.stage).ok()?;
    let (point, inside) =
        crate::sprite_stage::stage_world_point(position, host, computed, transform);
    let (map_transform, tilemap) = maps.get(mode.map?).ok()?;
    Some((cell_at(point, map_transform, tilemap.cell_size), inside))
}

/// A press on the stage in tile edit mode starts a stroke with the active
/// tool.
fn on_tile_press(
    event: On<PointerPress>,
    ui_scale: Res<UiScale>,
    mode: Res<TileEditMode>,
    hosts: Query<&Viewport2dPanelHost>,
    stages: Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dViewport>>,
    maps: Query<(&GlobalTransform, &Tilemap)>,
    mut state: ResMut<TileStrokeState>,
    mut commands: Commands,
) {
    let button = match event.button {
        PointerButton::Primary => MouseButton::Left,
        PointerButton::Secondary => MouseButton::Right,
        PointerButton::Middle => return,
    };
    let position = event.pointer.position / ui_scale.0;
    let target = event.event_target();
    if state.button.is_some() {
        return;
    }
    let Some((cell, true)) = stage_cell(target, position, &mode, &hosts, &stages, &maps) else {
        return;
    };
    state.cell = Some(cell);
    state.button = Some(button);
    let id = match mode.tool {
        TileTool::Brush => TilemapBrushOp::ID,
        TileTool::Eraser => TilemapEraserOp::ID,
    };
    commands.queue(move |world: &mut World| {
        let result = world
            .operator(id)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: false,
            })
            .call();
        if !matches!(result, Ok(OperatorResult::Running)) {
            *world.resource_mut::<TileStrokeState>() = TileStrokeState::default();
        }
    });
}

/// A drag that a press on the map began moves the stroke's cell.
fn on_tile_drag(
    event: On<PointerDrag>,
    ui_scale: Res<UiScale>,
    mode: Res<TileEditMode>,
    hosts: Query<&Viewport2dPanelHost>,
    stages: Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dViewport>>,
    maps: Query<(&GlobalTransform, &Tilemap)>,
    mut state: ResMut<TileStrokeState>,
) {
    let position = event.pointer.position / ui_scale.0;
    let target = event.event_target();
    if state.button.is_some()
        && let Some((cell, _)) = stage_cell(target, position, &mode, &hosts, &stages, &maps)
    {
        state.cell = Some(cell);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_lands_in_the_cell_of_the_map_holding_it() {
        let at = |point: Vec2, transform: Transform, cell: u32| {
            cell_at(point, &GlobalTransform::from(transform), UVec2::splat(cell))
        };
        assert_eq!(
            at(Vec2::new(5.0, 20.0), Transform::IDENTITY, 16),
            CellCoord::new(0, 1)
        );
        assert_eq!(
            at(Vec2::new(-0.5, -16.0), Transform::IDENTITY, 16),
            CellCoord::new(-1, -1)
        );
        let moved = Transform::from_xyz(100.0, -50.0, 3.0);
        assert_eq!(at(Vec2::new(100.0, -50.0), moved, 16), CellCoord::new(0, 0));
        assert_eq!(
            at(Vec2::new(99.0, -51.0), moved, 16),
            CellCoord::new(-1, -1)
        );
        let scaled = Transform::from_xyz(10.0, 10.0, 0.0).with_scale(Vec3::splat(2.0));
        assert_eq!(at(Vec2::new(43.0, 9.0), scaled, 16), CellCoord::new(1, -1));
        assert_eq!(
            at(Vec2::new(-55.0, 74.0), scaled, 16),
            CellCoord::new(-3, 2)
        );
    }
}
