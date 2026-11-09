use std::collections::{BTreeSet, HashMap};

use bevy::picking::events::{
    PointerCancel, PointerDragEnd, PointerMove, PointerPress, PointerRelease,
};
use bevy::picking::pointer::{PointerButton, PointerId, PointerPressState};
use bevy::prelude::*;
use bevy::window::WindowFocused;
use jackdaw_api::prelude::*;
use jackdaw_feathers::tokens;
use jackdaw_tilemap::{Coverage, MatchMode, Peer, TerrainId, TileAddress, TileTerrains};
use jackdaw_tilemap::{SetupTile, TilesetSetup};
use jackdaw_tileset::Tileset;

use crate::tileset_dock::{
    DockAction, DockControl, DockView, PaintedCount, TilesetDockViews, dispatch, dock_tileset,
    refill, swatch,
};
use crate::tileset_edit::{
    peer, queue_edit, terrain_or_empty, terrain_set, text, tile, with_setup,
};
use crate::tileset_layout::GhostZones;
use crate::tileset_modes::{ModeId, ModeSheet};
use crate::zone_overlay::{
    ZoneLayout, ZoneOverlay, ZoneValues, redraw_zone_rows, spawn_zone_overlay,
};

/// On-screen size of a missing shape's glyph.
const GLYPH: f32 = 21.0;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<BitStroke>()
        .add_observer(on_press)
        .add_observer(on_move)
        .add_observer(
            |end: On<PointerRelease>, stroke: Res<BitStroke>, mut commands: Commands| {
                end_stroke(end.button, &stroke, &mut commands);
            },
        )
        .add_observer(
            |end: On<PointerDragEnd>, stroke: Res<BitStroke>, mut commands: Commands| {
                end_stroke(end.button, &stroke, &mut commands);
            },
        )
        .add_systems(
            Update,
            cancel_on_interruption.run_if(|stroke: Res<BitStroke>| stroke.0.is_some()),
        );
}

fn end_stroke(button: PointerButton, stroke: &BitStroke, commands: &mut Commands) {
    if stroke.0.as_ref().is_some_and(|s| s.button == button) {
        commands.queue(move |world: &mut World| finish(world, button));
    }
}

/// A cancelled pointer or a window losing focus drops the stroke unapplied.
fn cancel_on_interruption(
    mut focus: MessageReader<WindowFocused>,
    mut cancels: MessageReader<PointerCancel>,
    mut commands: Commands,
) {
    let lost = focus.read().any(|f| !f.focused);
    let cancelled = cancels.read().count() > 0;
    if lost || cancelled {
        commands.queue(|world: &mut World| {
            if let Some(stroke) = world.resource_mut::<BitStroke>().0.take() {
                refill(world, &stroke.path);
            }
        });
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TilesetTilePaintBitsOp>();
}

/// A shape the terrain set at index `set` has no tile for, drawn as a 3x3
/// glyph of its peers.
#[derive(Component, Clone, Copy, Debug)]
pub struct MissingGlyph {
    pub root: Entity,
    pub set: usize,
}

/// The coverage line of the terrain set at index `set`.
#[derive(Component, Clone, Copy, Debug)]
pub struct CoverageLine {
    pub root: Entity,
    pub set: usize,
}

/// The missing shape summary of the active terrain set at index `set`.
#[derive(Component, Clone, Copy, Debug)]
pub struct CoverageSummary {
    pub root: Entity,
    pub set: usize,
}

/// The row of missing shape glyphs of the terrain set at index `set`.
#[derive(Component, Clone, Copy, Debug)]
pub struct MissingShapes {
    pub root: Entity,
    pub set: usize,
}

/// The stroke a pointer is painting on a sheet, applied when it lifts.
#[derive(Resource, Default)]
struct BitStroke(Option<Stroke>);

struct Stroke {
    root: Entity,
    path: String,
    sheet: u32,
    set: usize,
    mode: MatchMode,
    terrain: TerrainId,
    button: PointerButton,
    tileset: Tileset,
    /// The setup with the stroke painted so far.
    setup: TilesetSetup,
    zones: BTreeSet<(u32, Peer)>,
    /// The last point painted, in sheet pixels.
    last: Vec2,
}

/// Paint one terrain on many tile zones as one history entry.
#[operator(
    id = "tileset.tile.paint_bits",
    writes_disk = true,
    label = "Paint Tile Bits",
    description = "Paint a terrain on zones of sheet tiles in one terrain set, as one undo \
                   entry; zones its match mode does not use and tiles of objects are left. \
                   Nothing changed adds no entry.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        set(String, doc = "Index or name of the terrain set."),
        terrain(String, doc = "Terrain to paint, one of the set's; empty erases."),
        zones(
            String,
            doc = "Comma-separated sheet:tile:peer, the peer 0 to 8 or top_left, top, ..., \
                   bottom_right."
        ),
    )
)]
pub fn tileset_tile_paint_bits(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let set = text(&params, "set");
    let (on, zones) = (text(&params, "terrain"), text(&params, "zones"));
    queue_edit(
        &mut commands,
        "tileset.tile.paint_bits",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let index = terrain_set(tileset, &set)?;
                let id = terrain_or_empty(tileset, &on)?;
                let found = setup.sets.get(index).ok_or("no such terrain set")?;
                if !found.has(id) {
                    return Err(format!(
                        "`{}` is not in the terrain set \"{}\"; add it with \
                         tileset.terrain_set.add_terrain",
                        tileset.terrain_name(id),
                        found.name
                    ));
                }
                let zones = zone_list(tileset, &zones)?;
                let changed =
                    (setup.paint_zones(index, id, &zones)).map_err(|e| setup.explain(e))?;
                Ok(format!("{changed} of {} zones painted", zones.len()))
            })
        },
    )
}

