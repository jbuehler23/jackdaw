use std::collections::{HashMap, HashSet};
use std::ops::Range;

use bevy::math::Rot2;
use bevy::picking::events::{PointerMove, PointerOut};
use bevy::prelude::*;
use bevy::ui::UiTransform;
use jackdaw_tilemap::{MatchMode, Peer};
use jackdaw_tileset::Tileset;

use crate::tileset_dock::{DockAction, DockControl, zoom};

/// Alpha of the dim layer over tiles that hold no value.
const DIM: f32 = 0.55;
/// Alpha of the fill of a shape of the picked value; other values have none.
const PICKED_FILL: f32 = 0.22;
/// Alpha of the outline of a shape of a value other than the picked one.
const QUIET: f32 = 0.6;
/// Width of the outline of a shape of the picked value, and of the others.
const PICKED_LINE: f32 = 1.5;
const QUIET_LINE: f32 = 1.0;
/// Width of the dark stroke around the outline of a picked shape.
const STROKE: f32 = 1.0;
/// Gap between a shape and the edge of its tile.
const INSET: f32 = 1.5;
/// Blur and alpha of the glow around the picked value's outlines.
const GLOW: f32 = 3.0;
const GLOW_ALPHA: f32 = 0.35;
/// Alpha of the fill of the zone under the pointer.
const HOVER: f32 = 0.35;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<HoveredZones>()
        .add_observer(on_hover)
        .add_observer(on_leave);
}

/// The tile under the pointer and the point in it, by dock and sheet, so an
/// overlay drawn again keeps its zone outline.
#[derive(Resource, Default)]
struct HoveredZones(HashMap<(Entity, u32), (u32, Vec2)>);

/// How a tile is cut into zones that each hold a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneLayout {
    /// One zone, the tile.
    Whole,
    /// The peers a match mode reads: four quarters for corners, thirds
    /// otherwise.
    Peers(MatchMode),
}

impl ZoneLayout {
    /// Cells across one side of a tile.
    pub fn cells(self) -> u32 {
        match self {
            ZoneLayout::Whole => 1,
            ZoneLayout::Peers(MatchMode::Corners) => 2,
            ZoneLayout::Peers(_) => 3,
        }
    }

    /// The value cell `cell` of a tile with peer values `peers` shows. A
    /// corner of a sides tile shows its two sides' value when they agree.
    pub fn value(self, peers: &[u32; 9], cell: UVec2) -> u32 {
        match self {
            ZoneLayout::Whole => peers[Peer::Centre.index()],
            ZoneLayout::Peers(MatchMode::Corners) => peers[(cell.y * 6 + cell.x * 2) as usize],
            ZoneLayout::Peers(MatchMode::CornersAndSides) => peers[(cell.y * 3 + cell.x) as usize],
            ZoneLayout::Peers(MatchMode::Sides) => {
                let peer = Peer::ALL[(cell.y * 3 + cell.x) as usize];
                match peer.corner_sides() {
                    Some([a, b]) if peers[a.index()] == peers[b.index()] => peers[a.index()],
                    Some(_) => 0,
                    None => peers[peer.index()],
                }
            }
        }
    }

    /// The cell of the zone under `at`, a point of the tile from (0, 0) at
    /// its top left to (1, 1); `None` where the layout has no zone.
    pub fn cell_under(self, at: Vec2) -> Option<UVec2> {
        let ZoneLayout::Peers(mode) = self else {
            return Some(UVec2::ZERO);
        };
        let peer = Peer::under(at, mode);
        if !mode.reads(peer) {
            return None;
        }
        let index = peer.index() as u32;
        let cell = UVec2::new(index % 3, index / 3);
        Some(if mode == MatchMode::Corners {
            cell / 2
        } else {
            cell
        })
    }
}

/// The values an overlay shows on one sheet.
#[derive(Clone, Debug)]
pub struct ZoneValues {
    pub layout: ZoneLayout,
    /// Peer values by sheet tile, 0 for none; a tile left out holds none.
    pub tiles: HashMap<u32, [u32; 9]>,
    /// The value being painted, tinted and glowing.
    pub picked: Option<u32>,
    pub colors: HashMap<u32, Color>,
    /// Whether tiles holding no value are dimmed.
    pub dim: bool,
    /// The fill of the zone under the pointer, when it is outlined.
    pub hover: Option<Color>,
    /// Tiles drawn as a polygon of one value in place of their zones, by
    /// sheet tile.
    pub outlines: HashMap<u32, ZoneOutline>,
}

/// A polygon a tile shows in place of zones.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneOutline {
    pub value: u32,
    /// Rings in tile fractions, from (0, 0) at the tile's top left to
    /// (1, 1).
    pub rings: Vec<Vec<Vec2>>,
}

