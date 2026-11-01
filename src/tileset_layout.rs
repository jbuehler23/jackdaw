use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::picking::events::{PointerDragEnd, PointerMove, PointerPress, PointerRelease};
use bevy::picking::hover::Hovered;
use bevy::picking::pointer::{PointerButton, PointerId, PointerPressState};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui_widgets::Button;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_feathers::button::{ButtonProps, ButtonVariant, button};
use jackdaw_feathers::combobox::{ComboBoxChangeEvent, combobox_compact_with_selected};
use jackdaw_feathers::popover::{PopoverDismissed, PopoverPlacement, PopoverProps, popover};
use jackdaw_feathers::tokens;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_tilemap::{
    BlockEvidence, BlockFit, LayoutTemplate, PaintPreview, Peer, Placement, RgbaImage, SetupError,
    SheetInput, TerrainId, TerrainLook, TileAddress,
};
use jackdaw_tileset::Tileset;

use crate::operator_tooltip::OperatorTooltip;
use crate::tileset_bits::{BitMark, spawn_mark};
use crate::tileset_dock::{
    DockAction, DockControl, DockPage, DockTool, DockView, TilesetDock, TilesetDockViews,
    dock_tileset, focused_dock, in_rect, rect_cells, refill, sheet_image, spawn_text, swatch,
    take_keyboard, tile_image, tile_outline, zoom,
};
use crate::tileset_edit::apply_edit;
use crate::tileset_layout_ops::{PLACEMENTS, TEMPLATES, order_text, template_edit};

/// Each placement's heading, its tooltip and its layouts in the order shown.
const GROUPS: [(Placement, &str, &str, &[LayoutTemplate]); 2] = [
    (
        Placement::BetweenCells,
        "Ground, tiles between cells",
        "A corner set on a dual grid: each tile sits between four cells, so single cells \
         and thin paths paint.",
        &[
            LayoutTemplate::Corner16,
            LayoutTemplate::Minimal3x3Inner,
            LayoutTemplate::Minimal3x3,
            LayoutTemplate::Corner2x2,
        ],
    ),
    (
        Placement::OnCells,
        "Walls and paths, tiles on cells",
        "One tile on each cell, matched by its sides and corners (blob, edge set, 3x3 \
         minimal).",
        &[
            LayoutTemplate::Blob47,
            LayoutTemplate::Side16,
            LayoutTemplate::Minimal3x3Inner,
            LayoutTemplate::Minimal3x3,
            LayoutTemplate::Corner2x2,
        ],
    ),
];
/// Layouts a block of their size reads as without asking.
const PRESELECT: [LayoutTemplate; 2] = [LayoutTemplate::Corner16, LayoutTemplate::Blob47];
/// Layouts of one placement a block of their exact shape reads as when it fits best.
const BY_FIT: [(LayoutTemplate, Placement); 3] = [
    (LayoutTemplate::Corner16, Placement::BetweenCells),
    (LayoutTemplate::Side16, Placement::OnCells),
    (LayoutTemplate::Blob47, Placement::OnCells),
];
/// On-screen size of a tile in the unsure list and the slot grid.
const SLOT: f32 = 20.0;
const POPOVER_WIDTH: f32 = 360.0;
/// On-screen size a layout thumbnail fits in.
const THUMB: f32 = 56.0;
/// On-screen size of a cell in a Can paint preview.
const PREVIEW_CELL: f32 = 10.0;
const THUMB_INNER: [u8; 4] = [219, 184, 115, 255];
const THUMB_OUTER: [u8; 4] = [92, 140, 77, 255];
/// Terrain ids the Can paint previews resolve with; only their tiles show.
const PREVIEW_PAIR: (TerrainId, TerrainId) = (2, 1);

pub(crate) fn plugin(app: &mut App) {
    let release = |button: PointerButton, commands: &mut Commands| {
        if button == PointerButton::Primary {
            commands.queue(finish_drag);
        }
    };
    app.init_resource::<BlockDrag>()
        .init_resource::<Thumbnails>()
        .add_observer(on_press)
        .add_observer(on_move)
        .add_observer(move |end: On<PointerRelease>, mut commands: Commands| {
            release(end.button, &mut commands);
        })
        .add_observer(move |end: On<PointerDragEnd>, mut commands: Commands| {
            release(end.button, &mut commands);
        })
        .add_observer(on_terrain_pick)
        .add_observer(on_dismiss);
}

/// A dragged block of sheet tiles and the layout it is read as.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutDraft {
    pub sheet: u32,
    /// Column and row of the block's top-left tile.
    pub origin: UVec2,
    pub size: UVec2,
    pub pick: Option<(LayoutTemplate, Placement)>,
    pub inner: TerrainId,
    pub outer: TerrainId,
    /// Why the pick cannot be applied as it stands.
    pub blocked: Option<String>,
    /// Why the last apply failed.
    pub error: Option<String>,
    /// The block cell, row by row, each template tile takes when the block
    /// is not in the template's own order.
    pub order: Option<Vec<Option<u32>>>,
    /// Block cells whose template tile the fit is unsure of.
    pub unsure: Vec<u32>,
    /// The unsure cell being placed by hand.
    pub checking: Option<u32>,
    /// Whether the layout was picked by hand, so terrain changes keep it.
    pub by_hand: bool,
    /// Template tiles and the cells put in them by hand, kept across refits
    /// of the same layout.
    pub placed: Vec<(usize, u32)>,
}

impl LayoutDraft {
    /// A draft of the block with the brush as its inner terrain when it has
    /// one, and the layout its size or the pair's set names.
    fn new(tileset: &Tileset, brush: TerrainId, sheet: u32, origin: UVec2, size: UVec2) -> Self {
        let ids: Vec<TerrainId> = tileset.terrains.iter().map(|t| t.id).collect();
        let inner = Some(brush)
            .filter(|b| *b != 0 && ids.contains(b))
            .or_else(|| ids.first().copied())
            .unwrap_or(0);
        let outer = ids.iter().copied().find(|id| *id != inner).unwrap_or(0);
        let mut draft = Self {
            sheet,
            origin,
            size,
            inner,
            outer,
            ..default()
        };
        draft.preselect(tileset);
        draft
    }

