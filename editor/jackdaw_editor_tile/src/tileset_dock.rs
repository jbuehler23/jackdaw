use std::collections::{HashMap, HashSet};
use std::path::Path;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::time::Real;
use bevy::ui_widgets::{Activate, Button, ValueChange};
use jackdaw_api::DefaultArea;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_feathers::button::{ButtonClickEvent, ButtonProps, ButtonSize, ButtonVariant, button};
use jackdaw_feathers::color_picker::{ColorPickerCommitEvent, ColorPickerProps, color_picker};
use jackdaw_feathers::combobox::{ComboBoxChangeEvent, combobox_compact_with_selected};
use jackdaw_feathers::context_menu::spawn_context_menu;
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::menu_bar::{OP_ACTION_PREFIX, SECTION_ACTION_PREFIX, menu_glyph_button};
use jackdaw_feathers::popover::PopoverTracker;
use jackdaw_feathers::text_edit::{TextEditCommitEvent, TextEditProps, text_edit};
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_feathers::{segmented, tokens};
use jackdaw_input::prelude::{ContextActivity, ContextPriority, InputContext};
use jackdaw_panels::area::DockWindow;
use jackdaw_panels::tree::{DockTree, TabId};
use jackdaw_scene_types::{Tilemap, TilemapLayerKind};
use jackdaw_tilemap::{
    Coverage, LayoutTemplate, Placement, ShapeTurn, Suggestion, TerrainId, TilesetSetup,
};
use jackdaw_tileset::{CollisionPreset, TerrainMatch, Tileset, TilesetTerrain, TilesetTerrainSet};
use jackdaw_widgets::context_menu::{ContextMenuAction, ContextMenuState};

use crate::tileset_bits::{
    CoverageSummary, MissingShapes, painted_count, spawn_missing, summary_line,
};
use crate::tileset_edit::{
    TilesetTerrainAddOp, apply_edit, clear_auto, open_tileset, suggested_name, text,
};
use crate::tileset_import::{TILESET_SUFFIX, decode_sheet};
use crate::tileset_layout::{LayoutDraft, TilesetLayoutApplyOp, spawn_draft};
use crate::tileset_modes::{ModeId, ModePanel, ModeSheet, TilesetModeOp, TilesetModes};
use crate::tileset_shape::ShapeEdit;
use crate::tileset_suggest::{TilesetSuggestOp, spawn_outline, spawn_suggestions};
use jackdaw_editor_core::asset_index::assets_dir;
use jackdaw_editor_core::definition_assets::{entry_for_file, resolve_project_path};
use jackdaw_editor_core::operator_tooltip::OperatorTooltip;

pub const TILESET_WINDOW_ID: &str = "jackdaw.tileset";

/// On-screen size a sheet tile is drawn at, at the least.
const READABLE_TILE: f32 = 32.0;
/// On-screen size of the selected tile's preview.
const PREVIEW: f32 = 72.0;
/// Width of the terrains and sets column beside the sheet.
const LIST_WIDTH: f32 = 248.0;
/// Seconds between two clicks of a row that make a double-click.
const DOUBLE_CLICK: f64 = 0.35;
/// What a context menu item of the dock names in its action.
const MENU_ACTION: &str = "tileset.dock.menu.";
/// Match modes, as the operator names them and as the dock shows them.
const MODES: [(&str, &str); 3] = [
    ("corners", "Corners"),
    ("sides", "Sides"),
    ("corners_and_sides", "Corners and sides"),
];

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TilesetDockViews>()
        .init_resource::<ShownTilesets>()
        .init_resource::<SheetImages>()
        .init_resource::<DockMenu>()
        .add_systems(Update, refresh_on_tileset_events)
        .add_observer(on_activate)
        .add_observer(on_mode_change)
        .add_observer(on_set_mode_pick)
        .add_observer(on_row_menu)
        .add_observer(on_menu_action)
        .add_observer(on_color_commit)
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
        .register_operator::<TilesetTerrainFromTileOp>()
        .register_operator::<TilesetModeOp>()
        .register_operator::<TilesetLayoutApplyOp>()
        .register_operator::<TilesetSuggestOp>();
    let context = ctx
        .spawn((
            TilesetDockInputContext,
            ContextPriority(5),
            ContextActivity::INACTIVE,
            Name::new("TilesetDockInputContext"),
            jackdaw_editor_core::EditorEntity,
        ))
        .id();
    ctx.bind_operator_in::<TilesetTerrainFromTileOp>(context, [PresetInput::key("KeyN")]);
    ctx.bind_operator_in::<TilesetLayoutApplyOp>(context, [PresetInput::key("Enter")]);
}

/// The input context of the Tileset window, active while a tileset tab has
/// the keyboard.
#[derive(Component, Default)]
#[require(InputContext)]
pub struct TilesetDockInputContext;

/// What one tileset tab shows; kept across rebuilds of its content.
#[derive(Clone, Debug, Default)]
pub struct DockView {
    pub mode: ModeId,
    /// Sheet and tile.
    pub selected: Option<(u32, u32)>,
    /// Terrain a peer click paints; 0 is empty.
    pub brush: TerrainId,
    /// First corner of the object rectangle; `selected` is the other.
    pub anchor: Option<(u32, u32)>,
    /// The name field open in the terrains list.
    pub naming: Option<NameField>,
    /// The last terrain or set row clicked and when, for a double-click.
    pub last_pick: Option<(DockAction, f64)>,
    /// Terrain sets, by name, whose missing shapes are listed.
    pub expanded: HashSet<String>,
    /// The terrain set, by name, a stroke paints in.
    pub active: Option<String>,
    /// The block whose layout the layout popover is picking.
    pub layout: Option<LayoutDraft>,
    /// Blocks and objects `tileset.suggest` found, while its list is open.
    pub suggestions: Option<Vec<Suggestion>>,
    /// The suggestion outlined on the sheet, as an index of `suggestions`.
    pub shown: Option<usize>,
    /// Whether the suggested objects are listed one by one.
    pub objects_open: bool,
    /// What a stroke of the collision mode paints.
    pub collision: CollisionBrush,
    /// The tile whose collision shape the shape editor is drawing.
    pub shape: Option<ShapeEdit>,
}

/// What a stroke of the collision mode paints, and the physics layer it
/// shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CollisionBrush {
    pub preset: CollisionPreset,
    /// Index into the tileset's physics layers.
    pub layer: u32,
    pub trigger: bool,
}

impl DockView {
    /// Index in `sets` of the active set.
    pub fn active_set(&self, sets: &[TilesetTerrainSet]) -> Option<usize> {
        let name = self.active.as_deref()?;
        sets.iter().position(|s| s.name == name)
    }

    /// The set a stroke paints and the overlay shows: the brush terrain's
    /// set, or the active set when no terrain is picked.
    pub fn paint_set(&self, tileset: &Tileset) -> Option<usize> {
        match self.brush {
            0 => self.active_set(&tileset.terrain_sets),
            brush => tileset.set_of(brush),
        }
    }

    /// A mode switch drops the object rectangle and the layout being picked.
    pub fn set_mode(&mut self, mode: ModeId) {
        self.mode = mode;
        self.anchor = None;
        self.layout = None;
        self.shape = None;
    }
}

/// The match mode picker in the header of set `set`.
#[derive(Component, Clone, Copy, Debug)]
pub struct SetModePick {
    pub root: Entity,
    pub set: usize,
}

/// What a name field of the terrains list names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameField {
    /// A new terrain shown by the selected tile.
    New,
    Rename(TerrainId),
    /// The set at this index.
    RenameSet(usize),
    /// The picked terrain, in the block under the list.
    Picked(TerrainId),
}