impl ZoneValues {
    /// The zones holding a value, as tile and peer index.
    pub fn zones(&self) -> impl Iterator<Item = (u32, usize, u32)> + '_ {
        (self.tiles.iter()).flat_map(|(tile, peers)| {
            (peers.iter().enumerate())
                .filter(|(_, v)| **v != 0)
                .map(|(peer, v)| (*tile, peer, *v))
        })
    }
}

/// The on-screen rectangle of each tile of a sheet, in its frame.
#[derive(Clone, Debug, Default)]
pub struct SheetGrid {
    pub columns: u32,
    pub rows: u32,
    pub tiles: Vec<Rect>,
}

impl SheetGrid {
    pub fn of(tileset: &Tileset, sheet: u32) -> Option<Self> {
        let found = tileset.sheets.get(sheet as usize)?;
        let scale = zoom(tileset.tile_size.as_vec2());
        let tiles = (0..found.columns * found.rows)
            .map(|t| {
                let (min, size) = tileset.sheet_rect(sheet, t).unwrap_or_default();
                let min = min.as_vec2() * scale;
                Rect::from_corners(min, min + size.as_vec2() * scale)
            })
            .collect();
        Some(Self {
            columns: found.columns,
            rows: found.rows,
            tiles,
        })
    }

    fn cell_rect(&self, layout: ZoneLayout, tile: u32, cell: UVec2) -> Rect {
        let rect = self.tiles.get(tile as usize).copied().unwrap_or_default();
        let size = rect.size() / layout.cells() as f32;
        let min = rect.min + cell.as_vec2() * size;
        Rect::from_corners(min, min + size)
    }
}

/// The values drawn over one sheet of the dock `root`, on the node holding
/// the drawing.
#[derive(Component, Clone, Debug)]
pub struct ZoneOverlay {
    pub root: Entity,
    pub sheet: u32,
    pub values: ZoneValues,
    pub grid: SheetGrid,
    /// The outline of the zone under the pointer.
    pub hover: Option<Entity>,
}

/// The tile row a drawn part of an overlay belongs to.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZoneRow(pub u32);

/// One drawn part of an overlay. A value's zones in one tile make one
/// shape: a rectangle is drawn whole, any other shape as fills and edges.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub enum ZonePiece {
    /// Over a run of tiles holding no value.
    Dim,
    /// A rectangular shape, filled and outlined.
    Shape { value: u32, picked: bool },
    /// Part of the fill of a picked shape that is no rectangle.
    Tint { value: u32 },
    /// Part of the outline of a shape that is no rectangle.
    Edge { value: u32, picked: bool },
    /// The dark stroke outside an edge of a picked shape.
    Stroke,
    /// One side of an outline that is no rectangle, turned by `angle`
    /// radians about its center.
    Segment {
        value: u32,
        picked: bool,
        angle: f32,
    },
}

/// The outline of the zone under the pointer, showing tile `tile`.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ZoneHover {
    pub tile: Option<u32>,
}

/// Draw `values` over sheet `sheet` in `frame`, as a node holding the
/// drawing; returns it.
pub fn spawn_zone_overlay(
    world: &mut World,
    frame: Entity,
    root: Entity,
    (tileset, sheet): (&Tileset, u32),
    values: ZoneValues,
) -> Option<Entity> {
    let grid = SheetGrid::of(tileset, sheet)?;
    let overlay = world
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(0.0),
                top: px(0.0),
                width: percent(100),
                height: percent(100),
                ..default()
            },
            Pickable::IGNORE,
            ChildOf(frame),
        ))
        .id();
    spawn_pieces(
        world,
        overlay,
        &values,
        pieces(&grid, &values, 0..grid.rows),
    );
    let hover = values.hover.map(|fill| {
        world
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    display: Display::None,
                    border: UiRect::all(px(1.5)),
                    ..default()
                },
                BackgroundColor(fill.with_alpha(fill.alpha().min(HOVER))),
                BorderColor::all(Color::WHITE),
                ZIndex(1),
                ZoneHover::default(),
                Pickable::IGNORE,
                ChildOf(overlay),
            ))
            .id()
    });
    let held =
        (world.get_resource::<HoveredZones>()).and_then(|h| h.0.get(&(root, sheet)).copied());
    if let Some(((tile, at), hover)) = held.zip(hover)
        && let Some(cell) = values.layout.cell_under(at)
        && let Some(mut node) = world.get_mut::<Node>(hover)
    {
        show_at(&mut node, grid.cell_rect(values.layout, tile, cell));
        if let Some(mut state) = world.get_mut::<ZoneHover>(hover) {
            state.tile = Some(tile);
        }
    }
    world.entity_mut(overlay).insert(ZoneOverlay {
        root,
        sheet,
        values,
        grid,
        hover,
    });
    Some(overlay)
}