    /// Pick the placement the pair's set already draws, else the layout
    /// whose shape the block has, else one it fits; ambiguous blocks stay
    /// unpicked.
    fn preselect(&mut self, tileset: &Tileset) {
        let setup = tileset.setup().ok();
        let had = (setup.as_ref()).and_then(|s| Some(s.set(self.inner, self.outer)?.mode.value));
        let size = self.size;
        let existing = had.and_then(|mode| {
            (GROUPS.iter())
                .flat_map(|(p, _, _, order)| order.iter().map(move |t| (*t, *p)))
                .find(|(t, p)| t.fits(size) && t.placements().contains(p) && t.mode_at(*p) == mode)
        });
        let by_size = || {
            (PRESELECT.iter().find(|t| t.shape() == size))
                .or_else(|| PRESELECT.iter().find(|t| t.fits(size)))
                .map(|t| (*t, t.placements()[0]))
        };
        if !self.by_hand {
            self.pick = existing.or(self.pick).or_else(by_size);
        }
        self.error = None;
        self.blocked = self.conflict(tileset);
    }

    /// The refusal the kernel gives when the pair's set draws another
    /// placement with tiles outside the block.
    fn conflict(&self, tileset: &Tileset) -> Option<String> {
        let (template, placement) = self.pick?;
        let setup = tileset.setup().ok()?;
        let had = setup.set(self.inner, self.outer)?.mode.value;
        let index = tileset.setup_sheet(self.sheet).ok()?;
        let columns = tileset.sheets.get(self.sheet as usize)?.columns;
        let block: Vec<TileAddress> = (0..self.size.y)
            .flat_map(|y| (0..self.size.x).map(move |x| UVec2::new(x, y)))
            .map(|c| (self.origin + c).y * columns + (self.origin + c).x)
            .map(|tile| TileAddress { sheet: index, tile })
            .collect();
        match setup.drawn_elsewhere((self.inner, self.outer), &block) {
            0 => None,
            _ if had == template.mode_at(placement) => None,
            n => Some(SetupError::SetDrawn(had, n).to_string()),
        }
    }

    pub fn holds(&self, sheet: u32, tile: u32, columns: u32) -> bool {
        let cell = UVec2::new(tile % columns.max(1), tile / columns.max(1));
        sheet == self.sheet
            && cell.cmpge(self.origin).all()
            && cell.cmplt(self.origin + self.size).all()
    }

    pub fn ready(&self) -> bool {
        self.pick.is_some() && self.inner != 0 && self.inner != self.outer && self.blocked.is_none()
    }

    /// The sheet tile in each slot of the picked layout.
    fn slot_tiles(&self, columns: u32) -> Vec<Option<u32>> {
        let Some((template, _)) = self.pick else {
            return Vec::new();
        };
        let order = (self.order.clone()).unwrap_or_else(|| template.canonical_order(self.size));
        (order.into_iter())
            .map(|cell| cell.map(|c| self.cell_tile(c, columns)))
            .collect()
    }

    /// The sheet tile of block cell `cell`, counted row by row.
    fn cell_tile(&self, cell: u32, columns: u32) -> u32 {
        let at = self.origin + UVec2::new(cell % self.size.x, cell / self.size.x);
        at.y * columns + at.x
    }

    /// Put the cell being checked in template tile `slot`, swapping it with
    /// the cell there, which becomes unsure; a stale cell or slot changes
    /// nothing.
    fn place(&mut self, slot: usize) {
        let (Some(cell), Some((template, _))) = (self.checking.take(), self.pick) else {
            return;
        };
        let mut order = (self.order.clone()).unwrap_or_else(|| template.canonical_order(self.size));
        let from = order.iter().position(|c| *c == Some(cell));
        let Some(from) = from.filter(|_| slot < order.len()) else {
            return;
        };
        let displaced = order[slot].filter(|d| *d != cell);
        order.swap(from, slot);
        self.order = Some(order);
        self.unsure.retain(|c| *c != cell);
        self.unsure
            .extend(displaced.filter(|d| !self.unsure.contains(d)));
        self.placed
            .retain(|(s, c)| *s != slot && *c != cell && Some(*c) != displaced);
        self.placed.push((slot, cell));
    }

    /// The `tileset.apply_template` parameters of a ready draft.
    fn params(&self) -> Option<Vec<(&'static str, String)>> {
        let (template, placement) = self.pick.filter(|_| self.ready())?;
        let name = |found: Option<&'static str>| found.unwrap_or_default().to_string();
        let order = self.order.as_deref().map(|o| ("order", order_text(o)));
        let params = vec![
            ("inner", self.inner.to_string()),
            ("outer", self.outer.to_string()),
            (
                "template",
                name(TEMPLATES.iter().find(|e| e.0 == template).map(|e| e.1)),
            ),
            (
                "placement",
                name(PLACEMENTS.iter().find(|e| e.0 == placement).map(|e| e.1)),
            ),
            ("origin", format!("{},{}", self.origin.x, self.origin.y)),
            ("size", format!("{},{}", self.size.x, self.size.y)),
            ("sheet", self.sheet.to_string()),
        ];
        Some(params.into_iter().chain(order).collect())
    }