fn zone_list(tileset: &Tileset, text: &str) -> Result<Vec<(TileAddress, Peer)>, String> {
    (text.split(',').map(str::trim))
        .filter(|z| !z.is_empty())
        .map(|zone| {
            let (at, which) = (zone.rsplit_once(':')).ok_or(format!("`{zone}` is no zone"))?;
            let (sheet, index) = tile(tileset, at)?;
            let address = (tileset.setup_address(sheet, index)).map_err(|e| e.to_string())?;
            Ok((address, peer(which)?))
        })
        .collect()
}

/// The coverage line of the terrain set at index `set` and its coverage.
pub(crate) fn coverage_line(setup: &TilesetSetup, set: usize) -> Option<(String, Coverage)> {
    let coverage = setup.coverage(set).ok()?;
    let name = &setup.sets.get(set)?.name;
    let mode = mode_word(coverage.mode);
    let line = format!(
        "{name} ({mode}): {} of {} shapes",
        coverage.covered, coverage.total
    );
    Some((line, coverage))
}

/// How many shapes of its layout a set misses, with its mode.
pub(crate) fn summary_line(coverage: &Coverage) -> String {
    let mode = mode_word(coverage.mode);
    match coverage.missing.len() {
        0 => format!("{mode}: all {} shapes", coverage.total),
        missing => format!("{mode}: {missing} shapes missing"),
    }
}

/// A glyph in `row` for each shape `coverage` misses.
pub(crate) fn spawn_missing(
    world: &mut World,
    row: Entity,
    shapes: MissingShapes,
    tileset: &Tileset,
    coverage: &Coverage,
) {
    let MissingShapes { root, set } = shapes;
    for shape in &coverage.missing {
        let glyph = MissingGlyph { root, set };
        spawn_glyph(world, row, glyph, tileset, coverage.mode, shape);
    }
}

/// A match mode as the dock names it.
pub(crate) fn mode_word(mode: MatchMode) -> &'static str {
    match mode {
        MatchMode::Corners => "Corners",
        MatchMode::Sides => "Sides",
        MatchMode::CornersAndSides => "Corners and sides",
    }
}

/// How many tiles of `setup` have painted bits, as the dock header says it.
pub(crate) fn painted_count(setup: &TilesetSetup) -> String {
    let count = (setup.tiles.values())
        .filter(|t| t.terrains.peers.iter().any(|p| *p != 0))
        .count();
    match count {
        1 => "1 tile painted".to_string(),
        n => format!("{n} tiles painted"),
    }
}

