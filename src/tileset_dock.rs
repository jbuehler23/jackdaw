use std::collections::{HashMap, HashSet};
use std::path::Path;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::input::keyboard::KeyboardInput;
use bevy::input_focus::{FocusCause, FocusedInput, InputFocus};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui_widgets::{Activate, Button, ValueChange};
use jackdaw_api::DefaultArea;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_feathers::button::{ButtonProps, ButtonVariant, button};
use jackdaw_feathers::combobox::{ComboBoxChangeEvent, combobox_compact_with_selected};
use jackdaw_feathers::{segmented, tokens};
use jackdaw_panels::area::DockWindow;
use jackdaw_panels::tree::{DockTree, TabId};
use jackdaw_tilemap::{TerrainId, TilesetSetup};
use jackdaw_tileset::{TerrainCollision, Tileset, TilesetTerrain};

use crate::asset_index::assets_dir;
use crate::definition_assets::{entry_for_file, resolve_project_path};
use crate::tileset_edit::{open_tileset, text};
use crate::tileset_import::decode_sheet;

pub const TILESET_WINDOW_ID: &str = "jackdaw.tileset";

/// On-screen pixels per sheet pixel.
const SCALE: f32 = 2.0;
/// On-screen size of the selected tile's preview.
const PREVIEW: f32 = 96.0;
/// Opacity of the terrain swatch laid over each sheet tile.
const TINT: f32 = 0.35;
const FLAGGED: Color = tokens::TEXT_WARNING;
const COLLISIONS: [&str; 3] = ["solid", "trigger", "passable"];

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TilesetDockViews>()
        .init_resource::<SheetImages>()
        .add_systems(Update, refresh_on_tileset_events)
        .add_observer(on_activate)
        .add_observer(on_page_change)
        .add_observer(on_collision_pick)
        .add_observer(prune_views);
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_window(
        WindowDescriptor::new(TILESET_WINDOW_ID)
            .with_name("Tileset")
            .with_default_area(DefaultArea::Center)
            .with_priority(50)
            .unlisted()
            .with_build(|window| {
                let parent = window.target_entity();
                build_dock(window.world_mut(), parent);
            }),
    );
    ctx.register_operator::<TilesetEditOp>();
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DockPage {
    #[default]
    Review,
    Terrains,
    Objects,
}

/// What one tileset tab shows; kept across rebuilds of its content.
#[derive(Clone, Debug, Default)]
pub struct DockView {
    pub page: DockPage,
    /// Sheet and tile.
    pub selected: Option<(u32, u32)>,
    /// Terrain a peer click paints; 0 is empty.
    pub brush: TerrainId,
    /// First corner of the object rectangle; `selected` is the other.
    pub anchor: Option<(u32, u32)>,
}

/// The view of each open tileset, by its path. A tileset has one tab, so the
/// view follows the tab wherever it is docked.
#[derive(Resource, Default)]
pub struct TilesetDockViews(pub HashMap<String, DockView>);

/// Nearest-sampled copies of the source sheets the docks draw, by path.
#[derive(Resource, Default)]
struct SheetImages(HashMap<String, Handle<Image>>);

/// The content of one tileset tab.
#[derive(Component, Clone, Debug)]
pub struct TilesetDock {
    pub tab: TabId,
    /// The tileset path the tab carries.
    pub path: Option<String>,
    /// The tileset asset shown, once its file is indexed.
    pub asset: Option<AssetId<Tileset>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DockAction {
    Page(DockPage),
    Tile(u32, u32),
    Brush(TerrainId),
    Peer(usize),
    Accept,
    Reset,
    AddObject,
    RemoveObject(u32, UVec2),
}

/// A control of the tileset dock `root`.
#[derive(Component, Clone, Copy, Debug)]
pub struct DockControl {
    pub root: Entity,
    pub action: DockAction,
}

/// The collision combo of terrain `terrain`.
#[derive(Component, Clone, Copy, Debug)]
pub struct CollisionCombo {
    pub root: Entity,
    pub terrain: TerrainId,
}

/// Open a tileset in the Tileset window.
#[operator(
    id = "tileset.edit",
    label = "Edit Tileset",
    description = "Open a tileset in its own Tileset window tab, or bring its tab to the front.",
    allows_undo = false,
    params(path(
        String,
        doc = "Tileset file under the project's assets; defaults to the open tileset."
    ))
)]
pub fn tileset_edit(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let path = text(&params, "path");
    commands.queue(move |world: &mut World| match open_tab(world, &path) {
        Ok(message) => report_to_caller(world, message),
        Err(err) => warn_caller(world, format!("tileset.edit: {err}")),
    });
    OperatorResult::Finished
}