    /// Fit the pick to the block's pixels as `fitting` asks; choosing, with
    /// no set of the pair yet, it first picks the best fitting layout of the
    /// block's exact shape when it fits well. Tiles placed by hand keep their
    /// place while the layout stays. Without icon tiles the template's own
    /// order stays.
    fn refit(&mut self, world: &mut World, tileset: &Tileset, fitting: Fitting) {
        if fitting == Fitting::Keep {
            return;
        }
        let (before, choose) = (self.pick, fitting == Fitting::Choose);
        self.checking = None;
        let evidence = block_evidence(world, tileset, self);
        let free = (tileset.setup().ok()).is_none_or(|s| s.set(self.inner, self.outer).is_none());
        let shaped = BY_FIT.into_iter().filter(|(t, _)| t.shape() == self.size);
        let others = shaped.filter(|l| choose && free && self.pick != Some(*l));
        let mut fits: Vec<((LayoutTemplate, Placement), BlockFit)> = (self.pick.into_iter())
            .chain(others)
            .filter_map(|layout| Some((layout, evidence.as_ref()?.fit(layout)?)))
            .collect();
        let mean = |f: &BlockFit| f.confidence.iter().sum::<f32>() / f.confidence.len() as f32;
        if choose && free {
            fits.sort_by(|a, b| mean(&b.1).total_cmp(&mean(&a.1)));
            fits.retain(|(l, f)| f.good() || self.pick == Some(*l));
        }
        let fit = fits.into_iter().next();
        if let Some((layout, _)) = &fit {
            self.pick = Some(*layout);
        }
        if self.pick != before {
            self.placed.clear();
        }
        let Some((template, _)) = self.pick else {
            (self.order, self.unsure) = (None, Vec::new());
            return;
        };
        let canonical = template.canonical_order(self.size);
        let (mut order, mut unsure) = match fit {
            Some((_, fit)) => (fit.order, fit.unsure),
            None => (canonical.clone(), Vec::new()),
        };
        for (slot, cell) in &self.placed {
            let from = order.iter().position(|c| *c == Some(*cell));
            if let Some(from) = from.filter(|_| *slot < order.len()) {
                order.swap(from, *slot);
            }
            unsure.retain(|c| c != cell);
        }
        self.order = (order != canonical).then_some(order);
        self.unsure = unsure;
        self.blocked = self.conflict(tileset);
    }
}

/// The block of `draft` measured against the terrains' icon tiles.
fn block_evidence(
    world: &mut World,
    tileset: &Tileset,
    draft: &LayoutDraft,
) -> Option<BlockEvidence> {
    let icon = |id: TerrainId| (tileset.terrains.iter()).find(|t| t.id == id)?.icon;
    let inner = icon(draft.inner)?;
    let outer = match draft.outer {
        0 => None,
        id => Some(icon(id)?),
    };
    let mut handles = HashMap::new();
    for sheet in [draft.sheet, inner.0].into_iter().chain(outer.map(|o| o.0)) {
        let source = &tileset.sheets.get(sheet as usize)?.source;
        handles.insert(sheet, sheet_image(world, source)?);
    }
    let images = world.resource::<Assets<Image>>();
    let input = |sheet: u32| {
        let image = images.get(handles.get(&sheet)?)?;
        let pixels = image.data.as_deref()?;
        Some(SheetInput {
            name: "",
            image: RgbaImage::new(image.width(), image.height(), pixels)?,
            layout: tileset.sheet_layout(sheet)?,
        })
    };
    let look = |(sheet, tile): (u32, u32)| TerrainLook::of_tile(&input(sheet)?, tile);
    let outer = match outer {
        Some(icon) => look(icon)?,
        None => TerrainLook::empty(tileset.tile_size),
    };
    let block = (draft.origin, draft.size);
    BlockEvidence::measure(&input(draft.sheet)?, block, (&look(inner)?, &outer))
}

/// The layout popover of the dock `root`.
#[derive(Component, Clone, Copy, Debug)]
pub struct LayoutPopover {
    pub root: Entity,
}

/// The outline of the dragged block the popover points at.
#[derive(Component, Clone, Copy, Debug)]
pub struct LayoutAnchor {
    pub root: Entity,
}

/// A bit the picked layout would give peer `peer` of sheet tile `tile`.
#[derive(Component, Clone, Copy, Debug)]
pub struct GhostMark {
    pub root: Entity,
    pub tile: u32,
    pub peer: usize,
    pub terrain: TerrainId,
}

/// The outline of a block tile whose place the fit is unsure of.
#[derive(Component, Clone, Copy, Debug)]
pub struct UnsureMark {
    pub root: Entity,
    pub tile: u32,
}

/// The inner or outer terrain picker of the layout popover.
#[derive(Component, Clone, Copy, Debug)]
pub struct LayoutTerrainPick {
    pub root: Entity,
    pub outer: bool,
}

/// A small map the picked layout paints with the block's tiles.
#[derive(Component, Clone, Copy, Debug)]
pub struct CanPaintPreview {
    pub root: Entity,
    pub name: &'static str,
}

/// The sheet tiles a primary press is dragging across, as dock, sheet,
/// first tile and current tile.
#[derive(Resource, Default)]
struct BlockDrag(Option<(Entity, u32, u32, u32)>);

#[derive(Resource, Default)]
struct Thumbnails(HashMap<(LayoutTemplate, Placement), Handle<Image>>);

/// Apply the layout picked in the layout popover.
#[operator(
    id = "tileset.layout.apply",
    label = "Apply Layout",
    description = "Apply the layout picked in the layout popover of the Tileset tab with the \
                   keyboard to its block, as tileset.apply_template does. The popover stays \
                   open with the reason when it cannot.",
    allows_undo = false,
    is_available = a_layout_is_picked
)]
pub fn tileset_layout_apply(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let applied = focused_dock(world)
            .ok_or_else(|| "no Tileset tab has the keyboard".to_string())
            .and_then(|root| apply(world, root));
        match applied {
            Ok(message) => report_to_caller(world, message),
            Err(err) => warn_caller(world, format!("tileset.layout.apply: {err}")),
        }
    });
    OperatorResult::Finished
}

