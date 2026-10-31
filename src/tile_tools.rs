//! Tile edit mode on a selected tilemap: its own input context for the tool
//! keys, the cell under the cursor, and the tools: Brush, Eraser, Rectangle,
//! Line and Bucket write one history entry each through
//! [`TilemapStrokeBuilder`]; the Picker loads the brush from a cell.

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
use jackdaw_tilemap::{
    CHUNK_SIZE, CellCoord, CellRect, Grid, LayerData, TerrainId, TileRef, orthogonal_line,
};
use jackdaw_tileset::Tileset;

use crate::core_extension::CoreExtensionInputContext;
use crate::selection::Selection;
use crate::tile_stamp::{StampBrush, TileStamp};
use crate::tilemap::{TilemapDataStore, editable_key};
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
        .register_operator::<TilemapEraserOp>()
        .register_operator::<TilemapRectangleOp>()
        .register_operator::<TilemapLineOp>()
        .register_operator::<TilemapBucketOp>()
        .register_operator::<TilemapPickerOp>();
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
        .bind_operator_in::<TilemapRectangleOp>(context, [PresetInput::key("KeyU")])
        .bind_operator_in::<TilemapLineOp>(context, [PresetInput::key("KeyL")])
        .bind_operator_in::<TilemapBucketOp>(context, [PresetInput::key("KeyG")])
        .bind_operator_in::<TilemapPickerOp>(context, [PresetInput::key("KeyI")])
        .bind_operator_in::<TilemapEditModeExitOp>(context, [PresetInput::key("Escape")]);
    crate::tile_palette::add_to_extension(ctx, context);
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
    Rectangle,
    Line,
    Bucket,
    Picker,
}

impl TileTool {
    /// Whether the tool writes one shape between the press and release cells.
    pub(crate) fn is_shape(self) -> bool {
        matches!(self, Self::Rectangle | Self::Line)
    }
}

/// The most cells one Bucket fill, Rectangle or Line writes; a larger one is
/// refused.
pub const MAX_FILL_CELLS: usize = 1 << 20;

/// Tile edit mode: on while it names a map.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileEditMode {
    pub map: Option<Entity>,
    /// The index of the layer strokes paint.
    pub layer: usize,
    pub tool: TileTool,
}

/// How a terrain stroke joins the terrain around it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StrokeMode {
    /// Joins every neighbour of the same terrain.
    #[default]
    Connect,
    /// Joins only the cells of the same stroke.
    Path,
}

/// What the brush paints with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BrushSource {
    /// Terrain on a terrain layer; the tile on a tile layer.
    #[default]
    Terrain,
    /// The tile, hand-placed over a terrain layer's terrain.
    Tile,
    /// The loaded [`crate::tile_stamp::StampBrush`], painted by the Brush.
    Stamp,
}

/// What the painting tools write: terrain on a terrain layer, the tile on a
/// tile layer, or the tile when the source names it.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileBrush {
    pub terrain: TerrainId,
    /// With its orientation bits.
    pub tile: TileRef,
    pub mode: StrokeMode,
    pub source: BrushSource,
    /// Whether a Bucket press fills only the cells joined to the pressed one.
    pub contiguous: bool,
}

impl Default for TileBrush {
    fn default() -> Self {
        Self {
            terrain: 1,
            tile: TileRef::new(0),
            mode: StrokeMode::Connect,
            source: BrushSource::Terrain,
            contiguous: true,
        }
    }
}

/// The map cell under the pointer while a button is held on the map, that
/// button, and the stroke it is drawing.
#[derive(Resource, Default)]
pub struct TileStrokeState {
    pub cell: Option<CellCoord>,
    pub button: Option<MouseButton>,
    stroke: Option<(TilemapStrokeBuilder, Paint)>,
    /// The last cell a Brush or Eraser stroke wrote, or a shape's press cell.
    last: Option<CellCoord>,
}