fn open_tab(world: &mut World, path: &str) -> Result<String, String> {
    let (path, tileset) = open_tileset(world, path)?;
    let payload = path.to_string_lossy().into_owned();
    let open = (world.resource::<DockTree>().tabs())
        .find(|(_, t)| t.window_id == TILESET_WINDOW_ID && t.payload.as_deref() == Some(&payload))
        .map(|(leaf, t)| (leaf, t.id));
    if let Some((leaf, tab)) = open {
        world.resource_mut::<DockTree>().set_active(leaf, tab);
        return Ok(format!("{payload} is in front"));
    }
    let tab = crate::open_window_in_default_area(world, TILESET_WINDOW_ID)
        .ok_or("no dock area takes the Tileset window")?;
    let mut tree = world.resource_mut::<DockTree>();
    let entry = tree.tab_mut(tab).ok_or("the new tab is gone")?;
    entry.title = path.file_name().map(|n| n.to_string_lossy().into_owned());
    entry.payload = Some(payload.clone());
    (world.resource_mut::<TilesetDockViews>().0).insert(payload.clone(), first_view(&tileset));
    Ok(format!("{payload} opened"))
}

fn first_view(tileset: &Tileset) -> DockView {
    DockView {
        selected: flagged(tileset).first().copied(),
        ..default()
    }
}

/// Flagged tiles as sheet and tile, in setup order.
pub fn flagged(tileset: &Tileset) -> Vec<(u32, u32)> {
    let Ok(setup) = tileset.setup() else {
        return Vec::new();
    };
    (setup.flagged().into_iter())
        .filter_map(|a| Some((*tileset.setup_sheets.get(a.sheet)?, a.tile)))
        .collect()
}

/// The tileset at `path` and its asset id; `None` while it loads.
fn load(world: &mut World, path: &str) -> Result<(AssetId<Tileset>, Option<Tileset>), String> {
    let file = resolve_project_path(world, Path::new(path));
    let entry = entry_for_file(world, &file)
        .ok_or_else(|| format!("{path} is not an asset file of this project"))?;
    if entry.type_path != Tileset::type_path() {
        return Err(format!("{path} is not a tileset"));
    }
    let id = (entry.value.handle())
        .ok_or("the tileset cannot be loaded")?
        .id()
        .typed::<Tileset>();
    Ok((id, world.resource::<Assets<Tileset>>().get(id).cloned()))
}

fn build_dock(world: &mut World, parent: Entity) {
    let Some(tab) = world.get::<DockWindow>(parent).map(|w| w.tab_id) else {
        return;
    };
    let path = (world.resource::<DockTree>().tab(tab)).and_then(|t| t.payload.clone());
    let root = world
        .spawn((
            TilesetDock {
                tab,
                path,
                asset: None,
            },
            Node {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                min_height: px(0.0),
                ..default()
            },
            BackgroundColor(tokens::PANEL_BG),
            ChildOf(parent),
        ))
        .observe(on_dock_key)
        .id();
    fill(world, root);
}

/// Build `root`'s content again from its tileset and view.
fn fill(world: &mut World, root: Entity) {
    let Some(path) = world.get::<TilesetDock>(root).map(|d| d.path.clone()) else {
        return;
    };
    world.entity_mut(root).despawn_related::<Children>();
    let Some(path) = path else {
        spawn_text(world, root, "this tab names no tileset", tokens::TEXT_ERROR);
        return;
    };
    match load(world, &path) {
        Ok((id, Some(tileset))) => {
            set_asset(world, root, id);
            let view = (world.resource::<TilesetDockViews>().0.get(&path).cloned())
                .unwrap_or_else(|| first_view(&tileset));
            (world.resource_mut::<TilesetDockViews>().0).insert(path.clone(), view.clone());
            if let Err(err) = fill_tileset(world, root, Path::new(&path), &tileset, &view) {
                spawn_text(world, root, err, tokens::TEXT_ERROR);
            }
        }
        Ok((id, None)) => {
            set_asset(world, root, id);
            spawn_text(world, root, "loading...", tokens::TEXT_SECONDARY);
        }
        Err(err) => {
            spawn_text(world, root, err, tokens::TEXT_ERROR);
        }
    }
}