/// The context menu a row of a dock opened: its items' actions by index.
#[derive(Resource, Default)]
struct DockMenu {
    root: Option<Entity>,
    actions: Vec<DockAction>,
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
    Mode(ModeId),
    Tile(u32, u32),
    Brush(TerrainId),
    Peer(usize),
    AddObject,
    RemoveObject(u32, UVec2),
    /// Add a terrain to the active set and name it.
    AddTerrain,
    RenameTerrain(TerrainId),
    RemoveTerrain(TerrainId),
    SetIcon(TerrainId),
    /// Move the terrain one place up its set's list, or down.
    MoveTerrain {
        terrain: TerrainId,
        down: bool,
    },
    /// Show or hide the missing shapes of the set at this index.
    ToggleMissing(usize),
    /// Paint in the set at this index.
    ActiveSet(usize),
    RenameSet(usize),
    /// Remove the set at this index.
    RemoveSet(usize),
    /// Read the dragged block as this layout drawn at this placement.
    LayoutPick(LayoutTemplate, Placement),
    LayoutApply,
    LayoutCancel,
    /// Pick or drop the unsure block cell to place by hand.
    LayoutCheck(u32),
    /// Put the cell being checked in this template tile.
    LayoutSlot(usize),
    SuggestionsClose,
    /// Outline the suggestion at this index of the list and scroll to it.
    SuggestionShow(usize),
    SuggestionDismiss(usize),
    /// Open the layout popover on the suggested block.
    SuggestionLayOut(usize),
    SuggestionAddObject(usize),
    /// Show or hide the suggested objects.
    SuggestionObjects,
    /// Paint this collision preset.
    CollisionPreset(CollisionPreset),
    /// Paint triggers, or solid shapes again.
    CollisionTrigger,
    /// Add a physics layer and show it.
    AddPhysicsLayer,
    /// Pick the shape editor's polygon at this index.
    ShapePolygon(usize),
    ShapeAddPolygon,
    ShapeRemovePolygon(usize),
    /// Replace the edited shape with the tile's opaque pixels traced.
    ShapeFromTraced,
    ShapeTurn(ShapeTurn),
    ShapeTrigger,
    ShapeOneWay,
}

/// The drawn image of sheet `sheet` in the dock `root`.
#[derive(Component, Clone, Copy, Debug)]
pub struct SheetPicture {
    pub root: Entity,
    pub sheet: u32,
}

/// The name field of the dock `root`.
#[derive(Component, Clone, Debug)]
pub struct TerrainNameField {
    pub root: Entity,
    pub field: NameField,
    /// The name the field opened with.
    pub shown: String,
}

/// The color picker of terrain `terrain`.
#[derive(Component, Clone, Copy, Debug)]
pub struct TerrainSwatch {
    pub root: Entity,
    pub terrain: TerrainId,
    /// A bare swatch, not the picker with its hex.
    pub compact: bool,
}

/// A control of the tileset dock `root`.
#[derive(Component, Clone, Copy, Debug)]
pub struct DockControl {
    pub root: Entity,
    pub action: DockAction,
}

/// A region of the dock `root`.
#[derive(Component, Clone, Copy, Debug)]
pub struct DockPane {
    pub root: Entity,
    pub pane: Pane,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    /// The terrains and sets beside the sheet.
    List,
    /// What is picked, under the list.
    Detail,
    Sheet,
}

/// How many tiles of the dock's tileset have painted zones, as its More
/// menu says it.
#[derive(Component, Clone, Debug, Default)]
pub struct PaintedCount(pub String);

/// The More menu of the dock `root`.
#[derive(Component, Clone, Copy, Debug)]
pub struct MoreMenu {
    pub root: Entity,
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
    let tab = jackdaw_editor_core::open_window_in_default_area(world, TILESET_WINDOW_ID)
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
    let scrolls: Vec<(Pane, ScrollPosition)> = (world.query::<(&DockPane, &ScrollPosition)>())
        .iter(world)
        .filter(|(p, _)| p.root == root)
        .map(|(p, s)| (p.pane, s.clone()))
        .collect();
    let coloring = open_swatch(world, root);
    world.entity_mut(root).despawn_related::<Children>();
    fill_path(world, root, path);
    if let Some(swatch) = coloring {
        reopen_swatch(world, swatch);
    }
    let panes: Vec<(Entity, Pane)> = (world.query::<(Entity, &DockPane)>())
        .iter(world)
        .filter(|(_, p)| p.root == root)
        .map(|(e, p)| (e, p.pane))
        .collect();
    for (entity, pane) in panes {
        if let Some((_, scroll)) = scrolls.iter().find(|(p, _)| *p == pane) {
            world.entity_mut(entity).insert(scroll.clone());
        }
    }
}

fn fill_path(world: &mut World, root: Entity, path: Option<String>) {
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
            view.active = brush_set(&tileset, view.brush, view.active.as_deref());
            let sets = &tileset.terrain_sets;
            (view.expanded).retain(|name| sets.iter().any(|s| s.name == *name));
            crate::tileset_suggest::prune(&mut view, &tileset);
            crate::tileset_shape::prune(&mut view, &tileset);
            let layers = tileset.physics_layer_names().len() as u32;
            view.collision.layer = view.collision.layer.min(layers.saturating_sub(1));
            let modes = world.get_resource::<TilesetModes>();
            if let Some(modes) = modes.filter(|m| m.get(view.mode).is_none()) {
                view.mode = modes.iter().next().map_or(ModeId::TILES, |m| m.id);
            }
            (world.resource_mut::<TilesetDockViews>().0).insert(path.clone(), view.clone());
            if let Err(err) = fill_tileset(world, root, &tileset, &view) {
                spawn_text(world, root, err, tokens::TEXT_ERROR);
            }
            crate::tileset_shape::sync(world, root, &tileset, &view);
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
        jackdaw_editor_core::status_bar::notify_info(world, notice);
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
    tileset: &Tileset,
    view: &DockView,
) -> Result<(), String> {
    let setup = tileset.setup().map_err(|e| e.to_string())?;
    let modes = world
        .get_resource::<TilesetModes>()
        .cloned()
        .unwrap_or_default();
    let mode = modes
        .get(view.mode)
        .or_else(|| modes.iter().next())
        .cloned();
    world
        .entity_mut(root)
        .insert(PaintedCount(painted_count(&setup)));
    spawn_header(world, root, &modes, view);
    let body = world
        .spawn((
            Node {
                flex_grow: 1.0,
                min_height: px(0.0),
                column_gap: px(tokens::SPACING_MD),
                padding: UiRect::all(px(tokens::SPACING_MD)),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    let side = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                width: px(LIST_WIDTH),
                flex_shrink: 0.0,
                min_height: px(0.0),
                row_gap: px(tokens::SPACING_MD),
                ..default()
            },
            ChildOf(body),
        ))
        .id();
    let list = spawn_pane(world, side, root, Pane::List);
    let detail = spawn_pane(world, side, root, Pane::Detail);
    if let Some(mut node) = world.get_mut::<Node>(detail) {
        node.flex_grow = 0.0;
        node.flex_shrink = 0.0;
        node.max_height = percent(50);
    }
    spawn_suggestions(world, detail, root, tileset, view);
    if let Some(mode) = &mode {
        let panel = ModePanel {
            root,
            list,
            detail,
            tileset,
            setup: &setup,
            view,
        };
        (mode.panel)(world, &panel);
    }
    if world.get::<Children>(detail).is_some_and(|c| !c.is_empty()) {
        world
            .entity_mut(detail)
            .insert(BorderColor::all(tokens::BORDER_SUBTLE));
        if let Some(mut node) = world.get_mut::<Node>(detail) {
            node.border = UiRect::top(px(1.0));
            node.padding.top = px(tokens::SPACING_MD);
        }
    }
    let sheet = spawn_pane(world, body, root, Pane::Sheet);
    if let Some(mut node) = world.get_mut::<Node>(sheet) {
        node.min_width = px(0.0);
        node.row_gap = px(tokens::SPACING_MD);
        node.overflow = Overflow::scroll();
    }
    let overlay = mode.and_then(|m| m.overlay);
    for index in shown_sheets(tileset) {
        spawn_sheet(world, sheet, root, (tileset, &setup), index, view, overlay);
    }
    Ok(())
}

