//! The Select tool: a rectangle of cells on the active layer, dragged out
//! and outlined, then erased, copied into a stamp the Brush paints, or cut.
//! Its keys sit on a context above Core that is active only while cells are
//! selected, so they hide the same chords from the entity operators.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::operator::warn_caller;
use jackdaw_input::prelude::{ContextActivity, ContextPriority, InputContext};
use jackdaw_scene_types::Tilemap;
use jackdaw_tilemap::{CellCoord, TileRef};
use jackdaw_tileset::Tileset;

use crate::tile_preview::TilePreviewGizmos;
use crate::tile_stamp::{StampBrush, TileStamp};
use crate::tile_tools::{
    BrushSource, MAX_FILL_CELLS, TileBrush, TileEditMode, TileStrokeState, TileTool, clipped_rect,
    has_tile_map, refuse, shown_tile,
};
use crate::tilemap::{TilemapDataStore, store_key_of};
use crate::tilemap_stroke::{TileWrite, TilemapStrokeBuilder};

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TileSelection>()
        .add_observer(on_context_added)
        .add_systems(
            Update,
            (
                drop_stale_selection.run_if(resource_changed::<TileEditMode>),
                follow_selection.run_if(resource_changed::<TileSelection>),
                outline_selection.run_if(in_state(crate::AppState::Editor)),
            )
                .chain(),
        );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext, tile_context: Entity) {
    ctx.register_operator::<TilemapSelectOp>()
        .register_operator::<TilemapSelectionSetOp>()
        .register_operator::<TilemapSelectionClearOp>()
        .register_operator::<TilemapSelectionEraseOp>()
        .register_operator::<TilemapSelectionCopyOp>()
        .register_operator::<TilemapSelectionCutOp>()
        .register_operator::<TilemapStampPasteOp>();
    ctx.bind_operator_in::<TilemapSelectOp>(tile_context, [PresetInput::key("KeyM")])
        .bind_operator_in::<TilemapStampPasteOp>(
            tile_context,
            [PresetInput::key("KeyV").ctrl_or_super()],
        );
    let context = ctx
        .spawn((
            TileSelectionInputContext,
            ContextPriority(6),
            ContextActivity::INACTIVE,
            Name::new("TileSelectionInputContext"),
            crate::EditorEntity,
        ))
        .id();
    ctx.bind_operator_in::<TilemapSelectionClearOp>(context, [PresetInput::key("Escape")])
        .bind_operator_in::<TilemapSelectionEraseOp>(context, [PresetInput::key("Delete")])
        .bind_operator_in::<TilemapSelectionCopyOp>(
            context,
            [PresetInput::key("KeyC").ctrl_or_super()],
        )
        .bind_operator_in::<TilemapSelectionCutOp>(
            context,
            [PresetInput::key("KeyX").ctrl_or_super()],
        );
}

/// The input context of the selection keys, active while cells are selected.
#[derive(Component, Default)]
#[require(InputContext)]
pub struct TileSelectionInputContext;

/// A rectangle of cells on one layer of a map, both corners included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileArea {
    pub map: Entity,
    pub layer: usize,
    pub min: CellCoord,
    pub max: CellCoord,
}

impl TileArea {
    /// The rectangle between corners `a` and `b`, clipped to `bounds`.
    fn between(
        map: Entity,
        layer: usize,
        a: CellCoord,
        b: CellCoord,
        bounds: Option<IRect>,
    ) -> Option<Self> {
        clipped_rect(a, b, bounds).map(|(min, max)| Self {
            map,
            layer,
            min: CellCoord(min),
            max: CellCoord(max),
        })
    }

    /// The cells row by row from the top, or why there are too many.
    fn cells(&self) -> Result<Vec<CellCoord>, String> {
        let span = (self.max.0.as_i64vec2() - self.min.0.as_i64vec2()).as_u64vec2() + 1;
        let count = span.x.saturating_mul(span.y);
        if count > MAX_FILL_CELLS as u64 {
            return Err(format!(
                "the selection holds {count} cells, more than {MAX_FILL_CELLS}"
            ));
        }
        let (min, max) = (self.min.0, self.max.0);
        Ok((min.y..=max.y)
            .rev()
            .flat_map(|y| (min.x..=max.x).map(move |x| CellCoord::new(x, y)))
            .collect())
    }
}