fn set_asset(world: &mut World, root: Entity, id: AssetId<Tileset>) {
    if let Some(mut dock) = world.get_mut::<TilesetDock>(root) {
        dock.asset = Some(id);
    }
}
fn fill_tileset(
    world: &mut World,
    root: Entity,
    path: &Path,
    tileset: &Tileset,
    view: &DockView,
) -> Result<(), String> {
    let setup = tileset.setup().map_err(|e| e.to_string())?;
    let flagged = flagged(tileset);
    let header = spawn_row(world, root, tokens::SPACING_MD);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    spawn_text(world, header, name, tokens::TEXT_PRIMARY);
    spawn_pages(world, header, root, view.page);
    let count = format!("flagged: {} of {}", flagged.len(), setup.tiles.len());
    spawn_text(world, header, count, tokens::TEXT_SECONDARY);

    let columns = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                flex_grow: 1.0,
                min_height: px(0.0),
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(tokens::SPACING_MD),
                row_gap: px(tokens::SPACING_MD),
                padding: UiRect::all(px(tokens::SPACING_MD)),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    let left = spawn_column(world, columns, Val::Px(220.0));
    match view.page {
        DockPage::Review => {
            spawn_terrains(world, left, root, tileset, &setup, view, false);
            spawn_flagged(world, left, root, &flagged, view);
        }
        DockPage::Terrains => spawn_terrains(world, left, root, tileset, &setup, view, true),
        DockPage::Objects => spawn_objects(world, left, root, tileset),
    }
    let centre = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                min_width: px(240.0),
                row_gap: px(tokens::SPACING_MD),
                overflow: Overflow::scroll(),
                ..default()
            },
            ChildOf(columns),
        ))
        .id();
    for sheet in &tileset.setup_sheets {
        spawn_sheet(world, centre, root, tileset, &setup, *sheet, view, &flagged);
    }
    let right = spawn_column(world, columns, Val::Px(200.0));
    match view.page {
        DockPage::Objects => spawn_object_pick(world, right, root, view),
        DockPage::Review | DockPage::Terrains => {
            spawn_tile_editor(world, right, root, tileset, &setup, view);
        }
    }
    Ok(())
}

fn spawn_pages(world: &mut World, parent: Entity, root: Entity, current: DockPage) {
    let bar = world
        .spawn((segmented::segmented_bar(), ChildOf(parent)))
        .id();
    for (page, label) in [
        (DockPage::Review, "Review"),
        (DockPage::Terrains, "Terrains"),
        (DockPage::Objects, "Objects"),
    ] {
        let segment = world
            .spawn((
                segmented::segment(label),
                DockControl {
                    root,
                    action: DockAction::Page(page),
                },
                ChildOf(bar),
            ))
            .id();
        let checked = page == current;
        world
            .entity_mut(segment)
            .insert(BackgroundColor(segmented::segment_background(checked)));
        if checked {
            world.entity_mut(segment).insert(bevy::ui::Checked);
        }
    }
}