/// A ready draft in the dock with the keyboard, unless a text field there
/// has it.
fn a_layout_is_picked(world: &World) -> bool {
    let typing = (world.get_resource::<bevy::input_focus::InputFocus>())
        .and_then(bevy::input_focus::InputFocus::get)
        .is_some_and(|e| world.get::<bevy::text::EditableText>(e).is_some());
    let path = focused_dock(world).and_then(|root| world.get::<TilesetDock>(root)?.path.clone());
    let views = world.resource::<TilesetDockViews>();
    let ready = path.and_then(|p| views.0.get(&p)?.layout.as_ref().map(LayoutDraft::ready));
    !typing && ready == Some(true)
}

/// Handle a popover control of the dock; false for any other control.
pub(crate) fn act(world: &mut World, control: DockControl) -> bool {
    let root = control.root;
    match control.action {
        DockAction::LayoutPick(template, placement) => {
            take_keyboard(world, root);
            edit_draft(world, root, |d| {
                if d.pick == Some((template, placement)) {
                    return Fitting::Keep;
                }
                (d.pick, d.by_hand) = (Some((template, placement)), true);
                Fitting::Refit
            });
        }
        DockAction::LayoutCheck(cell) => {
            take_keyboard(world, root);
            edit_draft(world, root, |d| {
                d.checking = (d.checking != Some(cell)).then_some(cell);
                Fitting::Keep
            });
        }
        DockAction::LayoutSlot(slot) => {
            take_keyboard(world, root);
            edit_draft(world, root, |d| {
                d.place(slot);
                Fitting::Keep
            });
        }
        DockAction::LayoutApply => {
            take_keyboard(world, root);
            if let Err(err) = world.operator(TilesetLayoutApplyOp::ID).call() {
                warn!("{}: {err}", TilesetLayoutApplyOp::ID);
            }
        }
        DockAction::LayoutCancel => close(world, root),
        _ => return false,
    }
    true
}

/// Apply the draft of `root` as one edit: on success it closes, on failure
/// it stays open with the error.
fn apply(world: &mut World, root: Entity) -> Result<String, String> {
    let (path, _) = dock_tileset(world, root).ok_or("the tileset is not loaded")?;
    let draft = (world.resource::<TilesetDockViews>().0.get(&path))
        .and_then(|v| v.layout.clone())
        .ok_or("no block is being laid out")?;
    let params = draft
        .params()
        .ok_or("pick a layout and two different terrains")?;
    let applied = apply_edit(
        world,
        "tileset.apply_template",
        &path,
        template_edit(&params),
    );
    if let Some(view) = world.resource_mut::<TilesetDockViews>().0.get_mut(&path) {
        match &applied {
            Ok(_) => {
                view.active = Some((draft.inner, draft.outer));
                collapse(view);
            }
            Err(err) => view
                .layout
                .iter_mut()
                .for_each(|d| d.error = Some(err.clone())),
        }
    }
    if let Err(err) = &applied {
        crate::status_bar::notify_warn(world, format!("Layout not applied: {err}"));
    }
    refill(world, &path);
    applied
}

/// Drop the draft and leave one tile selected.
fn collapse(view: &mut DockView) {
    view.layout = None;
    view.anchor = view.selected;
}

fn close(world: &mut World, root: Entity) {
    let Some(path) = world.get::<TilesetDock>(root).and_then(|d| d.path.clone()) else {
        return;
    };
    let mut views = world.resource_mut::<TilesetDockViews>();
    if let Some(view) = views.0.get_mut(&path).filter(|v| v.layout.is_some()) {
        collapse(view);
        refill(world, &path);
    }
}

/// What a draft edit asks of the fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fitting {
    /// Keep the order as it is.
    Keep,
    /// Fit the order of the picked layout again.
    Refit,
    /// Pick the best fitting layout too.
    Choose,
}

/// Edit the draft of `root`, then fit it again as the edit asks.
fn edit_draft(world: &mut World, root: Entity, edit: impl FnOnce(&mut LayoutDraft) -> Fitting) {
    let Some((path, tileset)) = dock_tileset(world, root) else {
        return;
    };
    let views = world.resource::<TilesetDockViews>();
    let Some(mut draft) = views.0.get(&path).and_then(|v| v.layout.clone()) else {
        return;
    };
    let fitting = edit(&mut draft);
    draft.error = None;
    draft.blocked = draft.conflict(&tileset);
    draft.refit(world, &tileset, fitting);
    let mut views = world.resource_mut::<TilesetDockViews>();
    if let Some(view) = views.0.get_mut(&path) {
        view.layout = Some(draft);
    }
    refresh(world, root, &path, &tileset);
}

/// Build the popover and the ghost marks of `root` again in place.
fn refresh(world: &mut World, root: Entity, path: &str, tileset: &Tileset) {
    let Some(draft) =
        (world.resource::<TilesetDockViews>().0.get(path)).and_then(|v| v.layout.clone())
    else {
        return;
    };
    let columns = tileset
        .sheets
        .get(draft.sheet as usize)
        .map_or(1, |s| s.columns);
    let picked = draft.pick.is_some();
    let mut marks = world.query::<(
        Entity,
        Option<&GhostMark>,
        Option<&BitMark>,
        Option<&UnsureMark>,
    )>();
    let stale: Vec<Entity> = (marks.iter(world))
        .filter(|(_, ghost, bit, unsure)| {
            ghost.is_some_and(|g| g.root == root)
                || unsure.is_some_and(|u| u.root == root)
                || bit.is_some_and(|b| {
                    b.root == root && picked && draft.holds(b.sheet, b.tile, columns)
                })
        })
        .map(|(e, ..)| e)
        .collect();
    for entity in stale {
        world.entity_mut(entity).despawn();
    }
    let popover = (world.query::<(Entity, &LayoutPopover)>().iter(world))
        .find(|(_, p)| p.root == root)
        .map(|(e, _)| e);
    if let Some(popover) = popover {
        world.entity_mut(popover).despawn_related::<Children>();
        fill_popover(world, popover, root, tileset, &draft);
    }
    let frame = (world.query::<(&LayoutAnchor, &ChildOf)>().iter(world))
        .find(|(a, _)| a.root == root)
        .map(|(_, parent)| parent.parent());
    if let Some(frame) = frame {
        spawn_ghosts(world, frame, root, tileset, &draft);
    }
}