impl TileStrokeState {
    /// What the held stroke paints and, for a shape, its press cell.
    pub(crate) fn held(&self) -> Option<(&Paint, Option<CellCoord>)> {
        (self.stroke.as_ref()).map(|(_, paint)| (paint, self.last))
    }
}

/// What a stroke paints: one write on every cell it reaches, or the loaded
/// stamp placed at each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Paint {
    Write(TileWrite),
    Stamp(TileStamp),
}

impl Paint {
    /// The writes painting at each of `cells`, each with the cells it lands
    /// on.
    pub fn writes(&self, cells: &[CellCoord]) -> Vec<(TileWrite, Vec<CellCoord>)> {
        match self {
            Paint::Write(write) => vec![(*write, cells.to_vec())],
            Paint::Stamp(stamp) => stamp.writes(cells),
        }
    }
}

/// What a stroke of `tool` with `button` paints: the loaded stamp for a
/// Brush stroke while the source is a stamp of the layer's tileset, else
/// what [`stroke_write`] gives.
pub(crate) fn stroke_paint(
    world: &mut World,
    tool: TileTool,
    button: MouseButton,
) -> Result<Paint, String> {
    let stamped = world.resource::<TileBrush>().source == BrushSource::Stamp;
    if !stamped || tool != TileTool::Brush || button != MouseButton::Left {
        return stroke_write(world, tool, button).map(Paint::Write);
    }
    let id = active_tileset(world)?;
    let brush = world.resource::<StampBrush>();
    if brush.tileset != Some(id) {
        return Err("the brush stamp is not from the layer's tileset".to_string());
    }
    (brush.stamp.clone().map(Paint::Stamp)).ok_or_else(|| "no stamp is loaded".to_string())
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

pub(crate) fn has_tile_map(mode: Res<TileEditMode>, maps: Query<(), With<Tilemap>>) -> bool {
    mode.map.is_some_and(|map| maps.contains(map))
}

/// Paints `layer` of `map`, entering tile edit mode on it when the mode is
/// off or on another map; the tool stays when only the layer changes.
pub(crate) fn enter_layer(world: &mut World, map: Entity, layer: usize) {
    let mode = *world.resource::<TileEditMode>();
    if mode.map == Some(map) {
        world.resource_mut::<TileEditMode>().layer = layer;
        return;
    }
    if mode.map.is_some() {
        leave_edit_mode(world);
    }
    *world.resource_mut::<TileEditMode>() = TileEditMode {
        map: Some(map),
        layer,
        tool: TileTool::Brush,
    };
    set_context_active(world, true);
}

pub(crate) fn leave_edit_mode(world: &mut World) {
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
        mode(
            String,
            doc = "connect joins every neighbour of the terrain; path joins only the cells of \
                   the same stroke. Unset keeps the brush mode, connect at first."
        ),
    )
)]
pub(crate) fn tilemap_brush(
    params: In<OperatorParameters>,
    world: &mut World,
) -> Option<OperatorResult> {
    shape_or_brush(world, &params, TileTool::Brush)
}