fn spawn_terrains(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    setup: &TilesetSetup,
    view: &DockView,
    missing: bool,
) {
    spawn_text(world, parent, "TERRAINS", tokens::TEXT_SECONDARY);
    let empty = TilesetTerrain {
        name: "empty".into(),
        ..default()
    };
    for terrain in std::iter::once(&empty).chain(&tileset.terrains) {
        let row = spawn_pick(
            world,
            parent,
            root,
            DockAction::Brush(terrain.id),
            view.brush == terrain.id,
        );
        spawn_swatch(world, row, swatch(tileset, terrain.id, 1.0), 12.0);
        let name = if terrain.background {
            format!("{} {}  bg", terrain.id, terrain.name)
        } else {
            format!("{} {}", terrain.id, terrain.name)
        };
        let label = spawn_text(world, row, name, tokens::TEXT_PRIMARY);
        world.entity_mut(label).insert(Node {
            flex_grow: 1.0,
            ..default()
        });
        if terrain.id == 0 {
            continue;
        }
        let index = match terrain.collision {
            TerrainCollision::Solid => 0,
            TerrainCollision::Trigger => 1,
            TerrainCollision::Passable => 2,
        };
        world.spawn((
            combobox_compact_with_selected(COLLISIONS.to_vec(), index),
            CollisionCombo {
                root,
                terrain: terrain.id,
            },
            ChildOf(row),
        ));
    }
    for set in &tileset.terrain_sets {
        let Ok(coverage) = setup.coverage(set.inner, set.outer) else {
            continue;
        };
        let line = format!(
            "set {} over {}: {:?} {}/{}",
            set.inner, set.outer, set.mode, coverage.covered, coverage.total
        );
        spawn_text(world, parent, line, tokens::TEXT_SECONDARY);
        if missing {
            for shape in &coverage.missing {
                let peers: Vec<String> = shape.peers.iter().map(u8::to_string).collect();
                let line = format!("  missing {}", peers.join(","));
                spawn_text(world, parent, line, FLAGGED);
            }
        }
    }
}

fn spawn_flagged(
    world: &mut World,
    parent: Entity,
    root: Entity,
    flagged: &[(u32, u32)],
    view: &DockView,
) {
    let title = format!("FLAGGED ({})", flagged.len());
    spawn_text(world, parent, title, tokens::TEXT_SECONDARY);
    for &(sheet, tile) in flagged {
        let row = spawn_pick(
            world,
            parent,
            root,
            DockAction::Tile(sheet, tile),
            view.selected == Some((sheet, tile)),
        );
        spawn_text(world, row, format!("tile {sheet}:{tile}"), FLAGGED);
    }
}

fn spawn_objects(world: &mut World, parent: Entity, root: Entity, tileset: &Tileset) {
    let title = format!("OBJECTS ({})", tileset.objects.len());
    spawn_text(world, parent, title, tokens::TEXT_SECONDARY);
    for object in &tileset.objects {
        let row = spawn_row(world, parent, tokens::SPACING_SM);
        let line = format!(
            "{},{} {}x{}",
            object.origin.x, object.origin.y, object.size.x, object.size.y
        );
        let label = spawn_text(world, row, line, tokens::TEXT_PRIMARY);
        world.entity_mut(label).insert(Node {
            flex_grow: 1.0,
            ..default()
        });
        world.spawn((
            button(ButtonProps::new("Remove").with_variant(ButtonVariant::Ghost)),
            DockControl {
                root,
                action: DockAction::RemoveObject(object.sheet, object.origin),
            },
            ChildOf(row),
        ));
    }
}

