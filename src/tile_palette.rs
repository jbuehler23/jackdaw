//! The Tile Palette window and the brush it loads: the terrains, tiles and
//! saved stamps of the active layer's tileset, a dragged rectangle of tiles as
//! a stamp, flip and rotate of a loaded tile or stamp, and the Bucket's
//! Contiguous toggle. Every change goes through an operator.

use std::path::Path;

use bevy::feathers::controls::FeathersCheckbox;
use bevy::feathers::theme::ThemedText;
use bevy::picking::events::{PointerDragEnd, PointerDragEnter, PointerDragStart};
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{Button, ValueChange};
use jackdaw_api::DefaultArea;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::operator::warn_caller;
use jackdaw_feathers::button::{ButtonOperatorCall, ButtonProps, ButtonVariant, button};
use jackdaw_feathers::scroll::{scroll_area, scrollbar};
use jackdaw_feathers::tokens;
use jackdaw_panels::area::DockWindow;
use jackdaw_scene_types::{Tilemap, TilemapLayerKind};
use jackdaw_tilemap::{TerrainId, TileRef};
use jackdaw_tileset::{TerrainMatch, Tileset, TilesetTile};

use crate::tile_stamp::{StampBrush, TileStamp, flipped_x, flipped_y, rectangle_tiles, rotated};
use crate::tile_tools::{BrushSource, TileBrush, TileEditMode, TileTool, has_tile_map};
use crate::tilemap_stroke::packed_tile;
use crate::tileset_dock::{sheet_image, spawn_text, swatch};

pub const TILE_PALETTE_WINDOW_ID: &str = "jackdaw.tile_palette";

/// On-screen size of one palette tile.
const CELL: f32 = 32.0;

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            refresh_on_tileset_events,
            refresh_on_state.run_if(
                |mode: Res<TileEditMode>, brush: Res<TileBrush>, stamp: Res<StampBrush>| {
                    mode.is_changed() || brush.is_changed() || stamp.is_changed()
                },
            ),
        ),
    )
    .init_resource::<PaletteDrag>()
    .add_observer(on_contiguous_change)
    .add_observer(on_tile_drag_start)
    .add_observer(on_tile_drag_enter)
    .add_observer(on_tile_drag_end);
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext, context: Entity) {
    ctx.register_window(
        WindowDescriptor::new(TILE_PALETTE_WINDOW_ID)
            .with_name("Tile Palette")
            .with_default_area(DefaultArea::BottomDock)
            .with_priority(60)
            .with_build(|window| {
                let parent = window.target_entity();
                build_palette(window.world_mut(), parent);
            }),
    );
    ctx.register_operator::<TilemapPalettePickOp>()
        .register_operator::<TilemapBrushFlipXOp>()
        .register_operator::<TilemapBrushFlipYOp>()
        .register_operator::<TilemapBrushRotateOp>()
        .register_operator::<TilemapBucketContiguousOp>();
    crate::tile_stamp::add_to_extension(ctx);
    ctx.bind_operator_in::<TilemapBrushFlipXOp>(context, [PresetInput::key("KeyX")])
        .bind_operator_in::<TilemapBrushFlipYOp>(context, [PresetInput::key("KeyY")])
        .bind_operator_in::<TilemapBrushRotateOp>(context, [PresetInput::key("KeyZ")]);
}

/// The content of one Tile Palette window, and what it was last built from.
#[derive(Component, Default)]
pub struct TilePalette {
    built: Option<Shown>,
}

#[derive(Clone, PartialEq)]
struct Shown {
    map: Option<Entity>,
    layer: usize,
    terrain_layer: bool,
    path: String,
    /// The tileset's asset, or why the path names none.
    tileset: Result<AssetId<Tileset>, String>,
}