/// The outline, ghost marks and popover of the block `view` lays out, when
/// it is on sheet `index`.
pub(crate) fn spawn_draft(
    world: &mut World,
    frame: Entity,
    root: Entity,
    tileset: &Tileset,
    index: u32,
    view: &DockView,
) {
    let Some(draft) = view.layout.as_ref().filter(|d| d.sheet == index) else {
        return;
    };
    let Some(columns) = tileset.sheets.get(index as usize).map(|s| s.columns) else {
        return;
    };
    let last = draft.origin + draft.size - UVec2::ONE;
    let first = tileset.sheet_rect(index, draft.origin.y * columns + draft.origin.x);
    let end = tileset.sheet_rect(index, last.y * columns + last.x);
    let Some(((min, _), (max, size))) = first.zip(end) else {
        return;
    };
    let scale = zoom(tileset.tile_size.as_vec2());
    let span = (max + size - min).as_vec2() * scale;
    let anchor = world
        .spawn((
            absolute(min.as_vec2() * scale, span),
            BorderColor::all(tokens::SELECTED_BORDER),
            LayoutAnchor { root },
            Pickable::IGNORE,
            ChildOf(frame),
        ))
        .id();
    if let Some(mut node) = world.get_mut::<Node>(anchor) {
        node.border = UiRect::all(px(2.0));
    }
    spawn_ghosts(world, frame, root, tileset, draft);
    let props = PopoverProps::new(anchor)
        .with_placement(PopoverPlacement::RightStart)
        .with_padding(tokens::SPACING_MD)
        .with_gap(tokens::SPACING_MD)
        .with_node(Node {
            width: px(POPOVER_WIDTH),
            ..default()
        });
    let popover = world.spawn((popover(props), LayoutPopover { root })).id();
    fill_popover(world, popover, root, tileset, draft);
}

/// An absolutely placed node at `at` of `size`, in pixels.
fn absolute(at: Vec2, size: Vec2) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: px(at.x),
        top: px(at.y),
        width: px(size.x),
        height: px(size.y),
        ..default()
    }
}

fn spawn_ghosts(
    world: &mut World,
    frame: Entity,
    root: Entity,
    tileset: &Tileset,
    draft: &LayoutDraft,
) {
    let Some((template, placement)) = draft.pick else {
        return;
    };
    let columns = tileset
        .sheets
        .get(draft.sheet as usize)
        .map_or(1, |s| s.columns);
    let scale = zoom(tileset.tile_size.as_vec2());
    let mode = template.mode_at(placement);
    let shapes = template.placed_tiles(placement, draft.inner, draft.outer);
    for (tile, shape) in draft.slot_tiles(columns).into_iter().zip(shapes) {
        let Some(tile) = tile else {
            continue;
        };
        for peer in Peer::ALL.into_iter().filter(|p| mode.reads(*p)) {
            let terrain = shape.get(peer);
            if terrain == 0 {
                continue;
            }
            let ghost = GhostMark {
                root,
                tile,
                peer: peer.index(),
                terrain,
            };
            let at = (draft.sheet, tile, peer.index());
            let color = swatch(tileset, terrain, 0.45);
            spawn_mark(world, frame, at, ghost, (tileset, scale), color);
        }
    }
    for cell in &draft.unsure {
        let tile = draft.cell_tile(*cell, columns);
        let Some((min, size)) = tileset.sheet_rect(draft.sheet, tile) else {
            continue;
        };
        let mut node = absolute(min.as_vec2() * scale, size.as_vec2() * scale);
        node.border = UiRect::all(px(2.0));
        let outline = unsure_outline(draft.checking == Some(*cell));
        let mark = UnsureMark { root, tile };
        let bundle = (node, BorderColor::all(outline), mark, Pickable::IGNORE);
        world.spawn((bundle, ChildOf(frame)));
    }
}

fn unsure_outline(checking: bool) -> Color {
    if checking {
        tokens::SELECTED_BORDER
    } else {
        tokens::TEXT_WARNING
    }
}

/// The note on unsure tiles, a button per unsure tile, and once one is
/// picked the template's tiles as drawn to put it in.
fn spawn_unsure(
    world: &mut World,
    popover: Entity,
    root: Entity,
    tileset: &Tileset,
    draft: &LayoutDraft,
) {
    let Some((template, _)) = draft.pick.filter(|_| !draft.unsure.is_empty()) else {
        return;
    };
    let note = match draft.unsure.len() {
        1 => "1 tile unsure, check it".to_string(),
        n => format!("{n} tiles unsure, check them"),
    };
    spawn_text(world, popover, note, tokens::TEXT_WARNING);
    let columns = (tileset.sheets.get(draft.sheet as usize)).map_or(1, |s| s.columns);
    let row = spawn_wrap(world, popover);
    for cell in &draft.unsure {
        let tile = Some(draft.cell_tile(*cell, columns));
        let outline = unsure_outline(draft.checking == Some(*cell));
        let button = spawn_tile_button(world, row, (root, DockAction::LayoutCheck(*cell)), outline);
        fill_tile(world, button, tileset, draft.sheet, tile);
        let tip = Tooltip::title("Check this tile").with_description("Pick where it goes below.");
        world.entity_mut(button).insert(tip);
    }
    let Some(checking) = draft.checking else {
        return;
    };
    let words = "Click where it belongs";
    spawn_text(world, popover, words, tokens::TEXT_SECONDARY);
    let shape = template.shape().as_vec2() * SLOT;
    let grid = Node {
        width: px(shape.x),
        height: px(shape.y),
        ..default()
    };
    let grid = world.spawn((grid, ChildOf(popover))).id();
    let tiles = draft.slot_tiles(columns);
    let checked = Some(draft.cell_tile(checking, columns));
    for (slot, at) in template.slots(template.shape()).into_iter().enumerate() {
        let Some(at) = at else {
            continue;
        };
        let tile = tiles.get(slot).copied().flatten();
        let unsure = (draft.unsure.iter()).any(|c| Some(draft.cell_tile(*c, columns)) == tile);
        let outline = if tile == checked || unsure {
            unsure_outline(tile == checked)
        } else {
            tokens::BORDER_SUBTLE
        };
        let button = spawn_tile_button(world, grid, (root, DockAction::LayoutSlot(slot)), outline);
        let mut node = absolute(at.as_vec2() * SLOT, Vec2::splat(SLOT));
        node.border = UiRect::all(px(1.0));
        world.entity_mut(button).insert(node);
        fill_tile(world, button, tileset, draft.sheet, tile);
    }
}

