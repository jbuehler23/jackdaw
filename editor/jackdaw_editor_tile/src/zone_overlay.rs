use std::collections::{HashMap, HashSet};
use std::ops::Range;

use bevy::picking::events::{PointerMove, PointerOut};
use bevy::prelude::*;
use jackdaw_tilemap::{MatchMode, Peer};
use jackdaw_tileset::Tileset;

use crate::tileset_dock::{DockAction, DockControl, zoom};

/// Alpha of the dim layer over tiles that hold no value.
const DIM: f32 = 0.55;
/// Alpha of the tint on zones holding the picked value.
const TINT: f32 = 0.22;
/// Alpha of an edge line of a value other than the picked one.
const QUIET: f32 = 0.6;
/// Width of the edge line of the picked value, and of the others.
const PICKED_LINE: f32 = 2.0;
const QUIET_LINE: f32 = 1.0;
/// Blur of the glow around the picked value's edge lines.
const GLOW: f32 = 6.0;
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

/// One drawn part of an overlay.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub enum ZonePiece {
    /// Over a run of tiles holding no value.
    Dim,
    /// Over a run of zones holding the picked value.
    Tint(u32),
    /// A line along the side of a run of zones where `value` meets another.
    Edge { value: u32, picked: bool },
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
            ZonePiece::Tint(value) => {
                node.insert(BackgroundColor(color(values, value).with_alpha(TINT)));
            }
            ZonePiece::Edge {
                value,
                picked: false,
            } => {
                node.insert(BackgroundColor(color(values, value).with_alpha(QUIET)));
            }
            ZonePiece::Edge {
                value,
                picked: true,
            } => {
                let line = color(values, value);
                node.insert((
                    BackgroundColor(line),
                    BoxShadow::new(line.with_alpha(0.8), px(0), px(0), px(1), px(GLOW)),
                ));
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
/// belongs to. Each row draws its dim runs, then its tint runs, then the
/// edge lines of other values, then those of the picked value. Adjacent
/// pieces of one kind merge into runs along a row or a column of zones
/// within a tile row, so a row can be drawn again on its own.
pub fn pieces(
    grid: &SheetGrid,
    values: &ZoneValues,
    rows: Range<u32>,
) -> Vec<(u32, ZonePiece, Rect)> {
    let layout = values.layout;
    let n = layout.cells();
    let (width, height) = (grid.columns * n, grid.rows * n);
    let tile_of = |x: u32, y: u32| (y / n) * grid.columns + x / n;
    let value = |x: u32, y: u32| {
        let cell = UVec2::new(x % n, y % n);
        (values.tiles.get(&tile_of(x, y))).map_or(0, |peers| layout.value(peers, cell))
    };
    let rect = |x: u32, y: u32| grid.cell_rect(layout, tile_of(x, y), UVec2::new(x % n, y % n));
    let line = |value: u32| {
        let picked = values.picked == Some(value);
        let piece = ZonePiece::Edge { value, picked };
        (piece, if picked { PICKED_LINE } else { QUIET_LINE })
    };
    let mut out = Vec::new();
    for row in rows.start..rows.end.min(grid.rows) {
        let mut found = Vec::new();
        if values.dim {
            let mut runs = Runs::default();
            for column in 0..grid.columns {
                let tile = row * grid.columns + column;
                let held = (values.tiles.get(&tile)).is_some_and(|p| p.iter().any(|v| *v != 0));
                let at = grid.tiles.get(tile as usize).copied().unwrap_or_default();
                runs.push_across((!held).then_some(ZonePiece::Dim), at);
            }
            found.extend(runs.done());
        }
        let band = row * n..(row + 1) * n;
        if let Some(picked) = values.picked.filter(|p| *p != 0) {
            for y in band.clone() {
                let mut runs = Runs::default();
                for x in 0..width {
                    let piece = (value(x, y) == picked).then_some(ZonePiece::Tint(picked));
                    runs.push_across(piece, rect(x, y));
                }
                found.extend(runs.done());
            }
        }
        let mut edges = Vec::new();
        for x in 0..=width {
            let (mut before, mut after) = (Runs::default(), Runs::default());
            for y in band.clone() {
                let left = if x > 0 { value(x - 1, y) } else { 0 };
                let right = if x < width { value(x, y) } else { 0 };
                let differ = left != right;
                let side = |runs: &mut Runs, v: u32, cell: Option<Rect>, inward: f32| {
                    let piece = (differ && v != 0).then(|| line(v));
                    let at = cell.map(|c| {
                        let edge = if inward < 0.0 { c.max.x } else { c.min.x };
                        let t = piece.map_or(0.0, |(_, t)| t) * inward;
                        Rect::new(edge, c.min.y, edge + t, c.max.y)
                    });
                    runs.push_down(piece.map(|(p, _)| p), at.unwrap_or_default());
                };
                side(&mut before, left, (x > 0).then(|| rect(x - 1, y)), -1.0);
                side(&mut after, right, (x < width).then(|| rect(x, y)), 1.0);
            }
            edges.extend(before.done());
            edges.extend(after.done());
        }
        for y in band.start..=band.end {
            let (mut before, mut after) = (Runs::default(), Runs::default());
            let (above, below) = (y > band.start, y < band.end);
            for x in 0..width {
                let top = if y > 0 { value(x, y - 1) } else { 0 };
                let bottom = if y < height { value(x, y) } else { 0 };
                let differ = top != bottom;
                let side = |runs: &mut Runs, v: u32, cell: Option<Rect>, inward: f32| {
                    let piece = (differ && v != 0).then(|| line(v));
                    let at = cell.map(|c| {
                        let edge = if inward < 0.0 { c.max.y } else { c.min.y };
                        let t = piece.map_or(0.0, |(_, t)| t) * inward;
                        Rect::new(c.min.x, edge, c.max.x, edge + t)
                    });
                    runs.push_across(piece.map(|(p, _)| p), at.unwrap_or_default());
                };
                if above {
                    side(&mut before, top, Some(rect(x, y - 1)), -1.0);
                }
                if below {
                    side(&mut after, bottom, Some(rect(x, y)), 1.0);
                }
            }
            edges.extend(before.done());
            edges.extend(after.done());
        }
        edges.sort_by_key(|(piece, _)| matches!(piece, ZonePiece::Edge { picked: true, .. }));
        found.extend(edges);
        out.extend(found.into_iter().map(|(piece, at)| (row, piece, at)));
    }
    out
}

/// Pieces of one kind in a line, merged while each starts where the last
/// ends.
#[derive(Default)]
struct Runs {
    open: Option<(ZonePiece, Rect)>,
    done: Vec<(ZonePiece, Rect)>,
}

impl Runs {
    fn push_across(&mut self, piece: Option<ZonePiece>, at: Rect) {
        self.push(piece, at, |open, at| {
            (open.max.x - at.min.x).abs() < 0.01 && open.min.y == at.min.y && open.max.y == at.max.y
        });
    }

    fn push_down(&mut self, piece: Option<ZonePiece>, at: Rect) {
        self.push(piece, at, |open, at| {
            (open.max.y - at.min.y).abs() < 0.01 && open.min.x == at.min.x && open.max.x == at.max.x
        });
    }

    fn push(&mut self, piece: Option<ZonePiece>, at: Rect, joins: impl Fn(Rect, Rect) -> bool) {
        if let (Some(piece), Some((open, rect))) = (piece, self.open.as_mut())
            && *open == piece
            && joins(*rect, at)
        {
            *rect = rect.union(at);
            return;
        }
        self.done.extend(self.open.take());
        self.open = piece.map(|p| (p, at));
    }

    fn done(mut self) -> Vec<(ZonePiece, Rect)> {
        self.done.extend(self.open.take());
        self.done
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
    fn a_region_across_tile_seams_merges_into_one_tint_run_per_row() {
        let values = corners(&[(0, [1, 1, 1, 1]), (1, [1, 1, 1, 1])], Some(1));
        let found = drawn(&grid(3, 1), &values);
        let tints: Vec<Rect> = (found.iter())
            .filter(|(p, _)| *p == ZonePiece::Tint(1))
            .map(|(_, r)| *r)
            .collect();
        assert_eq!(tints.len(), 2, "one run per zone row: {tints:?}");
        assert_eq!(tints[0], Rect::new(0.0, 0.0, 64.0, 16.0));
        let edges = count(&found, |p| {
            matches!(p, ZonePiece::Edge { picked: true, .. })
        });
        assert_eq!(edges, 4, "one line per side of the region");
        let dims = (found.iter()).filter(|(p, _)| *p == ZonePiece::Dim).count();
        assert_eq!(dims, 1, "the empty third tile");
    }

    #[test]
    fn other_values_get_only_quiet_edges() {
        let values = corners(&[(0, [1, 2, 1, 2])], Some(1));
        let found = drawn(&grid(1, 1), &values);
        assert_eq!(count(&found, |p| *p == ZonePiece::Tint(2)), 0);
        let quiet = |p: &ZonePiece| {
            matches!(
                p,
                ZonePiece::Edge {
                    value: 2,
                    picked: false
                }
            )
        };
        assert_eq!(count(&found, quiet), 4, "the right half's outline");
        let line = (found.iter()).find(|(p, r)| quiet(p) && r.min.x == 16.0 && r.height() > 1.0);
        assert_eq!(line.map(|(_, r)| r.width()), Some(QUIET_LINE));
    }

    #[test]
    fn a_sides_corner_shows_its_sides_when_they_agree() {
        let layout = ZoneLayout::Peers(MatchMode::Sides);
        let peers = [0, 1, 0, 1, 1, 2, 0, 2, 0];
        assert_eq!(layout.value(&peers, UVec2::new(0, 0)), 1);
        assert_eq!(layout.value(&peers, UVec2::new(2, 0)), 0);
        assert_eq!(layout.value(&peers, UVec2::new(2, 2)), 2);
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
