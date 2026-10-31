use std::collections::{HashMap, HashSet};
use std::path::Path;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui_widgets::{Activate, Button, ValueChange};
use jackdaw_api::DefaultArea;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_feathers::button::{ButtonProps, ButtonVariant, button};
use jackdaw_feathers::combobox::{ComboBoxChangeEvent, combobox_compact_with_selected};
use jackdaw_feathers::text_edit::{TextEditCommitEvent, TextEditProps, text_edit};
use jackdaw_feathers::{segmented, tokens};
use jackdaw_input::prelude::{ContextActivity, ContextPriority, InputContext};
use jackdaw_panels::area::DockWindow;
use jackdaw_panels::tree::{DockTree, TabId};
use jackdaw_scene_types::{Tilemap, TilemapLayerKind};
use jackdaw_tilemap::{TerrainId, TilesetSetup};
use jackdaw_tileset::{TerrainCollision, Tileset, TilesetTerrain};

use crate::asset_index::assets_dir;
use crate::definition_assets::{entry_for_file, resolve_project_path};
use crate::tileset_edit::{apply_edit, clear_auto, open_tileset, suggested_name, text};
use crate::tileset_import::{TILESET_SUFFIX, decode_sheet};

pub const TILESET_WINDOW_ID: &str = "jackdaw.tileset";

/// On-screen size a sheet tile is drawn at, at the least.
const READABLE_TILE: f32 = 32.0;
/// On-screen size of the selected tile's preview.
const PREVIEW: f32 = 96.0;
/// On-screen size of a terrain's icon in the terrains list.
const ICON: f32 = 20.0;
const COLLISIONS: [&str; 3] = ["solid", "trigger", "passable"];

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TilesetDockViews>()
        .init_resource::<ShownTilesets>()
        .init_resource::<SheetImages>()
        .add_systems(Update, refresh_on_tileset_events)
        .add_observer(on_activate)
        .add_observer(on_page_change)
        .add_observer(on_collision_pick)
        .add_systems(
            Update,
            follow_focus.run_if(resource_exists_and_changed::<InputFocus>),
        )
        .add_observer(|_: On<Remove<TilesetDock>>, mut commands: Commands| {
            commands.run_system_cached(follow_focus);
        })
        .add_observer(on_name_commit)
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
    ctx.register_operator::<TilesetEditOp>()
        .register_operator::<TilesetTerrainFromTileOp>();
    let context = ctx
        .spawn((
            TilesetDockInputContext,
            ContextPriority(5),
            ContextActivity::INACTIVE,
            Name::new("TilesetDockInputContext"),
            crate::EditorEntity,
        ))
        .id();
    ctx.bind_operator_in::<TilesetTerrainFromTileOp>(context, [PresetInput::key("KeyN")]);
}

/// The input context of the Tileset window, active while a tileset tab has
/// the keyboard.
#[derive(Component, Default)]
#[require(InputContext)]
pub struct TilesetDockInputContext;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DockPage {
    #[default]
    Sheet,
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
    /// The name field open in the terrains list.
    pub naming: Option<NameField>,
    /// Terrain sets, as inner and outer, whose missing shapes are listed.
    pub expanded: HashSet<(TerrainId, TerrainId)>,
}

/// What a name field of the terrains list names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameField {
    /// A new terrain shown by the selected tile.
    New,
    Rename(TerrainId),
}

/// The view of each open tileset, by its path. A tileset has one tab, so the
/// view follows the tab wherever it is docked.
#[derive(Resource, Default)]
pub struct TilesetDockViews(pub HashMap<String, DockView>);

/// Tilesets a dock has shown this session, by path: each is checked for
/// automatic setup to clear once, so an undo of the clear sticks.
#[derive(Resource, Default)]
pub(crate) struct ShownTilesets(pub(crate) HashSet<String>);

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
    AddObject,
    RemoveObject(u32, UVec2),
    NewTerrain,
    RenameTerrain(TerrainId),
    RemoveTerrain(TerrainId),
    SetIcon(TerrainId),
    /// Show or hide the missing shapes of the set `inner` over `outer`.
    ToggleMissing(TerrainId, TerrainId),
}

/// The drawn image of sheet `sheet` in the dock `root`.
#[derive(Component, Clone, Copy, Debug)]
pub struct SheetPicture {
    pub root: Entity,
    pub sheet: u32,
}