/// A bordered button of `SLOT` size for `control`.
fn spawn_tile_button(
    world: &mut World,
    parent: Entity,
    (root, action): (Entity, DockAction),
    outline: Color,
) -> Entity {
    let node = Node {
        width: px(SLOT),
        height: px(SLOT),
        border: UiRect::all(px(1.0)),
        ..default()
    };
    let control = DockControl { root, action };
    let bundle = (
        node,
        BorderColor::all(outline),
        Button,
        Hovered::default(),
        control,
    );
    world.spawn((bundle, ChildOf(parent))).id()
}

/// The image of `tile` filling `button`.
fn fill_tile(world: &mut World, button: Entity, tileset: &Tileset, sheet: u32, tile: Option<u32>) {
    let Some(image) = tile.and_then(|t| tile_image(world, tileset, sheet, t)) else {
        return;
    };
    let node = Node {
        width: percent(100.0),
        height: percent(100.0),
        ..default()
    };
    world.spawn((node, image, Pickable::IGNORE, ChildOf(button)));
}

fn fill_popover(
    world: &mut World,
    popover: Entity,
    root: Entity,
    tileset: &Tileset,
    draft: &LayoutDraft,
) {
    let title = format!("Layout for {}x{} tiles", draft.size.x, draft.size.y);
    spawn_text(world, popover, title, tokens::TEXT_PRIMARY);
    let mut offered = false;
    for (placement, words, tip, order) in GROUPS {
        let fitting: Vec<LayoutTemplate> = (order.iter().copied())
            .filter(|t| t.fits(draft.size) && t.placements().contains(&placement))
            .collect();
        if fitting.is_empty() {
            continue;
        }
        offered = true;
        let heading = spawn_text(world, popover, words, tokens::TEXT_SECONDARY);
        let tooltip = Tooltip::title(words).with_description(tip);
        (world.entity_mut(heading)).insert((tooltip, Hovered::default(), Pickable::default()));
        let row = spawn_wrap(world, popover);
        for template in fitting {
            spawn_card(world, row, root, (template, placement), draft.pick);
        }
    }
    let notes = [
        (!offered).then_some("No layout holds a block of this size"),
        (offered && draft.pick.is_none()).then_some("Pick how this block is drawn"),
    ];
    for note in notes.into_iter().flatten() {
        spawn_text(world, popover, note, tokens::TEXT_WARNING);
    }
    spawn_terrain_pickers(world, popover, root, tileset, draft);
    if let Some(pick) = draft.pick {
        spawn_text(world, popover, "Can paint", tokens::TEXT_SECONDARY);
        let row = spawn_wrap(world, popover);
        for preview in PaintPreview::ALL {
            spawn_preview(world, row, root, tileset, draft, (pick, preview));
        }
    }
    spawn_unsure(world, popover, root, tileset, draft);
    for problem in [&draft.blocked, &draft.error].into_iter().flatten() {
        spawn_text(world, popover, problem.clone(), tokens::TEXT_ERROR);
    }
    let row = spawn_wrap(world, popover);
    if let Some(mut node) = world.get_mut::<Node>(row) {
        node.justify_content = JustifyContent::FlexEnd;
    }
    let variant = if draft.ready() {
        ButtonVariant::Primary
    } else {
        ButtonVariant::Disabled
    };
    let control = |action| DockControl { root, action };
    world.spawn((
        button(ButtonProps::new("Apply").with_variant(variant)),
        control(DockAction::LayoutApply),
        OperatorTooltip(TilesetLayoutApplyOp::ID),
        ChildOf(row),
    ));
    world.spawn((
        button(ButtonProps::new("Cancel")),
        control(DockAction::LayoutCancel),
        Tooltip::title("Cancel").with_keybind("Escape"),
        ChildOf(row),
    ));
}

fn spawn_wrap(world: &mut World, parent: Entity) -> Entity {
    let node = Node {
        flex_wrap: FlexWrap::Wrap,
        align_items: AlignItems::Center,
        column_gap: px(tokens::SPACING_SM),
        row_gap: px(tokens::SPACING_SM),
        ..default()
    };
    world.spawn((node, ChildOf(parent))).id()
}

/// A column of centred items under `parent`.
fn spawn_column(world: &mut World, parent: Entity) -> Entity {
    let node = Node {
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        row_gap: px(tokens::SPACING_XS),
        ..default()
    };
    world.spawn((node, ChildOf(parent))).id()
}

fn label(template: LayoutTemplate) -> &'static str {
    match template {
        LayoutTemplate::Corner16 => "16 corner",
        LayoutTemplate::Side16 => "16 side",
        LayoutTemplate::Minimal3x3 => "3x3",
        LayoutTemplate::Corner2x2 => "2x2 inner corners",
        LayoutTemplate::Blob47 => "47 blob",
        LayoutTemplate::Minimal3x3Inner => "3x3 + inner corners",
    }
}

