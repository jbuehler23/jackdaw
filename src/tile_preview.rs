//! What the tile tools would change before they change it: the tiles a press,
//! or the shape being dragged, writes are drawn already resolved and opaque
//! over the map, cells it would empty are tinted, and an outline runs round
//! the cells it writes. The status bar reads out the hovered cell. The
//! preview is built again only when the hover cell, Alt, the dragged shape,
//! the brush, the mode, the map's cells or the layer's tileset change.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use bevy::{
    picking::events::{PointerMove, PointerOut},
    prelude::*,
    ui::{ComputedNode, UiGlobalTransform},
};
use jackdaw_scene_types::Tilemap;
use jackdaw_tilemap::{CHUNK_AREA, CellCoord, ChunkCoord, LayerData, TileRef};
use jackdaw_tileset::Tileset;
use jackdaw_tileset::chunk::ChunkDraw;

use crate::tile_stamp::StampBrush;
use crate::tile_tools::{
    Paint, TileBrush, TileEditMode, TileStrokeState, TileTool, active_layer, active_tileset,
    shape_cells, stage_cell, stroke_paint,
};
use crate::tilemap::TilemapDataStore;
use crate::tilemap_stroke::{in_bounds, preview_write};
use crate::viewport_2d::{Scene2dViewport, Viewport2dPanelHost};

/// The most cells a preview lists and resolves; a larger shape is outlined
/// by its corners or end cells only.
pub const MAX_PREVIEW_CELLS: usize = 1 << 14;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TileHover>()
        .init_resource::<TilePreview>()
        .init_resource::<TileHoverReadout>()
        .init_resource::<TilesetRevision>()
        .init_gizmo_group::<TilePreviewGizmos>()
        .register_required_components::<TilePreviewChunk, crate::EditorHidden>()
        .register_required_components::<TilePreviewChunk, crate::NonSerializable>()
        .register_required_components::<TilePreviewEmptied, crate::EditorHidden>()
        .register_required_components::<TilePreviewEmptied, crate::NonSerializable>()
        .add_observer(on_hover_move)
        .add_observer(on_hover_out)
        .add_systems(
            Update,
            (
                (track_alt, note_tileset_events),
                build_preview.run_if(preview_inputs_changed),
                draw_preview.run_if(resource_changed::<TilePreview>),
                outline_preview,
            )
                .chain()
                .run_if(in_state(crate::AppState::Editor)),
        );
}

/// The map cell under the pointer in tile edit mode, and whether Alt, which
/// picks, is held.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub struct TileHover {
    pub cell: Option<CellCoord>,
    pub alt: bool,
}

/// The hovered cell as the status bar reads it out.
#[derive(Resource, Default, Clone, PartialEq, Eq)]
pub struct TileHoverReadout(pub Option<String>);

/// Advances on every load, change or unload of the previewed tileset.
#[derive(Resource, Default)]
struct TilesetRevision(u64);

/// One chunk of previewed tiles, drawn over its map's layer.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub struct TilePreviewChunk {
    pub layer: usize,
    pub chunk: ChunkCoord,
}

/// A tinted run of cells the preview would leave empty.
#[derive(Component)]
pub struct TilePreviewEmptied;

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct TilePreviewGizmos;

/// What the active tool would write at the hover cell, or across the shape
/// being dragged.
#[derive(Resource, Default)]
pub struct TilePreview {
    pub map: Option<Entity>,
    pub layer: usize,
    /// The cells the tool writes; empty for a shape past
    /// [`MAX_PREVIEW_CELLS`].
    pub cells: Vec<CellCoord>,
    /// Each cell whose tile would change, with that tile, resolved.
    pub tiles: Vec<(CellCoord, TileRef)>,
    /// The outline, as segments between cell corners.
    pub edges: Vec<(IVec2, IVec2)>,
    /// Times the preview was built.
    pub builds: usize,
    tileset: Option<AssetId<Tileset>>,
    built_from: Option<PreviewInputs>,
}

/// The cells a preview covers, before they are listed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Cell(Option<CellCoord>),
    Drag(TileTool, CellCoord, CellCoord),
}