/// The name field of the dock `root`.
#[derive(Component, Clone, Copy, Debug)]
pub struct TerrainNameField {
    pub root: Entity,
    pub field: NameField,
}

/// The icon tile or colour terrain `terrain` is listed by.
#[derive(Component, Clone, Copy, Debug)]
pub struct TerrainSwatch {
    pub root: Entity,
    pub terrain: TerrainId,
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
    description = "Open a tileset in its own Tileset window tab, or bring its tab to the front. \
                   The first time a tileset is shown in a session, automatic setup nobody has \
                   touched is cleared as one undo entry, unless a map paints with its terrains.",
    allows_undo = false,
    params(path(
        String,
        doc = "Tileset file under the project's assets; defaults to the open tileset."
    ))
)]
pub fn tileset_edit(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let path = text(&params, "path");
    commands.queue(move |world: &mut World| edit_tileset(world, &path));
    OperatorResult::Finished
}

/// What `tileset.edit` does, for a caller already inside an operator.
pub(crate) fn edit_tileset(world: &mut World, path: &str) {
    match open_tab(world, path) {
        Ok(message) => report_to_caller(world, message),
        Err(err) => warn_caller(world, format!("tileset.edit: {err}")),
    }
}

fn open_tab(world: &mut World, path: &str) -> Result<String, String> {
    let (path, _) = open_tileset(world, path)?;
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
    (world.resource_mut::<TilesetDockViews>().0).insert(payload.clone(), DockView::default());
    Ok(format!("{payload} opened"))
}

/// The tileset at `path` and its asset id; `None` while it loads.
pub(crate) fn load(
    world: &mut World,
    path: &str,
) -> Result<(AssetId<Tileset>, Option<Tileset>), String> {
    let id = tileset_id(world, path)?;
    Ok((id, world.resource::<Assets<Tileset>>().get(id).cloned()))
}