/// The sheets a tab draws, top to bottom: those under terrain setup in its
/// order, then the others.
pub(crate) fn shown_sheets(tileset: &Tileset) -> Vec<u32> {
    let others = (0..tileset.sheets.len() as u32).filter(|s| !tileset.setup_sheets.contains(s));
    tileset.setup_sheets.iter().copied().chain(others).collect()
}

/// The side panel of the tiles and terrains modes: the terrains under their
/// sets, then the picked tile, the picked terrain and the active set's
/// coverage.
pub(crate) fn sheet_panel(world: &mut World, panel: &ModePanel) {
    let ModePanel {
        root,
        list,
        detail,
        tileset,
        setup,
        view,
    } = *panel;
    spawn_tree(world, list, root, tileset, view);
    let active = (view.paint_set(tileset)).and_then(|set| Some((set, setup.coverage(set).ok()?)));
    spawn_detail(world, detail, root, (tileset, setup), view, active);
}

/// The side panel of the objects mode: the objects, then the rectangle
/// being picked.
pub(crate) fn objects_panel(world: &mut World, panel: &ModePanel) {
    spawn_objects(world, panel.list, panel.root, panel.tileset);
    spawn_object_pick(world, panel.detail, panel.root, panel.tileset, panel.view);
}

/// An outline over each object of the sheet.
pub(crate) fn object_outlines(world: &mut World, sheet: &ModeSheet) {
    let (tileset, index) = (sheet.tileset, sheet.sheet);
    for object in tileset.objects.iter().filter(|o| o.sheet == index) {
        let Some((corner, span)) = block_rect(tileset, index, object.origin, object.size) else {
            continue;
        };
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
            ChildOf(sheet.frame),
        ));
    }
}

/// A column of the dock that grows and scrolls on its own.
fn spawn_pane(world: &mut World, parent: Entity, root: Entity, pane: Pane) -> Entity {
    world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                min_height: px(0.0),
                row_gap: px(tokens::SPACING_XS),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            DockPane { root, pane },
            ChildOf(parent),
        ))
        .id()
}

/// The mode bar, and the More menu at the far end.
fn spawn_header(world: &mut World, root: Entity, modes: &TilesetModes, view: &DockView) {
    let header = world
        .spawn((
            Node {
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_LG),
                padding: UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_SM)),
                border: UiRect::bottom(px(1.0)),
                ..default()
            },
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(root),
        ))
        .id();
    let bar: Vec<(DockAction, &str)> = (modes.iter())
        .map(|m| (DockAction::Mode(m.id), m.label))
        .collect();
    let current = DockAction::Mode(view.mode);
    spawn_segments(world, header, root, &bar, current, Some(TilesetModeOp::ID));
    world.spawn((
        Node {
            flex_grow: 1.0,
            ..default()
        },
        ChildOf(header),
    ));
    world.spawn((
        menu_glyph_button(
            "More",
            Icon::Ellipsis,
            std::sync::Arc::new(move |world: &World| more_rows(world, root)),
        ),
        Tooltip::title("More: terrain sets, suggestions and counts"),
        MoreMenu { root },
        ChildOf(header),
    ));
}

/// The rows of the More menu of the dock `root`.
pub fn more_rows(world: &World, root: Entity) -> Vec<(String, String)> {
    let Some(path) = world.get::<TilesetDock>(root).and_then(|d| d.path.clone()) else {
        return Vec::new();
    };
    let suggest = format!("{OP_ACTION_PREFIX}{}?path={path}", TilesetSuggestOp::ID);
    let new_set = format!("{OP_ACTION_PREFIX}tileset.terrain_set.add?path={path}");
    let mut rows = vec![
        (new_set, "New terrain set".to_string()),
        (suggest, "Suggest blocks and objects".to_string()),
    ];
    if let Some(count) = world.get::<PaintedCount>(root) {
        rows.push((format!("{SECTION_ACTION_PREFIX}{}", count.0), String::new()));
    }
    rows
}

fn spawn_segments(
    world: &mut World,
    parent: Entity,
    root: Entity,
    segments: &[(DockAction, &str)],
    current: DockAction,
    operator: Option<&'static str>,
) {
    let bar = world
        .spawn((segmented::segmented_bar(), ChildOf(parent)))
        .id();
    if let Some(mut node) = world.get_mut::<Node>(bar) {
        node.height = px(tokens::ROW_HEIGHT);
    }
    for (action, label) in segments {
        let checked = *action == current;
        let segment = world
            .spawn((
                segmented::segment(*label),
                DockControl {
                    root,
                    action: *action,
                },
                ChildOf(bar),
            ))
            .id();
        let background = BackgroundColor(segmented::segment_background(checked));
        world.entity_mut(segment).insert(background);
        if checked {
            world.entity_mut(segment).insert(bevy::ui::Checked);
        }
        if let Some(id) = operator {
            world
                .entity_mut(segment)
                .insert((OperatorTooltip(id), Hovered::default()));
        }
    }
}

pub(crate) fn spawn_heading(world: &mut World, parent: Entity, text: &str) {
    let heading = spawn_text(world, parent, text, tokens::TEXT_SECONDARY);
    world.entity_mut(heading).insert(Node {
        margin: UiRect::axes(px(tokens::SPACING_SM), px(tokens::SPACING_XS)),
        ..default()
    });
}

/// Each set's terrains under a header of its name and match mode, then Add
/// terrain. A tileset of one set shows its terrains alone.
fn spawn_tree(world: &mut World, parent: Entity, root: Entity, tileset: &Tileset, view: &DockView) {
    let headed = tileset.terrain_sets.len() > 1;
    for (index, set) in tileset.terrain_sets.iter().enumerate() {
        if headed {
            spawn_set_header(world, parent, root, (index, set), view);
        }
        let own = (set.terrains.iter()).filter(|id| tileset.set_of(**id) == Some(index));
        for id in own {
            if let Some(terrain) = tileset.terrains.iter().find(|t| t.id == *id) {
                spawn_terrain_row(world, parent, root, terrain, (view, headed));
            }
        }
    }
    let add = world
        .spawn((
            button(
                ButtonProps::new("Add terrain")
                    .with_left_icon(Icon::Plus)
                    .with_variant(ButtonVariant::Ghost),
            ),
            DockControl {
                root,
                action: DockAction::AddTerrain,
            },
            OperatorTooltip(TilesetTerrainAddOp::ID),
            ChildOf(parent),
        ))
        .id();
    if let Some(mut node) = world.get_mut::<Node>(add) {
        node.align_self = AlignSelf::FlexStart;
    }
}