/// Draw `overlay` again with `values`, in tile rows `rows` only.
pub fn redraw_zone_rows(
    world: &mut World,
    overlay: Entity,
    values: ZoneValues,
    rows: impl IntoIterator<Item = u32>,
) {
    let Some(grid) = world.get::<ZoneOverlay>(overlay).map(|o| o.grid.clone()) else {
        return;
    };
    let rows: HashSet<u32> = rows.into_iter().filter(|r| *r < grid.rows).collect();
    let stale: Vec<Entity> = (world.get::<Children>(overlay).into_iter().flatten())
        .filter(|e| {
            world
                .get::<ZoneRow>(**e)
                .is_some_and(|r| rows.contains(&r.0))
        })
        .copied()
        .collect();
    for entity in stale {
        world.entity_mut(entity).despawn();
    }
    for row in rows {
        spawn_pieces(
            world,
            overlay,
            &values,
            pieces(&grid, &values, row..row + 1),
        );
    }
    if let Some(mut shown) = world.get_mut::<ZoneOverlay>(overlay) {
        shown.values = values;
    }
}

fn spawn_pieces(
    world: &mut World,
    overlay: Entity,
    values: &ZoneValues,
    found: Vec<(u32, ZonePiece, Rect)>,
) {
    let dark = Color::BLACK.with_alpha(0.7);
    let glow =
        |line: Color| BoxShadow::new(line.with_alpha(GLOW_ALPHA), px(0), px(0), px(0), px(GLOW));
    let line = |value: u32, picked: bool| {
        let alpha = if picked { 1.0 } else { QUIET };
        color(values, value).with_alpha(alpha)
    };
    for (row, piece, rect) in found {
        let mut node = world.spawn((
            absolute(rect),
            piece,
            ZoneRow(row),
            Pickable::IGNORE,
            ChildOf(overlay),
        ));
        match piece {
            ZonePiece::Dim => {
                node.insert(BackgroundColor(Color::BLACK.with_alpha(DIM)));
            }
            ZonePiece::Shape { value, picked } => {
                let width = if picked { PICKED_LINE } else { QUIET_LINE };
                let mut shape = absolute(rect);
                shape.border = UiRect::all(px(width));
                node.insert((shape, BorderColor::all(line(value, picked))));
                if picked {
                    node.insert((
                        BackgroundColor(color(values, value).with_alpha(PICKED_FILL)),
                        Outline::new(px(STROKE), px(0), dark),
                        glow(line(value, true)),
                    ));
                }
            }
            ZonePiece::Tint { value } => {
                node.insert(BackgroundColor(
                    color(values, value).with_alpha(PICKED_FILL),
                ));
            }
            ZonePiece::Edge { value, picked } => {
                node.insert(BackgroundColor(line(value, picked)));
                if picked {
                    node.insert(glow(line(value, true)));
                }
            }
            ZonePiece::Stroke => {
                node.insert(BackgroundColor(dark));
            }
            ZonePiece::Segment {
                value,
                picked,
                angle,
            } => {
                node.insert((
                    BackgroundColor(line(value, picked)),
                    UiTransform {
                        rotation: Rot2::radians(angle),
                        ..UiTransform::IDENTITY
                    },
                ));
                if picked {
                    node.insert((
                        Outline::new(px(STROKE), px(0), dark),
                        glow(line(value, true)),
                    ));
                }
            }
        }
    }
}

fn show_at(node: &mut Node, rect: Rect) {
    let shown = absolute(rect);
    if node.left != shown.left || node.top != shown.top || node.display != Display::Flex {
        node.left = shown.left;
        node.top = shown.top;
        node.width = shown.width;
        node.height = shown.height;
        node.display = Display::Flex;
    }
}

fn color(values: &ZoneValues, value: u32) -> Color {
    values.colors.get(&value).copied().unwrap_or(Color::WHITE)
}

fn absolute(rect: Rect) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: px(rect.min.x),
        top: px(rect.min.y),
        width: px(rect.width()),
        height: px(rect.height()),
        ..default()
    }
}

