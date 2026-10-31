use std::collections::BTreeSet;

use bevy::picking::events::{
    PointerCancel, PointerDragEnd, PointerMove, PointerPress, PointerRelease,
};
use bevy::picking::pointer::{PointerButton, PointerId, PointerPressState};
use bevy::prelude::*;
use bevy::window::WindowFocused;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_feathers::tokens;
use jackdaw_tilemap::TilesetSetup;
use jackdaw_tilemap::{Coverage, MatchMode, Peer, TerrainId, TileAddress, TileTerrains};
use jackdaw_tileset::Tileset;

use crate::tileset_dock::{
    BitsOverlay, DockAction, DockControl, DockPage, DockTool, DockView, TilesetDockViews, dispatch,
    dock_path, dock_tileset, refill, swatch, zoom,
};
use crate::tileset_edit::{peer, queue_edit, terrain, terrain_or_empty, text, tile, with_setup};

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

/// A painted zone the bits overlay draws on sheet tile `tile`.
#[derive(Component, Clone, Copy, Debug)]
pub struct BitMark {
    pub root: Entity,
    pub sheet: u32,
    pub tile: u32,
    pub peer: usize,
}

/// A shape the set of `inner` over `outer` has no tile for, drawn as a 3x3
/// glyph of its peers.
#[derive(Component, Clone, Copy, Debug)]
pub struct MissingGlyph {
    pub root: Entity,
    pub inner: TerrainId,
    pub outer: TerrainId,
}

/// The coverage line of the set of `inner` over `outer`.
#[derive(Component, Clone, Copy, Debug)]
pub struct CoverageLine {
    pub root: Entity,
    pub inner: TerrainId,
    pub outer: TerrainId,
}

/// The stroke a pointer is painting on a sheet, applied when it lifts.
#[derive(Resource, Default)]
struct BitStroke(Option<Stroke>);

struct Stroke {
    root: Entity,
    path: String,
    sheet: u32,
    set: (TerrainId, TerrainId),
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
        inner(String, doc = "Inner terrain id or name of the set."),
        outer(String, doc = "Outer terrain id or name of the set, or empty."),
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
    let (inner, outer) = (text(&params, "inner"), text(&params, "outer"));
    let (on, zones) = (text(&params, "terrain"), text(&params, "zones"));
    queue_edit(
        &mut commands,
        "tileset.tile.paint_bits",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let (inner, outer) = (
                    terrain(tileset, &inner)?,
                    terrain_or_empty(tileset, &outer)?,
                );
                let id = terrain_or_empty(tileset, &on)?;
                let name = |id| tileset.terrain_name(id);
                let set = format!("{} over {}", name(inner), name(outer));
                if setup.set(inner, outer).is_none() {
                    return Err(format!(
                        "no terrain set {set}; add it with tileset.terrain_set.add"
                    ));
                }
                if ![0, inner, outer].contains(&id) {
                    return Err(format!("`{}` is not in the set {set}", name(id)));
                }
                let zones = zone_list(tileset, &zones)?;
                let changed =
                    (setup.paint_zones(inner, outer, id, &zones)).map_err(|e| e.to_string())?;
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

/// Show the bits overlay off, for the active set or for all sets.
#[operator(
    id = "tileset.bits_overlay",
    label = "Bits Overlay",
    description = "Show the painted terrain bits on a Tileset tab's sheet: off, for the \
                   active terrain set only, or for all sets. With no mode it steps to the next.",
    allows_undo = false,
    is_available = a_tileset_is_shown,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the tab with the \
                   keyboard, then the open tileset."
        ),
        mode(String, doc = "off, set or all; empty steps to the next."),
    )
)]
pub fn tileset_bits_overlay(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (path, mode) = (text(&params, "path"), text(&params, "mode"));
    commands.queue(
        move |world: &mut World| match set_overlay(world, &path, &mode) {
            Ok(message) => report_to_caller(world, message),
            Err(err) => warn_caller(world, format!("tileset.bits_overlay: {err}")),
        },
    );
    OperatorResult::Finished
}

fn a_tileset_is_shown(views: Res<TilesetDockViews>) -> bool {
    !views.0.is_empty()
}