/// Sets the brush from `params` before a stroke, then runs the stroke frame.
fn shape_or_brush(
    world: &mut World,
    params: &OperatorParameters,
    tool: TileTool,
) -> Option<OperatorResult> {
    if world.resource::<TileStrokeState>().stroke.is_none()
        && let Err(err) = set_brush(world, params)
    {
        warn_caller(world, format!("tile tool: {err}"));
        return Some(OperatorResult::Cancelled);
    }
    tile_stroke(world, tool)
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
pub(crate) fn active_tileset(world: &mut World) -> Result<AssetId<Tileset>, String> {
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
    let mut brush = *world.resource::<TileBrush>();
    brush.mode = match text("mode").as_str() {
        "" => brush.mode,
        "connect" => StrokeMode::Connect,
        "path" => StrokeMode::Path,
        other => return Err(format!("mode is connect or path, not `{other}`")),
    };
    if !terrain.is_empty() || !tile.is_empty() {
        let mode = *world.resource::<TileEditMode>();
        let terrain_layer = (mode.map.and_then(|map| world.get::<Tilemap>(map)))
            .and_then(|map| map.layers.get(mode.layer))
            .is_some_and(|layer| layer.kind == TilemapLayerKind::Terrain);
        let id = active_tileset(world)?;
        let tileset = (world.resource::<Assets<Tileset>>().get(id))
            .ok_or("the layer's tileset is not loaded")?;
        if !tile.is_empty() {
            brush.tile = packed_tile(tileset, &tile)?;
            brush.source = BrushSource::Tile;
        }
        if !terrain.is_empty() {
            brush.terrain = crate::tileset_edit::terrain(tileset, &terrain)?;
            if terrain_layer || tile.is_empty() {
                brush.source = BrushSource::Terrain;
            }
        }
    }
    world.resource_mut::<TileBrush>().set_if_neq(brush);
    Ok(())
}

/// The layer tile edit mode paints.
pub(crate) struct ActiveLayer {
    pub map: Entity,
    pub index: usize,
    pub key: String,
    pub terrain: bool,
}

pub(crate) fn active_layer(world: &mut World) -> Result<ActiveLayer, String> {
    const NO_MAP: &str = "no tilemap is in tile edit mode";
    let mode = *world.resource::<TileEditMode>();
    let map = mode.map.ok_or(NO_MAP)?;
    let layer = (world.get::<Tilemap>(map))
        .and_then(|t| t.layers.get(mode.layer))
        .ok_or(NO_MAP)?;
    let terrain = layer.kind == TilemapLayerKind::Terrain;
    let key = editable_key(world, map)?;
    Ok(ActiveLayer {
        map,
        index: mode.layer,
        key,
        terrain,
    })
}

/// What a stroke with `button` writes on the active layer, or why the brush
/// cannot paint it: its terrain or tile is not in the layer's tileset.
pub(crate) fn stroke_write(
    world: &mut World,
    tool: TileTool,
    button: MouseButton,
) -> Result<TileWrite, String> {
    if tool == TileTool::Eraser || button == MouseButton::Right {
        return Ok(TileWrite::Erase);
    }
    let terrain_layer = active_layer(world)?.terrain;
    let brush = *world.resource::<TileBrush>();
    if brush.source == BrushSource::Stamp {
        return Err("a stamp paints with the Brush only".to_string());
    }
    let id = active_tileset(world)?;
    let tileset =
        (world.resource::<Assets<Tileset>>().get(id)).ok_or("the layer's tileset is not loaded")?;
    if terrain_layer && brush.source == BrushSource::Terrain {
        (tileset.terrains.iter().any(|t| t.id == brush.terrain))
            .then_some(match brush.mode {
                StrokeMode::Path if tool != TileTool::Bucket => TileWrite::Path(brush.terrain),
                _ => TileWrite::Terrain(brush.terrain),
            })
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

fn refuse(world: &mut World, err: &str) -> OperatorResult {
    crate::status_bar::notify_warn(world, format!("Tile tool refused: {err}"));
    warn_caller(world, format!("tile tool: {err}"));
    OperatorResult::Cancelled
}

/// Opens a stroke of `tool` on the active layer, erasing when `erase`.
fn begin_stroke(
    world: &mut World,
    tool: TileTool,
    erase: bool,
) -> Result<TilemapStrokeBuilder, String> {
    let label = match tool {
        TileTool::Rectangle if erase => "Erase rectangle",
        TileTool::Rectangle => "Fill rectangle",
        TileTool::Line if erase => "Erase line",
        TileTool::Line => "Draw line",
        TileTool::Bucket if erase => "Erase region",
        TileTool::Bucket => "Bucket fill",
        _ if erase => "Erase tiles",
        _ => "Paint tiles",
    };
    let mode = *world.resource::<TileEditMode>();
    let map = mode.map.ok_or("no tilemap is in tile edit mode")?;
    TilemapStrokeBuilder::begin(world, map, mode.layer, label)
}

/// The cells of a Rectangle (both corners included, clipped to `bounds`) or
/// Line from `from` to `to`, or why there are too many.
pub(crate) fn shape_cells(
    tool: TileTool,
    from: CellCoord,
    to: CellCoord,
    bounds: Option<IRect>,
) -> Result<Vec<CellCoord>, String> {
    let too_many =
        |count: u64| format!("the shape holds {count} cells, more than {MAX_FILL_CELLS}");
    if tool != TileTool::Rectangle {
        let span = to.0.as_i64vec2() - from.0.as_i64vec2();
        let count = span.x.unsigned_abs() + span.y.unsigned_abs() + 1;
        if count > MAX_FILL_CELLS as u64 {
            return Err(too_many(count));
        }
        return Ok(orthogonal_line(from, to));
    }
    let (mut min, mut max) = (from.0.min(to.0), from.0.max(to.0));
    if let Some(b) = bounds {
        min = min.max(b.min);
        max = max.min(b.max.saturating_sub(IVec2::ONE));
        if min.cmpgt(max).any() {
            return Ok(Vec::new());
        }
    }
    let span = (max.as_i64vec2() - min.as_i64vec2()).as_u64vec2() + 1;
    let count = span.x.saturating_mul(span.y);
    if count > MAX_FILL_CELLS as u64 {
        return Err(too_many(count));
    }
    Ok((min.y..=max.y)
        .flat_map(|y| (min.x..=max.x).map(move |x| CellCoord::new(x, y)))
        .collect())
}

/// One frame of a stroke. Invoked with no press on the map, it only makes
/// `tool` the one a press uses. The secondary button erases with any tool.
/// Brush and Eraser write as the cursor moves, and their first frame paints
/// the pressed cell even when the button is already up again; Rectangle and
/// Line write their shape on release.
fn tile_stroke(world: &mut World, tool: TileTool) -> Option<OperatorResult> {
    let mut state = std::mem::take(&mut *world.resource_mut::<TileStrokeState>());
    let mouse = world.resource::<ButtonInput<MouseButton>>();
    let held = state.button.filter(|b| mouse.pressed(*b));
    if state.stroke.is_none() {
        let Some(button) = state.button else {
            world.resource_mut::<TileEditMode>().tool = tool;
            return Some(OperatorResult::Finished);
        };
        let begun = stroke_paint(world, tool, button).and_then(|paint| {
            let erase = paint == Paint::Write(TileWrite::Erase);
            Ok((begin_stroke(world, tool, erase)?, paint))
        });
        match begun {
            Ok(stroke) => state.stroke = Some(stroke),
            Err(err) => return Some(refuse(world, &err)),
        }
        if tool.is_shape() {
            state.last = state.cell;
        }
    }
    if !tool.is_shape()
        && let Some(cell) = state.cell
        && state.last != Some(cell)
        && let Some((builder, paint)) = &mut state.stroke
    {
        let cells = state
            .last
            .map_or(vec![cell], |last| orthogonal_line(last, cell));
        for (write, cells) in paint.writes(&cells) {
            builder.apply(world, &cells, write);
        }
        state.last = Some(cell);
    }
    if held.is_some() {
        *world.resource_mut::<TileStrokeState>() = state;
        return Some(OperatorResult::Running);
    }
    let Some((mut builder, paint)) = state.stroke else {
        return Some(OperatorResult::Finished);
    };
    if tool.is_shape()
        && let (Some(from), Some(to)) = (state.last, state.cell)
    {
        let map = world.resource::<TileEditMode>().map;
        let bounds = map
            .and_then(|map| world.get::<Tilemap>(map))
            .and_then(|t| t.bounds);
        match shape_cells(tool, from, to, bounds) {
            Ok(cells) => {
                for (write, cells) in paint.writes(&cells) {
                    builder.apply(world, &cells, write);
                }
            }
            Err(err) => {
                builder.cancel(world);
                return Some(refuse(world, &err));
            }
        }
    }
    builder.end(world);
    Some(OperatorResult::Finished)
}

/// Draw a rectangle.
#[operator(
    id = "tilemap.rectangle",
    label = "Tile Rectangle",
    description = "Choose the Rectangle, or fill the rectangle between the press and release \
                   cells with the brush; the secondary button erases it.",
    modal = true,
    allows_undo = false,
    is_available = has_tile_map,
    cancel = cancel_tile_stroke,
    params(mode(
        String,
        doc = "connect joins every neighbour of the terrain; path joins only the cells of the \
               same stroke. Unset keeps the brush mode, connect at first."
    ))
)]
pub(crate) fn tilemap_rectangle(
    params: In<OperatorParameters>,
    world: &mut World,
) -> Option<OperatorResult> {
    shape_or_brush(world, &params, TileTool::Rectangle)
}

/// Draw a line.
#[operator(
    id = "tilemap.line",
    label = "Tile Line",
    description = "Choose the Line, or draw a line of cells from the press to the release cell \
                   with the brush; the secondary button erases it.",
    modal = true,
    allows_undo = false,
    is_available = has_tile_map,
    cancel = cancel_tile_stroke,
    params(mode(
        String,
        doc = "connect joins every neighbour of the terrain; path joins only the cells of the \
               same stroke. Unset keeps the brush mode, connect at first."
    ))
)]
pub(crate) fn tilemap_line(
    params: In<OperatorParameters>,
    world: &mut World,
) -> Option<OperatorResult> {
    shape_or_brush(world, &params, TileTool::Line)
}