/// What an overlay draws over tile rows `rows`, with the row each piece
/// belongs to: each row's dim runs, then the shapes of its tiles, the
/// picked value's last.
pub fn pieces(
    grid: &SheetGrid,
    values: &ZoneValues,
    rows: Range<u32>,
) -> Vec<(u32, ZonePiece, Rect)> {
    let mut out = Vec::new();
    for row in rows.start..rows.end.min(grid.rows) {
        if values.dim {
            let mut open: Option<Rect> = None;
            for column in 0..grid.columns {
                let tile = row * grid.columns + column;
                let held = values.outlines.contains_key(&tile)
                    || (values.tiles.get(&tile)).is_some_and(|p| p.iter().any(|v| *v != 0));
                let at = grid.tiles.get(tile as usize).copied().unwrap_or_default();
                match (held, open.as_mut()) {
                    (false, Some(run)) if (run.max.x - at.min.x).abs() < 0.01 => {
                        *run = run.union(at);
                    }
                    (false, _) => {
                        out.extend(open.replace(at).map(|r| (row, ZonePiece::Dim, r)));
                    }
                    (true, _) => out.extend(open.take().map(|r| (row, ZonePiece::Dim, r))),
                }
            }
            out.extend(open.map(|r| (row, ZonePiece::Dim, r)));
        }
        let mut shapes = Vec::new();
        for column in 0..grid.columns {
            let tile = row * grid.columns + column;
            if let Some(outline) = values.outlines.get(&tile) {
                shapes.extend(outline_shapes(grid, values, tile, outline));
            } else if let Some(peers) = values.tiles.get(&tile) {
                shapes.extend(tile_shapes(grid, values, tile, peers));
            }
        }
        shapes.sort_by_key(|(piece, _)| match piece {
            ZonePiece::Shape { picked, .. }
            | ZonePiece::Edge { picked, .. }
            | ZonePiece::Segment { picked, .. } => *picked,
            ZonePiece::Dim => false,
            ZonePiece::Tint { .. } | ZonePiece::Stroke => true,
        });
        out.extend(shapes.into_iter().map(|(piece, at)| (row, piece, at)));
    }
    out
}

/// The shape of each value on tile `tile`, inset from the tile's edges.
fn tile_shapes(
    grid: &SheetGrid,
    values: &ZoneValues,
    tile: u32,
    peers: &[u32; 9],
) -> Vec<(ZonePiece, Rect)> {
    let layout = values.layout;
    let n = layout.cells();
    let value_at = |x: u32, y: u32| layout.value(peers, UVec2::new(x, y));
    let cell = |x: u32, y: u32| {
        let mut at = grid.cell_rect(layout, tile, UVec2::new(x, y));
        if x == 0 {
            at.min.x += INSET;
        }
        if y == 0 {
            at.min.y += INSET;
        }
        if x + 1 == n {
            at.max.x -= INSET;
        }
        if y + 1 == n {
            at.max.y -= INSET;
        }
        at
    };
    let mut held: Vec<u32> = (0..n * n).map(|i| value_at(i % n, i / n)).collect();
    held.retain(|v| *v != 0);
    held.sort_unstable();
    held.dedup();
    let mut out = Vec::new();
    for value in held {
        let picked = values.picked == Some(value);
        let inside = |x: i64, y: i64| {
            (0..i64::from(n)).contains(&x)
                && (0..i64::from(n)).contains(&y)
                && value_at(x as u32, y as u32) == value
        };
        let cells: Vec<UVec2> = (0..n * n)
            .map(|i| UVec2::new(i % n, i / n))
            .filter(|c| inside(i64::from(c.x), i64::from(c.y)))
            .collect();
        let (lo, hi) = cells.iter().fold((UVec2::MAX, UVec2::ZERO), |(lo, hi), c| {
            (lo.min(*c), hi.max(*c))
        });
        let span = hi - lo + UVec2::ONE;
        if span.x * span.y == cells.len() as u32 {
            let at = cell(lo.x, lo.y).union(cell(hi.x, hi.y));
            out.push((ZonePiece::Shape { value, picked }, at));
            continue;
        }
        let mut runs: Vec<(u32, u32, u32, Rect)> = Vec::new();
        for y in 0..n {
            let mut x = 0;
            while x < n {
                if !inside(i64::from(x), i64::from(y)) {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < n && inside(i64::from(x), i64::from(y)) {
                    x += 1;
                }
                let at = cell(start, y).union(cell(x - 1, y));
                match runs
                    .iter_mut()
                    .find(|r| r.0 + 1 == y && (r.1, r.2) == (start, x))
                {
                    Some(run) => {
                        run.0 = y;
                        run.3 = run.3.union(at);
                    }
                    None => runs.push((y, start, x, at)),
                }
            }
        }
        if picked {
            let tint = ZonePiece::Tint { value };
            out.extend(runs.into_iter().map(|(.., at)| (tint, at)));
        }
        let width = if picked { PICKED_LINE } else { QUIET_LINE };
        let edge = ZonePiece::Edge { value, picked };
        for (dx, dy) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
            let across = dx == 0;
            let at = |a: u32, b: u32| if across { (b, a) } else { (a, b) };
            let side = |x: u32, y: u32| {
                inside(i64::from(x), i64::from(y)) && !inside(i64::from(x) + dx, i64::from(y) + dy)
            };
            for a in 0..n {
                let mut b = 0;
                while b < n {
                    let (x, y) = at(a, b);
                    if !side(x, y) {
                        b += 1;
                        continue;
                    }
                    let start = b;
                    while b < n && side(at(a, b).0, at(a, b).1) {
                        b += 1;
                    }
                    let (first, last) = (at(a, start), at(a, b - 1));
                    let step = if across { (1, 0) } else { (0, 1) };
                    let turns_in = |(x, y): (u32, u32), sign: i64| {
                        inside(i64::from(x) + sign * step.0, i64::from(y) + sign * step.1)
                    };
                    let ends = (turns_in(first, -1), turns_in(last, 1));
                    let span = cell(first.0, first.1).union(cell(last.0, last.1));
                    let (line, stroke) = boundary(span, (dx, dy), width, ends);
                    out.push((edge, line));
                    if picked {
                        out.push((ZonePiece::Stroke, stroke));
                    }
                }
            }
        }
    }
    out
}