/// The selected cells in tile edit mode, and the drag selecting them.
#[derive(Resource, Default)]
pub struct TileSelection {
    pub area: Option<TileArea>,
    /// The press cell of a drag, and the selection it replaces.
    drag: Option<(CellCoord, Option<TileArea>)>,
}

/// A selection belongs to the map and layer tile edit mode paints.
fn drop_stale_selection(mode: Res<TileEditMode>, mut selection: ResMut<TileSelection>) {
    let stale = (selection.area).is_some_and(|a| mode.map != Some(a.map) || mode.layer != a.layer);
    if stale || (mode.map.is_none() && selection.drag.is_some()) {
        *selection = TileSelection::default();
    }
}

fn follow_selection(
    selection: Res<TileSelection>,
    mut contexts: Query<&mut ContextActivity, With<TileSelectionInputContext>>,
) {
    for mut activity in &mut contexts {
        activity.set_if_neq(ContextActivity(selection.area.is_some()));
    }
}

/// A context spawned after the selection was made starts in step with it.
fn on_context_added(
    event: On<Add<TileSelectionInputContext>>,
    selection: Option<Res<TileSelection>>,
    mut contexts: Query<&mut ContextActivity>,
) {
    let selected = selection.is_some_and(|s| s.area.is_some());
    if let Ok(mut activity) = contexts.get_mut(event.entity) {
        activity.set_if_neq(ContextActivity(selected));
    }
}

fn outline_selection(
    selection: Res<TileSelection>,
    maps: Query<(&GlobalTransform, &Tilemap)>,
    mut gizmos: Gizmos<TilePreviewGizmos>,
) {
    let Some(area) = selection.area else {
        return;
    };
    let Ok((transform, tilemap)) = maps.get(area.map) else {
        return;
    };
    let cell = tilemap.cell_size.as_vec2();
    let z = area.layer as f32 + 0.65;
    let at = |corner: IVec2| transform.transform_point((corner.as_vec2() * cell).extend(z));
    let (min, top) = (area.min.0, area.max.0.saturating_add(IVec2::ONE));
    let corners = [
        min,
        IVec2::new(top.x, min.y),
        top,
        IVec2::new(min.x, top.y),
        min,
    ];
    gizmos.linestrip(corners.map(at), jackdaw_feathers::tokens::TEXT_WARNING);
}

fn has_selection(
    mode: Res<TileEditMode>,
    selection: Res<TileSelection>,
    active: ActiveModalQuery,
) -> bool {
    (selection.area).is_some_and(|a| mode.map == Some(a.map) && mode.layer == a.layer)
        && !active.is_modal_running()
}

/// Select a rectangle of cells.
#[operator(
    id = "tilemap.select",
    label = "Tile Select",
    description = "Choose Select, or select the rectangle of cells between the press and release \
                   cells on the active layer; the secondary button clears the selection.",
    modal = true,
    allows_undo = false,
    is_available = has_tile_map,
    cancel = cancel_select
)]
pub(crate) fn tilemap_select(
    _: In<OperatorParameters>,
    world: &mut World,
) -> Option<OperatorResult> {
    let state = world.resource::<TileStrokeState>();
    let (Some(cell), Some(button)) = (state.cell, state.button) else {
        world.resource_mut::<TileEditMode>().tool = TileTool::Select;
        return Some(OperatorResult::Finished);
    };
    let mode = *world.resource::<TileEditMode>();
    let bounds = (mode.map.and_then(|map| world.get::<Tilemap>(map))).and_then(|t| t.bounds);
    let mut selection = world.resource_mut::<TileSelection>();
    let previous = selection.area;
    let (from, _) = *selection.drag.get_or_insert((cell, previous));
    selection.area = match mode.map {
        Some(map) if button == MouseButton::Left => {
            TileArea::between(map, mode.layer, from, cell, bounds)
        }
        _ => None,
    };
    if button == MouseButton::Left && world.resource::<ButtonInput<MouseButton>>().pressed(button) {
        return Some(OperatorResult::Running);
    }
    world.resource_mut::<TileSelection>().drag = None;
    *world.resource_mut::<TileStrokeState>() = TileStrokeState::default();
    Some(OperatorResult::Finished)
}