#[derive(Clone, PartialEq)]
struct PreviewInputs {
    map: Entity,
    layer: usize,
    shape: Shape,
    paint: Option<Paint>,
    hover: Option<CellCoord>,
    cells_revision: u64,
    tileset_revision: u64,
    set: Option<usize>,
}

fn preview_inputs_changed(
    hover: Res<TileHover>,
    state: Res<TileStrokeState>,
    brush: Res<TileBrush>,
    mode: Res<TileEditMode>,
    store: Res<TilemapDataStore>,
    tileset: Res<TilesetRevision>,
    stamp: Res<StampBrush>,
) -> bool {
    stamp.is_changed()
        || hover.is_changed()
        || state.is_changed()
        || brush.is_changed()
        || mode.is_changed()
        || store.is_changed()
        || tileset.is_changed()
}

fn track_alt(keys: Res<ButtonInput<KeyCode>>, mut hover: ResMut<TileHover>) {
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    if hover.alt != alt {
        hover.alt = alt;
    }
}

fn note_tileset_events(
    mut events: MessageReader<AssetEvent<Tileset>>,
    preview: Res<TilePreview>,
    mut revision: ResMut<TilesetRevision>,
) {
    let ours = |event: &AssetEvent<Tileset>| match *event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::Unused { id }
        | AssetEvent::LoadedWithDependencies { id } => preview.tileset.is_none_or(|t| t == id),
    };
    if events.read().filter(|e| ours(e)).count() > 0 {
        revision.0 += 1;
    }
}

/// A move over a 2D panel's stage hovers the map cell under it, or nothing
/// when the panel cannot paint the map.
fn on_hover_move(
    event: On<PointerMove>,
    ui_scale: Res<UiScale>,
    mode: Res<TileEditMode>,
    hosts: Query<&Viewport2dPanelHost>,
    stages: Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dViewport>>,
    maps: Query<(&GlobalTransform, &Tilemap)>,
    mut hover: ResMut<TileHover>,
) {
    let target = event.event_target();
    if !hosts.iter().any(|host| host.stage == target) {
        return;
    }
    let position = event.pointer.position / ui_scale.0;
    let cell = stage_cell(target, position, &mode, &hosts, &stages, &maps)
        .and_then(|(cell, inside)| inside.then_some(cell));
    if hover.cell != cell {
        hover.cell = cell;
    }
}

fn on_hover_out(
    event: On<PointerOut>,
    hosts: Query<&Viewport2dPanelHost>,
    mut hover: ResMut<TileHover>,
) {
    if hover.cell.is_some() && hosts.iter().any(|host| host.stage == event.event_target()) {
        hover.cell = None;
    }
}

/// The shape the active tool covers and what it writes there: the dragged
/// shape, or the hover cell before a press. A Brush or Eraser stroke writes
/// as it goes, and the Bucket, the Picker and a held Alt act on a press, so
/// they only outline the hover cell.
fn shape_and_write(world: &mut World, hover: TileHover) -> (Shape, Option<Paint>) {
    let mode = *world.resource::<TileEditMode>();
    let state = world.resource::<TileStrokeState>();
    let (held, drag) = (state.held(), state.cell);
    if let Some((paint, Some(from))) = held
        && mode.tool.is_shape()
        && let Some(to) = drag
    {
        return (Shape::Drag(mode.tool, from, to), Some(paint.clone()));
    }
    let paint = match mode.tool {
        _ if held.is_some() || hover.alt || hover.cell.is_none() => None,
        TileTool::Bucket | TileTool::Picker => None,
        tool => stroke_paint(world, tool, MouseButton::Left).ok(),
    };
    (Shape::Cell(hover.cell), paint)
}