/// The pieces of `outline` on tile `tile`, inside the tile less the inset:
/// a rectangle is one shape, any other ring one segment per side.
fn outline_shapes(
    grid: &SheetGrid,
    values: &ZoneValues,
    tile: u32,
    outline: &ZoneOutline,
) -> Vec<(ZonePiece, Rect)> {
    let rect = grid.tiles.get(tile as usize).copied().unwrap_or_default();
    let inner = Rect::from_corners(rect.min + INSET, rect.max - INSET);
    let at = |p: Vec2| inner.min + p.clamp(Vec2::ZERO, Vec2::ONE) * inner.size();
    let value = outline.value;
    let picked = values.picked == Some(value);
    let width = if picked { PICKED_LINE } else { QUIET_LINE };
    let mut out = Vec::new();
    for ring in &outline.rings {
        if let Some((lo, hi)) = axis_rect(ring) {
            let shape = Rect::from_corners(at(lo), at(hi));
            out.push((ZonePiece::Shape { value, picked }, shape));
            continue;
        }
        let next = ring.iter().cycle().skip(1);
        for (a, b) in ring.iter().zip(next) {
            let (a, b) = (at(*a), at(*b));
            let length = a.distance(b);
            if length < 0.01 {
                continue;
            }
            let angle = (b - a).to_angle();
            let side = Rect::from_center_size((a + b) / 2.0, Vec2::new(length + width, width));
            let segment = ZonePiece::Segment {
                value,
                picked,
                angle,
            };
            out.push((segment, side));
        }
    }
    out
}

/// The corners of `ring` when it is an axis-aligned rectangle.
fn axis_rect(ring: &[Vec2]) -> Option<(Vec2, Vec2)> {
    if ring.len() != 4 {
        return None;
    }
    let next = ring.iter().cycle().skip(1);
    let straight = (ring.iter().zip(next)).all(|(a, b)| a.x == b.x || a.y == b.y);
    let lo = ring.iter().copied().reduce(Vec2::min)?;
    let hi = ring.iter().copied().reduce(Vec2::max)?;
    (straight && (hi - lo).cmpgt(Vec2::ZERO).all()).then_some((lo, hi))
}

/// The line along side `(dx, dy)` of the cells spanning `span`, inside it
/// and `width` across, and the dark stroke just outside it. `ends` says
/// whether the outline turns into the shape before and after the run.
/// Lines along a row close the inner corners and take the outer ones;
/// lines along a column stop short of both, so every corner is drawn once.
fn boundary(span: Rect, (dx, dy): (i64, i64), width: f32, ends: (bool, bool)) -> (Rect, Rect) {
    let (before, after) = ends;
    if dx == 0 {
        let grow = |turns: bool| if turns { width } else { 0.0 };
        let reach = |turns: bool| if turns { -STROKE } else { STROKE };
        let (x0, x1) = (span.min.x - grow(before), span.max.x + grow(after));
        let (s0, s1) = (span.min.x - reach(before), span.max.x + reach(after));
        if dy < 0 {
            let line = Rect::new(x0, span.min.y, x1, span.min.y + width);
            (line, Rect::new(s0, span.min.y - STROKE, s1, span.min.y))
        } else {
            let line = Rect::new(x0, span.max.y - width, x1, span.max.y);
            (line, Rect::new(s0, span.max.y, s1, span.max.y + STROKE))
        }
    } else {
        let trim = |turns: bool| if turns { 0.0 } else { width };
        let (y0, y1) = (span.min.y + trim(before), span.max.y - trim(after));
        if dx < 0 {
            let line = Rect::new(span.min.x, y0, span.min.x + width, y1);
            (
                line,
                Rect::new(span.min.x - STROKE, span.min.y, span.min.x, span.max.y),
            )
        } else {
            let line = Rect::new(span.max.x - width, y0, span.max.x, y1);
            (
                line,
                Rect::new(span.max.x, span.min.y, span.max.x + STROKE, span.max.y),
            )
        }
    }
}