fn cancel_select(world: &mut World) {
    let mut selection = world.resource_mut::<TileSelection>();
    if let Some((_, previous)) = selection.drag.take() {
        selection.area = previous;
    }
    *world.resource_mut::<TileStrokeState>() = TileStrokeState::default();
}

fn can_set_selection(
    mode: Res<TileEditMode>,
    maps: Query<(), With<Tilemap>>,
    active: ActiveModalQuery,
) -> bool {
    mode.map.is_some_and(|map| maps.contains(map)) && !active.is_modal_running()
}

/// Select a rectangle of cells by its corners.
#[operator(
    id = "tilemap.selection.set",
    label = "Select Tiles",
    description = "Select the rectangle of cells between two corners on the active layer.",
    allows_undo = false,
    is_available = can_set_selection,
    params(rect(String, doc = "Corner cells as x0,y0,x1,y1, both included."))
)]
pub(crate) fn tilemap_selection_set(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    match set_selection(world, params.as_str("rect").unwrap_or_default()) {
        Ok(()) => OperatorResult::Finished,
        Err(err) => {
            warn_caller(world, format!("{}: {err}", TilemapSelectionSetOp::ID));
            OperatorResult::Cancelled
        }
    }
}

fn set_selection(world: &mut World, rect: &str) -> Result<(), String> {
    let numbers: Result<Vec<i32>, _> = rect.split(',').map(|n| n.trim().parse()).collect();
    let numbers = numbers.unwrap_or_default();
    let [x0, y0, x1, y1] = numbers[..] else {
        return Err(format!("rect is x0,y0,x1,y1, not `{rect}`"));
    };
    let mode = *world.resource::<TileEditMode>();
    let map = mode.map.ok_or("no tilemap is in tile edit mode")?;
    let bounds = world.get::<Tilemap>(map).and_then(|t| t.bounds);
    let (a, b) = (CellCoord::new(x0, y0), CellCoord::new(x1, y1));
    let area = TileArea::between(map, mode.layer, a, b, bounds)
        .ok_or("the rectangle is outside the map's bounds")?;
    world.resource_mut::<TileSelection>().area = Some(area);
    Ok(())
}

/// Clear the tile selection.
#[operator(
    id = "tilemap.selection.clear",
    label = "Clear Tile Selection",
    description = "Select no cells.",
    allows_undo = false,
    is_available = has_selection
)]
pub(crate) fn tilemap_selection_clear(
    _: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    world.resource_mut::<TileSelection>().area = None;
    OperatorResult::Finished
}

/// Erase the selected cells.
#[operator(
    id = "tilemap.selection.erase",
    label = "Erase Tile Selection",
    description = "Erase the selected cells, terrain and hand-placed tiles, as one undo entry.",
    allows_undo = false,
    is_available = has_selection
)]
pub(crate) fn tilemap_selection_erase(
    _: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    match erase(world, "Erase selection") {
        Ok(()) => OperatorResult::Finished,
        Err(err) => refuse(world, &err),
    }
}

fn erase(world: &mut World, label: &str) -> Result<(), String> {
    let area = (world.resource::<TileSelection>().area).ok_or("no cells are selected")?;
    let cells = area.cells()?;
    let mut builder = TilemapStrokeBuilder::begin(world, area.map, area.layer, label)?;
    builder.apply(world, &cells, TileWrite::Erase);
    builder.end(world);
    Ok(())
}