/// The scrolling body of a palette.
#[derive(Component)]
pub struct PaletteBody;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PaletteSection {
    /// Shown while no map is in tile edit mode.
    Empty,
    Terrains,
    Tiles,
    Stamps,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteItem {
    Terrain(TerrainId),
    /// Sheet and tile.
    Tile(u32, u32),
    /// Index into the tileset's stamps.
    Stamp(usize),
}

#[derive(Component, Clone, Copy, Debug)]
pub struct PaletteCell {
    pub item: PaletteItem,
}

/// Marks the palette cells the brush holds.
#[derive(Component)]
pub struct PaletteSelected;

/// The first and current tile of a drag over the palette tiles.
#[derive(Resource, Default)]
struct PaletteDrag {
    from: Option<(u32, u32)>,
    to: Option<(u32, u32)>,
}

/// The Bucket's Contiguous checkbox.
#[derive(Component)]
pub struct ContiguousToggle;

/// Load the brush from the palette.
#[operator(
    id = "tilemap.palette.pick",
    label = "Pick From Tile Palette",
    description = "Load the brush with a terrain or a tile of the active layer's tileset, and \
                   choose the Brush.",
    allows_undo = false,
    is_available = has_tile_map,
    params(
        terrain(String, doc = "Terrain id or name."),
        tile(String, doc = "Tile as sheet:tile or packed layer, placed unturned."),
        to(
            String,
            doc = "Opposite corner from `tile` as sheet:tile on the same sheet: the \
                   rectangle between them becomes a stamp."
        ),
        stamp(String, doc = "Name of a stamp saved with the tileset."),
        stamp_index(i64, doc = "Index of a stamp saved with the tileset."),
    )
)]
pub(crate) fn tilemap_palette_pick(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    match pick(world, &params) {
        Ok(()) => OperatorResult::Finished,
        Err(err) => {
            warn_caller(world, format!("{}: {err}", TilemapPalettePickOp::ID));
            OperatorResult::Cancelled
        }
    }
}

fn pick(world: &mut World, params: &OperatorParameters) -> Result<(), String> {
    let text = |name: &str| crate::tileset_edit::text(params, name);
    let id = crate::tile_tools::active_tileset(world)?;
    let tileset =
        (world.resource::<Assets<Tileset>>().get(id)).ok_or("the layer's tileset is not loaded")?;
    let mut brush = *world.resource::<TileBrush>();
    let mut stamp = StampBrush {
        tileset: Some(id),
        ..default()
    };
    let (terrain, tile, to, saved) = (text("terrain"), text("tile"), text("to"), text("stamp"));
    let index = params.as_int("stamp_index");
    if !terrain.is_empty() {
        brush.terrain = crate::tileset_edit::terrain(tileset, &terrain)?;
        brush.source = BrushSource::Terrain;
    } else if !saved.is_empty() || index.is_some() {
        let at = index.and_then(|i| usize::try_from(i).ok());
        let found = match at {
            Some(at) => tileset.stamps.get(at),
            None => tileset.stamps.iter().find(|s| s.name == saved),
        };
        let found = found.ok_or("no such stamp is saved with the tileset")?;
        stamp.stamp = Some(TileStamp::from_saved(found)?);
        stamp.name = Some(found.name.clone());
        brush.source = BrushSource::Stamp;
    } else if !tile.is_empty() {
        let (sheet, from) = crate::tileset_edit::tile(tileset, &tile)?;
        let corner = match to.as_str() {
            "" => (sheet, from),
            to => crate::tileset_edit::tile(tileset, to)?,
        };
        if corner.0 != sheet {
            return Err("a rectangle of tiles stays on one sheet".to_string());
        }
        if corner.1 == from {
            brush.tile = packed_tile(tileset, &tile)?;
            brush.source = BrushSource::Tile;
        } else {
            let (block, tiles) = TileStamp::rectangle(tileset, sheet, from, corner.1)?;
            (stamp.stamp, stamp.tiles) = (Some(block), tiles);
            brush.source = BrushSource::Stamp;
        }
    } else {
        return Err("name a terrain, a tile or a stamp".to_string());
    }
    world.resource_mut::<TileEditMode>().tool = TileTool::Brush;
    world.resource_mut::<TileBrush>().set_if_neq(brush);
    world.resource_mut::<StampBrush>().set_if_neq(stamp);
    Ok(())
}