fn spawn_card(
    world: &mut World,
    parent: Entity,
    root: Entity,
    layout: (LayoutTemplate, Placement),
    pick: Option<(LayoutTemplate, Placement)>,
) {
    let current = pick == Some(layout);
    let (background, border) = if current {
        (tokens::SELECTED_BG, tokens::SELECTED_BORDER)
    } else {
        (Color::NONE, tokens::BORDER_SUBTLE)
    };
    let card = spawn_column(world, parent);
    let action = DockAction::LayoutPick(layout.0, layout.1);
    world.entity_mut(card).insert((
        BackgroundColor(background),
        BorderColor::all(border),
        Button,
        Hovered::default(),
        DockControl { root, action },
    ));
    if let Some(mut node) = world.get_mut::<Node>(card) {
        node.padding = UiRect::all(px(tokens::SPACING_SM));
        node.border = UiRect::all(px(1.0));
        node.border_radius = BorderRadius::all(px(tokens::BORDER_RADIUS_SM));
    }
    if current {
        world.entity_mut(card).insert(bevy::ui::Checked);
    }
    let (image, size) = thumbnail(world, layout);
    let node = Node {
        width: px(size.x),
        height: px(size.y),
        ..default()
    };
    world.spawn((node, ImageNode::new(image), Pickable::IGNORE, ChildOf(card)));
    spawn_text(world, card, label(layout.0), tokens::TEXT_PRIMARY);
}