/// A missing shape as a 3x3 glyph of the peers `mode` matches on.
pub(crate) fn spawn_glyph(
    world: &mut World,
    parent: Entity,
    glyph: MissingGlyph,
    tileset: &Tileset,
    mode: MatchMode,
    shape: &TileTerrains,
) {
    let node = world
        .spawn((
            Node {
                width: px(GLYPH),
                height: px(GLYPH),
                display: Display::Grid,
                grid_template_columns: RepeatedGridTrack::flex(3, 1.0),
                grid_template_rows: RepeatedGridTrack::flex(3, 1.0),
                border: UiRect::all(px(1.0)),
                flex_shrink: 0.0,
                ..default()
            },
            BorderColor::all(tokens::BORDER_SUBTLE),
            glyph,
            ChildOf(parent),
        ))
        .id();
    for peer in Peer::ALL {
        let color = if mode.reads(peer) {
            swatch(tileset, shape.get(peer), 1.0)
        } else {
            Color::NONE
        };
        world.spawn((Node::default(), BackgroundColor(color), ChildOf(node)));
    }
}

/// The active set's painted zones over a sheet of the terrains mode.
pub(crate) fn terrain_overlay(world: &mut World, sheet: &ModeSheet) {
    let ModeSheet {
        root,
        frame,
        sheet,
        tileset,
        setup,
        view,
    } = *sheet;
    if let Some(values) = terrain_zones(tileset, setup, sheet, view) {
        spawn_zone_overlay(world, frame, root, (tileset, sheet), values);
    }
}

/// Whether the overlay of set `active` shows `tile`: the set holds it, or
/// every terrain painted on it is one of the set's.
fn shows(setup: &TilesetSetup, active: usize, tile: &SetupTile) -> bool {
    let Some(set) = setup.sets.get(active) else {
        return false;
    };
    let painted = tile.terrains.peers.iter().filter(|id| **id != 0);
    setup.holds(active, tile)
        || (painted.clone().count() > 0 && painted.into_iter().all(|id| set.terrains.contains(id)))
}

/// The zones of sheet `sheet` held by tiles of the active set, the brush
/// terrain picked.
fn terrain_zones(
    tileset: &Tileset,
    setup: &TilesetSetup,
    sheet: u32,
    view: &DockView,
) -> Option<ZoneValues> {
    let on = tileset.setup_sheet(sheet).ok()?;
    let active = view.active_set(&tileset.sets())?;
    let set = setup.sets.get(active)?;
    let mode = set.mode.value;
    let tiles = (setup.tiles.iter())
        .filter(|(a, tile)| a.sheet == on && shows(setup, active, tile))
        .map(|(address, tile)| {
            let peers = Peer::ALL.map(|peer| {
                let read = mode.reads(peer);
                if read {
                    u32::from(tile.terrains.get(peer))
                } else {
                    0
                }
            });
            (address.tile, peers)
        })
        .collect();
    let colors: HashMap<u32, Color> = (set.terrains.iter())
        .map(|id| (u32::from(*id), swatch(tileset, *id, 1.0)))
        .collect();
    let brush = (view.brush != 0 && set.has(view.brush)).then_some(u32::from(view.brush));
    let hover = brush.and_then(|b| colors.get(&b).copied());
    Some(ZoneValues {
        layout: ZoneLayout::Peers(mode),
        tiles,
        picked: brush,
        colors,
        dim: true,
        hover: Some(hover.unwrap_or(Color::NONE)),
    })
}

/// The dock, frame, sheet tile and point in it, from (0, 0) at its top left
/// to (1, 1), of a pointer event on a sheet tile.
type TileHit = (Entity, Entity, u32, u32, Vec2);

fn tile_hit(
    entity: Entity,
    position: Option<Vec3>,
    controls: &Query<(&DockControl, &ChildOf)>,
) -> Option<TileHit> {
    let (control, parent) = controls.get(entity).ok()?;
    let DockAction::Tile(sheet, tile) = control.action else {
        return None;
    };
    let inside = position?.truncate() + 0.5;
    Some((control.root, parent.parent(), sheet, tile, inside))
}

fn on_press(
    press: On<PointerPress>,
    controls: Query<(&DockControl, &ChildOf)>,
    stroke: Res<BitStroke>,
    mut commands: Commands,
) {
    if stroke.0.is_some() {
        return;
    }
    if let Some(hit) = tile_hit(press.entity, press.hit.position, &controls) {
        let button = press.button;
        commands.queue(move |world: &mut World| begin(world, hit, button));
    }
}