#[expect(clippy::too_many_arguments, reason = "one sheet of one view")]
fn spawn_sheet(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    setup: &TilesetSetup,
    index: u32,
    view: &DockView,
    flagged: &[(u32, u32)],
) {
    let Some(sheet) = tileset.sheets.get(index as usize) else {
        return;
    };
    let tile = tileset.tile_size.as_vec2();
    let step = tile + sheet.spacing.as_vec2();
    let cells = UVec2::new(sheet.columns, sheet.rows).as_vec2();
    let size = (sheet.margin.as_vec2() * 2.0 + cells * step - sheet.spacing.as_vec2()) * SCALE;
    let image = sheet_image(world, &sheet.source);
    let frame = world
        .spawn((
            Node {
                width: px(size.x),
                height: px(size.y),
                flex_shrink: 0.0,
                ..default()
            },
            image.map(ImageNode::new).unwrap_or_default(),
            ChildOf(parent),
        ))
        .id();
    let rect = view.selected.zip(view.anchor.or(view.selected));
    let at = |tile_index: u32| {
        let cell = UVec2::new(tile_index % sheet.columns, tile_index / sheet.columns);
        (sheet.margin.as_vec2() + cell.as_vec2() * step) * SCALE
    };
    for t in 0..sheet.columns * sheet.rows {
        let peers = (tileset.setup_address(index, t).ok())
            .and_then(|a| setup.tiles.get(&a))
            .map(|found| found.terrains.peers);
        let centre = peers.map_or(0, |p| p[4]);
        let picked = view.selected == Some((index, t))
            || (view.page == DockPage::Objects
                && rect.is_some_and(|(a, b)| in_rect(sheet.columns, a, b, (index, t))));
        let outline = if picked {
            tokens::SELECTED_BORDER
        } else if flagged.contains(&(index, t)) {
            FLAGGED
        } else {
            Color::NONE
        };
        let corner = at(t);
        let cell = world
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: px(corner.x),
                    top: px(corner.y),
                    width: px(tile.x * SCALE),
                    height: px(tile.y * SCALE),
                    border: UiRect::all(px(2.0)),
                    ..default()
                },
                BackgroundColor(swatch(tileset, centre, TINT)),
                BorderColor::all(outline),
                Button,
                DockControl {
                    root,
                    action: DockAction::Tile(index, t),
                },
                ChildOf(frame),
            ))
            .id();
        if centre != 0 {
            // The overlay alone vanishes on a tile of its own colour.
            world.spawn((
                Node {
                    width: px(8.0),
                    height: px(8.0),
                    border: UiRect::all(px(1.0)),
                    ..default()
                },
                BackgroundColor(swatch(tileset, centre, 1.0)),
                BorderColor::all(tokens::WINDOW_BG),
                Pickable::IGNORE,
                ChildOf(cell),
            ));
        }
    }
    for object in tileset.objects.iter().filter(|o| o.sheet == index) {
        let corner = at(object.origin.y * sheet.columns + object.origin.x);
        let span = (object.size.as_vec2() * step - sheet.spacing.as_vec2()) * SCALE;
        world.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(corner.x),
                top: px(corner.y),
                width: px(span.x),
                height: px(span.y),
                border: UiRect::all(px(1.0)),
                ..default()
            },
            BorderColor::all(tokens::TEXT_PRIMARY),
            Pickable::IGNORE,
            ChildOf(frame),
        ));
    }
}

fn in_rect(columns: u32, a: (u32, u32), b: (u32, u32), tile: (u32, u32)) -> bool {
    let (lo, hi) = rect_cells(columns, a, b);
    let cell = UVec2::new(tile.1 % columns, tile.1 / columns);
    a.0 == tile.0 && cell.cmpge(lo).all() && cell.cmple(hi).all()
}

fn rect_cells(columns: u32, a: (u32, u32), b: (u32, u32)) -> (UVec2, UVec2) {
    let a = UVec2::new(a.1 % columns, a.1 / columns);
    let b = UVec2::new(b.1 % columns, b.1 / columns);
    (a.min(b), a.max(b))
}

fn spawn_tile_editor(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    setup: &TilesetSetup,
    view: &DockView,
) {
    spawn_text(world, parent, "SELECTED TILE", tokens::TEXT_SECONDARY);
    let Some((sheet, tile)) = view.selected else {
        spawn_text(world, parent, "click a tile", tokens::TEXT_SECONDARY);
        return;
    };
    spawn_text(
        world,
        parent,
        format!("tile {sheet}:{tile}"),
        tokens::TEXT_PRIMARY,
    );
    let peers = (tileset.setup_address(sheet, tile).ok())
        .and_then(|a| setup.tiles.get(&a))
        .map(|t| t.terrains.peers)
        .unwrap_or_default();
    let mut preview = ImageNode::default();
    if let Some(source) = tileset.sheets.get(sheet as usize) {
        let size = tileset.tile_size.as_vec2();
        let cell = UVec2::new(tile % source.columns, tile / source.columns).as_vec2();
        let min = source.margin.as_vec2() + cell * (size + source.spacing.as_vec2());
        preview = sheet_image(world, &source.source)
            .map(ImageNode::new)
            .unwrap_or_default();
        preview.rect = Some(Rect::from_corners(min, min + size));
    }
    let grid = world
        .spawn((
            Node {
                width: px(PREVIEW),
                height: px(PREVIEW),
                display: Display::Grid,
                grid_template_columns: RepeatedGridTrack::flex(3, 1.0),
                grid_template_rows: RepeatedGridTrack::flex(3, 1.0),
                flex_shrink: 0.0,
                ..default()
            },
            preview,
            ChildOf(parent),
        ))
        .id();
    for (peer, terrain) in peers.iter().enumerate() {
        let cell = world
            .spawn((
                Node {
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    border: UiRect::all(px(1.0)),
                    ..default()
                },
                BackgroundColor(swatch(tileset, *terrain, 0.45)),
                BorderColor::all(tokens::BORDER_SUBTLE),
                Button,
                DockControl {
                    root,
                    action: DockAction::Peer(peer),
                },
                ChildOf(grid),
            ))
            .id();
        spawn_text(world, cell, terrain.to_string(), tokens::TEXT_PRIMARY);
    }
    let brush = (tileset.terrains.iter())
        .find(|t| t.id == view.brush)
        .map_or("empty".to_string(), |t| t.name.clone());
    spawn_text(
        world,
        parent,
        format!("brush: {brush}"),
        tokens::TEXT_PRIMARY,
    );
    let row = spawn_row(world, parent, tokens::SPACING_SM);
    for (label, action, variant) in [
        ("Accept", DockAction::Accept, ButtonVariant::Primary),
        ("Reset to auto", DockAction::Reset, ButtonVariant::Default),
    ] {
        world.spawn((
            button(ButtonProps::new(label).with_variant(variant)),
            DockControl { root, action },
            ChildOf(row),
        ));
    }
}