/// A tool that acts on a press is available in tile edit mode while no
/// stroke is held.
fn can_act_on_press(
    mode: Res<TileEditMode>,
    maps: Query<(), With<Tilemap>>,
    state: Res<TileStrokeState>,
    active: ActiveModalQuery,
) -> bool {
    mode.map.is_some_and(|map| maps.contains(map))
        && state.stroke.is_none()
        && !active.is_modal_running()
}

/// The press a tool acting on a press takes, or `None` when there is none.
/// Its callers return first while a stroke is held, so no stroke is dropped.
fn take_press(world: &mut World) -> Option<(CellCoord, MouseButton)> {
    let state = world.resource::<TileStrokeState>();
    let press = state.cell.zip(state.button);
    *world.resource_mut::<TileStrokeState>() = TileStrokeState::default();
    press
}

/// Flood fill.
#[operator(
    id = "tilemap.bucket",
    label = "Tile Bucket",
    description = "Choose the Bucket, or fill the cells matching the pressed one with the \
                   brush; the secondary button erases them.",
    allows_undo = false,
    is_available = can_act_on_press,
    params(contiguous(
        bool,
        doc = "Fill only the matching cells joined to the pressed one; off fills every \
               matching cell on the layer. Unset follows the palette's Contiguous toggle."
    )),
)]
pub(crate) fn tilemap_bucket(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    if world.resource::<TileStrokeState>().stroke.is_some() {
        return OperatorResult::Cancelled;
    }
    let Some((cell, button)) = take_press(world) else {
        world.resource_mut::<TileEditMode>().tool = TileTool::Bucket;
        return OperatorResult::Finished;
    };
    let contiguous =
        (params.as_bool("contiguous")).unwrap_or_else(|| world.resource::<TileBrush>().contiguous);
    match bucket_fill(world, cell, button, contiguous) {
        Ok(()) => OperatorResult::Finished,
        Err(err) => refuse(world, &err),
    }
}