/// A tile or stamp brush is loaded: either source, or any brush on a tile
/// layer.
fn turns_brush(mode: Res<TileEditMode>, brush: Res<TileBrush>, maps: Query<&Tilemap>) -> bool {
    let layer =
        (mode.map.and_then(|map| maps.get(map).ok())).and_then(|t| t.layers.get(mode.layer));
    layer.is_some_and(|l| l.kind == TilemapLayerKind::Tile || brush.source != BrushSource::Terrain)
}

/// Applies `tile` to the loaded tile, or `stamp` to the loaded stamp.
fn turn(
    world: &mut World,
    tile: fn(TileRef) -> TileRef,
    stamp: fn(&mut TileStamp),
) -> OperatorResult {
    if world.resource::<TileBrush>().source != BrushSource::Stamp {
        let mut brush = world.resource_mut::<TileBrush>();
        brush.tile = tile(brush.tile);
    } else {
        let mut loaded = world.resource_mut::<StampBrush>();
        loaded.name = None;
        if let Some(block) = loaded.stamp.as_mut() {
            stamp(block);
        }
    }
    OperatorResult::Finished
}

/// Flip the loaded tile or stamp left to right.
#[operator(
    id = "tilemap.brush.flip_x",
    label = "Flip Brush Horizontally",
    description = "Flip the loaded tile or stamp left to right.",
    allows_undo = false,
    is_available = turns_brush
)]
pub(crate) fn tilemap_brush_flip_x(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    turn(world, flipped_x, TileStamp::flip_x)
}

/// Flip the loaded tile or stamp top to bottom.
#[operator(
    id = "tilemap.brush.flip_y",
    label = "Flip Brush Vertically",
    description = "Flip the loaded tile or stamp top to bottom.",
    allows_undo = false,
    is_available = turns_brush
)]
pub(crate) fn tilemap_brush_flip_y(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    turn(world, flipped_y, TileStamp::flip_y)
}

/// Turn the loaded tile or stamp a quarter clockwise.
#[operator(
    id = "tilemap.brush.rotate",
    label = "Rotate Brush",
    description = "Turn the loaded tile or stamp a quarter clockwise.",
    allows_undo = false,
    is_available = turns_brush
)]
pub(crate) fn tilemap_brush_rotate(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    turn(world, rotated, TileStamp::rotate)
}

/// Set whether a Bucket press fills only the joined cells.
#[operator(
    id = "tilemap.bucket.contiguous",
    label = "Bucket Contiguous",
    description = "Set whether a Bucket press fills only the matching cells joined to the \
                   pressed one, or every matching cell on the layer.",
    allows_undo = false,
    params(contiguous(bool, doc = "On fills the joined cells only; unset toggles."))
)]
pub(crate) fn tilemap_bucket_contiguous(
    params: In<OperatorParameters>,
    mut brush: ResMut<TileBrush>,
) -> OperatorResult {
    let on = params.as_bool("contiguous").unwrap_or(!brush.contiguous);
    brush.contiguous = on;
    OperatorResult::Finished
}

fn build_palette(world: &mut World, parent: Entity) {
    if world.get::<DockWindow>(parent).is_none() {
        return;
    }
    let root = world
        .spawn((
            TilePalette::default(),
            Node {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                min_height: px(0.0),
                ..default()
            },
            BackgroundColor(tokens::PANEL_BG),
            ChildOf(parent),
        ))
        .id();
    fill(world, root);
    mark(world);
}