/// A row that picks terrain `terrain` for painting: its swatch and its name,
/// or its name field while it is renamed.
fn spawn_terrain_row(
    world: &mut World,
    parent: Entity,
    root: Entity,
    terrain: &TilesetTerrain,
    (view, indented): (&DockView, bool),
) {
    let pick = DockAction::Brush(terrain.id);
    let field = NameField::Rename(terrain.id);
    let row = if view.naming == Some(field) {
        let row = spawn_row(world, parent, tokens::SPACING_SM);
        world
            .entity_mut(row)
            .insert(BackgroundColor(tokens::SELECTED_BG));
        row
    } else {
        spawn_pick(world, parent, root, pick, view.brush == terrain.id)
    };
    if indented && let Some(mut node) = world.get_mut::<Node>(row) {
        node.padding.left = px(tokens::SPACING_LG);
    }
    spawn_terrain_swatch(world, row, root, terrain, true);
    if view.naming == Some(field) {
        spawn_name_field(world, row, root, field, terrain.name.clone());
        return;
    }
    let label = spawn_text(world, row, terrain.name.clone(), tokens::TEXT_PRIMARY);
    world.entity_mut(label).insert(Node {
        flex_grow: 1.0,
        ..default()
    });
    world.entity_mut(row).insert((
        Tooltip::title(terrain.name.clone())
            .with_description("Click to paint it, double-click to rename, right-click for more"),
        Hovered::default(),
    ));
}

/// The header of set `index`: a row that makes it active, with its name, and
/// its match mode picker.
fn spawn_set_header(
    world: &mut World,
    parent: Entity,
    root: Entity,
    (index, set): (usize, &TilesetTerrainSet),
    view: &DockView,
) {
    let holder = spawn_row(world, parent, tokens::SPACING_SM);
    if let Some(mut node) = world.get_mut::<Node>(holder) {
        node.padding = UiRect::top(px(tokens::SPACING_SM));
    }
    let field = NameField::RenameSet(index);
    let naming = view.naming == Some(field);
    let active = view.active.as_deref() == Some(set.name.as_str());
    let row = if naming {
        spawn_row(world, holder, tokens::SPACING_SM)
    } else {
        spawn_pick(world, holder, root, DockAction::ActiveSet(index), active)
    };
    if let Some(mut node) = world.get_mut::<Node>(row) {
        node.flex_grow = 1.0;
        node.flex_shrink = 1.0;
        node.min_width = px(0.0);
        node.overflow = Overflow::clip_x();
    }
    if naming {
        spawn_name_field(world, row, root, field, set.name.clone());
    } else {
        spawn_text(world, row, set.name.clone(), tokens::TEXT_SECONDARY);
        world.entity_mut(row).insert((
            Tooltip::title(set.name.clone())
                .with_description("Double-click to rename, right-click for more"),
            Hovered::default(),
        ));
    }
    let modes = MODES.map(|(_, word)| word.to_string()).to_vec();
    world.spawn((
        combobox_compact_with_selected(modes, mode_index(set.mode)),
        SetModePick { root, set: index },
        ChildOf(holder),
    ));
}

/// The index in [`MODES`] of `mode`.
fn mode_index(mode: TerrainMatch) -> usize {
    match mode {
        TerrainMatch::Corners => 0,
        TerrainMatch::Sides => 1,
        TerrainMatch::CornersAndSides => 2,
    }
}

/// The picked tile, the picked terrain's name and color, what a click on
/// the sheet paints, and the active set's missing shapes.
fn spawn_detail(
    world: &mut World,
    parent: Entity,
    root: Entity,
    (tileset, setup): (&Tileset, &TilesetSetup),
    view: &DockView,
    active: Option<(usize, Coverage)>,
) {
    if let Some((sheet, tile)) = view.selected {
        spawn_tile_editor(world, parent, root, (tileset, setup), view, (sheet, tile));
    }
    if view.naming == Some(NameField::New) {
        let row = spawn_row(world, parent, tokens::SPACING_SM);
        spawn_text(world, row, "New terrain", tokens::TEXT_SECONDARY);
        spawn_name_field(world, row, root, NameField::New, suggested_name(tileset));
    }
    let picked = (tileset.terrains.iter()).find(|t| t.id == view.brush);
    if let Some(picked) = picked {
        let row = spawn_row(world, parent, tokens::SPACING_SM);
        spawn_terrain_swatch(world, row, root, picked, false);
        let field = NameField::Picked(picked.id);
        let props = TextEditProps::default().with_default_value(picked.name.clone());
        let holder = world
            .spawn((
                Node {
                    flex_grow: 1.0,
                    min_width: px(0.0),
                    ..default()
                },
                TerrainNameField {
                    root,
                    field,
                    shown: picked.name.clone(),
                },
                ChildOf(row),
            ))
            .id();
        world.spawn((text_edit(props), ChildOf(holder)));
    }
    if view.mode == ModeId::TERRAINS {
        let hint = paint_hint(tileset, picked);
        spawn_text(world, parent, hint, tokens::TEXT_SECONDARY);
    }
    let Some((set, coverage)) = active else {
        return;
    };
    let row = spawn_row(world, parent, tokens::SPACING_SM);
    let label = spawn_text(world, row, summary_line(&coverage), tokens::TEXT_SECONDARY);
    world.entity_mut(label).insert((
        Node {
            flex_grow: 1.0,
            ..default()
        },
        CoverageSummary { root, set },
    ));
    if coverage.missing.is_empty() {
        return;
    }
    let name = setup
        .sets
        .get(set)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    let open = view.expanded.contains(&name);
    let toggle = if open { "Hide" } else { "Show" };
    let action = DockAction::ToggleMissing(set);
    spawn_button(world, row, root, toggle, action);
    if !open {
        return;
    }
    let glyphs = spawn_row(world, parent, tokens::SPACING_XS);
    let shapes = MissingShapes { root, set };
    world.entity_mut(glyphs).insert(shapes);
    if let Some(mut node) = world.get_mut::<Node>(glyphs) {
        node.flex_wrap = FlexWrap::Wrap;
        node.row_gap = px(tokens::SPACING_XS);
    }
    spawn_missing(world, glyphs, shapes, tileset, &coverage);
}

/// The name of the active set: the set of `brush`, else `current` while a
/// set has it, else the first set.
fn brush_set(tileset: &Tileset, brush: TerrainId, current: Option<&str>) -> Option<String> {
    let sets = &tileset.terrain_sets;
    (tileset.set_of(brush).and_then(|set| sets.get(set)))
        .or_else(|| current.and_then(|name| sets.iter().find(|s| s.name == name)))
        .or_else(|| sets.first())
        .map(|s| s.name.clone())
}

/// What a click on the sheet paints, worded by the zones its set matches on.
fn paint_hint(tileset: &Tileset, picked: Option<&TilesetTerrain>) -> String {
    let Some(picked) = picked else {
        return if tileset.terrains.is_empty() {
            "Add a terrain to paint it on the tiles".to_string()
        } else {
            "Pick a terrain to paint it on the tiles".to_string()
        };
    };
    let set = tileset.set_of(picked.id);
    let zones = match set.map(|s| tileset.terrain_sets[s].mode) {
        Some(TerrainMatch::Corners) => "corners",
        Some(TerrainMatch::Sides) => "sides",
        _ => "corners and sides",
    };
    format!("Click {zones} on the tiles to paint {}", picked.name)
}

fn spawn_button(world: &mut World, parent: Entity, root: Entity, label: &str, action: DockAction) {
    world.spawn((
        button(ButtonProps::new(label).with_variant(ButtonVariant::Ghost)),
        DockControl { root, action },
        ChildOf(parent),
    ));
}