/// The cells of `shape` inside `bounds`, unless there are more than
/// [`MAX_PREVIEW_CELLS`], and its outline.
fn footprint(shape: Shape, bounds: Option<IRect>) -> (Vec<CellCoord>, Vec<(IVec2, IVec2)>) {
    let (tool, from, to) = match shape {
        Shape::Cell(cell) => {
            let cells = in_bounds(bounds, &Vec::from_iter(cell));
            let edges = outline(&cells);
            return (cells, edges);
        }
        Shape::Drag(tool, from, to) => (tool, from, to),
    };
    if tool == TileTool::Rectangle {
        let (mut min, mut max) = (from.0.min(to.0), from.0.max(to.0));
        if let Some(b) = bounds {
            min = min.max(b.min);
            max = max.min(b.max.saturating_sub(IVec2::ONE));
        }
        if min.cmpgt(max).any() {
            return (Vec::new(), Vec::new());
        }
        let span = (max.as_i64vec2() - min.as_i64vec2()).as_u64vec2() + 1;
        let cells = if span.x.saturating_mul(span.y) <= MAX_PREVIEW_CELLS as u64 {
            shape_cells(tool, from, to, bounds).unwrap_or_default()
        } else {
            Vec::new()
        };
        let top = max.saturating_add(IVec2::ONE);
        let edges = vec![
            (min, IVec2::new(top.x, min.y)),
            (IVec2::new(top.x, min.y), top),
            (top, IVec2::new(min.x, top.y)),
            (IVec2::new(min.x, top.y), min),
        ];
        return (cells, edges);
    }
    let span = from.0.as_i64vec2() - to.0.as_i64vec2();
    if span.x.unsigned_abs() + span.y.unsigned_abs() < MAX_PREVIEW_CELLS as u64 {
        let line = shape_cells(tool, from, to, bounds).unwrap_or_default();
        let cells = in_bounds(bounds, &line);
        let edges = outline(&cells);
        return (cells, edges);
    }
    (Vec::new(), outline(&in_bounds(bounds, &[from, to])))
}

fn build_preview(world: &mut World) {
    let hover = *world.resource::<TileHover>();
    let Ok(active) = active_layer(world) else {
        if hover.cell.is_some() {
            world.resource_mut::<TileHover>().cell = None;
        }
        world
            .resource_mut::<TileHoverReadout>()
            .set_if_neq(TileHoverReadout(None));
        if world.resource::<TilePreview>().map.is_some() {
            *world.resource_mut::<TilePreview>() = TilePreview::default();
        }
        return;
    };
    let (shape, paint) = shape_and_write(world, hover);
    let store = world.resource::<TilemapDataStore>();
    let set = store.resolved_with(&active.key, active.index).cloned();
    let inputs = PreviewInputs {
        map: active.map,
        layer: active.index,
        shape,
        paint,
        hover: hover.cell,
        cells_revision: store.revision(),
        tileset_revision: world.resource::<TilesetRevision>().0,
        set: set.as_ref().map(|set| std::sync::Arc::as_ptr(set) as usize),
    };
    if world.resource::<TilePreview>().built_from.as_ref() == Some(&inputs) {
        return;
    }
    let bounds = world.get::<Tilemap>(active.map).and_then(|t| t.bounds);
    let (mut cells, mut edges) = footprint(shape, bounds);
    let tileset_id = active_tileset(world).ok();
    let store = world.resource::<TilemapDataStore>();
    let layer = (store.get(&active.key)).and_then(|data| data.layers.get(active.index));
    let mut tiles = Vec::new();
    if let Some(paint) = inputs.paint.as_ref().filter(|_| !cells.is_empty()) {
        let writes = paint.writes(&cells);
        if let Paint::Stamp(_) = paint {
            cells = (writes.iter())
                .flat_map(|(_, c)| in_bounds(bounds, c))
                .collect();
            cells.sort_unstable();
            edges = outline(&cells);
        }
        for (write, c) in writes {
            let c = in_bounds(bounds, &c);
            tiles.extend(preview_write(
                layer,
                active.terrain,
                set.as_deref(),
                &c,
                write,
            ));
        }
    }
    let tileset = tileset_id.and_then(|id| world.resource::<Assets<Tileset>>().get(id));
    let readout = hover
        .cell
        .map(|cell| hover_readout(cell, layer, active.terrain, tileset));
    world
        .resource_mut::<TileHoverReadout>()
        .set_if_neq(TileHoverReadout(readout));
    let mut preview = world.resource_mut::<TilePreview>();
    let builds = preview.builds + 1;
    *preview = TilePreview {
        map: Some(active.map),
        layer: active.index,
        cells,
        tiles,
        edges,
        builds,
        tileset: tileset_id,
        built_from: Some(inputs),
    };
}

