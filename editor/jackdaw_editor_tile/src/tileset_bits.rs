use std::collections::{BTreeSet, HashMap};

use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::tokens;
use jackdaw_tilemap::{Coverage, MatchMode, Peer, TerrainId, TileAddress, TileTerrains};
use jackdaw_tilemap::{SetupTile, TilesetSetup};
use jackdaw_tileset::Tileset;

use crate::sheet_stroke::{SheetPaint, StrokeInfo, StrokeStart};
use crate::tileset_dock::{DockCall, DockView, PaintedCount, TilesetDockViews, swatch};
use crate::tileset_edit::{
    peer, queue_edit, terrain_or_empty, terrain_set, text, tile, with_setup,
};
use crate::tileset_layout::GhostZones;
use crate::tileset_modes::ModeSheet;
use crate::zone_overlay::{
    ZoneLayout, ZoneOverlay, ZoneValues, redraw_zone_rows, spawn_zone_overlay,
};

/// On-screen size of a missing shape's glyph.
const GLYPH: f32 = 21.0;

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

/// Paint one terrain on many tile zones as one history entry.
#[operator(
    id = "tileset.tile.paint_bits",
    writes_disk = true,
    label = "Paint Tile Bits",
    description = "Paint a terrain on zones of sheet tiles in one terrain set, as one undo \
                   entry; zones its match mode does not use and tiles of objects are left. A \
                   sheet outside terrain setup comes under it in the same entry. Nothing \
                   changed adds no entry.",
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
            let mut covered = tileset.clone();
            for sheet in zone_sheets(tileset, &zones)? {
                covered.cover_sheet(sheet).map_err(|e| e.to_string())?;
            }
            let mut changed = 0;
            let message = with_setup(&mut covered, |tileset, setup| {
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
                changed = (setup.paint_zones(index, id, &zones)).map_err(|e| setup.explain(e))?;
                Ok(format!("{changed} of {} zones painted", zones.len()))
            })?;
            if changed > 0 {
                *tileset = covered;
            }
            Ok(message)
        },
    )
}

/// The sheets the zones `text` lists fall on.
fn zone_sheets(tileset: &Tileset, text: &str) -> Result<BTreeSet<u32>, String> {
    (text.split(',').map(str::trim))
        .filter(|z| !z.is_empty())
        .map(|zone| {
            let (at, _) = (zone.rsplit_once(':')).ok_or(format!("`{zone}` is no zone"))?;
            Ok(tile(tileset, at)?.0)
        })
        .collect()
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

/// The zones of sheet `sheet` held by tiles of the set the dock paints in,
/// the brush terrain picked.
fn terrain_zones(
    tileset: &Tileset,
    setup: &TilesetSetup,
    sheet: u32,
    view: &DockView,
) -> Option<ZoneValues> {
    let on = tileset.setup_sheet(sheet).ok();
    let active = view.paint_set(tileset)?;
    let set = setup.sets.get(active)?;
    let mode = set.mode.value;
    let tiles = (setup.tiles.iter())
        .filter(|(a, tile)| Some(a.sheet) == on && shows(setup, active, tile))
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
        outlines: HashMap::new(),
    })
}

/// A stroke of the terrains mode: the brush terrain painted on tile zones
/// in its set, or erased with the right button.
struct BitPaint {
    set: usize,
    mode: MatchMode,
    terrain: TerrainId,
    /// The tileset with the stroke's sheet under terrain setup.
    tileset: Tileset,
    /// The setup with the stroke painted so far.
    setup: TilesetSetup,
    zones: BTreeSet<(u32, Peer)>,
}

/// Start a stroke of the terrains mode: right erases, left paints the brush.
pub(crate) fn start_stroke(world: &mut World, start: &StrokeStart) -> Option<Box<dyn SheetPaint>> {
    let terrain = match start.button {
        PointerButton::Primary => start.view.brush,
        _ => 0,
    };
    let mut covered = start.tileset.clone();
    covered.cover_sheet(start.sheet).ok()?;
    let tileset = &covered;
    let setup = tileset.setup().ok()?;
    let set = (start.view.paint_set(tileset))
        .filter(|s| setup.sets.get(*s).is_some_and(|s| s.has(terrain)));
    let Some((set, mode)) = set.and_then(|s| Some((s, setup.sets.get(s)?.mode.value))) else {
        if terrain != 0 {
            let name = tileset.terrain_name(terrain);
            let notice = format!("no terrain set holds {name}; add one to paint its bits");
            jackdaw_editor_core::status_bar::notify_info(world, notice);
        }
        return None;
    };
    Some(Box::new(BitPaint {
        set,
        mode,
        terrain,
        setup,
        tileset: covered,
        zones: BTreeSet::new(),
    }))
}