fn on_move(
    moved: On<PointerMove>,
    controls: Query<(&DockControl, &ChildOf)>,
    pointers: Query<(&PointerId, &PointerPressState)>,
    stroke: Res<BitStroke>,
    mut commands: Commands,
) {
    let Some(button) = stroke.0.as_ref().map(|s| s.button) else {
        return;
    };
    let held = (pointers.iter())
        .find(|(id, _)| **id == moved.pointer.id)
        .is_some_and(|(_, state)| match button {
            PointerButton::Primary => state.is_primary_pressed(),
            PointerButton::Secondary => state.is_secondary_pressed(),
            PointerButton::Middle => state.is_middle_pressed(),
        });
    if !held {
        return;
    }
    if let Some(hit) = tile_hit(moved.entity, moved.hit.position, &controls) {
        commands.queue(move |world: &mut World| extend(world, hit));
    }
}

/// Start a stroke in a dock that paints: right erases, left paints the brush.
fn begin(world: &mut World, hit: TileHit, button: PointerButton) {
    let (root, _, sheet, tile, inside) = hit;
    if world.resource::<BitStroke>().0.is_some() {
        return;
    }
    let Some((path, tileset)) = dock_tileset(world, root) else {
        return;
    };
    let Some(view) = world.resource::<TilesetDockViews>().0.get(&path).cloned() else {
        return;
    };
    let terrain = match button {
        PointerButton::Primary => view.brush,
        PointerButton::Secondary => 0,
        PointerButton::Middle => return,
    };
    if view.mode != ModeId::TERRAINS {
        return;
    }
    let Ok(setup) = tileset.setup() else {
        return;
    };
    let set = (view.active_set(&tileset.sets()))
        .filter(|s| setup.sets.get(*s).is_some_and(|s| s.has(terrain)));
    let Some(mode) = set.and_then(|s| setup.sets.get(s)).map(|s| s.mode.value) else {
        if terrain != 0 {
            let name = tileset.terrain_name(terrain);
            let notice = format!("no terrain set holds {name}; add one to paint its bits");
            jackdaw_editor_core::status_bar::notify_info(world, notice);
        }
        return;
    };
    let Some((origin, size)) = tileset.sheet_rect(sheet, tile) else {
        return;
    };
    let stroke = Stroke {
        root,
        path,
        sheet,
        set: set.unwrap_or_default(),
        mode,
        terrain,
        button,
        tileset,
        setup,
        zones: BTreeSet::new(),
        last: origin.as_vec2() + inside * size.as_vec2(),
    };
    world.resource_mut::<BitStroke>().0 = Some(stroke);
    extend(world, hit);
}

/// Paint every zone on the line from the stroke's last point to the hit.
fn extend(world: &mut World, (root, frame, sheet, tile, inside): TileHit) {
    let Some(mut stroke) = world.resource_mut::<BitStroke>().0.take() else {
        return;
    };
    if let Some((origin, size)) = (stroke.tileset.sheet_rect(sheet, tile))
        .filter(|_| stroke.root == root && stroke.sheet == sheet)
    {
        let at = origin.as_vec2() + inside * size.as_vec2();
        let step = size.as_vec2().min_element() / 8.0;
        let steps = (stroke.last.distance(at) / step.max(0.5)).ceil().max(1.0) as u32;
        let mut fresh = Vec::new();
        for i in 0..=steps {
            let point = stroke.last.lerp(at, i as f32 / steps as f32);
            let zone = stroke.tileset.zone_at(sheet, point, stroke.mode);
            if let Some(zone) = zone.filter(|z| stroke.zones.insert(*z)) {
                fresh.push(zone);
            }
        }
        stroke.last = at;
        show(world, &mut stroke, frame, &fresh);
    }
    world.resource_mut::<BitStroke>().0 = Some(stroke);
}