fn bucket_fill(
    world: &mut World,
    start: CellCoord,
    button: MouseButton,
    contiguous: bool,
) -> Result<(), String> {
    let active = active_layer(world)?;
    let write = stroke_write(world, TileTool::Bucket, button)?;
    let bounds = (world.get::<Tilemap>(active.map)).and_then(|t| t.bounds);
    let store = world.resource::<TilemapDataStore>();
    let layer = (store.get(&active.key)).and_then(|data| data.layers.get(active.index));
    let cells = fill_cells(layer, active.terrain, bounds, start, write, contiguous)?;
    if cells.is_empty() {
        return Ok(());
    }
    let mut builder = begin_stroke(world, TileTool::Bucket, write == TileWrite::Erase)?;
    builder.apply(world, &cells, write);
    builder.end(world);
    Ok(())
}

/// The cells a fill writing `write` from `start` reaches: those matching its
/// terrain on a terrain layer, or the tile it shows (hand-placed, else
/// resolved) on a tile layer or when the fill writes a tile, joined to it when
/// `contiguous`. No cells when the write would leave `start` as it is. A fill
/// stays inside the map's bounds, or on an unbounded map inside the bounds of
/// the painted cells and `start`, grown by one chunk.
fn fill_cells(
    layer: Option<&LayerData>,
    terrain_layer: bool,
    bounds: Option<IRect>,
    start: CellCoord,
    write: TileWrite,
    contiguous: bool,
) -> Result<Vec<CellCoord>, String> {
    let authoring = layer.and_then(|l| l.authoring.as_ref());
    let by_terrain = terrain_layer && !matches!(write, TileWrite::Tile(_));
    let value = |cell: CellCoord| {
        if by_terrain {
            return TileWrite::Terrain(authoring.map_or(0, |a| a.terrain.get(cell)));
        }
        let placed = authoring.and_then(|a| a.overrides.get(&cell).copied());
        TileWrite::Tile(
            placed.unwrap_or_else(|| layer.map_or(TileRef::EMPTY, |l| l.tiles.get(cell))),
        )
    };
    let target = value(start);
    let empty = target == TileWrite::Terrain(0) || target == TileWrite::Tile(TileRef::EMPTY);
    if write == target || (write == TileWrite::Erase && empty) {
        return Ok(Vec::new());
    }
    let region = match bounds {
        Some(b) => CellRect {
            min: b.min,
            max: b.max,
        },
        None => {
            let painted = if terrain_layer {
                authoring.and_then(|a| a.terrain.painted_bounds())
            } else {
                layer.and_then(|l| l.tiles.painted_bounds())
            };
            let seed = start.0;
            let (min, max) = painted.map_or((seed, seed + 1), |p| {
                (p.min.min(seed), p.max.max(seed.saturating_add(IVec2::ONE)))
            });
            CellRect {
                min: min.saturating_sub(IVec2::splat(CHUNK_SIZE)),
                max: max.saturating_add(IVec2::splat(CHUNK_SIZE)),
            }
        }
    };
    let inside = |cell: CellCoord| cell.0.cmpge(region.min).all() && cell.0.cmplt(region.max).all();
    if !inside(start) {
        return Ok(Vec::new());
    }
    let too_many = || format!("the fill reaches more than {MAX_FILL_CELLS} cells");
    if contiguous {
        let mut seen = std::collections::HashSet::from([start]);
        let mut open = vec![start];
        while let Some(cell) = open.pop() {
            for step in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
                let next = CellCoord(cell.0.saturating_add(step));
                if inside(next) && value(next) == target && seen.insert(next) {
                    if seen.len() > MAX_FILL_CELLS {
                        return Err(too_many());
                    }
                    open.push(next);
                }
            }
        }
        let mut cells: Vec<CellCoord> = seen.into_iter().collect();
        cells.sort_unstable();
        return Ok(cells);
    }
    let candidates: Box<dyn Iterator<Item = CellCoord>> = if empty {
        Box::new(
            (region.min.y..region.max.y)
                .flat_map(move |y| (region.min.x..region.max.x).map(move |x| CellCoord::new(x, y))),
        )
    } else if by_terrain {
        Box::new((authoring.into_iter()).flat_map(|a| a.terrain.cells().map(|(c, _)| c)))
    } else {
        let placed = (authoring.into_iter()).flat_map(|a| a.overrides.keys().copied());
        let shown = (layer.into_iter()).flat_map(|l| l.tiles.cells().map(|(c, _)| c));
        let mut cells: Vec<CellCoord> = shown.chain(placed).collect();
        cells.sort_unstable();
        cells.dedup();
        Box::new(cells.into_iter())
    };
    let mut found = Vec::new();
    for cell in candidates.filter(|&c| inside(c) && value(c) == target) {
        found.push(cell);
        if found.len() > MAX_FILL_CELLS {
            return Err(too_many());
        }
    }
    Ok(found)
}