/// The dock, sheet and tile of the sheet tile `entity`, the zone outline of
/// the overlay over it, and the zone's rectangle when `at` is in one.
fn hovered(
    entity: Entity,
    at: Option<Vec2>,
    controls: &Query<(&DockControl, &ChildOf)>,
    frames: &Query<&Children>,
    overlays: &Query<&ZoneOverlay>,
) -> Option<((Entity, u32, u32), Entity, Option<Rect>)> {
    let (control, parent) = controls.get(entity).ok()?;
    let DockAction::Tile(sheet, tile) = control.action else {
        return None;
    };
    let overlay = (frames.get(parent.parent()).ok()?.iter())
        .filter_map(|child| overlays.get(child).ok())
        .find(|o| o.root == control.root && o.sheet == sheet && o.hover.is_some())?;
    let layout = overlay.values.layout;
    let zone = (at.and_then(|inside| layout.cell_under(inside)))
        .map(|cell| overlay.grid.cell_rect(layout, tile, cell));
    Some(((control.root, sheet, tile), overlay.hover?, zone))
}

/// Outline the zone under the pointer in the overlay of its sheet.
fn on_hover(
    moved: On<PointerMove>,
    controls: Query<(&DockControl, &ChildOf)>,
    frames: Query<&Children>,
    overlays: Query<&ZoneOverlay>,
    mut hovers: Query<(&mut Node, &mut ZoneHover)>,
    mut held: ResMut<HoveredZones>,
) -> Result {
    let at = moved.hit.position.map(|p| p.truncate() + 0.5);
    let Some(((root, sheet, tile), hover, zone)) =
        hovered(moved.entity, at, &controls, &frames, &overlays)
    else {
        return Ok(());
    };
    let (mut node, mut state) = hovers.get_mut(hover)?;
    if let Some(at) = at {
        held.0.insert((root, sheet), (tile, at));
    }
    let Some(rect) = zone else {
        node.display = Display::None;
        state.tile = None;
        return Ok(());
    };
    show_at(&mut node, rect);
    state.tile = Some(tile);
    Ok(())
}