pub(crate) fn spawn_icon_button(
    world: &mut World,
    parent: Entity,
    root: Entity,
    icon: Icon,
    tip: &str,
    action: DockAction,
) -> Entity {
    let props = ButtonProps::new("")
        .with_left_icon(icon)
        .with_size(ButtonSize::IconSM)
        .with_variant(ButtonVariant::Ghost);
    world
        .spawn((
            button(props),
            Tooltip::title(tip),
            DockControl { root, action },
            ChildOf(parent),
        ))
        .id()
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
        .with_default_value(name.clone())
        .select_all_on_open()
        .auto_focus();
    let holder = world
        .spawn((
            Node {
                flex_grow: 1.0,
                min_width: px(0.0),
                ..default()
            },
            TerrainNameField {
                root,
                field,
                shown: name,
            },
            ChildOf(parent),
        ))
        .id();
    world.spawn((text_edit(props), ChildOf(holder)));
}

/// The color picker of `terrain`: a bare swatch, or with its hex when not
/// `compact`.
fn spawn_terrain_swatch(
    world: &mut World,
    parent: Entity,
    root: Entity,
    terrain: &TilesetTerrain,
    compact: bool,
) {
    let [r, g, b, _] = terrain.swatch.to_f32_array();
    let props = ColorPickerProps::new().with_color([r, g, b, 1.0]);
    let props = if compact { props.compact() } else { props };
    world.spawn((
        color_picker(props),
        TerrainSwatch {
            root,
            terrain: terrain.id,
            compact,
        },
        Tooltip::title("Color").with_description("The color it shows in on the sheet and the map"),
        Hovered::default(),
        ChildOf(parent),
    ));
}

/// The swatch whose color popover is open in the dock `root`.
fn open_swatch(world: &mut World, root: Entity) -> Option<TerrainSwatch> {
    (world
        .query::<(&TerrainSwatch, &PopoverTracker)>()
        .iter(world))
    .find(|(s, tracker)| s.root == root && tracker.popover.is_some())
    .map(|(s, _)| *s)
}

/// Open the color popover of `swatch` again after its dock was built anew.
fn reopen_swatch(world: &mut World, swatch: TerrainSwatch) {
    let picker = (world.query::<(Entity, &TerrainSwatch)>().iter(world))
        .find(|(_, s)| {
            s.root == swatch.root && s.terrain == swatch.terrain && s.compact == swatch.compact
        })
        .map(|(e, _)| e);
    let trigger = picker
        .and_then(|p| world.get::<Children>(p))
        .and_then(|c| c.first().copied());
    if let Some(entity) = trigger {
        world.trigger(ButtonClickEvent { entity });
    }
}

fn spawn_objects(world: &mut World, parent: Entity, root: Entity, tileset: &Tileset) {
    spawn_heading(world, parent, "Objects");
    for object in &tileset.objects {
        let row = spawn_row(world, parent, tokens::SPACING_SM);
        let (at, size) = (object.origin, object.size);
        let line = format!("{} by {} at {}, {}", size.x, size.y, at.x, at.y);
        let label = spawn_text(world, row, line, tokens::TEXT_PRIMARY);
        world.entity_mut(label).insert(Node {
            flex_grow: 1.0,
            ..default()
        });
        let remove = DockAction::RemoveObject(object.sheet, at);
        spawn_icon_button(world, row, root, Icon::Trash2, "Remove object", remove);
    }
}

fn spawn_sheet(
    world: &mut World,
    parent: Entity,
    root: Entity,
    (tileset, setup): (&Tileset, &TilesetSetup),
    index: u32,
    view: &DockView,
    overlay: Option<fn(&mut World, &ModeSheet)>,
) {
    let Some(sheet) = tileset.sheets.get(index as usize) else {
        return;
    };
    let tile = tileset.tile_size.as_vec2();
    let scale = zoom(tile);
    let image = sheet_image(world, &sheet.source);
    let pixels = (image.as_ref())
        .and_then(|handle| world.resource::<Assets<Image>>().get(handle))
        .map_or(Vec2::ZERO, |image| image.size().as_vec2());
    let size = frame_size(tileset, index, pixels);
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
            || (view.mode == ModeId::OBJECTS
                && rect.is_some_and(|(a, b)| in_rect(sheet.columns, a, b, (index, t))));
        let (outline, border) = tile_outline(picked);
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
    if let Some(overlay) = overlay {
        let sheet = ModeSheet {
            root,
            frame,
            sheet: index,
            tileset,
            setup,
            view,
        };
        overlay(world, &sheet);
    }
    spawn_draft(world, frame, root, tileset, index, view);
    spawn_outline(world, frame, root, tileset, index, view);
}

/// On-screen size of the frame sheet `index` is drawn in, for its image of
/// `pixels`.
pub(crate) fn frame_size(tileset: &Tileset, index: u32, pixels: Vec2) -> Vec2 {
    let Some(sheet) = tileset.sheets.get(index as usize) else {
        return Vec2::ZERO;
    };
    let scale = zoom(tileset.tile_size.as_vec2());
    let step = tileset.tile_size.as_vec2() + sheet.spacing.as_vec2();
    let cells = UVec2::new(sheet.columns, sheet.rows).as_vec2();
    let grid = (sheet.margin.as_vec2() * 2.0 + cells * step - sheet.spacing.as_vec2()) * scale;
    grid.max(pixels * scale)
}

/// The on-screen corner and size of the `size` block at `origin` of sheet
/// `index`, in its frame.
pub(crate) fn block_rect(
    tileset: &Tileset,
    index: u32,
    origin: UVec2,
    size: UVec2,
) -> Option<(Vec2, Vec2)> {
    let columns = tileset.sheets.get(index as usize)?.columns;
    let last = origin + size - UVec2::ONE;
    let (min, _) = tileset.sheet_rect(index, origin.y * columns + origin.x)?;
    let (max, tile) = tileset.sheet_rect(index, last.y * columns + last.x)?;
    let scale = zoom(tileset.tile_size.as_vec2());
    Some((min.as_vec2() * scale, (max + tile - min).as_vec2() * scale))
}

/// Screen pixels per sheet pixel: whole, and enough for a tile to read.
pub(crate) fn zoom(tile: Vec2) -> f32 {
    (READABLE_TILE / tile.max_element().max(1.0))
        .ceil()
        .max(1.0)
}

/// The border colour and width of a sheet tile, picked or not.
pub(crate) fn tile_outline(picked: bool) -> (Color, f32) {
    if picked {
        (tokens::SELECTED_BORDER, 2.0)
    } else {
        (tokens::BORDER_SUBTLE.with_alpha(0.3), 1.0)
    }
}

pub(crate) fn in_rect(columns: u32, a: (u32, u32), b: (u32, u32), tile: (u32, u32)) -> bool {
    let (lo, hi) = rect_cells(columns, a, b);
    let cell = UVec2::new(tile.1 % columns, tile.1 / columns);
    a.0 == tile.0 && cell.cmpge(lo).all() && cell.cmple(hi).all()
}

pub(crate) fn rect_cells(columns: u32, a: (u32, u32), b: (u32, u32)) -> (UVec2, UVec2) {
    let a = UVec2::new(a.1 % columns, a.1 / columns);
    let b = UVec2::new(b.1 % columns, b.1 / columns);
    (a.min(b), a.max(b))
}

