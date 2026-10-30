//! The Tile Palette window and the brush it loads: the terrains and tiles of
//! the active layer's tileset, flip and rotate of a loaded tile, and the
//! Bucket's Contiguous toggle. Every change goes through an operator.

use std::path::Path;

use bevy::feathers::controls::FeathersCheckbox;
use bevy::feathers::theme::ThemedText;
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{Button, ValueChange};
use jackdaw_api::DefaultArea;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::operator::warn_caller;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::scroll::{scroll_area, scrollbar};
use jackdaw_feathers::tokens;
use jackdaw_panels::area::DockWindow;
use jackdaw_scene_types::{Tilemap, TilemapLayerKind};
use jackdaw_tilemap::{TerrainId, TileRef};
use jackdaw_tileset::{TerrainMatch, Tileset, TilesetTile};

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
            refresh_on_state.run_if(|mode: Res<TileEditMode>, brush: Res<TileBrush>| {
                mode.is_changed() || brush.is_changed()
            }),
        ),
    )
    .add_observer(on_contiguous_change);
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
    ctx.bind_operator_in::<TilemapBrushFlipXOp>(context, [PresetInput::key("KeyX")])
        .bind_operator_in::<TilemapBrushFlipYOp>(context, [PresetInput::key("KeyY")])
        .bind_operator_in::<TilemapBrushRotateOp>(context, [PresetInput::key("KeyZ")]);
}

fn flipped_x(tile: TileRef) -> TileRef {
    tile.with_flip_x(!tile.flip_x())
}

fn flipped_y(tile: TileRef) -> TileRef {
    tile.with_flip_y(!tile.flip_y())
}

/// A quarter turn clockwise: the tile is transposed, then flipped across.
fn rotated(tile: TileRef) -> TileRef {
    let (x, y) = (tile.flip_x(), tile.flip_y());
    (tile.with_transpose(!tile.transpose()))
        .with_flip_x(!y)
        .with_flip_y(x)
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteItem {
    Terrain(TerrainId),
    /// Sheet and tile.
    Tile(u32, u32),
}

#[derive(Component, Clone, Copy, Debug)]
pub struct PaletteCell {
    pub item: PaletteItem,
}

/// Marks the palette cells the brush holds.
#[derive(Component)]
pub struct PaletteSelected;

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
    let (terrain, tile) = (text("terrain"), text("tile"));
    if !terrain.is_empty() {
        brush.terrain = crate::tileset_edit::terrain(tileset, &terrain)?;
        brush.source = BrushSource::Terrain;
    } else if !tile.is_empty() {
        brush.tile = packed_tile(tileset, &tile)?;
        brush.source = BrushSource::Tile;
    } else {
        return Err("name a terrain or a tile".to_string());
    }
    world.resource_mut::<TileEditMode>().tool = TileTool::Brush;
    world.resource_mut::<TileBrush>().set_if_neq(brush);
    Ok(())
}

/// A tile brush is loaded: the Tile source, or any brush on a tile layer.
fn turns_brush(mode: Res<TileEditMode>, brush: Res<TileBrush>, maps: Query<&Tilemap>) -> bool {
    let layer =
        (mode.map.and_then(|map| maps.get(map).ok())).and_then(|t| t.layers.get(mode.layer));
    layer.is_some_and(|l| l.kind == TilemapLayerKind::Tile || brush.source == BrushSource::Tile)
}

fn turn(world: &mut World, change: fn(TileRef) -> TileRef) -> OperatorResult {
    let mut brush = world.resource_mut::<TileBrush>();
    brush.tile = change(brush.tile);
    OperatorResult::Finished
}

/// Flip the loaded tile left to right.
#[operator(
    id = "tilemap.brush.flip_x",
    label = "Flip Brush Horizontally",
    description = "Flip the loaded tile left to right.",
    allows_undo = false,
    is_available = turns_brush
)]
pub(crate) fn tilemap_brush_flip_x(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    turn(world, flipped_x)
}

/// Flip the loaded tile top to bottom.
#[operator(
    id = "tilemap.brush.flip_y",
    label = "Flip Brush Vertically",
    description = "Flip the loaded tile top to bottom.",
    allows_undo = false,
    is_available = turns_brush
)]
pub(crate) fn tilemap_brush_flip_y(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    turn(world, flipped_y)
}

/// Turn the loaded tile a quarter clockwise.
#[operator(
    id = "tilemap.brush.rotate",
    label = "Rotate Brush",
    description = "Turn the loaded tile a quarter clockwise.",
    allows_undo = false,
    is_available = turns_brush
)]
pub(crate) fn tilemap_brush_rotate(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    turn(world, rotated)
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
    let brush = *world.resource::<TileBrush>();
    let tile_source = !now.terrain_layer || brush.source == BrushSource::Tile;
    let tileset = (now.tileset.ok()).and_then(|id| world.resource::<Assets<Tileset>>().get(id));
    let held = tileset.and_then(|t| t.tile_of_layer(u32::from(brush.tile.index())));
    let selected = |item: PaletteItem| match item {
        PaletteItem::Terrain(id) => !tile_source && brush.terrain == id,
        PaletteItem::Tile(sheet, tile) => tile_source && held == Some((sheet, tile)),
    };
    let cells: Vec<(Entity, bool, bool)> = (world
        .query::<(Entity, &PaletteCell, Has<PaletteSelected>)>()
        .iter(world))
    .map(|(e, cell, was)| (e, was, selected(cell.item)))
    .collect();
    for (cell, was, now) in cells {
        if was == now {
            continue;
        }
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
    if let Some(mut palette) = world.get_mut::<TilePalette>(root) {
        palette.built = Some(now.clone());
    }
    let id = match (&now.map, &now.tileset) {
        (None, _) => Err((
            "Select a tilemap and enter tile edit mode to paint.".to_string(),
            false,
        )),
        (Some(_), Err(err)) => Err((format!("The layer's tileset: {err}."), true)),
        (Some(_), Ok(id)) => Ok(*id),
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