fn spawn_object_pick(world: &mut World, parent: Entity, root: Entity, view: &DockView) {
    spawn_text(world, parent, "NEW OBJECT", tokens::TEXT_SECONDARY);
    let hint = match (view.anchor, view.selected) {
        (Some(a), Some(b)) => format!("tiles {}:{} to {}:{}", a.0, a.1, b.0, b.1),
        _ => "click two corner tiles".to_string(),
    };
    spawn_text(world, parent, hint, tokens::TEXT_PRIMARY);
    world.spawn((
        button(ButtonProps::new("Add object").with_variant(ButtonVariant::Primary)),
        DockControl {
            root,
            action: DockAction::AddObject,
        },
        ChildOf(parent),
    ));
}

/// A row that picks something when clicked, highlighted while it is the pick.
fn spawn_pick(
    world: &mut World,
    parent: Entity,
    root: Entity,
    action: DockAction,
    current: bool,
) -> Entity {
    let row = spawn_row(world, parent, tokens::SPACING_SM);
    let background = if current {
        tokens::SELECTED_BG
    } else {
        Color::NONE
    };
    world.entity_mut(row).insert((
        BackgroundColor(background),
        Button,
        DockControl { root, action },
    ));
    row
}

fn spawn_row(world: &mut World, parent: Entity, gap: f32) -> Entity {
    world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(gap),
                padding: UiRect::axes(px(tokens::SPACING_SM), px(tokens::SPACING_XS)),
                ..default()
            },
            ChildOf(parent),
        ))
        .id()
}

fn spawn_column(world: &mut World, parent: Entity, width: Val) -> Entity {
    world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                width,
                flex_shrink: 0.0,
                row_gap: px(tokens::SPACING_XS),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            ChildOf(parent),
        ))
        .id()
}

fn spawn_text(world: &mut World, parent: Entity, text: impl Into<String>, color: Color) -> Entity {
    world
        .spawn((
            Text::new(text.into()),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(color),
            Pickable::IGNORE,
            ChildOf(parent),
        ))
        .id()
}

fn spawn_swatch(world: &mut World, parent: Entity, color: Color, size: f32) {
    world.spawn((
        Node {
            width: px(size),
            height: px(size),
            border: UiRect::all(px(1.0)),
            ..default()
        },
        BackgroundColor(color),
        BorderColor::all(tokens::BORDER_SUBTLE),
        ChildOf(parent),
    ));
}

fn swatch(tileset: &Tileset, terrain: TerrainId, alpha: f32) -> Color {
    (tileset.terrains.iter())
        .find(|t| t.id == terrain)
        .map_or(Color::NONE, |t| Color::from(t.swatch.with_alpha(alpha)))
}