/// Hide the zone outline once the pointer leaves the tile it is on.
fn on_leave(
    out: On<PointerOut>,
    controls: Query<(&DockControl, &ChildOf)>,
    frames: Query<&Children>,
    overlays: Query<&ZoneOverlay>,
    mut hovers: Query<(&mut Node, &mut ZoneHover)>,
    mut held: ResMut<HoveredZones>,
) -> Result {
    let Some(((root, sheet, tile), hover, _)) =
        hovered(out.entity, None, &controls, &frames, &overlays)
    else {
        return Ok(());
    };
    let (mut node, mut state) = hovers.get_mut(hover)?;
    if state.tile == Some(tile) {
        node.display = Display::None;
        state.tile = None;
    }
    if held.0.get(&(root, sheet)).is_some_and(|(t, _)| *t == tile) {
        held.0.remove(&(root, sheet));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(columns: u32, rows: u32) -> SheetGrid {
        let tiles = (0..columns * rows)
            .map(|t| {
                let min = Vec2::new((t % columns) as f32, (t / columns) as f32) * 32.0;
                Rect::from_corners(min, min + 32.0)
            })
            .collect();
        SheetGrid {
            columns,
            rows,
            tiles,
        }
    }

    fn corners(tiles: &[(u32, [u32; 4])], picked: Option<u32>) -> ZoneValues {
        let tiles = (tiles.iter())
            .map(|(tile, [a, b, c, d])| (*tile, [*a, 0, *b, 0, 0, 0, *c, 0, *d]))
            .collect();
        ZoneValues {
            layout: ZoneLayout::Peers(MatchMode::Corners),
            tiles,
            picked,
            colors: HashMap::new(),
            dim: true,
            hover: None,
            outlines: HashMap::new(),
        }
    }

    fn drawn(grid: &SheetGrid, values: &ZoneValues) -> Vec<(ZonePiece, Rect)> {
        (pieces(grid, values, 0..grid.rows).into_iter())
            .map(|(_, piece, at)| (piece, at))
            .collect()
    }

    fn count(found: &[(ZonePiece, Rect)], hit: impl Fn(&ZonePiece) -> bool) -> usize {
        found.iter().filter(|(p, _)| hit(p)).count()
    }

    #[test]
    fn a_whole_tile_or_a_half_is_one_inset_shape() {
        let values = corners(&[(0, [1, 1, 1, 1]), (1, [1, 2, 1, 2])], Some(1));
        let found = drawn(&grid(3, 1), &values);
        let shapes: Vec<(ZonePiece, Rect)> = (found.iter())
            .filter(|(p, _)| matches!(p, ZonePiece::Shape { .. }))
            .copied()
            .collect();
        let picked = ZonePiece::Shape {
            value: 1,
            picked: true,
        };
        let other = ZonePiece::Shape {
            value: 2,
            picked: false,
        };
        let whole = Rect::new(INSET, INSET, 32.0 - INSET, 32.0 - INSET);
        assert_eq!(shapes.len(), 3, "{shapes:?}");
        assert!(shapes.contains(&(picked, whole)), "{shapes:?}");
        let right = Rect::new(48.0, INSET, 64.0 - INSET, 32.0 - INSET);
        assert!(shapes.contains(&(other, right)), "{shapes:?}");
        assert_eq!(count(&found, |p| *p == ZonePiece::Dim), 1, "the empty tile");
        assert_eq!(found.len(), 4, "no edge or fill pieces");
    }

    #[test]
    fn a_painted_tile_inside_a_region_keeps_its_own_outline() {
        let full = [1, 1, 1, 1];
        let values = corners(&[(0, full), (1, full), (2, full)], Some(1));
        let found = drawn(&grid(3, 1), &values);
        assert_eq!(count(&found, |p| matches!(p, ZonePiece::Shape { .. })), 3);
    }

    #[test]
    fn a_picked_l_shape_is_one_fill_with_an_outline_on_its_boundary_only() {
        let values = corners(&[(0, [1, 1, 1, 2])], Some(1));
        let found = drawn(&grid(1, 1), &values);
        let tints = count(&found, |p| matches!(p, ZonePiece::Tint { value: 1, .. }));
        assert_eq!(tints, 2, "the top row and the bottom left quarter");
        let edges = count(&found, |p| {
            matches!(
                p,
                ZonePiece::Edge {
                    value: 1,
                    picked: true
                }
            )
        });
        assert_eq!(edges, 6, "an L has six sides");
        assert_eq!(
            count(&found, |p| *p == ZonePiece::Stroke),
            6,
            "one per side"
        );
        let quarter = ZonePiece::Shape {
            value: 2,
            picked: false,
        };
        assert_eq!(count(&found, |p| *p == quarter), 1);
    }

    #[test]
    fn other_values_are_outlined_with_no_fill() {
        let values = corners(&[(0, [1, 1, 1, 2])], Some(2));
        let found = drawn(&grid(1, 1), &values);
        assert_eq!(count(&found, |p| matches!(p, ZonePiece::Tint { .. })), 0);
        assert_eq!(count(&found, |p| *p == ZonePiece::Stroke), 0);
        let quiet = |p: &ZonePiece| {
            matches!(
                p,
                ZonePiece::Edge {
                    value: 1,
                    picked: false
                }
            )
        };
        assert_eq!(count(&found, quiet), 6);
        let widths: Vec<f32> = (found.iter())
            .filter(|(p, _)| quiet(p))
            .map(|(_, r)| r.width().min(r.height()))
            .collect();
        assert!(widths.iter().all(|w| *w == QUIET_LINE), "{widths:?}");
    }

    /// How many of the pieces `hit` takes hold `point`.
    fn covering(
        found: &[(ZonePiece, Rect)],
        hit: impl Fn(&ZonePiece) -> bool,
        point: Vec2,
    ) -> usize {
        (found.iter())
            .filter(|(p, r)| hit(p) && r.contains(point))
            .count()
    }

    #[test]
    fn every_corner_of_an_l_outline_is_drawn_once() {
        let values = corners(&[(0, [1, 1, 1, 0])], Some(1));
        let found = drawn(&grid(1, 1), &values);
        let edge = |p: &ZonePiece| matches!(p, ZonePiece::Edge { .. });
        let stroke = |p: &ZonePiece| *p == ZonePiece::Stroke;
        let half = PICKED_LINE / 2.0;
        let lines = [
            ("outer top left", Vec2::splat(INSET + half)),
            (
                "outer top right",
                Vec2::new(32.0 - INSET - half, INSET + half),
            ),
            (
                "outer bottom left",
                Vec2::new(INSET + half, 32.0 - INSET - half),
            ),
            ("inner", Vec2::splat(16.0 - half)),
            (
                "outer at the notch",
                Vec2::new(32.0 - INSET - half, 16.0 - half),
            ),
        ];
        for (name, point) in lines {
            assert_eq!(covering(&found, edge, point), 1, "{name} line corner");
        }
        let half = STROKE / 2.0;
        let strokes = [
            ("outer top left", Vec2::splat(INSET - half)),
            (
                "outer top right",
                Vec2::new(32.0 - INSET + half, INSET - half),
            ),
            ("inner", Vec2::splat(16.0 + half)),
            (
                "outer at the notch",
                Vec2::new(32.0 - INSET + half, 16.0 + half),
            ),
        ];
        for (name, point) in strokes {
            assert_eq!(covering(&found, stroke, point), 1, "{name} stroke corner");
        }
    }

    #[test]
    fn a_fully_painted_sheet_draws_one_node_per_tile() {
        let tiles: Vec<(u32, [u32; 4])> = (0..27 * 65).map(|t| (t, [1, 1, 1, 1])).collect();
        let values = corners(&tiles, Some(1));
        assert_eq!(drawn(&grid(27, 65), &values).len(), 27 * 65);
    }

    #[test]
    fn a_sides_corner_shows_its_sides_when_they_agree() {
        let layout = ZoneLayout::Peers(MatchMode::Sides);
        let peers = [0, 1, 0, 1, 1, 2, 0, 2, 0];
        assert_eq!(layout.value(&peers, UVec2::new(0, 0)), 1);
        assert_eq!(layout.value(&peers, UVec2::new(2, 0)), 0);
        assert_eq!(layout.value(&peers, UVec2::new(2, 2)), 2);
    }

    fn outlined(rings: &[&[(f32, f32)]], picked: bool) -> ZoneValues {
        let rings = (rings.iter())
            .map(|r| r.iter().map(|&(x, y)| Vec2::new(x, y)).collect())
            .collect();
        ZoneValues {
            layout: ZoneLayout::Whole,
            tiles: HashMap::new(),
            picked: picked.then_some(1),
            colors: HashMap::new(),
            dim: true,
            hover: None,
            outlines: HashMap::from([(0, ZoneOutline { value: 1, rings })]),
        }
    }

    #[test]
    fn a_rectangular_outline_is_one_inset_shape_and_its_tile_is_not_dimmed() {
        let half = [(0.0, 0.5), (1.0, 0.5), (1.0, 1.0), (0.0, 1.0)];
        let found = drawn(&grid(2, 1), &outlined(&[&half], true));
        let shape = ZonePiece::Shape {
            value: 1,
            picked: true,
        };
        let lower = Rect::new(INSET, 16.0, 32.0 - INSET, 32.0 - INSET);
        assert!(found.contains(&(shape, lower)), "{found:?}");
        assert_eq!(count(&found, |p| *p == ZonePiece::Dim), 1, "the other tile");
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn a_slope_outline_is_one_segment_per_side() {
        let slope = [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0)];
        let found = drawn(&grid(1, 1), &outlined(&[&slope], false));
        let segments: Vec<f32> = (found.iter())
            .filter_map(|(p, r)| match p {
                ZonePiece::Segment { picked: false, .. } => Some(r.width()),
                _ => None,
            })
            .collect();
        assert_eq!(segments.len(), 3, "{found:?}");
        let side = 32.0 - 2.0 * INSET;
        let diagonal = side * std::f32::consts::SQRT_2 + QUIET_LINE;
        assert!(
            segments.iter().any(|w| (w - diagonal).abs() < 0.01),
            "{segments:?}"
        );
    }

    #[test]
    fn a_fully_outlined_sheet_draws_one_node_per_rectangle_and_three_per_slope() {
        let square = vec![vec![Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y]];
        let slope = vec![vec![Vec2::Y, Vec2::ONE, Vec2::X]];
        let sheet = |rings: &Vec<Vec<Vec2>>| ZoneValues {
            outlines: (0..27 * 65)
                .map(|t| {
                    let rings = rings.clone();
                    (t, ZoneOutline { value: 1, rings })
                })
                .collect(),
            ..outlined(&[], true)
        };
        assert_eq!(drawn(&grid(27, 65), &sheet(&square)).len(), 27 * 65);
        assert_eq!(drawn(&grid(27, 65), &sheet(&slope)).len(), 3 * 27 * 65);
    }

    #[test]
    fn the_zone_under_a_point_follows_the_layout() {
        let corners = ZoneLayout::Peers(MatchMode::Corners);
        assert_eq!(
            corners.cell_under(Vec2::new(0.9, 0.1)),
            Some(UVec2::new(1, 0))
        );
        assert_eq!(corners.cell_under(Vec2::splat(0.5)), None, "no center");
        let both = ZoneLayout::Peers(MatchMode::CornersAndSides);
        assert_eq!(both.cell_under(Vec2::splat(0.5)), Some(UVec2::ONE));
    }
}