/// The status bar line for `cell` of `layer`: its terrain on a terrain
/// layer, its tile by name or as sheet:index, and whether it is hand-placed.
pub fn hover_readout(
    cell: CellCoord,
    layer: Option<&LayerData>,
    terrain_layer: bool,
    tileset: Option<&Tileset>,
) -> String {
    let authoring = layer.and_then(|l| l.authoring.as_ref());
    let mut parts = vec![format!("Cell {},{}", cell.0.x, cell.0.y)];
    if terrain_layer {
        let name = match authoring.map_or(0, |a| a.terrain.get(cell)) {
            0 => "none".to_string(),
            id => (tileset.and_then(|t| t.terrains.iter().find(|t| t.id == id)))
                .map_or_else(|| id.to_string(), |t| t.name.clone()),
        };
        parts.push(format!("Terrain {name}"));
    }
    let placed = authoring.and_then(|a| a.overrides.get(&cell).copied());
    let tile = placed.unwrap_or_else(|| layer.map_or(TileRef::EMPTY, |l| l.tiles.get(cell)));
    let index = u32::from(tile.index());
    let name = if tile.is_empty() {
        "none".to_string()
    } else {
        (tileset.and_then(|t| t.properties.get(&index)))
            .and_then(|properties| properties.get("name").cloned())
            .or_else(|| {
                (tileset.and_then(|t| t.tile_of_layer(index)))
                    .map(|(sheet, at)| format!("{sheet}:{at}"))
            })
            .unwrap_or_else(|| index.to_string())
    };
    parts.push(format!("Tile {name}"));
    if placed.is_some() {
        parts.push("Hand-placed".to_string());
    }
    parts.join(" | ")
}

/// Runs of consecutive keys `(line, at)` as `(line, first, last)`.
fn runs(units: BTreeSet<(i32, i32)>) -> Vec<(i32, i32, i32)> {
    let mut runs: Vec<(i32, i32, i32)> = Vec::new();
    for (line, at) in units {
        match runs.last_mut() {
            Some(run) if run.0 == line && run.2.checked_add(1) == Some(at) => run.2 = at,
            _ => runs.push((line, at, at)),
        }
    }
    runs
}

/// The boundary of `cells` as segments between cell corners, with runs
/// along one line merged.
fn outline(cells: &[CellCoord]) -> Vec<(IVec2, IVec2)> {
    let inside: HashSet<IVec2> = cells.iter().map(|c| c.0).collect();
    let open = |cell: IVec2, step: IVec2| !inside.contains(&cell.saturating_add(step));
    let (mut rows, mut columns) = (BTreeSet::new(), BTreeSet::new());
    for &cell in &inside {
        for (step, line) in [(IVec2::NEG_Y, cell.y), (IVec2::Y, cell.y.saturating_add(1))] {
            if open(cell, step) {
                rows.insert((line, cell.x));
            }
        }
        for (step, line) in [(IVec2::NEG_X, cell.x), (IVec2::X, cell.x.saturating_add(1))] {
            if open(cell, step) {
                columns.insert((line, cell.y));
            }
        }
    }
    let end = |at: i32| at.saturating_add(1);
    let mut edges: Vec<(IVec2, IVec2)> = (runs(rows).into_iter())
        .map(|(y, a, b)| (IVec2::new(a, y), IVec2::new(end(b), y)))
        .collect();
    edges.extend(
        (runs(columns).into_iter()).map(|(x, a, b)| (IVec2::new(x, a), IVec2::new(x, end(b)))),
    );
    edges
}