/// Pick the brush from a cell.
#[operator(
    id = "tilemap.picker",
    label = "Tile Picker",
    description = "Choose the Picker, or load the brush from the pressed cell: its terrain on a \
                   terrain layer, and its hand-placed or resolved tile.",
    allows_undo = false,
    is_available = can_act_on_press
)]
pub(crate) fn tilemap_picker(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    if world.resource::<TileStrokeState>().stroke.is_some() {
        return OperatorResult::Cancelled;
    }
    let Some((cell, _)) = take_press(world) else {
        world.resource_mut::<TileEditMode>().tool = TileTool::Picker;
        return OperatorResult::Finished;
    };
    if let Err(err) = pick(world, cell) {
        return refuse(world, &err);
    }
    let mut mode = world.resource_mut::<TileEditMode>();
    if mode.tool == TileTool::Picker {
        mode.tool = TileTool::Brush;
    }
    OperatorResult::Finished
}

fn pick(world: &mut World, cell: CellCoord) -> Result<(), String> {
    let active = active_layer(world)?;
    let store = world.resource::<TilemapDataStore>();
    let layer = (store.get(&active.key)).and_then(|data| data.layers.get(active.index));
    let authoring = layer.and_then(|l| l.authoring.as_ref());
    let terrain = (authoring.filter(|_| active.terrain)).map_or(0, |a| a.terrain.get(cell));
    let placed = authoring.and_then(|a| a.overrides.get(&cell).copied());
    let tile = placed.unwrap_or_else(|| layer.map_or(TileRef::EMPTY, |l| l.tiles.get(cell)));
    if terrain == 0 && tile.is_empty() {
        return Err(format!("cell {},{} is empty", cell.0.x, cell.0.y));
    }
    let mut brush = world.resource_mut::<TileBrush>();
    if !tile.is_empty() {
        brush.tile = tile;
        brush.source = BrushSource::Tile;
    }
    if terrain != 0 {
        brush.terrain = terrain;
        if placed.is_none() {
            brush.source = BrushSource::Terrain;
        }
    }
    Ok(())
}

/// The map cell under a pointer on a 2D panel's stage in Edit mode, and
/// whether the pointer is on the stage.
pub(crate) fn stage_cell(
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

/// A press on the stage in tile edit mode runs the active tool, or the
/// Picker while Alt is held.
fn on_tile_press(
    event: On<PointerPress>,
    ui_scale: Res<UiScale>,
    mode: Res<TileEditMode>,
    keys: Res<ButtonInput<KeyCode>>,
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
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let id = match mode.tool {
        _ if alt => TilemapPickerOp::ID,
        TileTool::Brush => TilemapBrushOp::ID,
        TileTool::Eraser => TilemapEraserOp::ID,
        TileTool::Rectangle => TilemapRectangleOp::ID,
        TileTool::Line => TilemapLineOp::ID,
        TileTool::Bucket => TilemapBucketOp::ID,
        TileTool::Picker => TilemapPickerOp::ID,
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