/// The picked tile with a button over each zone that paints the brush
/// terrain on it.
fn spawn_tile_editor(
    world: &mut World,
    parent: Entity,
    root: Entity,
    (tileset, setup): (&Tileset, &TilesetSetup),
    view: &DockView,
    (sheet, tile): (u32, u32),
) {
    let row = spawn_row(world, parent, tokens::SPACING_MD);
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
            ChildOf(row),
        ))
        .id();
    for (peer, terrain) in peers.iter().enumerate() {
        world.spawn((
            Node {
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
        ));
    }
    let text = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                flex_shrink: 1.0,
                min_width: px(0.0),
                row_gap: px(tokens::SPACING_XS),
                ..default()
            },
            ChildOf(row),
        ))
        .id();
    let name = if tileset.sheets.len() > 1 {
        format!("Sheet {}, tile {tile}", sheet + 1)
    } else {
        format!("Tile {tile}")
    };
    spawn_text(world, text, name, tokens::TEXT_PRIMARY);
    let brush = (tileset.terrains.iter()).find(|t| t.id == view.brush);
    let hint = match brush {
        Some(brush) => format!("Click a zone to paint {}, again to clear it", brush.name),
        None => "Click a zone to clear it".to_string(),
    };
    spawn_text(world, text, hint, tokens::TEXT_SECONDARY);
}

fn spawn_object_pick(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    view: &DockView,
) {
    let hint = match (view.anchor, view.selected) {
        (Some(a), Some(b)) if tileset.sheets.len() > 1 => {
            format!("Sheet {}, tiles {} to {}", a.0 + 1, a.1, b.1)
        }
        (Some(a), Some(b)) => format!("Tiles {} to {}", a.1, b.1),
        _ => "Click two corner tiles".to_string(),
    };
    let row = spawn_row(world, parent, tokens::SPACING_SM);
    let label = spawn_text(world, row, hint, tokens::TEXT_SECONDARY);
    world.entity_mut(label).insert(Node {
        flex_grow: 1.0,
        ..default()
    });
    world.spawn((
        button(ButtonProps::new("Add object").with_variant(ButtonVariant::Primary)),
        DockControl {
            root,
            action: DockAction::AddObject,
        },
        ChildOf(row),
    ));
}