/// The layer and tileset a palette shows now.
fn shown(world: &mut World) -> Shown {
    let mode = *world.resource::<TileEditMode>();
    let layer = (mode.map.and_then(|map| world.get::<Tilemap>(map)))
        .and_then(|map| map.layers.get(mode.layer));
    let path = layer.map(|l| l.tileset.clone()).unwrap_or_default();
    let terrain_layer = layer.is_some_and(|l| l.kind == TilemapLayerKind::Terrain);
    Shown {
        map: mode.map,
        layer: mode.layer,
        terrain_layer,
        path,
        tileset: crate::tile_tools::active_tileset(world),
    }
}

/// Build every palette again whose layer or tileset changed, or all of them
/// when `force`, then bring the highlight and the toggle up to date.
fn refresh(world: &mut World, force: bool) {
    let now = shown(world);
    let roots: Vec<Entity> = (world.query::<(Entity, &TilePalette)>().iter(world))
        .filter(|(_, p)| force || p.built.as_ref() != Some(&now))
        .map(|(e, _)| e)
        .collect();
    for root in roots {
        fill(world, root);
    }
    mark(world);
}

fn refresh_on_state(palettes: Query<(), With<TilePalette>>, mut commands: Commands) {
    if !palettes.is_empty() {
        commands.queue(|world: &mut World| refresh(world, false));
    }
}

fn refresh_on_tileset_events(
    mut events: MessageReader<AssetEvent<Tileset>>,
    palettes: Query<(), With<TilePalette>>,
    mut commands: Commands,
) {
    let changed = events.read().any(|event| {
        matches!(
            event,
            AssetEvent::Added { .. }
                | AssetEvent::Modified { .. }
                | AssetEvent::LoadedWithDependencies { .. }
        )
    });
    if changed && !palettes.is_empty() {
        commands.queue(|world: &mut World| refresh(world, true));
    }
}

/// Outlines the cells the brush holds and sets the Contiguous toggle.
fn mark(world: &mut World) {
    let now = shown(world);
    let cells: Vec<(Entity, PaletteItem, bool)> = (world
        .query::<(Entity, &PaletteCell, Has<PaletteSelected>)>()
        .iter(world))
    .map(|(e, cell, was)| (e, cell.item, was))
    .collect();
    let brush = *world.resource::<TileBrush>();
    let stamp = world.resource::<StampBrush>();
    let drag = world.resource::<PaletteDrag>();
    let tileset =
        (now.tileset.as_ref().ok()).and_then(|id| world.resource::<Assets<Tileset>>().get(*id));
    let held = tileset.and_then(|t| t.tile_of_layer(u32::from(brush.tile.index())));
    let saved = tileset.and_then(|t| stamp.saved_index(t));
    let dragging = drag.from.zip(drag.to).filter(|(a, b)| a != b);
    let lit: Vec<u32> = (dragging.zip(tileset))
        .and_then(|((a, b), t)| {
            Some(rectangle_tiles(
                t.sheets.get(a.0 as usize)?.columns,
                a.1,
                b.1,
            ))
        })
        .map_or(Vec::new(), |block| {
            block.into_iter().map(|(_, tile)| tile).collect()
        });
    let selected = |item: PaletteItem| match item {
        PaletteItem::Tile(sheet, tile) => {
            if let Some((from, _)) = dragging {
                sheet == from.0 && lit.contains(&tile)
            } else if brush.source == BrushSource::Stamp {
                saved.is_none() && stamp.tiles.contains(&(sheet, tile))
            } else {
                (!now.terrain_layer || brush.source == BrushSource::Tile)
                    && held == Some((sheet, tile))
            }
        }
        PaletteItem::Terrain(id) => {
            now.terrain_layer && brush.source == BrushSource::Terrain && brush.terrain == id
        }
        PaletteItem::Stamp(index) => brush.source == BrushSource::Stamp && saved == Some(index),
    };
    let changed: Vec<(Entity, bool)> = (cells.into_iter())
        .filter_map(|(cell, item, was)| (selected(item) != was).then_some((cell, !was)))
        .collect();
    for (cell, now) in changed {
        let outline = if now {
            tokens::SELECTED_BORDER
        } else {
            Color::NONE
        };
        let mut entity = world.entity_mut(cell);
        entity.insert(BorderColor::all(outline));
        if now {
            entity.insert(PaletteSelected);
        } else {
            entity.remove::<PaletteSelected>();
        }
    }
    let toggles: Vec<(Entity, bool)> = (world
        .query_filtered::<(Entity, Has<Checked>), With<ContiguousToggle>>())
    .iter(world)
    .collect();
    for (toggle, checked) in toggles {
        if checked != brush.contiguous {
            if brush.contiguous {
                world.entity_mut(toggle).insert(Checked);
            } else {
                world.entity_mut(toggle).remove::<Checked>();
            }
        }
    }
}