/// A nearest-sampled copy of the source sheet `source`, decoded once.
fn sheet_image(world: &mut World, source: &str) -> Option<Handle<Image>> {
    if let Some(handle) = world.resource::<SheetImages>().0.get(source) {
        return Some(handle.clone());
    }
    let sheet = decode_sheet(&assets_dir(world)?, source.to_string()).ok()?;
    let size = Extent3d {
        width: sheet.width,
        height: sheet.height,
        depth_or_array_layers: 1,
    };
    let mut image = Image::new(
        size,
        TextureDimension::D2,
        sheet.pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::nearest();
    let handle = world.resource_mut::<Assets<Image>>().add(image);
    (world.resource_mut::<SheetImages>().0).insert(source.to_string(), handle.clone());
    Some(handle)
}

fn on_activate(activate: On<Activate>, controls: Query<&DockControl>, mut commands: Commands) {
    if let Ok(control) = controls.get(activate.entity).copied() {
        commands.queue(move |world: &mut World| act(world, control));
    }
}

fn on_page_change(
    change: On<ValueChange<Entity>>,
    controls: Query<&DockControl>,
    mut commands: Commands,
) {
    if let Ok(control) = controls.get(change.value).copied() {
        commands.queue(move |world: &mut World| act(world, control));
    }
}

fn on_collision_pick(
    change: On<ComboBoxChangeEvent>,
    combos: Query<&CollisionCombo>,
    mut commands: Commands,
) {
    let Ok(combo) = combos.get(change.entity).copied() else {
        return;
    };
    let Some(mode) = COLLISIONS.get(change.selected).copied() else {
        return;
    };
    commands.queue(move |world: &mut World| {
        let Some((path, _)) = dock_tileset(world, combo.root) else {
            return;
        };
        let called = world
            .operator("tileset.terrain.set_collision")
            .param("path", path)
            .param("terrain", combo.terrain.to_string())
            .param("mode", mode)
            .call();
        if let Err(err) = called {
            warn!("tileset.terrain.set_collision: {err}");
        }
    });
}

/// Build again each dock whose tileset was loaded or changed, by an edit, its
/// undo, a new import or the file on disk.
fn refresh_on_tileset_events(
    mut events: MessageReader<AssetEvent<Tileset>>,
    docks: Query<(Entity, &TilesetDock)>,
    mut commands: Commands,
) {
    let changed: HashSet<AssetId<Tileset>> = (events.read())
        .filter_map(|event| match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id } => Some(*id),
            AssetEvent::Removed { .. } | AssetEvent::Unused { .. } => None,
        })
        .collect();
    if changed.is_empty() {
        return;
    }
    for (root, dock) in &docks {
        if dock.asset.is_none_or(|id| changed.contains(&id)) {
            commands.queue(move |world: &mut World| {
                if world.get_entity(root).is_ok() {
                    fill(world, root);
                }
            });
        }
    }
}

/// Drop the view of a tileset once no tab shows it.
fn prune_views(
    remove: On<Remove<TilesetDock>>,
    docks: Query<&TilesetDock>,
    mut commands: Commands,
) {
    let Some(path) = docks.get(remove.entity).ok().and_then(|d| d.path.clone()) else {
        return;
    };
    commands.queue(move |world: &mut World| {
        let shown = (world.resource::<DockTree>().tabs())
            .any(|(_, t)| t.window_id == TILESET_WINDOW_ID && t.payload.as_deref() == Some(&path));
        if !shown {
            world.resource_mut::<TilesetDockViews>().0.remove(&path);
        }
    });
}

/// Enter accepts the selected tile and moves to the next flagged one, while
/// the dock itself holds focus rather than a field inside it.
fn on_dock_key(mut key: On<FocusedInput<KeyboardInput>>, mut commands: Commands) {
    let input = &key.input;
    if input.key_code != KeyCode::Enter || !input.state.is_pressed() || input.repeat {
        return;
    }
    let root = key.event_target();
    if key.original_event_target() != root {
        return;
    }
    key.propagate(false);
    commands.queue(move |world: &mut World| {
        act(
            world,
            DockControl {
                root,
                action: DockAction::Accept,
            },
        );
    });
}

/// The path and tileset of the dock `root`, once loaded.
fn dock_tileset(world: &mut World, root: Entity) -> Option<(String, Tileset)> {
    let path = world.get::<TilesetDock>(root)?.path.clone()?;
    let (_, tileset) = load(world, &path).ok()?;
    Some((path, tileset?))
}