impl SheetPaint for BitPaint {
    fn samples(&self) -> f32 {
        8.0
    }

    fn paint(&mut self, world: &mut World, stroke: &StrokeInfo, frame: Entity, points: &[Vec2]) {
        let mut fresh = Vec::new();
        for point in points {
            let zone = stroke.tileset.zone_at(stroke.sheet, *point, self.mode);
            if let Some(zone) = zone.filter(|z| self.zones.insert(*z)) {
                fresh.push(zone);
            }
        }
        self.show(world, stroke, frame, &fresh);
    }

    fn apply(&self, stroke: &StrokeInfo) -> Option<DockCall> {
        let zones: Vec<String> = (self.zones.iter())
            .map(|(tile, peer)| format!("{}:{tile}:{}", stroke.sheet, peer.index()))
            .collect();
        (!zones.is_empty()).then(|| {
            let params = vec![
                ("set", self.set.to_string()),
                ("terrain", self.terrain.to_string()),
                ("zones", zones.join(",")),
            ];
            ("tileset.tile.paint_bits", params)
        })
    }
}

impl BitPaint {
    /// Paint the new zones into the stroke's setup, draw them over `frame`
    /// and show the active set's coverage as the stroke would leave it.
    fn show(
        &mut self,
        world: &mut World,
        stroke: &StrokeInfo,
        frame: Entity,
        fresh: &[(u32, Peer)],
    ) {
        let Ok(on) = self.tileset.setup_sheet(stroke.sheet) else {
            return;
        };
        if fresh.is_empty() {
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
        if self
            .setup
            .paint_zones(self.set, self.terrain, &zones)
            .is_err()
        {
            return;
        }
        let root = stroke.root;
        self.redraw(world, stroke, frame, fresh);
        let count = painted_count(&self.setup);
        if let Ok(mut held) = world.get_entity_mut(root)
            && let Some(mut shown) = held.get_mut::<PaintedCount>()
        {
            shown.0 = count;
        }
        let Ok(coverage) = self.setup.coverage(self.set) else {
            return;
        };
        let set = self.set;
        let summary = summary_line(&coverage);
        retext::<CoverageSummary>(world, |l| l.root == root && l.set == set, &summary);
        let rows: Vec<(Entity, MissingShapes)> = (world.query::<(Entity, &MissingShapes)>())
            .iter(world)
            .filter(|(_, m)| m.root == root && m.set == set)
            .map(|(e, m)| (e, *m))
            .collect();
        for (row, shapes) in rows {
            world.entity_mut(row).despawn_related::<Children>();
            spawn_missing(world, row, shapes, &self.tileset, &coverage);
        }
    }

    /// Draw the tile rows of the stroke's sheet holding the `fresh` zones
    /// again from the stroke's setup. The whole sheet is drawn again once
    /// the stroke applies.
    fn redraw(&self, world: &mut World, stroke: &StrokeInfo, frame: Entity, fresh: &[(u32, Peer)]) {
        let Some(view) = (world.resource::<TilesetDockViews>().0.get(&stroke.path)).cloned() else {
            return;
        };
        let Some(overlay) = sheet_overlay(world, stroke.root, frame) else {
            let sheet = ModeSheet {
                root: stroke.root,
                frame,
                sheet: stroke.sheet,
                tileset: &self.tileset,
                setup: &self.setup,
                view: &view,
            };
            terrain_overlay(world, &sheet);
            return;
        };
        let Some(values) = terrain_zones(&self.tileset, &self.setup, stroke.sheet, &view) else {
            return;
        };
        let columns =
            (stroke.tileset.sheets.get(stroke.sheet as usize)).map_or(1, |s| s.columns.max(1));
        let rows: BTreeSet<u32> = fresh.iter().map(|(tile, _)| tile / columns).collect();
        redraw_zone_rows(world, overlay, values, rows);
    }
}

/// The zone overlay of the dock `root` drawn in `frame`.
pub(crate) fn sheet_overlay(world: &mut World, root: Entity, frame: Entity) -> Option<Entity> {
    (world.query_filtered::<(Entity, &ZoneOverlay, &ChildOf), Without<GhostZones>>())
        .iter(world)
        .find(|(_, o, parent)| o.root == root && parent.parent() == frame)
        .map(|(e, ..)| e)
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