fn set_overlay(world: &mut World, path: &str, mode: &str) -> Result<String, String> {
    edit_view(world, path, |view| {
        let (bits, name) = step(&BitsOverlay::ALL, view.bits, mode)?;
        view.bits = bits;
        Ok(format!("bits overlay {name}"))
    })
}

/// Make a press on a Tileset tab's sheet select tiles or paint bits.
#[operator(
    id = "tileset.paint_mode",
    label = "Paint Bits Mode",
    description = "Make a press on a Tileset tab's sheet select tiles, or paint terrain bits \
                   with the brush terrain (right erases). Painting shows the bits overlay if \
                   it is off. With no mode it switches between the two.",
    allows_undo = false,
    is_available = a_tileset_is_shown,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the tab with the \
                   keyboard, then the open tileset."
        ),
        mode(String, doc = "select or paint; empty switches."),
    )
)]
pub fn tileset_paint_mode(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (path, mode) = (text(&params, "path"), text(&params, "mode"));
    commands.queue(move |world: &mut World| {
        let set = edit_view(world, &path, |view| {
            let (tool, name) = step(&DockTool::ALL, view.tool, &mode)?;
            view.set_tool(tool);
            Ok(format!("{name} mode"))
        });
        match set {
            Ok(message) => report_to_caller(world, message),
            Err(err) => warn_caller(world, format!("tileset.paint_mode: {err}")),
        }
    });
    OperatorResult::Finished
}

/// Change the view of a Tileset tab and build it again.
fn edit_view(
    world: &mut World,
    path: &str,
    edit: impl FnOnce(&mut DockView) -> Result<String, String>,
) -> Result<String, String> {
    let path = dock_path(world, path)?;
    let mut views = world.resource_mut::<TilesetDockViews>();
    let view = (views.0.get_mut(&path)).ok_or_else(|| format!("{path} has no Tileset tab"))?;
    let message = edit(view)?;
    refill(world, &path);
    Ok(message)
}