fn fill(world: &mut World, root: Entity) {
    let now = shown(world);
    world.entity_mut(root).despawn_related::<Children>();
    *world.resource_mut::<PaletteDrag>() = PaletteDrag::default();
    if let Some(mut palette) = world.get_mut::<TilePalette>(root) {
        palette.built = Some(now.clone());
    }
    let id = if now.map.is_none() {
        Err((
            "Select a tilemap and enter tile edit mode to paint.".to_string(),
            false,
        ))
    } else {
        (now.tileset.clone()).map_err(|err| (format!("The layer's tileset: {err}."), true))
    };
    let tileset = id.and_then(|id| {
        (world.resource::<Assets<Tileset>>().get(id).cloned())
            .ok_or_else(|| ("The layer's tileset is loading.".to_string(), false))
    });
    let tileset = match tileset {
        Ok(tileset) => tileset,
        Err((hint, error)) => {
            let color = if error {
                tokens::TEXT_ERROR
            } else {
                tokens::TEXT_SECONDARY
            };
            let text = spawn_text(world, root, hint, color);
            world.entity_mut(text).insert(PaletteSection::Empty);
            return;
        }
    };
    let header = world
        .spawn((
            Node {
                column_gap: px(tokens::SPACING_MD),
                align_items: AlignItems::Center,
                padding: UiRect::all(px(tokens::SPACING_SM)),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    let name = Path::new(&now.path)
        .file_name()
        .map_or(now.path.clone(), |n| n.to_string_lossy().into_owned());
    spawn_text(world, header, name, tokens::TEXT_PRIMARY);
    spawn_contiguous(world, header);
    world.spawn((
        button(ButtonProps::new("Save Stamp").with_variant(ButtonVariant::Ghost)),
        ButtonOperatorCall::new(crate::tile_stamp::TilemapStampSaveOp::ID),
        ChildOf(header),
    ));
    let row = world
        .spawn((
            Node {
                flex_grow: 1.0,
                min_height: px(0.0),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    let body = world
        .spawn((
            PaletteBody,
            Node {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                row_gap: px(tokens::SPACING_MD),
                padding: UiRect::all(px(tokens::SPACING_SM)),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            scroll_area(),
            ChildOf(row),
        ))
        .id();
    world.spawn((scrollbar(body), ChildOf(row)));
    let images: Vec<Option<Handle<Image>>> = (tileset.sheets.iter())
        .map(|sheet| sheet_image(world, &sheet.source))
        .collect();
    if now.terrain_layer {
        let terrains = section(world, body, PaletteSection::Terrains, "Terrains");
        for terrain in &tileset.terrains {
            let cell = spawn_cell(world, terrains, PaletteItem::Terrain(terrain.id));
            let call = ButtonOperatorCall::new(TilemapPalettePickOp::ID)
                .with_param("terrain", terrain.id.to_string());
            world.entity_mut(cell).insert(call);
            match centre_tile(&tileset, terrain.id) {
                Some((sheet, tile)) => tile_image(world, cell, &tileset, &images, sheet, tile),
                None => {
                    let color = swatch(&tileset, terrain.id, 1.0);
                    world.entity_mut(cell).insert(BackgroundColor(color));
                }
            }
        }
    }
    let tiles = section(world, body, PaletteSection::Tiles, "Tiles");
    world.entity_mut(tiles).insert(Node {
        flex_direction: FlexDirection::Column,
        row_gap: px(tokens::SPACING_SM),
        ..default()
    });
    for (sheet, source) in (0..).zip(&tileset.sheets) {
        let grid = world
            .spawn((
                Node {
                    display: Display::Grid,
                    grid_template_columns: RepeatedGridTrack::px(
                        u16::try_from(source.columns.max(1)).unwrap_or(u16::MAX),
                        CELL,
                    ),
                    ..default()
                },
                ChildOf(tiles),
            ))
            .id();
        for tile in 0..source.columns * source.rows {
            let cell = spawn_cell(world, grid, PaletteItem::Tile(sheet, tile));
            let call = ButtonOperatorCall::new(TilemapPalettePickOp::ID)
                .with_param("tile", format!("{sheet}:{tile}"));
            world.entity_mut(cell).insert(call);
            tile_image(world, cell, &tileset, &images, sheet, tile);
        }
    }
    let stamps = section(world, body, PaletteSection::Stamps, "Stamps");
    if tileset.stamps.is_empty() {
        let hint = "Drag over tiles for a stamp, then save it.";
        spawn_text(world, stamps, hint, tokens::TEXT_SECONDARY);
    }
    for (index, saved) in tileset.stamps.iter().enumerate() {
        world.spawn((
            button(ButtonProps::new(saved.name.clone())),
            BorderColor::all(Color::NONE),
            PaletteCell {
                item: PaletteItem::Stamp(index),
            },
            ButtonOperatorCall::new(TilemapPalettePickOp::ID)
                .with_param("stamp_index", i64::try_from(index).unwrap_or(i64::MAX)),
            ChildOf(stamps),
        ));
    }
}

/// A titled section of the palette body, and the row its cells wrap in.
fn section(world: &mut World, body: Entity, kind: PaletteSection, title: &str) -> Entity {
    let column = world
        .spawn((
            kind,
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_SM),
                ..default()
            },
            ChildOf(body),
        ))
        .id();
    spawn_text(world, column, title, tokens::TEXT_SECONDARY);
    world
        .spawn((
            Node {
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(tokens::SPACING_XS),
                row_gap: px(tokens::SPACING_XS),
                ..default()
            },
            ChildOf(column),
        ))
        .id()
}

fn spawn_cell(world: &mut World, parent: Entity, item: PaletteItem) -> Entity {
    world
        .spawn((
            Node {
                width: px(CELL),
                height: px(CELL),
                border: UiRect::all(px(2.0)),
                ..default()
            },
            BorderColor::all(Color::NONE),
            Button,
            PaletteCell { item },
            ChildOf(parent),
        ))
        .id()
}

/// Draws tile `tile` of sheet `sheet` inside `cell`, cut from that sheet's
/// image in `images`.
fn tile_image(
    world: &mut World,
    cell: Entity,
    tileset: &Tileset,
    images: &[Option<Handle<Image>>],
    sheet: u32,
    tile: u32,
) {
    let image = images.get(sheet as usize).cloned().flatten();
    let (Some(image), Some((min, size))) = (image, tileset.sheet_rect(sheet, tile)) else {
        return;
    };
    world.spawn((
        Node {
            width: percent(100.0),
            height: percent(100.0),
            ..default()
        },
        ImageNode {
            image,
            rect: Some(Rect::from_corners(min.as_vec2(), (min + size).as_vec2())),
            ..default()
        },
        Pickable::IGNORE,
        ChildOf(cell),
    ));
}

/// A tile showing only `terrain`: one with all nine peers on it, else one
/// with every peer its terrain set matches on it (the corners of a corners
/// set, the sides and centre of a sides set).
fn centre_tile(tileset: &Tileset, terrain: TerrainId) -> Option<(u32, u32)> {
    const CORNERS: &[usize] = &[0, 2, 6, 8];
    const SIDES: &[usize] = &[1, 3, 4, 5, 7];
    const ALL: &[usize] = &[0, 1, 2, 3, 4, 5, 6, 7, 8];
    let used = |inner: TerrainId, outer: TerrainId| {
        let set = (tileset.terrain_sets.iter()).find(|s| s.inner == inner && s.outer == outer);
        match set.map(|s| s.mode) {
            Some(TerrainMatch::Corners) => CORNERS,
            Some(TerrainMatch::Sides) => SIDES,
            _ => ALL,
        }
    };
    let shows = |t: &&TilesetTile, peers: &[usize]| peers.iter().all(|&p| t.peers[p] == terrain);
    (tileset.tiles.iter())
        .find(|t| shows(t, ALL))
        .or_else(|| (tileset.tiles.iter()).find(|t| shows(t, used(t.set_inner, t.set_outer))))
        .map(|t| (t.sheet, t.tile))
}

fn spawn_contiguous(world: &mut World, parent: Entity) {
    let scene = bsn! {
        @FeathersCheckbox {
            @caption: bsn! { Text("Contiguous") ThemedText }
        }
    };
    match world.spawn_scene(scene) {
        Ok(mut checkbox) => {
            checkbox.insert((ContiguousToggle, ChildOf(parent)));
        }
        Err(err) => warn!("tile palette: {err}"),
    }
}

fn on_contiguous_change(
    change: On<ValueChange<bool>>,
    toggles: Query<(), With<ContiguousToggle>>,
    mut commands: Commands,
) {
    if !toggles.contains(change.source) {
        return;
    }
    let on = change.value;
    commands.queue(move |world: &mut World| {
        let called = world
            .operator(TilemapBucketContiguousOp::ID)
            .param("contiguous", on)
            .call();
        if let Err(err) = called {
            warn!("{}: {err}", TilemapBucketContiguousOp::ID);
        }
    });
}

fn on_tile_drag_start(
    start: On<PointerDragStart>,
    cells: Query<&PaletteCell>,
    mut drag: ResMut<PaletteDrag>,
) {
    if start.button == PointerButton::Primary
        && let Ok(PaletteCell {
            item: PaletteItem::Tile(sheet, tile),
        }) = cells.get(start.event_target())
    {
        *drag = PaletteDrag {
            from: Some((*sheet, *tile)),
            to: Some((*sheet, *tile)),
        };
    }
}

/// A drag from a palette tile over another of its sheet lights the
/// rectangle between them.
fn on_tile_drag_enter(
    enter: On<PointerDragEnter>,
    cells: Query<&PaletteCell>,
    mut drag: ResMut<PaletteDrag>,
    mut commands: Commands,
) {
    if let Ok(PaletteCell {
        item: PaletteItem::Tile(sheet, tile),
    }) = cells.get(enter.event_target())
        && drag.from.is_some_and(|from| from.0 == *sheet)
        && drag.to != Some((*sheet, *tile))
    {
        drag.to = Some((*sheet, *tile));
        commands.queue(mark);
    }
}

/// The end of a drag over the palette tiles loads their rectangle as a
/// stamp.
fn on_tile_drag_end(_: On<PointerDragEnd>, mut drag: ResMut<PaletteDrag>, mut commands: Commands) {
    let (Some(from), Some(to)) = (drag.from.take(), drag.to.take()) else {
        return;
    };
    commands.queue(move |world: &mut World| {
        if from != to {
            let called = world
                .operator(TilemapPalettePickOp::ID)
                .param("tile", format!("{}:{}", from.0, from.1))
                .param("to", format!("{}:{}", to.0, to.1))
                .call();
            if let Err(err) = called {
                warn!("{}: {err}", TilemapPalettePickOp::ID);
            }
        }
        mark(world);
    });
}