/// Paint the new zones into the stroke's setup, draw them over `frame` and
/// show the active set's coverage as the stroke would leave it.
fn show(world: &mut World, stroke: &mut Stroke, frame: Entity, fresh: &[(u32, Peer)]) {
    let Ok(on) = stroke.tileset.setup_sheet(stroke.sheet) else {
        return;
    };
    if fresh.is_empty() || world.get_entity(frame).is_err() {
        return;
    }
    let zones: Vec<(TileAddress, Peer)> = (fresh.iter())
        .map(|(tile, peer)| {
            (
                TileAddress {
                    sheet: on,
                    tile: *tile,
                },
                *peer,
            )
        })
        .collect();
    if (stroke.setup)
        .paint_zones(stroke.set, stroke.terrain, &zones)
        .is_err()
    {
        return;
    }
    let root = stroke.root;
    redraw(world, stroke, frame, fresh);
    let count = painted_count(&stroke.setup);
    if let Ok(mut held) = world.get_entity_mut(root)
        && let Some(mut shown) = held.get_mut::<PaintedCount>()
    {
        shown.0 = count;
    }
    let Some((text, coverage)) = coverage_line(&stroke.setup, stroke.set) else {
        return;
    };
    let set = stroke.set;
    retext::<CoverageLine>(world, |l| l.root == root && l.set == set, &text);
    let summary = summary_line(&coverage);
    retext::<CoverageSummary>(world, |l| l.root == root && l.set == set, &summary);
    let rows: Vec<(Entity, MissingShapes)> = (world.query::<(Entity, &MissingShapes)>())
        .iter(world)
        .filter(|(_, m)| m.root == root && m.set == set)
        .map(|(e, m)| (e, *m))
        .collect();
    for (row, shapes) in rows {
        world.entity_mut(row).despawn_related::<Children>();
        spawn_missing(world, row, shapes, &stroke.tileset, &coverage);
    }
}

/// Draw the tile rows of the stroke's sheet holding the `fresh` zones again
/// from the stroke's setup. The whole sheet is drawn again once the stroke
/// applies.
fn redraw(world: &mut World, stroke: &Stroke, frame: Entity, fresh: &[(u32, Peer)]) {
    let Some(view) = (world.resource::<TilesetDockViews>().0.get(&stroke.path)).cloned() else {
        return;
    };
    let overlay = (world.query_filtered::<(Entity, &ZoneOverlay, &ChildOf), Without<GhostZones>>())
        .iter(world)
        .find(|(_, o, parent)| o.root == stroke.root && parent.parent() == frame)
        .map(|(e, ..)| e);
    let Some(overlay) = overlay else {
        let sheet = ModeSheet {
            root: stroke.root,
            frame,
            sheet: stroke.sheet,
            tileset: &stroke.tileset,
            setup: &stroke.setup,
            view: &view,
        };
        terrain_overlay(world, &sheet);
        return;
    };
    let Some(values) = terrain_zones(&stroke.tileset, &stroke.setup, stroke.sheet, &view) else {
        return;
    };
    let columns =
        (stroke.tileset.sheets.get(stroke.sheet as usize)).map_or(1, |s| s.columns.max(1));
    let rows: BTreeSet<u32> = fresh.iter().map(|(tile, _)| tile / columns).collect();
    redraw_zone_rows(world, overlay, values, rows);
}

/// Show `text` in each text carrying a `C` that `pick` takes.
fn retext<C: Component>(world: &mut World, pick: impl Fn(&C) -> bool, text: &str) {
    let found: Vec<Entity> = (world.query::<(Entity, &C)>().iter(world))
        .filter(|(_, c)| pick(c))
        .map(|(e, _)| e)
        .collect();
    for entity in found {
        if let Some(mut node) = world.get_mut::<Text>(entity)
            && node.0 != text
        {
            text.clone_into(&mut node.0);
        }
    }
}

/// Apply the stroke `button` paints as one edit.
fn finish(world: &mut World, button: PointerButton) {
    let mut strokes = world.resource_mut::<BitStroke>();
    let Some(stroke) = strokes.0.take_if(|s| s.button == button) else {
        return;
    };
    let zones: Vec<String> = (stroke.zones.iter())
        .map(|(tile, peer)| format!("{}:{tile}:{}", stroke.sheet, peer.index()))
        .collect();
    let call = (!zones.is_empty()).then(|| {
        let params = vec![
            ("set", stroke.set.to_string()),
            ("terrain", stroke.terrain.to_string()),
            ("zones", zones.join(",")),
        ];
        ("tileset.tile.paint_bits", params)
    });
    dispatch(world, stroke.root, &stroke.path, &stroke.tileset, call);
}