/// The asset id of the tileset at `path`.
pub(crate) fn tileset_id(world: &mut World, path: &str) -> Result<AssetId<Tileset>, String> {
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
    Ok(id)
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
        Ok((id, Some(mut tileset))) => {
            set_asset(world, root, id);
            if clear_once(world, &path, &tileset)
                && let Ok((_, Some(cleared))) = load(world, &path)
            {
                tileset = cleared;
            }
            let mut view =
                (world.resource::<TilesetDockViews>().0.get(&path).cloned()).unwrap_or_default();
            if !tileset.terrains.iter().any(|t| t.id == view.brush) {
                view.brush = 0;
            }
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

/// Clear automatic setup nobody has touched from the tileset at `path` the
/// first time a dock shows it this session, unless a map paints with it.
/// Returns whether it was cleared.
fn clear_once(world: &mut World, path: &str, tileset: &Tileset) -> bool {
    let first = (world.resource_mut::<ShownTilesets>().0).insert(path.to_string());
    if !first || !tileset.untouched_auto() {
        return false;
    }
    if painted_with(world, path) {
        let notice = format!("{path} still has automatic terrains a map paints with; kept");
        crate::status_bar::notify_info(world, notice);
        return false;
    }
    let cleared = apply_edit(world, "tileset.clear_auto", path, |_, t| clear_auto(t));
    if let Err(err) = &cleared {
        warn!("tileset.clear_auto: {err}");
    }
    cleared.is_ok()
}

/// Whether an open map, or a scene of the project, has a terrain layer over
/// the tileset at `path`. A scene file counts when it names the tileset at
/// all, since its cells live in a sidecar.
fn painted_with(world: &mut World, path: &str) -> bool {
    let open = (world.query::<&Tilemap>().iter(world)).any(|map| {
        (map.layers.iter()).any(|l| l.kind == TilemapLayerKind::Terrain && l.tileset == path)
    });
    let Some(assets) = assets_dir(world).filter(|_| !open) else {
        return open;
    };
    let quoted = format!("\"{path}\"");
    (jackdaw_bsn::walk_files_with_extensions(&assets, &["bsn"]).iter())
        .filter(|file| !file.to_string_lossy().ends_with(TILESET_SUFFIX))
        .any(|file| std::fs::read_to_string(file).is_ok_and(|text| text.contains(&quoted)))
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
    let header = spawn_row(world, root, tokens::SPACING_MD);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    spawn_text(world, header, name, tokens::TEXT_PRIMARY);
    spawn_pages(world, header, root, view.page);

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
        DockPage::Sheet => spawn_terrains(world, left, root, tileset, &setup, view),
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
        spawn_sheet(world, centre, root, tileset, *sheet, view);
    }
    let right = spawn_column(world, columns, Val::Px(200.0));
    match view.page {
        DockPage::Objects => spawn_object_pick(world, right, root, view),
        DockPage::Sheet => {
            spawn_tile_editor(world, right, root, tileset, &setup, view);
        }
    }
    Ok(())
}

fn spawn_pages(world: &mut World, parent: Entity, root: Entity, current: DockPage) {
    let bar = world
        .spawn((segmented::segmented_bar(), ChildOf(parent)))
        .id();
    for (page, label) in [(DockPage::Sheet, "Sheet"), (DockPage::Objects, "Objects")] {
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
        spawn_terrain_swatch(world, row, root, tileset, terrain);
        let name = if terrain.background {
            format!("{}  bg", terrain.name)
        } else {
            terrain.name.clone()
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
    let brush = (tileset.terrains.iter()).find(|t| t.id == view.brush);
    if let Some(brush) = brush {
        if view.naming == Some(NameField::Rename(brush.id)) {
            let field = NameField::Rename(brush.id);
            spawn_name_field(world, parent, root, field, brush.name.clone());
        } else {
            let row = spawn_row(world, parent, tokens::SPACING_SM);
            spawn_button(
                world,
                row,
                root,
                "Rename",
                DockAction::RenameTerrain(brush.id),
            );
            spawn_button(
                world,
                row,
                root,
                "Remove",
                DockAction::RemoveTerrain(brush.id),
            );
            if view.selected.is_some() {
                let label = "Use tile as icon";
                spawn_button(world, parent, root, label, DockAction::SetIcon(brush.id));
            }
        }
    }
    if view.naming == Some(NameField::New) {
        let name = suggested_name(tileset);
        spawn_name_field(world, parent, root, NameField::New, name);
    } else if view.selected.is_some() {
        spawn_button(
            world,
            parent,
            root,
            "New terrain (N)",
            DockAction::NewTerrain,
        );
    } else {
        let hint = "Click a plain tile, then New terrain.";
        spawn_text(world, parent, hint, tokens::TEXT_SECONDARY);
    }
    spawn_text(world, parent, "TERRAIN SETS", tokens::TEXT_SECONDARY);
    for set in &tileset.terrain_sets {
        let Ok(coverage) = setup.coverage(set.inner, set.outer) else {
            continue;
        };
        let line = format!(
            "{} over {}: {:?} {}/{}",
            tileset.terrain_name(set.inner),
            tileset.terrain_name(set.outer),
            set.mode,
            coverage.covered,
            coverage.total
        );
        spawn_text(world, parent, line, tokens::TEXT_SECONDARY);
        if coverage.missing.is_empty() {
            continue;
        }
        let open = view.expanded.contains(&(set.inner, set.outer));
        let label = if open {
            "Hide missing".to_string()
        } else {
            format!("Show {} missing", coverage.missing.len())
        };
        let action = DockAction::ToggleMissing(set.inner, set.outer);
        spawn_button(world, parent, root, &label, action);
        for shape in coverage.missing.iter().filter(|_| open) {
            let peers: Vec<String> = shape.peers.iter().map(u8::to_string).collect();
            let line = format!("  missing {}", peers.join(","));
            spawn_text(world, parent, line, tokens::TEXT_WARNING);
        }
    }
}

fn spawn_button(world: &mut World, parent: Entity, root: Entity, label: &str, action: DockAction) {
    world.spawn((
        button(ButtonProps::new(label).with_variant(ButtonVariant::Ghost)),
        DockControl { root, action },
        ChildOf(parent),
    ));
}

/// A name field that has the keyboard as it opens, its text selected.
fn spawn_name_field(
    world: &mut World,
    parent: Entity,
    root: Entity,
    field: NameField,
    name: String,
) {
    let props = TextEditProps::default()
        .with_default_value(name)
        .select_all_on_open()
        .auto_focus();
    world.spawn((
        text_edit(props),
        TerrainNameField { root, field },
        ChildOf(parent),
    ));
}

/// The terrain's icon tile, or its colour when it has none.
fn spawn_terrain_swatch(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    terrain: &TilesetTerrain,
) {
    let icon = (terrain.icon).and_then(|(sheet, tile)| tile_image(world, tileset, sheet, tile));
    let mut swatch_node = world.spawn((
        Node {
            width: px(ICON),
            height: px(ICON),
            border: UiRect::all(px(1.0)),
            flex_shrink: 0.0,
            ..default()
        },
        BorderColor::all(tokens::BORDER_SUBTLE),
        TerrainSwatch {
            root,
            terrain: terrain.id,
        },
        Pickable::IGNORE,
        ChildOf(parent),
    ));
    match icon {
        Some(image) => swatch_node.insert(image),
        None => swatch_node.insert(BackgroundColor(swatch(tileset, terrain.id, 1.0))),
    };
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

fn spawn_sheet(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    index: u32,
    view: &DockView,
) {
    let Some(sheet) = tileset.sheets.get(index as usize) else {
        return;
    };
    let tile = tileset.tile_size.as_vec2();
    let scale = zoom(tile);
    let step = tile + sheet.spacing.as_vec2();
    let cells = UVec2::new(sheet.columns, sheet.rows).as_vec2();
    let grid = (sheet.margin.as_vec2() * 2.0 + cells * step - sheet.spacing.as_vec2()) * scale;
    let image = sheet_image(world, &sheet.source);
    let pixels = (image.as_ref())
        .and_then(|handle| world.resource::<Assets<Image>>().get(handle))
        .map_or(Vec2::ZERO, |image| image.size().as_vec2());
    let size = grid.max(pixels * scale);
    let frame = world
        .spawn((
            Node {
                width: px(size.x),
                height: px(size.y),
                flex_shrink: 0.0,
                ..default()
            },
            ChildOf(parent),
        ))
        .id();
    if let Some(image) = image {
        world.spawn((
            Node {
                position_type: PositionType::Absolute,
                width: px(pixels.x * scale),
                height: px(pixels.y * scale),
                ..default()
            },
            ImageNode::new(image).with_mode(NodeImageMode::Stretch),
            SheetPicture { root, sheet: index },
            Pickable::IGNORE,
            ChildOf(frame),
        ));
    }
    let rect = view.selected.zip(view.anchor.or(view.selected));
    let at = |tile_index: u32| {
        let origin = tileset.sheet_rect(index, tile_index).map(|(o, _)| o);
        origin.unwrap_or_default().as_vec2() * scale
    };
    for t in 0..sheet.columns * sheet.rows {
        let picked = view.selected == Some((index, t))
            || (view.page == DockPage::Objects
                && rect.is_some_and(|(a, b)| in_rect(sheet.columns, a, b, (index, t))));
        let (outline, border) = if picked {
            (tokens::SELECTED_BORDER, 2.0)
        } else {
            (tokens::BORDER_SUBTLE.with_alpha(0.3), 1.0)
        };
        let corner = at(t);
        world.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(corner.x),
                top: px(corner.y),
                width: px(tile.x * scale),
                height: px(tile.y * scale),
                border: UiRect::all(px(border)),
                ..default()
            },
            BorderColor::all(outline),
            Button,
            DockControl {
                root,
                action: DockAction::Tile(index, t),
            },
            ChildOf(frame),
        ));
    }
    for object in tileset.objects.iter().filter(|o| o.sheet == index) {
        let corner = at(object.origin.y * sheet.columns + object.origin.x);
        let span = (object.size.as_vec2() * step - sheet.spacing.as_vec2()) * scale;
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

/// Screen pixels per sheet pixel: whole, and enough for a tile to read.
fn zoom(tile: Vec2) -> f32 {
    (READABLE_TILE / tile.max_element().max(1.0))
        .ceil()
        .max(1.0)
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
    let preview = tile_image(world, tileset, sheet, tile).unwrap_or_default();
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

pub(crate) fn spawn_text(
    world: &mut World,
    parent: Entity,
    text: impl Into<String>,
    color: Color,
) -> Entity {
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

pub(crate) fn swatch(tileset: &Tileset, terrain: TerrainId, alpha: f32) -> Color {
    (tileset.terrains.iter())
        .find(|t| t.id == terrain)
        .map_or(Color::NONE, |t| Color::from(t.swatch.with_alpha(alpha)))
}

/// Tile `tile` of sheet `sheet`, cut from the sheet's image.
pub(crate) fn tile_image(
    world: &mut World,
    tileset: &Tileset,
    sheet: u32,
    tile: u32,
) -> Option<ImageNode> {
    let source = tileset.sheets.get(sheet as usize)?;
    let (min, size) = tileset.sheet_rect(sheet, tile)?;
    let mut image = ImageNode::new(sheet_image(world, &source.source)?);
    image.rect = Some(Rect::from_corners(min.as_vec2(), (min + size).as_vec2()));
    Some(image)
}

/// A nearest-sampled copy of the source sheet `source`, decoded once.
pub(crate) fn sheet_image(world: &mut World, source: &str) -> Option<Handle<Image>> {
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

/// The path and tileset of the dock `root`, once loaded.
fn dock_tileset(world: &mut World, root: Entity) -> Option<(String, Tileset)> {
    let path = world.get::<TilesetDock>(root)?.path.clone()?;
    let (_, tileset) = load(world, &path).ok()?;
    Some((path, tileset?))
}

/// An operator and its parameters, for the tileset of a dock.
type DockCall = (&'static str, Vec<(&'static str, String)>);

fn act(world: &mut World, control: DockControl) {
    let root = control.root;
    let Some((path, tileset)) = dock_tileset(world, root) else {
        return;
    };
    let mut view = (world.resource::<TilesetDockViews>().0.get(&path).cloned()).unwrap_or_default();
    view.naming = None;
    let tile_ref = view.selected.map(|(s, t)| format!("{s}:{t}"));
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
            ("tileset.tile.set_bits", params)
        }),
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
        DockAction::NewTerrain => None,
        DockAction::RenameTerrain(terrain) => {
            view.naming = Some(NameField::Rename(terrain));
            None
        }
        DockAction::SetIcon(terrain) => tile_ref.map(|icon| {
            let params = vec![("terrain", terrain.to_string()), ("icon", icon)];
            ("tileset.terrain.set_icon", params)
        }),
        DockAction::ToggleMissing(inner, outer) => {
            if !view.expanded.remove(&(inner, outer)) {
                view.expanded.insert((inner, outer));
            }
            None
        }
        DockAction::RemoveTerrain(terrain) => {
            let params = vec![("terrain", terrain.to_string())];
            Some(("tileset.terrain.remove", params))
        }
    };
    (world.resource_mut::<TilesetDockViews>().0).insert(path.clone(), view);
    if let Some(mut focus) = world.get_resource_mut::<InputFocus>() {
        focus.set(root, FocusCause::Pressed);
    }
    if control.action == DockAction::NewTerrain {
        call_operator(world, &path, TilesetTerrainFromTileOp::ID, Vec::new());
        return;
    }
    dispatch(world, root, &path, &tileset, call);
}

/// Run `call` on the tileset at `path`, then build `root` again unless the
/// tileset changed, which refreshes it through its asset event.
fn dispatch(world: &mut World, root: Entity, path: &str, before: &Tileset, call: Option<DockCall>) {
    if let Some((id, params)) = call {
        call_operator(world, path, id, params);
        let after = dock_tileset(world, root).map(|(_, t)| t);
        if after.as_ref() != Some(before) {
            return;
        }
    }
    if world.get_entity(root).is_ok() {
        fill(world, root);
    }
}

fn call_operator(world: &mut World, path: &str, id: &str, params: Vec<(&'static str, String)>) {
    let mut call = world
        .operator(id.to_string())
        .param("path", path.to_string());
    for (key, value) in params {
        call = call.param(key, value);
    }
    if let Err(err) = call.call() {
        warn!("{id}: {err}");
    }
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

/// Open a name field for a new terrain shown by the selected tile.
#[operator(
    id = "tileset.terrain.from_tile",
    label = "New Terrain From Tile",
    description = "Open a name field in the Tileset window for a new terrain shown by the \
                   selected sheet tile; Enter adds it through tileset.terrain.add, Escape \
                   cancels.",
    allows_undo = false,
    is_available = a_tile_is_selected,
    params(path(
        String,
        doc = "Tileset file under the project's assets; defaults to the tab with the \
               keyboard, then the open tileset."
    ))
)]
pub fn tileset_terrain_from_tile(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let path = text(&params, "path");
    commands.queue(
        move |world: &mut World| match open_name_field(world, &path) {
            Ok(message) => report_to_caller(world, message),
            Err(err) => warn_caller(world, format!("tileset.terrain.from_tile: {err}")),
        },
    );
    OperatorResult::Finished
}

fn a_tile_is_selected(views: Res<TilesetDockViews>) -> bool {
    views.0.values().any(|v| v.selected.is_some())
}

fn open_name_field(world: &mut World, path: &str) -> Result<String, String> {
    let focused = focused_dock(world).and_then(|root| world.get::<TilesetDock>(root)?.path.clone());
    let path = match focused {
        Some(found) if path.is_empty() => found,
        _ => open_tileset(world, path)?.0.to_string_lossy().into_owned(),
    };
    let mut views = world.resource_mut::<TilesetDockViews>();
    let view = (views.0.get_mut(&path)).ok_or_else(|| format!("{path} has no Tileset tab"))?;
    let (sheet, tile) = view.selected.ok_or("no sheet tile is selected")?;
    view.page = DockPage::Sheet;
    view.naming = Some(NameField::New);
    let roots: Vec<Entity> = (world.query::<(Entity, &TilesetDock)>().iter(world))
        .filter(|(_, d)| d.path.as_deref() == Some(path.as_str()))
        .map(|(root, _)| root)
        .collect();
    for root in roots {
        fill(world, root);
    }
    Ok(format!("naming a new terrain shown by tile {sheet}:{tile}"))
}

/// The tileset dock holding the keyboard focus.
fn focused_dock(world: &World) -> Option<Entity> {
    let focused = world.get_resource::<InputFocus>()?.get()?;
    dock_holding(
        focused,
        |e| world.get::<ChildOf>(e).map(ChildOf::parent),
        |e| world.get::<TilesetDock>(e).is_some(),
    )
}

/// The tileset dock `focused` is, or sits in.
fn dock_holding(
    focused: Entity,
    parent: impl Fn(Entity) -> Option<Entity>,
    is_dock: impl Fn(Entity) -> bool,
) -> Option<Entity> {
    std::iter::successors(Some(focused), |e| parent(*e)).find(|e| is_dock(*e))
}

fn follow_focus(
    focus: Res<InputFocus>,
    parents: Query<&ChildOf>,
    docks: Query<(), With<TilesetDock>>,
    mut contexts: Query<&mut ContextActivity, With<TilesetDockInputContext>>,
) {
    let inside = focus.get().is_some_and(|focused| {
        let parent = |e| parents.get(e).ok().map(ChildOf::parent);
        dock_holding(focused, parent, |e| docks.contains(e)).is_some()
    });
    for mut activity in &mut contexts {
        activity.set_if_neq(ContextActivity(inside));
    }
}

/// Enter in a name field adds or renames the terrain; Escape or a click
/// elsewhere closes it.
fn on_name_commit(
    commit: On<TextEditCommitEvent>,
    fields: Query<&TerrainNameField>,
    parents: Query<&ChildOf>,
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
) {
    let Some(field) = (parents.iter_ancestors(commit.entity)).find_map(|e| fields.get(e).ok())
    else {
        return;
    };
    let field = *field;
    let entered = keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]);
    let name = Some(commit.text.trim().to_string()).filter(|n| entered && !n.is_empty());
    commands.queue(move |world: &mut World| name_terrain(world, field, name));
}

fn name_terrain(world: &mut World, field: TerrainNameField, name: Option<String>) {
    let Some((path, tileset)) = dock_tileset(world, field.root) else {
        return;
    };
    let mut views = world.resource_mut::<TilesetDockViews>();
    let Some(view) = views
        .0
        .get_mut(&path)
        .filter(|v| v.naming == Some(field.field))
    else {
        return;
    };
    view.naming = None;
    let selected = view.selected;
    let call = name.and_then(|name| match field.field {
        NameField::New => selected.map(|(sheet, tile)| {
            let params = vec![("name", name), ("icon", format!("{sheet}:{tile}"))];
            ("tileset.terrain.add", params)
        }),
        NameField::Rename(terrain) => {
            let params = vec![("terrain", terrain.to_string()), ("name", name)];
            Some(("tileset.terrain.rename", params))
        }
    });
    dispatch(world, field.root, &path, &tileset, call);
}