/// Brings the preview chunks and the tinted runs of emptied cells in line
/// with the preview, updating the chunks it keeps in place.
fn draw_preview(world: &mut World) {
    let preview = world.resource::<TilePreview>();
    let (map, layer) = (preview.map, preview.layer);
    let mut chunks: BTreeMap<ChunkCoord, Box<[TileRef; CHUNK_AREA]>> = BTreeMap::new();
    let mut emptied = BTreeSet::new();
    for &(cell, tile) in &preview.tiles {
        if tile.is_empty() {
            emptied.insert((cell.0.y, cell.0.x));
            continue;
        }
        let cells =
            (chunks.entry(cell.chunk())).or_insert_with(|| Box::new([TileRef::EMPTY; CHUNK_AREA]));
        cells[cell.local_index()] = tile;
    }
    let tileset = active_tileset(world).ok();
    let tilemap = map.and_then(|map| world.get::<Tilemap>(map));
    let cell_size = tilemap.map_or(UVec2::ONE, |t| t.cell_size);
    let tileset = tileset.and_then(|id| world.resource::<Assets<Tileset>>().get(id));
    let draw = tileset.filter(|t| !t.image.is_empty()).map(|tileset| {
        let image = world.resource::<AssetServer>().load(&tileset.image);
        (
            ChunkDraw::new(tileset, image),
            tileset.display_size(cell_size),
        )
    });
    let mut wanted: BTreeMap<ChunkCoord, crate::tilemap_render::Wanted> = BTreeMap::new();
    if let Some((draw, tile_size)) = &draw {
        for (coord, cells) in chunks {
            let mut bundle = draw.bundle(coord, &cells, *tile_size, layer);
            bundle.2.translation.z += 0.5;
            wanted.insert(coord, bundle);
        }
    }
    let drawn: Vec<(Entity, TilePreviewChunk, Entity)> =
        (world.query::<(Entity, &TilePreviewChunk, &ChildOf)>())
            .iter(world)
            .map(|(entity, of, parent)| (entity, *of, parent.parent()))
            .collect();
    for (entity, of, parent) in drawn {
        match wanted.remove(&of.chunk) {
            Some(next) if Some(parent) == map && of.layer == layer => {
                crate::tilemap_render::update_chunk(world, entity, next);
            }
            next => {
                if let Some(next) = next {
                    wanted.insert(of.chunk, next);
                }
                world.entity_mut(entity).despawn();
            }
        }
    }
    let old: Vec<Entity> = (world.query_filtered::<Entity, With<TilePreviewEmptied>>())
        .iter(world)
        .collect();
    for entity in old {
        world.entity_mut(entity).despawn();
    }
    let Some(map) = map else {
        return;
    };
    for (chunk, bundle) in wanted {
        world.spawn((TilePreviewChunk { layer, chunk }, bundle, ChildOf(map)));
    }
    let size = cell_size.as_vec2();
    let tint = jackdaw_feathers::tokens::TEXT_ERROR.with_alpha(0.45);
    for (y, a, b) in runs(emptied) {
        let width = (b - a + 1) as f32 * size.x;
        let centre = Vec2::new(a as f32 * size.x + width / 2.0, (y as f32 + 0.5) * size.y);
        world.spawn((
            TilePreviewEmptied,
            Sprite::from_color(tint, Vec2::new(width, size.y)),
            Transform::from_translation(centre.extend(layer as f32 + 0.55)),
            ChildOf(map),
        ));
    }
}

fn outline_preview(
    preview: Res<TilePreview>,
    maps: Query<(&GlobalTransform, &Tilemap)>,
    mut gizmos: Gizmos<TilePreviewGizmos>,
) {
    let Some((transform, tilemap)) = preview.map.and_then(|map| maps.get(map).ok()) else {
        return;
    };
    let cell = tilemap.cell_size.as_vec2();
    let z = preview.layer as f32 + 0.6;
    let at = |corner: IVec2| transform.transform_point((corner.as_vec2() * cell).extend(z));
    let color = jackdaw_feathers::tokens::ACCENT_BLUE;
    for &(from, to) in &preview.edges {
        gizmos.line(at(from), at(to), color);
    }
}