/// A row that picks something when clicked, highlighted while it is the pick.
pub(crate) fn spawn_pick(
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

pub(crate) fn spawn_row(world: &mut World, parent: Entity, gap: f32) -> Entity {
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

pub fn spawn_text(
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

fn on_mode_change(
    change: On<ValueChange<Entity>>,
    controls: Query<&DockControl>,
    mut commands: Commands,
) {
    if let Ok(control) = controls.get(change.value).copied() {
        commands.queue(move |world: &mut World| act(world, control));
    }
}

/// A pick of a set's match mode picker changes its mode.
fn on_set_mode_pick(
    change: On<ComboBoxChangeEvent>,
    picks: Query<&SetModePick>,
    mut commands: Commands,
) {
    let Ok(pick) = picks.get(change.entity).copied() else {
        return;
    };
    let Some((mode, _)) = MODES.get(change.selected).copied() else {
        return;
    };
    commands.queue(move |world: &mut World| {
        let Some((path, tileset)) = dock_tileset(world, pick.root) else {
            return;
        };
        let params = vec![("set", pick.set.to_string()), ("mode", mode.to_string())];
        let call = Some(("tileset.terrain_set.set", params));
        dispatch(world, pick.root, &path, &tileset, call);
    });
}

/// A right-click on a terrain row or a set header opens its context menu.
fn on_row_menu(
    click: On<PointerClick>,
    controls: Query<&DockControl>,
    ui_scale: Res<UiScale>,
    mut commands: Commands,
) {
    if click.button != PointerButton::Secondary {
        return;
    }
    let Ok(control) = controls.get(click.entity).copied() else {
        return;
    };
    let at = click.pointer.position / ui_scale.0;
    commands.queue(move |world: &mut World| {
        open_row_menu(world, control.root, control.action, at);
    });
}

/// Open the context menu of the row of the dock `root` that `row` picks, at
/// `at`: Rename, Delete and, for a terrain, moves in its set and the
/// selected tile as its icon. Returns the menu.
pub fn open_row_menu(world: &mut World, root: Entity, row: DockAction, at: Vec2) -> Option<Entity> {
    let (path, tileset) = dock_tileset(world, root)?;
    let view = (world.resource::<TilesetDockViews>().0.get(&path).cloned()).unwrap_or_default();
    let items: Vec<(DockAction, &str)> = match row {
        DockAction::Brush(terrain) => {
            let set = tileset.set_of(terrain)?;
            let terrains = &tileset.terrain_sets[set].terrains;
            let at = terrains.iter().position(|t| *t == terrain)?;
            let mut items = vec![(DockAction::RenameTerrain(terrain), "Rename")];
            if at > 0 {
                let up = DockAction::MoveTerrain {
                    terrain,
                    down: false,
                };
                items.push((up, "Move up"));
            }
            if at + 1 < terrains.len() {
                let down = DockAction::MoveTerrain {
                    terrain,
                    down: true,
                };
                items.push((down, "Move down"));
            }
            if view.selected.is_some() {
                items.push((DockAction::SetIcon(terrain), "Use selected tile as icon"));
            }
            items.push((DockAction::RemoveTerrain(terrain), "Delete"));
            items
        }
        DockAction::ActiveSet(set) => vec![
            (DockAction::RenameSet(set), "Rename"),
            (DockAction::RemoveSet(set), "Delete"),
        ],
        _ => return None,
    };
    let labels: Vec<(String, &str)> = (items.iter().enumerate())
        .map(|(i, (_, label))| (format!("{MENU_ACTION}{i}"), *label))
        .collect();
    let rows: Vec<(&str, &str)> = labels.iter().map(|(a, l)| (a.as_str(), *l)).collect();
    let mut commands = world.commands();
    let menu = spawn_context_menu(&mut commands, at, Some(root), &rows);
    world.flush();
    let old = (world.get_resource_mut::<ContextMenuState>())
        .and_then(|mut state| state.menu_entity.replace(menu));
    if let Some(old) = old.filter(|old| *old != menu) {
        world.commands().entity(old).try_despawn();
    }
    *world.resource_mut::<DockMenu>() = DockMenu {
        root: Some(root),
        actions: items.into_iter().map(|(action, _)| action).collect(),
    };
    Some(menu)
}

/// An item of a row's context menu runs its action on the dock.
fn on_menu_action(event: On<ContextMenuAction>, menu: Res<DockMenu>, mut commands: Commands) {
    let Some(index) =
        (event.action.strip_prefix(MENU_ACTION)).and_then(|i| i.parse::<usize>().ok())
    else {
        return;
    };
    let (Some(root), Some(action)) = (menu.root, menu.actions.get(index).copied()) else {
        return;
    };
    commands.queue(move |world: &mut World| {
        if world.get_entity(root).is_ok() {
            act(world, DockControl { root, action });
        }
    });
}

/// A color picked on a terrain's swatch recolors the terrain.
fn on_color_commit(
    commit: On<ColorPickerCommitEvent>,
    swatches: Query<&TerrainSwatch>,
    mut commands: Commands,
) {
    let Ok(swatch) = swatches.get(commit.entity).copied() else {
        return;
    };
    let [r, g, b, _] = commit.color;
    let color = Srgba::new(r, g, b, 1.0).to_hex();
    commands.queue(move |world: &mut World| {
        let Some((path, tileset)) = dock_tileset(world, swatch.root) else {
            return;
        };
        let params = vec![("terrain", swatch.terrain.to_string()), ("color", color)];
        let call = Some(("tileset.terrain.set_color", params));
        dispatch(world, swatch.root, &path, &tileset, call);
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
pub(crate) fn dock_tileset(world: &mut World, root: Entity) -> Option<(String, Tileset)> {
    let path = world.get::<TilesetDock>(root)?.path.clone()?;
    let (_, tileset) = load(world, &path).ok()?;
    Some((path, tileset?))
}

/// An operator and its parameters, for the tileset of a dock.
pub type DockCall = (&'static str, Vec<(&'static str, String)>);

fn act(world: &mut World, control: DockControl) {
    if crate::tileset_layout::act(world, control)
        || crate::tileset_suggest::act(world, control)
        || crate::tileset_shape::act(world, control)
    {
        return;
    }
    let root = control.root;
    for (field, name) in typed_names(world, root) {
        name_terrain(world, field, Some(name));
    }
    let Some((path, tileset)) = dock_tileset(world, root) else {
        return;
    };
    let mut view = (world.resource::<TilesetDockViews>().0.get(&path).cloned()).unwrap_or_default();
    let paints = (world.get_resource::<TilesetModes>())
        .and_then(|modes| modes.get(view.mode))
        .is_some_and(|mode| mode.stroke.is_some());
    if paints && matches!(control.action, DockAction::Tile(..)) {
        take_keyboard(world, root);
        return;
    }
    view.naming = None;
    let now = (world.get_resource::<Time<Real>>()).map_or(0.0, Time::elapsed_secs_f64);
    let again = (view.last_pick.take())
        .is_some_and(|(last, at)| last == control.action && now - at < DOUBLE_CLICK);
    let tile_ref = view.selected.map(|(s, t)| format!("{s}:{t}"));
    let call = match control.action {
        DockAction::Mode(mode) => {
            take_keyboard(world, root);
            let params = vec![("mode", mode.0.to_string())];
            call_operator(world, &path, TilesetModeOp::ID, params);
            return;
        }
        DockAction::Brush(brush) if again => {
            view.naming = Some(NameField::Rename(brush));
            None
        }
        DockAction::Brush(brush) => {
            view.brush = brush;
            if let Some(set) = tileset.set_of(brush) {
                view.active = set_name(&tileset, set);
            }
            view.last_pick = Some((control.action, now));
            None
        }
        DockAction::ActiveSet(set) if again => {
            view.naming = Some(NameField::RenameSet(set));
            None
        }
        DockAction::ActiveSet(set) => {
            view.active = set_name(&tileset, set);
            if tileset.set_of(view.brush) != Some(set) {
                view.brush = 0;
            }
            view.last_pick = Some((control.action, now));
            None
        }
        DockAction::AddTerrain => {
            let mut params = vec![("name", suggested_name(&tileset))];
            if let Some(set) = view.active.clone() {
                params.push(("set", set));
            }
            Some((TilesetTerrainAddOp::ID, params))
        }
        DockAction::RenameSet(set) => {
            view.naming = Some(NameField::RenameSet(set));
            None
        }
        DockAction::MoveTerrain { terrain, down } => {
            let set = tileset.set_of(terrain);
            let at = set.and_then(|set| {
                (tileset.terrain_sets[set].terrains.iter()).position(|t| *t == terrain)
            });
            let to = at.and_then(|at| {
                if down {
                    Some(at + 1)
                } else {
                    at.checked_sub(1)
                }
            });
            set.zip(to).map(|(set, to)| {
                let params = vec![
                    ("set", set.to_string()),
                    ("terrain", terrain.to_string()),
                    ("to", to.to_string()),
                ];
                ("tileset.terrain_set.move_terrain", params)
            })
        }
        DockAction::Tile(sheet, tile) => {
            let extend = view.mode == ModeId::OBJECTS
                && view.anchor.is_some_and(|a| a.0 == sheet)
                && view.anchor == view.selected;
            if !extend {
                view.anchor = Some((sheet, tile));
            }
            view.selected = Some((sheet, tile));
            None
        }
        DockAction::Peer(peer) => view.selected.map(|(sheet, tile)| {
            let held = (tileset.tiles.iter())
                .find(|t| t.sheet == sheet && t.tile == tile)
                .and_then(|t| t.peers.get(peer).copied());
            let terrain = if held == Some(view.brush) {
                0
            } else {
                view.brush
            };
            let params = vec![
                ("tile", format!("{sheet}:{tile}")),
                ("peer", peer.to_string()),
                ("terrain", terrain.to_string()),
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
        DockAction::LayoutPick(..)
        | DockAction::LayoutApply
        | DockAction::LayoutCancel
        | DockAction::LayoutCheck(_)
        | DockAction::LayoutSlot(_)
        | DockAction::SuggestionsClose
        | DockAction::SuggestionShow(_)
        | DockAction::SuggestionDismiss(_)
        | DockAction::SuggestionLayOut(_)
        | DockAction::SuggestionAddObject(_)
        | DockAction::SuggestionObjects
        | DockAction::ShapePolygon(_)
        | DockAction::ShapeAddPolygon
        | DockAction::ShapeRemovePolygon(_)
        | DockAction::ShapeFromTraced
        | DockAction::ShapeTurn(_)
        | DockAction::ShapeTrigger
        | DockAction::ShapeOneWay => None,
        DockAction::CollisionPreset(preset) => {
            view.collision.preset = preset;
            None
        }
        DockAction::CollisionTrigger => {
            view.collision.trigger = !view.collision.trigger;
            None
        }
        DockAction::AddPhysicsLayer => {
            let params = vec![("action", "add".to_string())];
            Some((
                crate::tileset_collision::TilesetCollisionLayerOp::ID,
                params,
            ))
        }
        DockAction::RenameTerrain(terrain) => {
            view.naming = Some(NameField::Rename(terrain));
            None
        }
        DockAction::SetIcon(terrain) => tile_ref.map(|icon| {
            let params = vec![("terrain", terrain.to_string()), ("icon", icon)];
            ("tileset.terrain.set_icon", params)
        }),
        DockAction::ToggleMissing(set) => {
            if let Some(name) = set_name(&tileset, set)
                && !view.expanded.remove(&name)
            {
                view.expanded.insert(name);
            }
            None
        }
        DockAction::RemoveTerrain(terrain) => {
            let params = vec![("terrain", terrain.to_string())];
            Some(("tileset.terrain.remove", params))
        }
        DockAction::RemoveSet(set) => {
            let params = vec![("set", set.to_string())];
            Some(("tileset.terrain_set.remove", params))
        }
    };
    (world.resource_mut::<TilesetDockViews>().0).insert(path.clone(), view);
    take_keyboard(world, root);
    if control.action == DockAction::AddTerrain {
        add_terrain(world, root, &path, &tileset, call);
        return;
    }
    dispatch(world, root, &path, &tileset, call);
}

/// Run `call`, which adds a terrain, then pick the new terrain and open its
/// name field.
fn add_terrain(
    world: &mut World,
    root: Entity,
    path: &str,
    before: &Tileset,
    call: Option<DockCall>,
) {
    dispatch(world, root, path, before, call);
    let Some((_, after)) = dock_tileset(world, root) else {
        return;
    };
    let Some(added) =
        (after.terrains.iter()).find(|t| !before.terrains.iter().any(|b| b.id == t.id))
    else {
        return;
    };
    if let Some(view) = world.resource_mut::<TilesetDockViews>().0.get_mut(path) {
        view.brush = added.id;
        view.naming = Some(NameField::Rename(added.id));
        view.active = after.set_of(added.id).and_then(|set| set_name(&after, set));
    }
}

/// The name of the set at `set`.
fn set_name(tileset: &Tileset, set: usize) -> Option<String> {
    tileset.terrain_sets.get(set).map(|s| s.name.clone())
}

/// Make the set named `set` the one the Tileset tab of `path` adds and
/// paints in.
pub(crate) fn make_active(world: &mut World, path: &str, set: String) {
    let Ok((file, _)) = open_tileset(world, path) else {
        return;
    };
    let key = file.to_string_lossy().into_owned();
    if let Some(view) = world.resource_mut::<TilesetDockViews>().0.get_mut(&key) {
        view.active = Some(set);
    }
}

/// Give the dock `root` the keyboard, so its keys apply to it.
pub(crate) fn take_keyboard(world: &mut World, root: Entity) {
    if let Some(mut focus) = world.get_resource_mut::<InputFocus>() {
        focus.set(root, FocusCause::Pressed);
    }
}

/// Run `call` on the tileset at `path`, then build `root` again unless the
/// tileset changed, which refreshes it through its asset event.
pub(crate) fn dispatch(
    world: &mut World,
    root: Entity,
    path: &str,
    before: &Tileset,
    call: Option<DockCall>,
) {
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

pub(crate) fn call_operator(
    world: &mut World,
    path: &str,
    id: &str,
    params: Vec<(&'static str, String)>,
) {
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
                   selected sheet tile; Enter or leaving the field adds it through \
                   tileset.terrain.add, Escape cancels.",
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
    let path = dock_path(world, path)?;
    let mut views = world.resource_mut::<TilesetDockViews>();
    let view = (views.0.get_mut(&path)).ok_or_else(|| format!("{path} has no Tileset tab"))?;
    let (sheet, tile) = view.selected.ok_or("no sheet tile is selected")?;
    if view.mode != ModeId::TILES && view.mode != ModeId::TERRAINS {
        view.mode = ModeId::TILES;
    }
    view.naming = Some(NameField::New);
    refill(world, &path);
    Ok(format!("naming a new terrain shown by tile {sheet}:{tile}"))
}

/// The tileset of the tab with the keyboard when `path` is empty, else the
/// one `path` names.
pub(crate) fn dock_path(world: &mut World, path: &str) -> Result<String, String> {
    let focused = focused_dock(world).and_then(|root| world.get::<TilesetDock>(root)?.path.clone());
    match focused {
        Some(found) if path.is_empty() => Ok(found),
        _ => Ok(open_tileset(world, path)?.0.to_string_lossy().into_owned()),
    }
}

/// Build every dock showing the tileset at `path` again.
pub(crate) fn refill(world: &mut World, path: &str) {
    let roots: Vec<Entity> = (world.query::<(Entity, &TilesetDock)>().iter(world))
        .filter(|(_, d)| d.path.as_deref() == Some(path))
        .map(|(root, _)| root)
        .collect();
    for root in roots {
        fill(world, root);
    }
}

/// The tileset of the Tileset tab holding the keyboard focus.
pub(crate) fn focused_tileset(world: &World) -> Option<String> {
    world.get::<TilesetDock>(focused_dock(world)?)?.path.clone()
}

/// Build every tileset dock again.
pub(crate) fn refill_all(world: &mut World) {
    let roots: Vec<Entity> = (world
        .query_filtered::<Entity, With<TilesetDock>>()
        .iter(world))
    .collect();
    for root in roots {
        fill(world, root);
    }
}

/// The tileset of a Tileset tab in front of its dock area.
pub(crate) fn front_tileset(world: &World) -> Option<String> {
    let tree = world.resource::<DockTree>();
    (tree.tabs())
        .filter(|(_, t)| t.window_id == TILESET_WINDOW_ID)
        .find(|(leaf, t)| {
            (tree.get(*leaf).and_then(|node| node.as_leaf()))
                .is_some_and(|l| l.active == Some(t.id))
        })
        .and_then(|(_, t)| t.payload.clone())
}

/// The tileset dock holding the keyboard focus.
pub(crate) fn focused_dock(world: &World) -> Option<Entity> {
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

/// Enter in a name field, or the field losing the keyboard, adds or renames
/// the terrain; Escape closes it.
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
    let field = field.clone();
    let cancelled = keys.just_pressed(KeyCode::Escape);
    let name = Some(commit.text.trim().to_string()).filter(|n| !cancelled && !n.is_empty());
    commands.queue(move |world: &mut World| name_terrain(world, field, name));
}

/// The name fields of the dock `root` and the names typed in them, those
/// left empty skipped.
fn typed_names(world: &mut World, root: Entity) -> Vec<(TerrainNameField, String)> {
    let holders: Vec<(Entity, TerrainNameField)> = (world.query::<(Entity, &TerrainNameField)>())
        .iter(world)
        .filter(|(_, f)| f.root == root)
        .map(|(e, f)| (e, f.clone()))
        .collect();
    let mut typed = Vec::new();
    for (holder, field) in holders {
        let mut stack = vec![holder];
        while let Some(entity) = stack.pop() {
            if let Some(text) = world.get::<bevy::text::EditableText>(entity) {
                let name = text.value().trim().to_string();
                if !name.is_empty() {
                    typed.push((field, name));
                }
                break;
            }
            stack.extend(world.get::<Children>(entity).into_iter().flatten());
        }
    }
    typed
}

/// Add, rename or leave the terrain or set `field` names with `name`; `None`
/// closes the field, or puts the picked terrain's name back. A name the
/// field opened with renames nothing.
fn name_terrain(world: &mut World, field: TerrainNameField, name: Option<String>) {
    let Some((path, tileset)) = dock_tileset(world, field.root) else {
        return;
    };
    if let NameField::Picked(terrain) = field.field {
        let call = match name {
            Some(name) if name == field.shown => return,
            Some(name) => {
                let params = vec![("terrain", terrain.to_string()), ("name", name)];
                Some(("tileset.terrain.rename", params))
            }
            None => None,
        };
        dispatch(world, field.root, &path, &tileset, call);
        return;
    }
    let name = name.filter(|n| *n != field.shown);
    let mut views = world.resource_mut::<TilesetDockViews>();
    let Some(view) = views
        .0
        .get_mut(&path)
        .filter(|v| v.naming == Some(field.field))
    else {
        return;
    };
    view.naming = None;
    let (selected, active) = (view.selected, view.active.clone());
    let call = name.clone().and_then(|name| match field.field {
        NameField::New => selected.map(|(sheet, tile)| {
            let mut params = vec![("name", name), ("icon", format!("{sheet}:{tile}"))];
            if let Some(set) = active.clone() {
                params.push(("set", set));
            }
            ("tileset.terrain.add", params)
        }),
        NameField::Rename(terrain) => {
            let params = vec![("terrain", terrain.to_string()), ("name", name)];
            Some(("tileset.terrain.rename", params))
        }
        NameField::RenameSet(set) => {
            let params = vec![("set", set.to_string()), ("name", name)];
            Some(("tileset.terrain_set.rename", params))
        }
        NameField::Picked(_) => None,
    });
    dispatch(world, field.root, &path, &tileset, call);
    let NameField::RenameSet(set) = field.field else {
        return;
    };
    let renamed = dock_tileset(world, field.root)
        .and_then(|(_, after)| after.terrain_sets.get(set).map(|s| s.name.clone()))
        .filter(|now| name.as_ref() == Some(now));
    let was = tileset.terrain_sets.get(set).map(|s| s.name.clone());
    if let Some(renamed) = renamed
        && active.is_some()
        && active == was
        && let Some(view) = world.resource_mut::<TilesetDockViews>().0.get_mut(&path)
    {
        view.active = Some(renamed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dock_fills_with_only_its_own_plugin() {
        let mut app = App::new();
        app.add_plugins(plugin);
        let world = app.world_mut();
        let root = (world.spawn(TilesetDock {
            tab: TabId(1),
            path: None,
            asset: None,
        }))
        .id();
        world.spawn((Node::default(), ChildOf(root)));
        fill(world, root);
        assert_eq!(world.get::<Children>(root).map(Children::len), Some(1));
    }
}