/// The canonical arrangement of a layout as a small image, inner light on
/// outer green; between cells only corners show. Returns its drawn size.
fn thumbnail(
    world: &mut World,
    (template, placement): (LayoutTemplate, Placement),
) -> (Handle<Image>, Vec2) {
    let between = placement == Placement::BetweenCells;
    let side = if between { 2 } else { 3 };
    let size = template.shape() * (side + 1) - UVec2::ONE;
    let shown = size.as_vec2() * (THUMB / size.max_element() as f32).floor().max(1.0);
    if let Some(handle) = world.resource::<Thumbnails>().0.get(&(template, placement)) {
        return (handle.clone(), shown);
    }
    let mut pixels = vec![0u8; (size.x * size.y * 4) as usize];
    let mode = template.mode_at(placement);
    let tiles = template.placed_tiles(placement, 2, 1);
    for (slot, terrains) in template.slots(template.shape()).into_iter().zip(tiles) {
        for peer in Peer::ALL.into_iter().filter(|p| mode.reads(*p)) {
            let Some(slot) = slot else {
                continue;
            };
            let at = UVec2::new(peer.index() as u32 % 3, peer.index() as u32 / 3);
            let pixel = slot * (side + 1) + if between { at.min(UVec2::ONE) } else { at };
            let i = ((pixel.y * size.x + pixel.x) * 4) as usize;
            let colour = [THUMB_OUTER, THUMB_INNER][usize::from(terrains.get(peer) == 2)];
            pixels[i..i + 4].copy_from_slice(&colour);
        }
    }
    let extent = Extent3d {
        width: size.x,
        height: size.y,
        depth_or_array_layers: 1,
    };
    let (dimension, format) = (TextureDimension::D2, TextureFormat::Rgba8UnormSrgb);
    let mut image = Image::new(
        extent,
        dimension,
        pixels,
        format,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::nearest();
    let handle = world.resource_mut::<Assets<Image>>().add(image);
    (world.resource_mut::<Thumbnails>().0).insert((template, placement), handle.clone());
    (handle, shown)
}

fn spawn_terrain_pickers(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    draft: &LayoutDraft,
) {
    let names: Vec<String> = tileset.terrains.iter().map(|t| t.name.clone()).collect();
    if names.is_empty() {
        let none = "Add a terrain from a tile to lay out this block";
        spawn_text(world, parent, none, tokens::TEXT_WARNING);
        return;
    }
    let index = |id: TerrainId| tileset.terrains.iter().position(|t| t.id == id);
    let outers = std::iter::once("none".to_string())
        .chain(names.clone())
        .collect();
    let row = spawn_wrap(world, parent);
    let pickers = [
        ("Inner", names, index(draft.inner).unwrap_or(0), false),
        (
            "over",
            outers,
            index(draft.outer).map_or(0, |i| i + 1),
            true,
        ),
    ];
    for (word, options, selected, outer) in pickers {
        spawn_text(world, row, word, tokens::TEXT_SECONDARY);
        world.spawn((
            combobox_compact_with_selected(options, selected),
            LayoutTerrainPick { root, outer },
            ChildOf(row),
        ));
    }
    if draft.inner == draft.outer {
        let same = "Pick two different terrains";
        spawn_text(world, parent, same, tokens::TEXT_WARNING);
    }
}

/// A Can paint preview: the block's tiles the picked layout resolves for
/// `preview`, over the outer terrain's icon tile.
fn spawn_preview(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    draft: &LayoutDraft,
    ((template, placement), preview): ((LayoutTemplate, Placement), PaintPreview),
) {
    let column = spawn_column(world, parent);
    let side = px(PaintPreview::SIZE as f32 * PREVIEW_CELL);
    let map = Node {
        width: side,
        height: side,
        overflow: Overflow::clip(),
        ..default()
    };
    let name = preview.name();
    let map = (world.spawn((map, BackgroundColor(tokens::INPUT_BG), ChildOf(column))))
        .insert(CanPaintPreview { root, name })
        .id();
    let mut images: HashMap<(u32, u32), Option<ImageNode>> = HashMap::new();
    let mut place = |world: &mut World, (sheet, tile): (u32, u32), at: Vec2| {
        let image = (images.entry((sheet, tile)))
            .or_insert_with(|| tile_image(world, tileset, sheet, tile))
            .clone();
        let top = PaintPreview::SIZE as f32 - at.y - 1.0;
        let node = absolute(
            Vec2::new(at.x, top) * PREVIEW_CELL,
            Vec2::splat(PREVIEW_CELL),
        );
        if let Some(image) = image {
            world.spawn((node, image, Pickable::IGNORE, ChildOf(map)));
        }
    };
    let icon = (tileset.terrains.iter())
        .find(|t| t.id == draft.outer)
        .and_then(|t| t.icon);
    if let Some(icon) = icon {
        for i in 0..PaintPreview::SIZE * PaintPreview::SIZE {
            let at = Vec2::new(
                (i % PaintPreview::SIZE) as f32,
                (i / PaintPreview::SIZE) as f32,
            );
            place(world, icon, at);
        }
    }
    let columns = tileset
        .sheets
        .get(draft.sheet as usize)
        .map_or(1, |s| s.columns);
    let slots = draft.slot_tiles(columns);
    let (inner, outer) = PREVIEW_PAIR;
    let set = template.placed_set(placement, inner, outer, draft.size);
    for (key, tile) in preview.resolve(&set, inner, outer) {
        if let Some(sheet_tile) = slots.get(tile.index() as usize).copied().flatten() {
            place(
                world,
                (draft.sheet, sheet_tile),
                key.0.as_vec2() + placement.offset(),
            );
        }
    }
    spawn_text(world, column, name, tokens::TEXT_SECONDARY);
}

fn on_press(
    press: On<PointerPress>,
    controls: Query<&DockControl>,
    docks: Query<&TilesetDock>,
    views: Res<TilesetDockViews>,
    mut drag: ResMut<BlockDrag>,
) {
    let Some((root, sheet, tile)) = (press.button == PointerButton::Primary)
        .then(|| tile_of(&controls, press.entity))
        .flatten()
    else {
        return;
    };
    let selecting = (docks.get(root).ok())
        .and_then(|d| views.0.get(d.path.as_ref()?))
        .is_some_and(|v| v.page == DockPage::Sheet && v.tool == DockTool::Select);
    if selecting {
        drag.0 = Some((root, sheet, tile, tile));
    }
}

/// The dock, sheet and tile of a sheet tile control.
fn tile_of(controls: &Query<&DockControl>, entity: Entity) -> Option<(Entity, u32, u32)> {
    match controls.get(entity).ok()? {
        DockControl {
            root,
            action: DockAction::Tile(sheet, tile),
        } => Some((*root, *sheet, *tile)),
        _ => None,
    }
}

fn on_move(
    moved: On<PointerMove>,
    controls: Query<&DockControl>,
    pointers: Query<(&PointerId, &PointerPressState)>,
    mut drag: ResMut<BlockDrag>,
    mut commands: Commands,
) {
    let Some((root, sheet, first, last)) = drag.0 else {
        return;
    };
    let held = (pointers.iter())
        .find(|(id, _)| **id == moved.pointer.id)
        .is_some_and(|(_, state)| state.is_primary_pressed());
    let Some((on, _, tile)) = tile_of(&controls, moved.entity)
        .filter(|(r, s, t)| held && (*r, *s) == (root, sheet) && *t != last)
    else {
        return;
    };
    drag.0 = Some((on, sheet, first, tile));
    commands.queue(move |world: &mut World| outline_block(world, root, sheet, (first, tile)));
}

/// Outline the tiles of the block from `a` to `b` while it is dragged.
fn outline_block(world: &mut World, root: Entity, sheet: u32, (a, b): (u32, u32)) {
    let Some(columns) = dock_tileset(world, root)
        .and_then(|(_, t)| t.sheets.get(sheet as usize).map(|s| s.columns))
    else {
        return;
    };
    let mut tiles = world.query::<(&DockControl, &mut BorderColor, &mut Node)>();
    for (control, mut colour, mut node) in tiles.iter_mut(world) {
        let DockAction::Tile(on, tile) = control.action else {
            continue;
        };
        if control.root != root || on != sheet {
            continue;
        }
        let (outline, width) = tile_outline(in_rect(columns, (sheet, a), (sheet, b), (on, tile)));
        if node.border.left != px(width) {
            node.border = UiRect::all(px(width));
        }
        if colour.top != outline {
            *colour = BorderColor::all(outline);
        }
    }
}

/// Open the layout popover over a block dragged across more than one tile.
fn finish_drag(world: &mut World) {
    let Some((root, sheet, first, last)) = world.resource_mut::<BlockDrag>().0.take() else {
        return;
    };
    let Some((path, tileset)) = dock_tileset(world, root).filter(|_| first != last) else {
        return;
    };
    let Some(columns) = tileset.sheets.get(sheet as usize).map(|s| s.columns) else {
        return;
    };
    let (lo, hi) = rect_cells(columns, (sheet, first), (sheet, last));
    let mut views = world.resource_mut::<TilesetDockViews>();
    let Some(view) = views.0.get_mut(&path) else {
        return;
    };
    view.anchor = Some((sheet, first));
    view.selected = Some((sheet, last));
    let brush = view.brush;
    let mut draft = LayoutDraft::new(&tileset, brush, sheet, lo, hi - lo + UVec2::ONE);
    draft.refit(world, &tileset, Fitting::Choose);
    if let Some(view) = world.resource_mut::<TilesetDockViews>().0.get_mut(&path) {
        view.layout = Some(draft);
    }
    refill(world, &path);
    take_keyboard(world, root);
}

fn on_terrain_pick(
    change: On<ComboBoxChangeEvent>,
    picks: Query<&LayoutTerrainPick>,
    mut commands: Commands,
) {
    let Ok(pick) = picks.get(change.entity).copied() else {
        return;
    };
    let selected = change.selected;
    commands.queue(move |world: &mut World| {
        let Some((_, tileset)) = dock_tileset(world, pick.root) else {
            return;
        };
        let id = |i: usize| tileset.terrains.get(i).map_or(0, |t| t.id);
        edit_draft(world, pick.root, |draft| {
            if pick.outer {
                draft.outer = selected.checked_sub(1).map_or(0, id);
            } else {
                draft.inner = id(selected);
            }
            draft.preselect(&tileset);
            Fitting::Refit
        });
    });
}

/// A popover closed by Escape or a press outside it drops its draft.
fn on_dismiss(
    dismissed: On<PopoverDismissed>,
    popovers: Query<&LayoutPopover>,
    mut commands: Commands,
) {
    if let Ok(popover) = popovers.get(dismissed.entity).copied() {
        commands.queue(move |world: &mut World| close(world, popover.root));
    }
}
