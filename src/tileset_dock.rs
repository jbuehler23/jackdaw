use std::collections::{HashMap, HashSet};
use std::path::Path;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::{Activate, Button, ValueChange};
use jackdaw_api::DefaultArea;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_feathers::button::{ButtonProps, ButtonSize, ButtonVariant, button};
use jackdaw_feathers::combobox::{ComboBoxChangeEvent, combobox_compact_with_selected};
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::text_edit::{TextEditCommitEvent, TextEditProps, text_edit};
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_feathers::{segmented, tokens};
use jackdaw_input::prelude::{ContextActivity, ContextPriority, InputContext};
use jackdaw_panels::area::DockWindow;
use jackdaw_panels::tree::{DockTree, TabId};
use jackdaw_scene_types::{Tilemap, TilemapLayerKind};
use jackdaw_tilemap::{Coverage, LayoutTemplate, Placement, Suggestion, TerrainId, TilesetSetup};
use jackdaw_tileset::{TerrainCollision, Tileset, TilesetTerrain};

use crate::asset_index::assets_dir;
use crate::definition_assets::{entry_for_file, resolve_project_path};
use crate::operator_tooltip::OperatorTooltip;
use crate::tileset_bits::{
    BitsCount, CoverageLine, CoverageSummary, MissingShapes, TilesetBitsOverlayOp,
    TilesetPaintModeOp, coverage_line, mode_word, painted_count, spawn_marks, spawn_missing,
    summary_line,
};
use crate::tileset_edit::{apply_edit, clear_auto, open_tileset, suggested_name, text};
use crate::tileset_import::{TILESET_SUFFIX, decode_sheet};
use crate::tileset_layout::{LayoutDraft, TilesetLayoutApplyOp, spawn_draft};
use crate::tileset_suggest::{
    TilesetSuggestOp, spawn_outline, spawn_suggest_button, spawn_suggestions,
};

pub const TILESET_WINDOW_ID: &str = "jackdaw.tileset";

/// On-screen size a sheet tile is drawn at, at the least.
const READABLE_TILE: f32 = 32.0;
/// On-screen size of the selected tile's preview.
const PREVIEW: f32 = 72.0;
/// On-screen size of a terrain's icon in the terrains list.
const ICON: f32 = 20.0;
/// Width of the terrains and sets column beside the sheet.
const LIST_WIDTH: f32 = 248.0;
/// Collision modes, as the operator names them and as the dock shows them.
const COLLISIONS: [(&str, &str); 3] = [
    ("solid", "Solid"),
    ("trigger", "Trigger"),
    ("passable", "Passable"),
];
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
        .add_systems(Update, refresh_on_tileset_events)
        .add_observer(on_activate)
        .add_observer(on_page_change)
        .add_observer(on_collision_pick)
        .add_observer(on_new_set_pick)
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
        .register_operator::<TilesetBitsOverlayOp>()
        .register_operator::<TilesetPaintModeOp>()
        .register_operator::<TilesetLayoutApplyOp>()
        .register_operator::<TilesetSuggestOp>();
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
    ctx.bind_operator_in::<TilesetBitsOverlayOp>(context, [PresetInput::key("KeyO")]);
    ctx.bind_operator_in::<TilesetPaintModeOp>(context, [PresetInput::key("KeyP")]);
    ctx.bind_operator_in::<TilesetLayoutApplyOp>(context, [PresetInput::key("Enter")]);
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
    pub tool: DockTool,
    pub bits: BitsOverlay,
    /// Inner, outer and mode picked in the new set control, as indices of
    /// its lists.
    pub new_set: Option<[usize; 3]>,
    /// The terrain set, as inner and outer, a stroke paints in.
    pub active: Option<(TerrainId, TerrainId)>,
    /// The block whose layout the layout popover is picking.
    pub layout: Option<LayoutDraft>,
    /// Blocks and objects `tileset.suggest` found, while its list is open.
    pub suggestions: Option<Vec<Suggestion>>,
    /// The suggestion outlined on the sheet, as an index of `suggestions`.
    pub shown: Option<usize>,
    /// Whether the suggested objects are listed one by one.
    pub objects_open: bool,
}