/// Copy the selected cells into a stamp.
#[operator(
    id = "tilemap.selection.copy",
    label = "Copy Tile Selection",
    description = "Load the tiles the selected cells show as a stamp and choose the Brush to \
                   paint it.",
    allows_undo = false,
    is_available = has_selection
)]
pub(crate) fn tilemap_selection_copy(
    _: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    match copy(world) {
        Ok(()) => OperatorResult::Finished,
        Err(err) => refuse(world, &err),
    }
}

/// The stamp of the tiles the selected cells show, and the tileset they come
/// from, read from the selection's own layer.
fn selected_stamp(world: &mut World) -> Result<(TileStamp, AssetId<Tileset>), String> {
    let area = (world.resource::<TileSelection>().area).ok_or("no cells are selected")?;
    let cells = area.cells()?;
    let path = (world.get::<Tilemap>(area.map))
        .and_then(|map| map.layers.get(area.layer))
        .map(|layer| layer.tileset.clone())
        .ok_or("the selected layer is gone")?;
    let tileset = crate::tileset_dock::tileset_id(world, &path)?;
    let key = store_key_of(world, area.map).ok_or("the map's cells are not loaded")?;
    let store = world.resource::<TilemapDataStore>();
    let layer = (store.get(&key)).and_then(|data| data.layers.get(area.layer));
    let top_left = IVec2::new(area.min.0.x, area.max.0.y);
    let tiles: Vec<(UVec2, TileRef)> = (cells.into_iter())
        .map(|cell| {
            let at = (cell.0.saturating_sub(top_left) * IVec2::new(1, -1)).as_uvec2();
            (at, shown_tile(layer, cell).0)
        })
        .filter(|(_, tile)| !tile.is_empty())
        .collect();
    if tiles.is_empty() {
        return Err("the selected cells hold no tiles".to_string());
    }
    let size = (area.max.0.saturating_sub(area.min.0) + IVec2::ONE).as_uvec2();
    Ok((TileStamp { size, cells: tiles }, tileset))
}

/// Loads `stamp` as the brush and chooses the Brush to paint it.
fn load_stamp(world: &mut World, stamp: TileStamp, tileset: AssetId<Tileset>) {
    *world.resource_mut::<StampBrush>() = StampBrush {
        stamp: Some(stamp),
        tileset: Some(tileset),
        ..default()
    };
    world.resource_mut::<TileBrush>().source = BrushSource::Stamp;
    world.resource_mut::<TileEditMode>().tool = TileTool::Brush;
}

fn copy(world: &mut World) -> Result<(), String> {
    let (stamp, tileset) = selected_stamp(world)?;
    load_stamp(world, stamp, tileset);
    Ok(())
}

/// Cut the selected cells into a stamp.
#[operator(
    id = "tilemap.selection.cut",
    label = "Cut Tile Selection",
    description = "Copy the selected cells into a stamp and erase them, as one undo entry.",
    allows_undo = false,
    is_available = has_selection
)]
pub(crate) fn tilemap_selection_cut(
    _: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let cut = selected_stamp(world).and_then(|(stamp, tileset)| {
        erase(world, "Cut selection")?;
        load_stamp(world, stamp, tileset);
        Ok(())
    });
    match cut {
        Ok(()) => OperatorResult::Finished,
        Err(err) => refuse(world, &err),
    }
}

fn has_loaded_stamp(mode: Res<TileEditMode>, stamp: Res<StampBrush>) -> bool {
    mode.map.is_some() && stamp.stamp.is_some()
}

/// Paint the loaded stamp.
#[operator(
    id = "tilemap.stamp.paste",
    label = "Paste Stamp",
    description = "Choose the Brush with the loaded stamp, to paint the copied cells.",
    allows_undo = false,
    is_available = has_loaded_stamp
)]
pub(crate) fn tilemap_stamp_paste(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    world.resource_mut::<TileBrush>().source = BrushSource::Stamp;
    world.resource_mut::<TileEditMode>().tool = TileTool::Brush;
    OperatorResult::Finished
}