/// The option `name` names, or the one after `current` when it is empty.
fn step<T: Copy + PartialEq>(
    options: &[(T, &'static str)],
    current: T,
    name: &str,
) -> Result<(T, &'static str), String> {
    if name.is_empty() {
        let at = options.iter().position(|(o, _)| *o == current).unwrap_or(0);
        return Ok(options[(at + 1) % options.len()]);
    }
    let names: Vec<&str> = options.iter().map(|(_, n)| *n).collect();
    (options.iter().find(|(_, n)| *n == name).copied())
        .ok_or_else(|| format!("mode is one of {}, not `{name}`", names.join(", ")))
}

/// The coverage line of the set of `inner` over `outer` and its coverage.
pub(crate) fn coverage_line(
    tileset: &Tileset,
    setup: &TilesetSetup,
    (inner, outer): (TerrainId, TerrainId),
) -> Option<(String, Coverage)> {
    let mode = setup.set(inner, outer)?.mode.value;
    let coverage = setup.coverage(inner, outer).ok()?;
    let line = format!(
        "{} over {}: {mode:?} {}/{}",
        tileset.terrain_name(inner),
        tileset.terrain_name(outer),
        coverage.covered,
        coverage.total
    );
    Some((line, coverage))
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
        let color = if mode.uses(peer) {
            swatch(tileset, shape.get(peer), 1.0)
        } else {
            Color::NONE
        };
        world.spawn((Node::default(), BackgroundColor(color), ChildOf(node)));
    }
}

/// The painted zones of sheet `sheet` the overlay of `view` shows, over
/// `frame`.
pub(crate) fn spawn_marks(
    world: &mut World,
    frame: Entity,
    root: Entity,
    tileset: &Tileset,
    setup: &TilesetSetup,
    sheet: u32,
    view: &DockView,
) {
    let Ok(on) = tileset.setup_sheet(sheet) else {
        return;
    };
    if view.bits == BitsOverlay::Off {
        return;
    }
    let active = view
        .active
        .and_then(|(inner, outer)| setup.set(inner, outer));
    let scale = zoom(tileset.tile_size.as_vec2());
    for (address, tile) in setup.tiles.iter().filter(|(a, _)| a.sheet == on) {
        let peers = &tile.terrains;
        let holding = if view.bits == BitsOverlay::Set {
            let Some(set) = active.filter(|s| of_set(s.inner, s.outer, peers)) else {
                continue;
            };
            Some(set.mode.value)
        } else {
            (setup.sets.iter())
                .find(|s| setup.holds(s.inner, s.outer, peers))
                .map(|s| s.mode.value)
        };
        for peer in Peer::ALL {
            let id = peers.get(peer);
            if id == 0 || holding.is_some_and(|mode| !mode.uses(peer)) {
                continue;
            }
            let mark = BitMark {
                root,
                sheet,
                tile: address.tile,
                peer: peer.index(),
            };
            spawn_mark(world, frame, mark, tileset, scale, swatch(tileset, id, 0.8));
        }
    }
}

fn spawn_mark(
    world: &mut World,
    frame: Entity,
    mark: BitMark,
    tileset: &Tileset,
    scale: f32,
    color: Color,
) {
    let Some((origin, size)) = tileset.sheet_rect(mark.sheet, mark.tile) else {
        return;
    };
    let third = size.as_vec2() * scale / 3.0;
    let cell = Vec2::new((mark.peer % 3) as f32, (mark.peer / 3) as f32);
    let at = origin.as_vec2() * scale + cell * third + 1.0;
    world.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(at.x),
            top: px(at.y),
            width: px((third.x - 2.0).max(1.0)),
            height: px((third.y - 2.0).max(1.0)),
            border: UiRect::all(px(1.0)),
            ..default()
        },
        BackgroundColor(color),
        BorderColor::all(Color::BLACK.with_alpha(0.6)),
        mark,
        Pickable::IGNORE,
        ChildOf(frame),
    ));
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
    if view.page != DockPage::Sheet || view.tool != DockTool::Paint {
        return;
    }
    let Ok(setup) = tileset.setup() else {
        return;
    };
    let set = (view.active).filter(|(i, o)| [0, *i, *o].contains(&terrain));
    let Some(mode) = set.and_then(|(i, o)| setup.set(i, o)).map(|s| s.mode.value) else {
        if terrain != 0 {
            let name = tileset.terrain_name(terrain);
            let notice = format!("no terrain set holds {name}; add one to paint its bits");
            crate::status_bar::notify_info(world, notice);
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
    let color = match stroke.terrain {
        0 => Color::BLACK.with_alpha(0.7),
        id => swatch(&stroke.tileset, id, 0.8),
    };
    let scale = zoom(stroke.tileset.tile_size.as_vec2());
    for (tile, peer) in fresh {
        let mark = BitMark {
            root: stroke.root,
            sheet: stroke.sheet,
            tile: *tile,
            peer: peer.index(),
        };
        spawn_mark(world, frame, mark, &stroke.tileset, scale, color);
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
    let (inner, outer) = stroke.set;
    if (stroke.setup)
        .paint_zones(inner, outer, stroke.terrain, &zones)
        .is_err()
    {
        return;
    }
    let lines: Vec<Entity> = (world.query::<(Entity, &CoverageLine)>())
        .iter(world)
        .filter(|(_, l)| l.root == stroke.root && (l.inner, l.outer) == stroke.set)
        .map(|(e, _)| e)
        .collect();
    let Some((text, _)) = coverage_line(&stroke.tileset, &stroke.setup, stroke.set) else {
        return;
    };
    for entity in lines {
        if let Some(mut node) = world.get_mut::<Text>(entity) {
            node.0.clone_from(&text);
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
    let (inner, outer) = stroke.set;
    let call = (!zones.is_empty()).then(|| {
        let params = vec![
            ("inner", inner.to_string()),
            ("outer", outer.to_string()),
            ("terrain", stroke.terrain.to_string()),
            ("zones", zones.join(",")),
        ];
        ("tileset.tile.paint_bits", params)
    });
    dispatch(world, stroke.root, &stroke.path, &stroke.tileset, call);
}

/// Whether `peers` has bits and all of them are of `inner` or `outer`.
fn of_set(inner: TerrainId, outer: TerrainId, peers: &TileTerrains) -> bool {
    let bits = || peers.peers.iter().filter(|p| **p != 0);
    bits().next().is_some() && bits().all(|p| *p == inner || *p == outer)
}