/// What a press on the sheet does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DockTool {
    #[default]
    Select,
    /// Left paints the brush terrain, right erases.
    Paint,
}

impl DockTool {
    pub const ALL: [(DockTool, &'static str); 2] =
        [(DockTool::Select, "select"), (DockTool::Paint, "paint")];
}

impl DockView {
    /// Painting shows the bits overlay if it is off. A switch drops the
    /// layout being picked.
    pub fn set_tool(&mut self, tool: DockTool) {
        self.tool = tool;
        self.layout = None;
        if tool == DockTool::Paint && self.bits == BitsOverlay::Off {
            self.bits = BitsOverlay::Set;
        }
    }

    /// A page switch drops the object rectangle and the layout being picked.
    pub fn set_page(&mut self, page: DockPage) {
        self.page = page;
        self.anchor = None;
        self.layout = None;
    }
}

/// A picker of the new set control: 0 inner, 1 outer, 2 mode.
#[derive(Component, Clone, Copy, Debug)]
pub struct NewSetPick {
    pub root: Entity,
    pub field: usize,
}

/// Which tiles the sheet draws the painted zones of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BitsOverlay {
    Off,
    /// Those of the active set.
    #[default]
    Set,
    All,
}

impl BitsOverlay {
    pub const ALL: [(BitsOverlay, &'static str); 3] = [
        (BitsOverlay::Off, "off"),
        (BitsOverlay::Set, "set"),
        (BitsOverlay::All, "all"),
    ];
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
    Bits(BitsOverlay),
    /// Paint in the set `inner` over `outer`.
    ActiveSet(TerrainId, TerrainId),
    Tool(DockTool),
    /// Add the set picked in the new set control.
    CreateSet,
    /// Remove the set `inner` over `outer`.
    RemoveSet(TerrainId, TerrainId),
    /// Read the dragged block as this layout drawn at this placement.
    LayoutPick(LayoutTemplate, Placement),
    LayoutApply,
    LayoutCancel,
    /// Pick or drop the unsure block cell to place by hand.
    LayoutCheck(u32),
    /// Put the cell being checked in this template tile.
    LayoutSlot(usize),
    Suggest,
    SuggestionsClose,
    /// Outline the suggestion at this index of the list and scroll to it.
    SuggestionShow(usize),
    SuggestionDismiss(usize),
    /// Open the layout popover on the suggested block.
    SuggestionLayOut(usize),
    SuggestionAddObject(usize),
    /// Show or hide the suggested objects.
    SuggestionObjects,
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
    let scrolls: Vec<(Pane, ScrollPosition)> = (world.query::<(&DockPane, &ScrollPosition)>())
        .iter(world)
        .filter(|(p, _)| p.root == root)
        .map(|(p, s)| (p.pane, s.clone()))
        .collect();
    world.entity_mut(root).despawn_related::<Children>();
    fill_path(world, root, path);
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
            view.active = brush_set(&tileset, view.brush, view.active);
            crate::tileset_suggest::prune(&mut view, &tileset);
            (world.resource_mut::<TilesetDockViews>().0).insert(path.clone(), view.clone());
            if let Err(err) = fill_tileset(world, root, &tileset, &view) {
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
    tileset: &Tileset,
    view: &DockView,
) -> Result<(), String> {
    let setup = tileset.setup().map_err(|e| e.to_string())?;
    spawn_header(world, root, &setup, view);
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
    match view.page {
        DockPage::Sheet => {
            spawn_terrains(world, list, root, tileset, view);
            spawn_sets(world, list, root, tileset, &setup, view);
            spawn_new_set(world, list, root, tileset, view);
            let active =
                (view.active).and_then(|(i, o)| Some(((i, o), setup.coverage(i, o).ok()?)));
            spawn_detail(world, detail, root, (tileset, &setup), view, active);
        }
        DockPage::Objects => {
            spawn_objects(world, list, root, tileset);
            spawn_object_pick(world, detail, root, tileset, view);
        }
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
    for index in &tileset.setup_sheets {
        spawn_sheet(world, sheet, root, (tileset, &setup), *index, view);
    }
    Ok(())
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

/// Page, mode and overlay controls, the painted tile count and New terrain,
/// in groups.
fn spawn_header(world: &mut World, root: Entity, setup: &TilesetSetup, view: &DockView) {
    let header = world
        .spawn((
            Node {
                align_items: AlignItems::Center,
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(tokens::SPACING_LG),
                row_gap: px(tokens::SPACING_SM),
                padding: UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_SM)),
                border: UiRect::bottom(px(1.0)),
                ..default()
            },
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(root),
        ))
        .id();
    let pages = [
        (DockAction::Page(DockPage::Sheet), "Sheet"),
        (DockAction::Page(DockPage::Objects), "Objects"),
    ];
    spawn_segments(
        world,
        header,
        root,
        &pages,
        DockAction::Page(view.page),
        None,
    );
    if view.page != DockPage::Sheet {
        let actions = spawn_row(world, header, tokens::SPACING_MD);
        spawn_suggest_button(world, actions, root);
        return;
    }
    let tools = [
        (DockAction::Tool(DockTool::Select), "Select"),
        (DockAction::Tool(DockTool::Paint), "Paint"),
    ];
    let tool = DockAction::Tool(view.tool);
    spawn_segments(
        world,
        header,
        root,
        &tools,
        tool,
        Some(TilesetPaintModeOp::ID),
    );
    let overlay = spawn_row(world, header, tokens::SPACING_SM);
    spawn_text(world, overlay, "Bits", tokens::TEXT_SECONDARY);
    let bits = [
        (DockAction::Bits(BitsOverlay::Off), "Off"),
        (DockAction::Bits(BitsOverlay::Set), "Set"),
        (DockAction::Bits(BitsOverlay::All), "All"),
    ];
    let current = DockAction::Bits(view.bits);
    spawn_segments(
        world,
        overlay,
        root,
        &bits,
        current,
        Some(TilesetBitsOverlayOp::ID),
    );
    let actions = spawn_row(world, header, tokens::SPACING_MD);
    if let Some(mut node) = world.get_mut::<Node>(actions) {
        node.flex_grow = 1.0;
        node.justify_content = JustifyContent::FlexEnd;
    }
    let count = spawn_text(world, actions, painted_count(setup), tokens::TEXT_SECONDARY);
    world.entity_mut(count).insert(BitsCount { root });
    spawn_suggest_button(world, actions, root);
    let variant = if view.selected.is_some() {
        ButtonVariant::Default
    } else {
        ButtonVariant::Disabled
    };
    world.spawn((
        button(
            ButtonProps::new("New terrain")
                .with_left_icon(Icon::Plus)
                .with_variant(variant),
        ),
        DockControl {
            root,
            action: DockAction::NewTerrain,
        },
        OperatorTooltip(TilesetTerrainFromTileOp::ID),
        ChildOf(actions),
    ));
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

fn spawn_terrains(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    view: &DockView,
) {
    spawn_heading(world, parent, "Terrains");
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
        let label = spawn_text(world, row, terrain.name.clone(), tokens::TEXT_PRIMARY);
        world.entity_mut(label).insert(Node {
            flex_grow: 1.0,
            ..default()
        });
        if terrain.background {
            spawn_text(world, row, "background", tokens::TEXT_SECONDARY);
        }
        if terrain.id == 0 {
            continue;
        }
        let index = match terrain.collision {
            TerrainCollision::Solid => 0,
            TerrainCollision::Trigger => 1,
            TerrainCollision::Passable => 2,
        };
        let names = COLLISIONS.map(|(_, name)| name).to_vec();
        world.spawn((
            combobox_compact_with_selected(names, index),
            CollisionCombo {
                root,
                terrain: terrain.id,
            },
            ChildOf(row),
        ));
    }
    if tileset.terrains.is_empty() {
        let hint = "Pick a plain tile, then New terrain.";
        spawn_text(world, parent, hint, tokens::TEXT_SECONDARY);
    }
}

/// Each terrain set as a line that makes it active, and its remove button.
fn spawn_sets(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    setup: &TilesetSetup,
    view: &DockView,
) {
    spawn_heading(world, parent, "Terrain sets");
    for set in &tileset.terrain_sets {
        let (inner, outer) = (set.inner, set.outer);
        let Some((line, _)) = coverage_line(tileset, setup, (inner, outer)) else {
            continue;
        };
        let holder = world
            .spawn((
                Node {
                    align_items: AlignItems::Center,
                    ..default()
                },
                ChildOf(parent),
            ))
            .id();
        let active = view.active == Some((inner, outer));
        let row = spawn_pick(
            world,
            holder,
            root,
            DockAction::ActiveSet(inner, outer),
            active,
        );
        let mode = (setup.set(inner, outer)).map_or("", |s| mode_word(s.mode.value));
        world
            .entity_mut(row)
            .insert((Tooltip::title(mode), Hovered::default()));
        if let Some(mut node) = world.get_mut::<Node>(row) {
            node.flex_grow = 1.0;
            node.flex_shrink = 1.0;
            node.min_width = px(0.0);
            node.overflow = Overflow::clip_x();
        }
        let label = spawn_text(world, row, line, tokens::TEXT_PRIMARY);
        world
            .entity_mut(label)
            .insert(CoverageLine { root, inner, outer });
        let remove = DockAction::RemoveSet(inner, outer);
        spawn_icon_button(world, holder, root, Icon::X, "Remove terrain set", remove);
    }
}

/// The picked tile, the brush terrain and the active set's missing shapes.
fn spawn_detail(
    world: &mut World,
    parent: Entity,
    root: Entity,
    (tileset, setup): (&Tileset, &TilesetSetup),
    view: &DockView,
    active: Option<((TerrainId, TerrainId), Coverage)>,
) {
    if let Some((sheet, tile)) = view.selected {
        spawn_tile_editor(world, parent, root, (tileset, setup), view, (sheet, tile));
    }
    if view.naming == Some(NameField::New) {
        let row = spawn_row(world, parent, tokens::SPACING_SM);
        spawn_text(world, row, "New terrain", tokens::TEXT_SECONDARY);
        spawn_name_field(world, row, root, NameField::New, suggested_name(tileset));
    }
    if let Some(brush) = (tileset.terrains.iter()).find(|t| t.id == view.brush) {
        let row = spawn_row(world, parent, tokens::SPACING_SM);
        spawn_terrain_swatch(world, row, root, tileset, brush);
        let field = NameField::Rename(brush.id);
        if view.naming == Some(field) {
            spawn_name_field(world, row, root, field, brush.name.clone());
        } else {
            let label = spawn_text(world, row, brush.name.clone(), tokens::TEXT_PRIMARY);
            world.entity_mut(label).insert(Node {
                flex_grow: 1.0,
                ..default()
            });
        }
        let rename = DockAction::RenameTerrain(brush.id);
        spawn_icon_button(world, row, root, Icon::Pencil, "Rename terrain", rename);
        let icon = DockAction::SetIcon(brush.id);
        let button = spawn_icon_button(world, row, root, Icon::Image, "Use tile as icon", icon);
        if view.selected.is_none() {
            world.entity_mut(button).insert(InteractionDisabled);
        }
        let remove = DockAction::RemoveTerrain(brush.id);
        spawn_icon_button(world, row, root, Icon::Trash2, "Remove terrain", remove);
    }
    let Some(((inner, outer), coverage)) = active else {
        return;
    };
    let row = spawn_row(world, parent, tokens::SPACING_SM);
    let label = spawn_text(world, row, summary_line(&coverage), tokens::TEXT_SECONDARY);
    world.entity_mut(label).insert((
        Node {
            flex_grow: 1.0,
            ..default()
        },
        CoverageSummary { root, inner, outer },
    ));
    if coverage.missing.is_empty() {
        return;
    }
    let open = view.expanded.contains(&(inner, outer));
    let toggle = if open { "Hide" } else { "Show" };
    let action = DockAction::ToggleMissing(inner, outer);
    spawn_button(world, row, root, toggle, action);
    if !open {
        return;
    }
    let glyphs = spawn_row(world, parent, tokens::SPACING_XS);
    let shapes = MissingShapes { root, inner, outer };
    world.entity_mut(glyphs).insert(shapes);
    if let Some(mut node) = world.get_mut::<Node>(glyphs) {
        node.flex_wrap = FlexWrap::Wrap;
        node.row_gap = px(tokens::SPACING_XS);
    }
    spawn_missing(world, glyphs, shapes, tileset, &coverage);
}

/// Inner and outer terrain and mode pickers, and Add, for a new set.
fn spawn_new_set(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    view: &DockView,
) {
    if tileset.terrains.len() < 2 {
        return;
    }
    let names: Vec<String> = tileset.terrains.iter().map(|t| t.name.clone()).collect();
    let outers: Vec<String> = std::iter::once("empty".to_string())
        .chain(names.clone())
        .collect();
    let picks = new_set_picks(tileset, view);
    let mut row = spawn_row(world, parent, tokens::SPACING_SM);
    if let Some(mut node) = world.get_mut::<Node>(row) {
        node.margin.top = px(tokens::SPACING_MD);
    }
    let modes = MODES.map(|(_, word)| word.to_string()).to_vec();
    for (field, options) in [names, outers, modes].into_iter().enumerate() {
        if field == 2 {
            row = spawn_row(world, parent, tokens::SPACING_SM);
        }
        let selected = picks[field].min(options.len() - 1);
        world.spawn((
            combobox_compact_with_selected(options, selected),
            NewSetPick { root, field },
            ChildOf(row),
        ));
        if field == 0 {
            spawn_text(world, row, "over", tokens::TEXT_SECONDARY);
        }
    }
    world.spawn((
        button(ButtonProps::new("Add set")),
        DockControl {
            root,
            action: DockAction::CreateSet,
        },
        ChildOf(row),
    ));
}

/// The set a stroke paints in: `current` while it exists, else the first set
/// holding `brush`, else the first set.
fn brush_set(
    tileset: &Tileset,
    brush: TerrainId,
    current: Option<(TerrainId, TerrainId)>,
) -> Option<(TerrainId, TerrainId)> {
    let mut pairs = (tileset.terrain_sets.iter()).map(|s| (s.inner, s.outer));
    let holds = |p: &(TerrainId, TerrainId)| brush != 0 && (p.0 == brush || p.1 == brush);
    (current.filter(|c| pairs.clone().any(|p| p == *c)))
        .or_else(|| pairs.clone().find(holds))
        .or_else(|| pairs.next())
}

fn new_set_picks(tileset: &Tileset, view: &DockView) -> [usize; 3] {
    (view.new_set).unwrap_or([tileset.terrains.len().saturating_sub(1), 1, 2])
}

/// The inner and outer terrain and the index in [`MODES`] the new set
/// control of `view` picks: by default the last terrain over the first.
fn new_set(tileset: &Tileset, view: &DockView) -> (TerrainId, TerrainId, usize) {
    let terrains = &tileset.terrains;
    let [inner, outer, mode] = new_set_picks(tileset, view);
    let id = |i: usize| terrains.get(i).map_or(0, |t| t.id);
    let outer = outer.checked_sub(1).map_or(0, id);
    (id(inner), outer, mode.min(MODES.len() - 1))
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
        .with_default_value(name)
        .select_all_on_open()
        .auto_focus();
    let holder = world
        .spawn((
            Node {
                flex_grow: 1.0,
                min_width: px(0.0),
                ..default()
            },
            TerrainNameField { root, field },
            ChildOf(parent),
        ))
        .id();
    world.spawn((text_edit(props), ChildOf(holder)));
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
            || (view.page == DockPage::Objects
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
            ChildOf(frame),
        ));
    }
    if view.page == DockPage::Sheet {
        spawn_marks(world, frame, root, tileset, setup, index, view);
        spawn_draft(world, frame, root, tileset, index, view);
    }
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
    let brush = (tileset.terrains.iter())
        .find(|t| t.id == view.brush)
        .map_or("empty", |t| t.name.as_str());
    let hint = format!("Click a zone to paint {brush}");
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

/// A pick of the new set control, kept in the view.
fn on_new_set_pick(
    change: On<ComboBoxChangeEvent>,
    picks: Query<&NewSetPick>,
    mut commands: Commands,
) {
    let Ok(pick) = picks.get(change.entity).copied() else {
        return;
    };
    let selected = change.selected;
    commands.queue(move |world: &mut World| {
        let Some((path, tileset)) = dock_tileset(world, pick.root) else {
            return;
        };
        let mut views = world.resource_mut::<TilesetDockViews>();
        if let Some(view) = views.0.get_mut(&path) {
            let mut picks = new_set_picks(&tileset, view);
            picks[pick.field] = selected;
            view.new_set = Some(picks);
        }
    });
}

fn on_collision_pick(
    change: On<ComboBoxChangeEvent>,
    combos: Query<&CollisionCombo>,
    mut commands: Commands,
) {
    let Ok(combo) = combos.get(change.entity).copied() else {
        return;
    };
    let Some((mode, _)) = COLLISIONS.get(change.selected).copied() else {
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
pub(crate) fn dock_tileset(world: &mut World, root: Entity) -> Option<(String, Tileset)> {
    let path = world.get::<TilesetDock>(root)?.path.clone()?;
    let (_, tileset) = load(world, &path).ok()?;
    Some((path, tileset?))
}

/// An operator and its parameters, for the tileset of a dock.
type DockCall = (&'static str, Vec<(&'static str, String)>);

fn act(world: &mut World, control: DockControl) {
    if crate::tileset_layout::act(world, control) || crate::tileset_suggest::act(world, control) {
        return;
    }
    let root = control.root;
    if let Some((field, name)) = typed_name(world, root) {
        name_terrain(world, field, Some(name));
    }
    let Some((path, tileset)) = dock_tileset(world, root) else {
        return;
    };
    let mut view = (world.resource::<TilesetDockViews>().0.get(&path).cloned()).unwrap_or_default();
    let painting = view.tool == DockTool::Paint && view.page == DockPage::Sheet;
    if painting && matches!(control.action, DockAction::Tile(..)) {
        take_keyboard(world, root);
        return;
    }
    view.naming = None;
    let tile_ref = view.selected.map(|(s, t)| format!("{s}:{t}"));
    let call = match control.action {
        DockAction::Page(page) => {
            view.set_page(page);
            None
        }
        DockAction::Brush(brush) => {
            view.brush = brush;
            if view
                .active
                .is_some_and(|(i, o)| ![0, i, o].contains(&brush))
            {
                view.active = None;
            }
            None
        }
        DockAction::Bits(bits) => {
            view.bits = bits;
            None
        }
        DockAction::Tool(tool) => {
            view.set_tool(tool);
            None
        }
        DockAction::CreateSet => {
            let (inner, outer, mode) = new_set(&tileset, &view);
            view.active = Some((inner, outer));
            let params = vec![
                ("inner", inner.to_string()),
                ("outer", outer.to_string()),
                ("mode", MODES[mode].0.to_string()),
            ];
            Some(("tileset.terrain_set.add", params))
        }
        DockAction::ActiveSet(inner, outer) => {
            view.active = Some((inner, outer));
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
        DockAction::NewTerrain
        | DockAction::LayoutPick(..)
        | DockAction::LayoutApply
        | DockAction::LayoutCancel
        | DockAction::LayoutCheck(_)
        | DockAction::LayoutSlot(_)
        | DockAction::Suggest
        | DockAction::SuggestionsClose
        | DockAction::SuggestionShow(_)
        | DockAction::SuggestionDismiss(_)
        | DockAction::SuggestionLayOut(_)
        | DockAction::SuggestionAddObject(_)
        | DockAction::SuggestionObjects => None,
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
        DockAction::RemoveSet(inner, outer) => {
            let params = vec![("inner", inner.to_string()), ("outer", outer.to_string())];
            Some(("tileset.terrain_set.remove", params))
        }
    };
    (world.resource_mut::<TilesetDockViews>().0).insert(path.clone(), view);
    take_keyboard(world, root);
    if control.action == DockAction::NewTerrain {
        call_operator(world, &path, TilesetTerrainFromTileOp::ID, Vec::new());
        return;
    }
    dispatch(world, root, &path, &tileset, call);
    if let DockAction::RemoveSet(inner, outer) = control.action {
        forget_removed_set(world, root, &path, (inner, outer));
    }
}

/// Give the dock `root` the keyboard, so its keys apply to it.
pub(crate) fn take_keyboard(world: &mut World, root: Entity) {
    if let Some(mut focus) = world.get_resource_mut::<InputFocus>() {
        focus.set(root, FocusCause::Pressed);
    }
}

/// Drop the view state of the set `pair` once it is gone from the tileset.
fn forget_removed_set(world: &mut World, root: Entity, path: &str, pair: (TerrainId, TerrainId)) {
    let gone = dock_tileset(world, root).is_some_and(|(_, tileset)| {
        !(tileset.terrain_sets.iter()).any(|s| (s.inner, s.outer) == pair)
    });
    let mut views = world.resource_mut::<TilesetDockViews>();
    if let Some(view) = views.0.get_mut(path).filter(|_| gone) {
        if view.active == Some(pair) {
            view.active = None;
        }
        view.expanded.remove(&pair);
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
    view.page = DockPage::Sheet;
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
    let field = *field;
    let cancelled = keys.just_pressed(KeyCode::Escape);
    let name = Some(commit.text.trim().to_string()).filter(|n| !cancelled && !n.is_empty());
    commands.queue(move |world: &mut World| name_terrain(world, field, name));
}

/// The open name field of the dock `root` and the name typed in it, when
/// there is one.
fn typed_name(world: &mut World, root: Entity) -> Option<(TerrainNameField, String)> {
    let (holder, field) = (world.query::<(Entity, &TerrainNameField)>())
        .iter(world)
        .find(|(_, f)| f.root == root)
        .map(|(e, f)| (e, *f))?;
    let mut stack = vec![holder];
    while let Some(entity) = stack.pop() {
        if let Some(text) = world.get::<bevy::text::EditableText>(entity) {
            let name = text.value().trim().to_string();
            return (!name.is_empty()).then_some((field, name));
        }
        stack.extend(world.get::<Children>(entity).into_iter().flatten());
    }
    None
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
        NameField::Rename(terrain) if tileset.terrain_name(terrain) != name => {
            let params = vec![("terrain", terrain.to_string()), ("name", name)];
            Some(("tileset.terrain.rename", params))
        }
        NameField::Rename(_) => None,
    });
    dispatch(world, field.root, &path, &tileset, call);
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