fn act(world: &mut World, control: DockControl) {
    let root = control.root;
    let Some((path, tileset)) = dock_tileset(world, root) else {
        return;
    };
    let mut view = (world.resource::<TilesetDockViews>().0.get(&path).cloned()).unwrap_or_default();
    let tile_ref = view.selected.map(|(s, t)| format!("{s}:{t}"));
    let set_bits = "tileset.tile.set_bits";
    let call = match control.action {
        DockAction::Page(page) => {
            view.page = page;
            view.anchor = None;
            None
        }
        DockAction::Brush(brush) => {
            view.brush = brush;
            None
        }
        DockAction::Tile(sheet, tile) => {
            let extend = view.page == DockPage::Objects
                && view.anchor.is_some_and(|a| a.0 == sheet)
                && view.anchor == view.selected;
            if !extend {
                view.anchor = Some((sheet, tile));
            }
            view.selected = Some((sheet, tile));
            None
        }
        DockAction::Peer(peer) => tile_ref.map(|tile| {
            let params = vec![
                ("tile", tile),
                ("peer", peer.to_string()),
                ("terrain", view.brush.to_string()),
            ];
            (set_bits, params)
        }),
        DockAction::Accept => {
            let peers = view.selected.and_then(|(s, t)| peers_of(&tileset, s, t));
            let next = next_flagged(&tileset, view.selected);
            view.selected = next.or(view.selected);
            tile_ref.zip(peers).map(|(tile, peers)| {
                let peers: Vec<String> = peers.iter().map(u8::to_string).collect();
                (set_bits, vec![("tile", tile), ("peers", peers.join(","))])
            })
        }
        DockAction::Reset => {
            tile_ref.map(|tile| ("tileset.tile.reset_to_auto", vec![("tiles", tile)]))
        }
        DockAction::AddObject => {
            object_rect(&tileset, &view).map(|params| ("tileset.object.add", params))
        }
        DockAction::RemoveObject(sheet, origin) => Some((
            "tileset.object.remove",
            vec![
                ("sheet", sheet.to_string()),
                ("origin", format!("{},{}", origin.x, origin.y)),
            ],
        )),
    };
    (world.resource_mut::<TilesetDockViews>().0).insert(path.clone(), view);
    if let Some(mut focus) = world.get_resource_mut::<InputFocus>() {
        focus.set(root, FocusCause::Pressed);
    }
    if let Some((id, params)) = call {
        let mut call = world.operator(id).param("path", path.clone());
        for (key, value) in params {
            call = call.param(key, value);
        }
        if let Err(err) = call.call() {
            warn!("{id}: {err}");
        }
        // A change refreshes the dock through its asset event.
        let after = dock_tileset(world, root).map(|(_, t)| t);
        if after.as_ref() != Some(&tileset) {
            return;
        }
    }
    if world.get_entity(root).is_ok() {
        fill(world, root);
    }
}

fn peers_of(tileset: &Tileset, sheet: u32, tile: u32) -> Option<[TerrainId; 9]> {
    (tileset.tiles.iter())
        .find(|t| t.sheet == sheet && t.tile == tile)
        .map(|t| t.peers)
}

/// The flagged tile after `from` in setup order, wrapping, other than
/// `from` itself.
fn next_flagged(tileset: &Tileset, from: Option<(u32, u32)>) -> Option<(u32, u32)> {
    let order = |(sheet, tile): (u32, u32)| tileset.setup_sheet(sheet).ok().map(|s| (s, tile));
    let after = from.and_then(order);
    let others: Vec<(u32, u32)> = (flagged(tileset).into_iter())
        .filter(|t| Some(*t) != from)
        .collect();
    (others.iter().copied())
        .find(|t| after.is_none_or(|a| order(*t).is_some_and(|o| o > a)))
        .or_else(|| others.first().copied())
}

fn object_rect(tileset: &Tileset, view: &DockView) -> Option<Vec<(&'static str, String)>> {
    let (a, b) = view.anchor.zip(view.selected)?;
    let columns = tileset.sheets.get(a.0 as usize)?.columns;
    let (lo, hi) = rect_cells(columns, a, b);
    let size = hi - lo + UVec2::ONE;
    Some(vec![
        ("sheet", a.0.to_string()),
        ("origin", format!("{},{}", lo.x, lo.y)),
        ("size", format!("{},{}", size.x, size.y)),
    ])
}
