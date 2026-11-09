//! The shape editor of the collision mode: a popover drawing one tile's
//! collision polygons on the shown physics layer, zoomed, with tools to
//! add, move and delete their points, and the operators it calls.

use bevy::math::Rot2;
use bevy::picking::events::{PointerDrag, PointerDragEnd, PointerPress, PointerRelease};
use bevy::picking::pointer::{PointerButton, PointerId, PointerLocation};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, InteractionDisabled, UiGlobalTransform, UiTransform};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_feathers::button::{ButtonProps, ButtonSize, ButtonVariant, button};
use jackdaw_feathers::combobox::{ComboBoxChangeEvent, combobox_compact_with_selected};
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::popover::{PopoverDismissed, PopoverPlacement, PopoverProps, popover};
use jackdaw_feathers::tokens;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_input::prelude::{ContextActivity, ContextPriority, InputContext};
use jackdaw_tilemap::{ShapeTurn, ring_area};
use jackdaw_tileset::{CollisionPreset, Tileset, TilesetCollision};

use crate::tileset_collision::{SOLID, TRIGGER, TilesetCollisionSetOp, physics_layer};
use crate::tileset_dock::{
    DockAction, DockControl, DockView, TilesetDock, TilesetDockViews, call_operator, dock_tileset,
    refill, spawn_heading, spawn_icon_button, spawn_pick, spawn_row, spawn_text, take_keyboard,
    tile_image,
};
use crate::tileset_edit::{apply_edit, text, tile_list};
use crate::tileset_modes::ModeId;

/// On-screen size the edited tile is drawn at, about.
const CANVAS: f32 = 288.0;
/// On-screen size of a point's handle.
const HANDLE: f32 = 9.0;
/// How near an edge a click inserts a point on it, in screen pixels.
const EDGE_REACH: f32 = 6.0;
/// Height of one row of a polygon's fill, in screen pixels.
const FILL_ROW: f32 = 2.0;
/// Alpha of a polygon's fill, and of the picked polygon's.
const FILL: f32 = 0.16;
const PICKED_FILL: f32 = 0.3;
/// Width of a polygon's outline, and of the picked polygon's.
const LINE: f32 = 1.5;
const PICKED_LINE: f32 = 2.5;
/// Alpha of the pixel grid.
const GRID: f32 = 0.08;
/// Width of the polygons list beside the tile.
const SIDE_WIDTH: f32 = 168.0;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ShapeDrag>()
        .add_observer(on_press)
        .add_observer(on_drag)
        .add_observer(
            |end: On<PointerDragEnd>, drag: Res<ShapeDrag>, mut commands: Commands| {
                if end.button == PointerButton::Primary && drag.0.is_some() {
                    commands.queue(end_drag);
                }
            },
        )
        .add_observer(
            |end: On<PointerRelease>, drag: Res<ShapeDrag>, mut commands: Commands| {
                if end.button == PointerButton::Primary && drag.0.is_some() {
                    commands.queue(end_drag);
                }
            },
        )
        .add_observer(on_snap_pick)
        .add_observer(on_dismiss)
        .add_observer(on_popover_added)
        .add_observer(on_popover_removed)
        .add_observer(close_with_dock);
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TilesetCollisionSetPolygonOp>()
        .register_operator::<TilesetCollisionRemovePolygonOp>()
        .register_operator::<TilesetCollisionTransformOp>()
        .register_operator::<TilesetCollisionClosePolygonOp>();
    let context = ctx
        .spawn((
            ShapeEditorInputContext,
            ContextPriority(6),
            ContextActivity::INACTIVE,
            Name::new("ShapeEditorInputContext"),
            jackdaw_editor_core::EditorEntity,
        ))
        .id();
    ctx.bind_operator_in::<TilesetCollisionClosePolygonOp>(context, [PresetInput::key("Enter")]);
}

/// The input context of the shape editor, active while one is open.
#[derive(Component, Default)]
#[require(InputContext)]
pub struct ShapeEditorInputContext;

/// What the shape editor of a tileset tab is editing, and how.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeEdit {
    pub sheet: u32,
    pub tile: u32,
    /// Where the editor opened, in window logical pixels.
    pub at: Vec2,
    pub tool: ShapeTool,
    pub snap: ShapeSnap,
    /// Index of the picked polygon of the tile's shape.
    pub polygon: Option<usize>,
    /// Points of a polygon being drawn, in tile fractions y up.
    pub draft: Vec<Vec2>,
}

/// What a press on the shape editor's tile does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShapeTool {
    /// Insert a point on an edge, or draw a new polygon point by point.
    #[default]
    Add,
    Move,
    Delete,
}

impl ShapeTool {
    pub const ALL: [ShapeTool; 3] = [ShapeTool::Add, ShapeTool::Move, ShapeTool::Delete];

    fn icon(self) -> (Icon, &'static str) {
        match self {
            ShapeTool::Add => (Icon::PenTool, "Add points"),
            ShapeTool::Move => (Icon::Move, "Move points"),
            ShapeTool::Delete => (Icon::Eraser, "Delete points"),
        }
    }
}

/// The grid points snap to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShapeSnap {
    #[default]
    Pixel,
    HalfPixel,
    Off,
}

impl ShapeSnap {
    pub const ALL: [ShapeSnap; 3] = [ShapeSnap::Pixel, ShapeSnap::HalfPixel, ShapeSnap::Off];

    fn label(self) -> &'static str {
        match self {
            ShapeSnap::Pixel => "Pixel",
            ShapeSnap::HalfPixel => "Half pixel",
            ShapeSnap::Off => "Off",
        }
    }

    /// `at`, in tile fractions, on the grid of a tile `pixels` across,
    /// inside the tile.
    pub fn apply(self, at: Vec2, pixels: Vec2) -> Vec2 {
        let steps = match self {
            ShapeSnap::Pixel => pixels,
            ShapeSnap::HalfPixel => pixels * 2.0,
            ShapeSnap::Off => return at.clamp(Vec2::ZERO, Vec2::ONE),
        };
        ((at * steps).round() / steps.max(Vec2::ONE)).clamp(Vec2::ZERO, Vec2::ONE)
    }
}

/// The shape editor of the dock `root`, editing sheet tile `tile`.
#[derive(Component, Clone, Copy, Debug)]
pub struct ShapePopover {
    pub root: Entity,
    pub tile: (u32, u32),
}

/// The zoomed tile of the shape editor of the dock `root`.
#[derive(Component, Clone, Copy, Debug)]
pub struct ShapeCanvas {
    pub root: Entity,
}

/// The node holding the drawn fills and outlines of the shape editor.
#[derive(Component, Clone, Copy, Debug)]
pub struct ShapeLines {
    pub root: Entity,
}

/// The handle of a point of the shape editor: of the polygon at
/// `polygon`, or of the polygon being drawn.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShapeHandle {
    pub root: Entity,
    pub polygon: Option<usize>,
    pub point: usize,
}

/// One drawn part of a polygon in the shape editor.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapePiece {
    Fill { polygon: usize, picked: bool },
    Edge { polygon: Option<usize> },
}

/// The snap combo of the shape editor of the dock `root`.
#[derive(Component, Clone, Copy, Debug)]
pub struct ShapeSnapPick {
    pub root: Entity,
}

/// A point being dragged in the shape editor.
#[derive(Resource, Default)]
struct ShapeDrag(Option<Dragging>);

struct Dragging {
    root: Entity,
    handle: Entity,
    polygon: usize,
    point: usize,
    /// The tile's polygons with the drag applied so far.
    polygons: Vec<Vec<Vec2>>,
    color: Color,
    snap: ShapeSnap,
    pixels: Vec2,
    moved: bool,
}

/// Put or replace one polygon of a tile's collision shape.
#[operator(
    id = "tileset.collision.set_polygon",
    writes_disk = true,
    label = "Set Collision Polygon",
    description = "Put one polygon of a tile's collision shape on one physics layer, as one \
                   undo entry: replace the polygon at an index, or add one. A tile with no \
                   shape there takes a solid one. The shape is saved as custom unless it \
                   draws a preset.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        tile(String, doc = "The tile, a packed layer or sheet:tile."),
        layer(String, doc = "Physics layer index or name; layer 0 by default."),
        polygon(
            String,
            doc = "Index of the polygon to replace; empty or new adds one."
        ),
        points(
            String,
            doc = "At least three points as x,y;x,y;... in tile fractions from 0 to 1, y \
                   up from the tile's bottom-left. A clockwise ring is turned around."
        ),
    )
)]
pub fn tileset_collision_set_polygon(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (path, tile, layer) = (
        text(&params, "path"),
        text(&params, "tile"),
        text(&params, "layer"),
    );
    let (polygon, points) = (text(&params, "polygon"), text(&params, "points"));
    commands.queue(move |world: &mut World| {
        let op = TilesetCollisionSetPolygonOp::ID;
        let set = reshape(world, op, &path, &tile, &layer, |polygons| {
            let ring = parse_points(&points)?;
            let mut polygons = polygons.to_vec();
            let count = ring.len();
            match polygon.as_str() {
                "" | "new" => polygons.push(ring),
                index => {
                    let at = polygon_index(index, polygons.len())?;
                    polygons[at] = ring;
                }
            }
            Ok((polygons, format!("polygon of {count} points set")))
        });
        match set {
            Ok(message) => report_to_caller(world, message),
            Err(err) => warn_caller(world, format!("{op}: {err}")),
        }
    });
    OperatorResult::Finished
}

/// Take one polygon out of a tile's collision shape.
#[operator(
    id = "tileset.collision.remove_polygon",
    writes_disk = true,
    label = "Remove Collision Polygon",
    description = "Take one polygon out of a tile's collision shape on one physics layer, as \
                   one undo entry. Taking out the last one clears the layer's shape.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        tile(String, doc = "The tile, a packed layer or sheet:tile."),
        layer(String, doc = "Physics layer index or name; layer 0 by default."),
        polygon(String, doc = "Index of the polygon to take out."),
    )
)]
pub fn tileset_collision_remove_polygon(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (path, tile, layer) = (
        text(&params, "path"),
        text(&params, "tile"),
        text(&params, "layer"),
    );
    let polygon = text(&params, "polygon");
    commands.queue(move |world: &mut World| {
        let op = TilesetCollisionRemovePolygonOp::ID;
        let removed = reshape(world, op, &path, &tile, &layer, |polygons| {
            let at = polygon_index(&polygon, polygons.len())?;
            let mut polygons = polygons.to_vec();
            polygons.remove(at);
            Ok((polygons, format!("polygon {at} removed")))
        });
        match removed {
            Ok(message) => report_to_caller(world, message),
            Err(err) => warn_caller(world, format!("{op}: {err}")),
        }
    });
    OperatorResult::Finished
}

/// Flip or turn a tile's collision shape.
#[operator(
    id = "tileset.collision.transform",
    writes_disk = true,
    label = "Transform Collision Shape",
    description = "Flip a tile's collision shape on one physics layer across its middle, or \
                   turn it a quarter clockwise, as one undo entry.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        tile(String, doc = "The tile, a packed layer or sheet:tile."),
        layer(String, doc = "Physics layer index or name; layer 0 by default."),
        turn(String, doc = "flip_x, flip_y or rotate."),
    )
)]
pub fn tileset_collision_transform(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (path, tile, layer) = (
        text(&params, "path"),
        text(&params, "tile"),
        text(&params, "layer"),
    );
    let turn = text(&params, "turn");
    commands.queue(move |world: &mut World| {
        let op = TilesetCollisionTransformOp::ID;
        let turned = reshape(world, op, &path, &tile, &layer, |polygons| {
            let turn = turn_named(&turn)
                .ok_or_else(|| format!("turn is flip_x, flip_y or rotate, not `{turn}`"))?;
            if polygons.is_empty() {
                return Err("the tile has no shape on the layer".into());
            }
            Ok((
                turn.apply(polygons),
                format!("shape turned by {}", turn_name(turn)),
            ))
        });
        match turned {
            Ok(message) => report_to_caller(world, message),
            Err(err) => warn_caller(world, format!("{op}: {err}")),
        }
    });
    OperatorResult::Finished
}

/// Close the polygon being drawn in a shape editor.
#[operator(
    id = "tileset.collision.close_polygon",
    writes_disk = true,
    label = "Close Collision Polygon",
    description = "Close the polygon being drawn in the open shape editor of a Tileset tab and \
                   add it to the tile's collision shape, as tileset.collision.set_polygon does.",
    allows_undo = false,
    is_available = a_polygon_is_drawn
)]
pub fn tileset_collision_close_polygon(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| match drawing_dock(world) {
        Some(root) => close_draft(world, root),
        None => warn_caller(
            world,
            "tileset.collision.close_polygon: no shape editor is drawing a polygon",
        ),
    });
    OperatorResult::Finished
}

fn a_polygon_is_drawn(world: &World) -> bool {
    let views = world.resource::<TilesetDockViews>();
    let drawing = (views.0.values()).any(|v| v.shape.as_ref().is_some_and(|s| s.draft.len() >= 3));
    let open =
        (world.try_query::<&ShapePopover>()).is_some_and(|mut q| q.iter(world).next().is_some());
    drawing && open
}

/// The dock whose open shape editor draws a polygon of three points or
/// more.
fn drawing_dock(world: &mut World) -> Option<Entity> {
    let roots: Vec<Entity> = (world.query::<&ShapePopover>().iter(world))
        .map(|p| p.root)
        .collect();
    roots
        .into_iter()
        .find(|root| context(world, *root).is_some_and(|(.., edit)| edit.draft.len() >= 3))
}

fn turn_named(name: &str) -> Option<ShapeTurn> {
    [ShapeTurn::FlipX, ShapeTurn::FlipY, ShapeTurn::Rotate]
        .into_iter()
        .find(|t| turn_name(*t) == name)
}

fn turn_name(turn: ShapeTurn) -> &'static str {
    match turn {
        ShapeTurn::FlipX => "flip_x",
        ShapeTurn::FlipY => "flip_y",
        ShapeTurn::Rotate => "rotate",
    }
}

fn polygon_index(text: &str, count: usize) -> Result<usize, String> {
    let at = text
        .parse::<usize>()
        .map_err(|_| format!("polygon is an index, not `{text}`"))?;
    if at >= count {
        return Err(format!("the shape has {count} polygons, no polygon {at}"));
    }
    Ok(at)
}

/// A ring of points written x,y;x,y;... in tile fractions, counter-clockwise.
pub fn parse_points(text: &str) -> Result<Vec<Vec2>, String> {
    let mut ring = (text.split(';').map(str::trim))
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (x, y) = pair
                .split_once(',')
                .ok_or_else(|| format!("`{pair}` is no x,y point"))?;
            let number = |n: &str| {
                (n.trim().parse::<f32>()).map_err(|_| format!("`{pair}` is no x,y point"))
            };
            let point = Vec2::new(number(x)?, number(y)?);
            let inside = point.cmpge(Vec2::ZERO).all() && point.cmple(Vec2::ONE).all();
            if !inside {
                return Err(format!("{pair} is outside the tile"));
            }
            Ok(point)
        })
        .collect::<Result<Vec<Vec2>, String>>()?;
    ring.dedup();
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    if ring.len() < 3 {
        return Err("a polygon takes at least three points".into());
    }
    let area = ring_area(&ring);
    if area.abs() < 1e-6 {
        return Err("the points enclose no area".into());
    }
    if area < 0.0 {
        ring.reverse();
    }
    Ok(ring)
}

/// `ring` written as `parse_points` reads it.
pub fn format_points(ring: &[Vec2]) -> String {
    let points: Vec<String> = ring.iter().map(|p| format!("{},{}", p.x, p.y)).collect();
    points.join(";")
}

/// Change the polygons of one tile's shape on one physics layer with
/// `change`, keeping its flags; no polygons left clears the shape.
fn reshape(
    world: &mut World,
    op: &str,
    path: &str,
    tile: &str,
    layer: &str,
    change: impl FnOnce(&[Vec<Vec2>]) -> Result<(Vec<Vec<Vec2>>, String), String>,
) -> Result<String, String> {
    apply_edit(world, op, path, |_, tileset| {
        let tiles = tile_list(tileset, tile)?;
        let &[(sheet, tile)] = tiles.as_slice() else {
            return Err("give one tile".into());
        };
        let layer = physics_layer(tileset, layer)?;
        let own = (tileset.collision(sheet, tile).iter())
            .find(|c| c.layer == layer)
            .cloned();
        let before = own.as_ref().map_or(&[][..], |c| c.polygons.as_slice());
        let (polygons, message) = change(before)?;
        if polygons.is_empty() {
            tileset.clear_collision(sheet, tile, layer)
        } else {
            let preset = match &own {
                Some(own) if own.polygons == polygons => own.preset,
                _ => CollisionPreset::of_polygons(&polygons),
            };
            let shape = TilesetCollision {
                layer,
                trigger: own.as_ref().is_some_and(|c| c.trigger),
                one_way: own.as_ref().is_some_and(|c| c.one_way),
                preset,
                polygons,
            };
            tileset.set_collision(sheet, tile, shape)
        }
        .map_err(|e| e.to_string())?;
        Ok(message)
    })
}

/// Open the shape editor of the dock `root` on sheet tile `tile`, where the
/// pointer is.
pub(crate) fn open_shape_editor(world: &mut World, root: Entity, sheet: u32, tile: u32) {
    let Some((path, _)) = dock_tileset(world, root) else {
        return;
    };
    let at = (world.query::<(&PointerId, &PointerLocation)>().iter(world))
        .find(|(id, _)| **id == PointerId::Mouse)
        .and_then(|(_, l)| l.location.as_ref().map(|l| l.position))
        .unwrap_or_default();
    let mut views = world.resource_mut::<TilesetDockViews>();
    let Some(view) = views.0.get_mut(&path) else {
        return;
    };
    let (tool, snap) = view
        .shape
        .as_ref()
        .map_or_else(Default::default, |s| (s.tool, s.snap));
    view.shape = Some(ShapeEdit {
        sheet,
        tile,
        at,
        tool,
        snap,
        polygon: None,
        draft: Vec::new(),
    });
    take_keyboard(world, root);
    refill(world, &path);
}

/// Drop a shape edit whose tile or picked polygon is gone, or that is out
/// of the collision mode.
pub(crate) fn prune(view: &mut DockView, tileset: &Tileset) {
    let layer = view.collision.layer;
    let keep = view.mode == ModeId::COLLISION;
    let Some(edit) = view.shape.as_mut() else {
        return;
    };
    if !keep || tileset.sheet_rect(edit.sheet, edit.tile).is_none() {
        view.shape = None;
        return;
    }
    let count = polygons_of(tileset, edit, layer).len();
    edit.polygon = edit.polygon.filter(|p| *p < count);
}

fn polygons_of(tileset: &Tileset, edit: &ShapeEdit, layer: u32) -> Vec<Vec<Vec2>> {
    shape_of(tileset, edit, layer).map_or_else(Vec::new, |s| s.polygons)
}

fn shape_of(tileset: &Tileset, edit: &ShapeEdit, layer: u32) -> Option<TilesetCollision> {
    (tileset.collision(edit.sheet, edit.tile).iter())
        .find(|c| c.layer == layer)
        .cloned()
}

/// The path, tileset, view and shape edit of the dock `root`.
fn context(world: &mut World, root: Entity) -> Option<(String, Tileset, DockView, ShapeEdit)> {
    let (path, tileset) = dock_tileset(world, root)?;
    let view = world.resource::<TilesetDockViews>().0.get(&path)?.clone();
    let edit = view.shape.clone()?;
    Some((path, tileset, view, edit))
}

/// Change the shape edit of the dock `root`.
fn edit_view(world: &mut World, root: Entity, change: impl FnOnce(&mut ShapeEdit)) {
    let Some(path) = world.get::<TilesetDock>(root).and_then(|d| d.path.clone()) else {
        return;
    };
    let mut views = world.resource_mut::<TilesetDockViews>();
    if let Some(edit) = views.0.get_mut(&path).and_then(|v| v.shape.as_mut()) {
        change(edit);
    }
}

/// Build the shape editor of the dock `root` again from its tileset and
/// view.
fn refresh(world: &mut World, root: Entity) {
    let Some((path, tileset)) = dock_tileset(world, root) else {
        return;
    };
    let Some(mut view) = world.resource::<TilesetDockViews>().0.get(&path).cloned() else {
        return;
    };
    prune(&mut view, &tileset);
    sync(world, root, &tileset, &view);
}

/// Show the shape editor `view` asks for over the dock `root`, built again
/// in place, or close it when there is none.
pub(crate) fn sync(world: &mut World, root: Entity, tileset: &Tileset, view: &DockView) {
    let open: Vec<(Entity, ShapePopover)> = (world.query::<(Entity, &ShapePopover)>())
        .iter(world)
        .filter(|(_, p)| p.root == root)
        .map(|(e, p)| (e, *p))
        .collect();
    let edit = view.shape.as_ref();
    let tile = edit.map(|e| (e.sheet, e.tile));
    let mut kept = None;
    for (entity, popover) in open {
        if kept.is_none() && Some(popover.tile) == tile {
            kept = Some(entity);
        } else {
            world.entity_mut(entity).despawn();
        }
    }
    let Some(edit) = edit else {
        return;
    };
    let popover = match kept {
        Some(popover) => {
            world.entity_mut(popover).despawn_related::<Children>();
            popover
        }
        None => {
            let props = PopoverProps::new(root)
                .with_position(edit.at)
                .with_placement(PopoverPlacement::RightStart)
                .with_padding(tokens::SPACING_MD)
                .with_gap(tokens::SPACING_MD);
            let tile = (edit.sheet, edit.tile);
            world
                .spawn((popover(props), ShapePopover { root, tile }))
                .id()
        }
    };
    fill_popover(world, popover, root, tileset, view, edit);
}

fn fill_popover(
    world: &mut World,
    popover: Entity,
    root: Entity,
    tileset: &Tileset,
    view: &DockView,
    edit: &ShapeEdit,
) {
    let layer = view.collision.layer;
    let shape = shape_of(tileset, edit, layer);
    let tools = spawn_row(world, popover, tokens::SPACING_XS);
    for tool in ShapeTool::ALL {
        let (icon, tip) = tool.icon();
        let variant = if edit.tool == tool {
            ButtonVariant::Active
        } else {
            ButtonVariant::Ghost
        };
        let props = ButtonProps::new("")
            .with_left_icon(icon)
            .with_size(ButtonSize::IconSM)
            .with_variant(variant);
        let action = DockAction::ShapeTool(tool);
        let control = DockControl { root, action };
        world.spawn((button(props), Tooltip::title(tip), control, ChildOf(tools)));
    }
    world.spawn((
        Node {
            width: px(tokens::SPACING_MD),
            ..default()
        },
        ChildOf(tools),
    ));
    let turns = [
        (Icon::Wand, "From traced", DockAction::ShapeFromTraced),
        (
            Icon::FlipHorizontal2,
            "Flip horizontal",
            DockAction::ShapeTurn(ShapeTurn::FlipX),
        ),
        (
            Icon::FlipVertical2,
            "Flip vertical",
            DockAction::ShapeTurn(ShapeTurn::FlipY),
        ),
        (
            Icon::RotateCw,
            "Rotate 90",
            DockAction::ShapeTurn(ShapeTurn::Rotate),
        ),
    ];
    for (icon, tip, action) in turns {
        let button = spawn_icon_button(world, tools, root, icon, tip, action);
        if shape.is_none() && action != DockAction::ShapeFromTraced {
            world.entity_mut(button).insert(InteractionDisabled);
        }
    }
    world.spawn((
        Node {
            flex_grow: 1.0,
            ..default()
        },
        ChildOf(tools),
    ));
    let snaps: Vec<&str> = ShapeSnap::ALL.iter().map(|s| s.label()).collect();
    let snap = (ShapeSnap::ALL.iter()).position(|s| *s == edit.snap);
    world.spawn((
        combobox_compact_with_selected(snaps, snap.unwrap_or_default()),
        Tooltip::title("Snap"),
        ShapeSnapPick { root },
        ChildOf(tools),
    ));

    let body = world
        .spawn((
            Node {
                column_gap: px(tokens::SPACING_MD),
                align_items: AlignItems::FlexStart,
                ..default()
            },
            ChildOf(popover),
        ))
        .id();
    spawn_canvas(world, body, root, tileset, edit, shape.as_ref());
    let side = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                width: px(SIDE_WIDTH),
                row_gap: px(tokens::SPACING_XS),
                ..default()
            },
            ChildOf(body),
        ))
        .id();
    spawn_heading(world, side, "Polygons");
    let polygons = shape.as_ref().map_or(&[][..], |s| s.polygons.as_slice());
    for (index, ring) in polygons.iter().enumerate() {
        let holder = world
            .spawn((
                Node {
                    align_items: AlignItems::Center,
                    ..default()
                },
                ChildOf(side),
            ))
            .id();
        let pick = DockAction::ShapePolygon(index);
        let row = spawn_pick(world, holder, root, pick, edit.polygon == Some(index));
        if let Some(mut node) = world.get_mut::<Node>(row) {
            node.flex_grow = 1.0;
        }
        let name = spawn_text(
            world,
            row,
            format!("Polygon {}", index + 1),
            tokens::TEXT_PRIMARY,
        );
        world.entity_mut(name).insert(Node {
            flex_grow: 1.0,
            ..default()
        });
        spawn_text(
            world,
            row,
            format!("{} points", ring.len()),
            tokens::TEXT_SECONDARY,
        );
        let remove = DockAction::ShapeRemovePolygon(index);
        spawn_icon_button(world, holder, root, Icon::X, "Remove polygon", remove);
    }
    world.spawn((
        button(
            ButtonProps::new("Add polygon")
                .with_left_icon(Icon::Plus)
                .with_variant(ButtonVariant::Ghost)
                .align_left(),
        ),
        DockControl {
            root,
            action: DockAction::ShapeAddPolygon,
        },
        ChildOf(side),
    ));
    let flags = [
        (
            "Trigger",
            shape.as_ref().is_some_and(|s| s.trigger),
            DockAction::ShapeTrigger,
        ),
        (
            "One way",
            shape.as_ref().is_some_and(|s| s.one_way),
            DockAction::ShapeOneWay,
        ),
    ];
    for (label, checked, action) in flags {
        let toggle = world
            .spawn((
                button(
                    ButtonProps::new(label)
                        .with_variant(ButtonVariant::Ghost)
                        .align_left()
                        .with_left_checkbox(checked),
                ),
                DockControl { root, action },
                ChildOf(side),
            ))
            .id();
        if shape.is_none() {
            world.entity_mut(toggle).insert(InteractionDisabled);
        }
    }
}

/// On-screen pixels per tile pixel the editor draws `tileset`'s tiles at.
fn canvas_scale(tileset: &Tileset) -> f32 {
    (CANVAS / tileset.tile_size.max_element().max(1) as f32)
        .round()
        .max(1.0)
}

fn shape_color(shape: Option<&TilesetCollision>) -> Color {
    if shape.is_some_and(|s| s.trigger) {
        TRIGGER
    } else {
        SOLID
    }
}

fn spawn_canvas(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    edit: &ShapeEdit,
    shape: Option<&TilesetCollision>,
) {
    let pixels = tileset.tile_size.as_vec2();
    let scale = canvas_scale(tileset);
    let size = pixels * scale;
    let canvas = world
        .spawn((
            Node {
                width: px(size.x),
                height: px(size.y),
                flex_shrink: 0.0,
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(tokens::INPUT_BG),
            ShapeCanvas { root },
            ChildOf(parent),
        ))
        .id();
    if let Some(image) = tile_image(world, tileset, edit.sheet, edit.tile) {
        world.spawn((
            absolute(Rect::from_corners(Vec2::ZERO, size)),
            image.with_mode(NodeImageMode::Stretch),
            Pickable::IGNORE,
            ChildOf(canvas),
        ));
    }
    if scale >= 4.0 {
        let line = Color::WHITE.with_alpha(GRID);
        for x in 0..=pixels.x as u32 {
            let at = (x as f32 * scale).min(size.x - 1.0);
            let rect = Rect::new(at, 0.0, at + 1.0, size.y);
            world.spawn((
                absolute(rect),
                BackgroundColor(line),
                Pickable::IGNORE,
                ChildOf(canvas),
            ));
        }
        for y in 0..=pixels.y as u32 {
            let at = (y as f32 * scale).min(size.y - 1.0);
            let rect = Rect::new(0.0, at, size.x, at + 1.0);
            world.spawn((
                absolute(rect),
                BackgroundColor(line),
                Pickable::IGNORE,
                ChildOf(canvas),
            ));
        }
    }
    let lines = world
        .spawn((
            absolute(Rect::from_corners(Vec2::ZERO, size)),
            ShapeLines { root },
            Pickable::IGNORE,
            ChildOf(canvas),
        ))
        .id();
    let polygons = shape.map_or(&[][..], |s| s.polygons.as_slice());
    let color = shape_color(shape);
    draw_lines(
        world,
        lines,
        size,
        polygons,
        edit.polygon,
        &edit.draft,
        color,
    );
    for (index, ring) in polygons.iter().enumerate() {
        let picked = edit.polygon == Some(index);
        for (point, at) in ring.iter().enumerate() {
            let handle = ShapeHandle {
                root,
                polygon: Some(index),
                point,
            };
            let fill = if picked { Color::WHITE } else { color };
            spawn_handle(world, canvas, handle, on_canvas(*at, size), fill);
        }
    }
    for (point, at) in edit.draft.iter().enumerate() {
        let handle = ShapeHandle {
            root,
            polygon: None,
            point,
        };
        let fill = if point == 0 {
            tokens::SELECTED_BORDER
        } else {
            tokens::TEXT_PRIMARY
        };
        spawn_handle(world, canvas, handle, on_canvas(*at, size), fill);
    }
}

/// A point in tile fractions y up, on a canvas `size` across.
fn on_canvas(at: Vec2, size: Vec2) -> Vec2 {
    Vec2::new(at.x, 1.0 - at.y) * size
}

/// A point on a canvas `size` across, in tile fractions y up.
fn in_tile(at: Vec2, size: Vec2) -> Vec2 {
    let at = at / size.max(Vec2::ONE);
    Vec2::new(at.x, 1.0 - at.y)
}

fn handle_rect(at: Vec2) -> Rect {
    Rect::from_center_size(at, Vec2::splat(HANDLE))
}

fn spawn_handle(world: &mut World, canvas: Entity, handle: ShapeHandle, at: Vec2, fill: Color) {
    let mut node = absolute(handle_rect(at));
    node.border = UiRect::all(px(1.0));
    world.spawn((
        node,
        BackgroundColor(fill),
        BorderColor::all(Color::BLACK.with_alpha(0.8)),
        handle,
        ChildOf(canvas),
    ));
}

/// Draw the fills and outlines of `polygons`, and the open outline of
/// `draft`, in `lines`.
fn draw_lines(
    world: &mut World,
    lines: Entity,
    size: Vec2,
    polygons: &[Vec<Vec2>],
    picked: Option<usize>,
    draft: &[Vec2],
    color: Color,
) {
    world.entity_mut(lines).despawn_related::<Children>();
    for (index, ring) in polygons.iter().enumerate() {
        let is_picked = picked == Some(index);
        let points: Vec<Vec2> = ring.iter().map(|p| on_canvas(*p, size)).collect();
        let alpha = if is_picked { PICKED_FILL } else { FILL };
        for rect in fill_rects(&points, size.y) {
            world.spawn((
                absolute(rect),
                BackgroundColor(color.with_alpha(alpha)),
                ShapePiece::Fill {
                    polygon: index,
                    picked: is_picked,
                },
                Pickable::IGNORE,
                ChildOf(lines),
            ));
        }
        let width = if is_picked { PICKED_LINE } else { LINE };
        let next = points.iter().cycle().skip(1);
        for (a, b) in points.iter().zip(next) {
            let piece = ShapePiece::Edge {
                polygon: Some(index),
            };
            spawn_segment(world, lines, (*a, *b), width, color, piece);
        }
    }
    let points: Vec<Vec2> = draft.iter().map(|p| on_canvas(*p, size)).collect();
    for pair in points.windows(2) {
        let piece = ShapePiece::Edge { polygon: None };
        spawn_segment(
            world,
            lines,
            (pair[0], pair[1]),
            LINE,
            tokens::TEXT_PRIMARY,
            piece,
        );
    }
}

fn spawn_segment(
    world: &mut World,
    parent: Entity,
    (a, b): (Vec2, Vec2),
    width: f32,
    color: Color,
    piece: ShapePiece,
) {
    let length = a.distance(b);
    if length < 0.01 {
        return;
    }
    let side = Rect::from_center_size((a + b) / 2.0, Vec2::new(length + width, width));
    world.spawn((
        absolute(side),
        BackgroundColor(color),
        UiTransform {
            rotation: Rot2::radians((b - a).to_angle()),
            ..UiTransform::IDENTITY
        },
        piece,
        Pickable::IGNORE,
        ChildOf(parent),
    ));
}

/// The inside of the ring `points`, on screen, as rectangles: one per run
/// of rows `FILL_ROW` high that cross it the same way.
fn fill_rects(points: &[Vec2], height: f32) -> Vec<Rect> {
    let rows = (height / FILL_ROW).ceil() as usize;
    let mut out: Vec<Rect> = Vec::new();
    let mut open: Vec<Rect> = Vec::new();
    for row in 0..rows {
        let top = row as f32 * FILL_ROW;
        let middle = top + FILL_ROW / 2.0;
        let next = points.iter().cycle().skip(1);
        let mut cuts: Vec<f32> = (points.iter().zip(next))
            .filter(|(a, b)| (a.y <= middle) != (b.y <= middle))
            .map(|(a, b)| a.x + (middle - a.y) / (b.y - a.y) * (b.x - a.x))
            .collect();
        cuts.sort_by(f32::total_cmp);
        let spans: Vec<(f32, f32)> = cuts
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[l, r]| (*l, *r))
            .collect();
        let same = spans.len() == open.len()
            && (spans.iter().zip(&open))
                .all(|((l, r), o)| (l - o.min.x).abs() < 0.5 && (r - o.max.x).abs() < 0.5);
        if same {
            for rect in &mut open {
                rect.max.y = top + FILL_ROW;
            }
            continue;
        }
        out.append(&mut open);
        open = (spans.into_iter())
            .filter(|(l, r)| r > l)
            .map(|(l, r)| Rect::new(l, top, r, top + FILL_ROW))
            .collect();
    }
    out.append(&mut open);
    out
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

/// Handle a shape editor control of the dock; false for any other control.
pub(crate) fn act(world: &mut World, control: DockControl) -> bool {
    let root = control.root;
    let shape_action = matches!(
        control.action,
        DockAction::ShapeTool(_)
            | DockAction::ShapePolygon(_)
            | DockAction::ShapeAddPolygon
            | DockAction::ShapeRemovePolygon(_)
            | DockAction::ShapeFromTraced
            | DockAction::ShapeTurn(_)
            | DockAction::ShapeTrigger
            | DockAction::ShapeOneWay
    );
    if !shape_action {
        return false;
    }
    take_keyboard(world, root);
    let Some((path, tileset, view, edit)) = context(world, root) else {
        return true;
    };
    let layer = view.collision.layer;
    let shape = shape_of(&tileset, &edit, layer);
    let count = shape.as_ref().map_or(0, |s| s.polygons.len());
    match control.action {
        DockAction::ShapeTool(tool) => {
            edit_view(world, root, |e| e.tool = tool);
            refresh(world, root);
        }
        DockAction::ShapePolygon(index) => {
            edit_view(world, root, |e| {
                e.polygon = (e.polygon != Some(index)).then_some(index);
            });
            refresh(world, root);
        }
        DockAction::ShapeAddPolygon => {
            let square = [(0.25, 0.25), (0.75, 0.25), (0.75, 0.75), (0.25, 0.75)]
                .map(|(x, y)| Vec2::new(x, y));
            let params = vec![
                ("polygon", "new".to_string()),
                ("points", format_points(&square)),
            ];
            shape_call(
                world,
                root,
                &path,
                &edit,
                layer,
                set_polygon_id(),
                params,
                |e| {
                    (e.polygon, e.draft) = (Some(count), Vec::new());
                },
            );
        }
        DockAction::ShapeRemovePolygon(index) => {
            remove_polygon(world, root, &path, &edit, layer, index);
        }
        DockAction::ShapeFromTraced => {
            let params = flags_params(shape.as_ref(), "traced", None);
            shape_call(world, root, &path, &edit, layer, set_id(), params, |e| {
                e.polygon = None;
            });
        }
        DockAction::ShapeTurn(turn) => {
            let params = vec![("turn", turn_name(turn).to_string())];
            let id = TilesetCollisionTransformOp::ID;
            shape_call(world, root, &path, &edit, layer, id, params, |_| {});
        }
        DockAction::ShapeTrigger | DockAction::ShapeOneWay => {
            let Some(own) = shape.as_ref() else {
                return true;
            };
            let flip = Some(control.action == DockAction::ShapeTrigger);
            let params = flags_params(Some(own), own.preset.name(), flip);
            shape_call(world, root, &path, &edit, layer, set_id(), params, |_| {});
        }
        _ => {}
    }
    true
}

fn set_id() -> &'static str {
    TilesetCollisionSetOp::ID
}

fn set_polygon_id() -> &'static str {
    TilesetCollisionSetPolygonOp::ID
}

/// The parameters of `tileset.collision.set` putting `preset` with the
/// flags of `shape`, the trigger flag flipped for `Some(true)` and the one
/// way flag for `Some(false)`.
fn flags_params(
    shape: Option<&TilesetCollision>,
    preset: &str,
    flip: Option<bool>,
) -> Vec<(&'static str, String)> {
    let trigger = shape.is_some_and(|s| s.trigger) != (flip == Some(true));
    let one_way = shape.is_some_and(|s| s.one_way) != (flip == Some(false));
    vec![
        ("preset", preset.to_string()),
        ("trigger", trigger.to_string()),
        ("one_way", one_way.to_string()),
    ]
}

/// Run the shape operator `id` on the edited tile, then change the shape
/// edit with `after` when it changed the tileset; otherwise build the
/// editor again as it was.
#[expect(
    clippy::too_many_arguments,
    reason = "the dock, its tile and layer, and the call"
)]
fn shape_call(
    world: &mut World,
    root: Entity,
    path: &str,
    edit: &ShapeEdit,
    layer: u32,
    id: &'static str,
    mut params: Vec<(&'static str, String)>,
    after: impl FnOnce(&mut ShapeEdit),
) {
    let key = if id == set_id() { "tiles" } else { "tile" };
    params.push((key, format!("{}:{}", edit.sheet, edit.tile)));
    params.push(("layer", layer.to_string()));
    let before = dock_tileset(world, root).map(|(_, t)| t);
    call_operator(world, path, id, params);
    let changed = dock_tileset(world, root).map(|(_, t)| t) != before;
    if changed {
        edit_view(world, root, after);
    } else {
        refresh(world, root);
    }
}

fn set_polygon(
    world: &mut World,
    root: Entity,
    (path, edit, layer): (&str, &ShapeEdit, u32),
    polygon: Option<usize>,
    ring: &[Vec2],
    after: impl FnOnce(&mut ShapeEdit),
) {
    let index = polygon.map_or_else(|| "new".to_string(), |p| p.to_string());
    let params = vec![("polygon", index), ("points", format_points(ring))];
    shape_call(
        world,
        root,
        path,
        edit,
        layer,
        set_polygon_id(),
        params,
        after,
    );
}

fn remove_polygon(
    world: &mut World,
    root: Entity,
    path: &str,
    edit: &ShapeEdit,
    layer: u32,
    index: usize,
) {
    let params = vec![("polygon", index.to_string())];
    let id = TilesetCollisionRemovePolygonOp::ID;
    shape_call(world, root, path, edit, layer, id, params, |e| {
        e.polygon = match e.polygon {
            Some(p) if p == index => None,
            Some(p) if p > index => Some(p - 1),
            other => other,
        };
    });
}

/// Add the polygon being drawn in the shape editor of the dock `root`.
fn close_draft(world: &mut World, root: Entity) {
    let Some((path, tileset, view, edit)) = context(world, root) else {
        return;
    };
    if edit.draft.len() < 3 {
        return;
    }
    let layer = view.collision.layer;
    let count = polygons_of(&tileset, &edit, layer).len();
    let draft = edit.draft.clone();
    set_polygon(world, root, (&path, &edit, layer), None, &draft, |e| {
        (e.polygon, e.draft) = (Some(count), Vec::new());
    });
}

/// Delete point `point` of the polygon at `polygon`, or of the one being
/// drawn; a polygon left with fewer than three points is removed.
fn delete_point(world: &mut World, root: Entity, polygon: Option<usize>, point: usize) {
    let Some((path, tileset, view, edit)) = context(world, root) else {
        return;
    };
    let layer = view.collision.layer;
    let Some(index) = polygon else {
        edit_view(world, root, |e| {
            if point < e.draft.len() {
                e.draft.remove(point);
            }
        });
        refresh(world, root);
        return;
    };
    let polygons = polygons_of(&tileset, &edit, layer);
    let Some(mut ring) = polygons.get(index).cloned().filter(|r| point < r.len()) else {
        return;
    };
    ring.remove(point);
    if ring.len() < 3 {
        remove_polygon(world, root, &path, &edit, layer, index);
        return;
    }
    set_polygon(
        world,
        root,
        (&path, &edit, layer),
        Some(index),
        &ring,
        |e| {
            e.polygon = Some(index);
        },
    );
}

/// The rectangle of `canvas` on screen, in logical pixels.
fn screen_rect(
    canvases: &Query<(&ComputedNode, &UiGlobalTransform)>,
    canvas: Entity,
) -> Option<Rect> {
    let (node, at) = canvases.get(canvas).ok()?;
    let scale = node.inverse_scale_factor();
    Some(Rect::from_center_size(
        at.translation * scale,
        node.size() * scale,
    ))
}

fn on_press(
    mut press: On<PointerPress>,
    handles: Query<(&ShapeHandle, &ChildOf)>,
    canvases: Query<&ShapeCanvas>,
    nodes: Query<(&ComputedNode, &UiGlobalTransform)>,
    mut commands: Commands,
) {
    let entity = press.entity;
    let button = press.button;
    if let Ok((handle, parent)) = handles.get(entity) {
        let handle = *handle;
        let canvas = parent.parent();
        press.propagate(false);
        commands.queue(move |world: &mut World| {
            handle_press(world, handle, (entity, canvas), button);
        });
        return;
    }
    let Ok(canvas) = canvases.get(entity) else {
        return;
    };
    let Some(rect) = screen_rect(&nodes, entity) else {
        return;
    };
    press.propagate(false);
    let (root, local) = (canvas.root, press.pointer.position - rect.min);
    let size = rect.size();
    commands.queue(move |world: &mut World| canvas_press(world, root, local, size, button));
}

fn on_drag(
    drag: On<PointerDrag>,
    handles: Query<&ChildOf, With<ShapeHandle>>,
    nodes: Query<(&ComputedNode, &UiGlobalTransform)>,
    state: Res<ShapeDrag>,
    mut commands: Commands,
) {
    let dragging = state.0.as_ref().is_some_and(|d| d.handle == drag.entity);
    if !dragging || drag.button != PointerButton::Primary {
        return;
    }
    let Some(rect) = (handles.get(drag.entity).ok()).and_then(|p| screen_rect(&nodes, p.parent()))
    else {
        return;
    };
    let (local, size) = (drag.pointer.position - rect.min, rect.size());
    commands.queue(move |world: &mut World| move_drag(world, local, size));
}

/// A press on the handle `handle`, drawn in the canvas `canvas`.
fn handle_press(
    world: &mut World,
    handle: ShapeHandle,
    (entity, canvas): (Entity, Entity),
    button: PointerButton,
) {
    let root = handle.root;
    let Some((_, tileset, view, edit)) = context(world, root) else {
        return;
    };
    take_keyboard(world, root);
    let deletes = button == PointerButton::Secondary
        || (button == PointerButton::Primary && edit.tool == ShapeTool::Delete);
    if deletes {
        delete_point(world, root, handle.polygon, handle.point);
        return;
    }
    if button != PointerButton::Primary {
        return;
    }
    let Some(index) = handle.polygon else {
        if handle.point == 0 && edit.draft.len() >= 3 {
            close_draft(world, root);
        }
        return;
    };
    let layer = view.collision.layer;
    let shape = shape_of(&tileset, &edit, layer);
    let polygons = shape.as_ref().map_or_else(Vec::new, |s| s.polygons.clone());
    let Some(at) = polygons
        .get(index)
        .and_then(|r| r.get(handle.point))
        .copied()
    else {
        return;
    };
    if edit.tool == ShapeTool::Add && !edit.draft.is_empty() {
        if edit.draft.last() != Some(&at) {
            edit_view(world, root, |e| e.draft.push(at));
        }
        refresh(world, root);
        return;
    }
    if world.get_entity(canvas).is_err() {
        return;
    }
    world.resource_mut::<ShapeDrag>().0 = Some(Dragging {
        root,
        handle: entity,
        polygon: index,
        point: handle.point,
        polygons,
        color: shape_color(shape.as_ref()),
        snap: edit.snap,
        pixels: tileset.tile_size.as_vec2(),
        moved: false,
    });
}

/// A press on the canvas of the dock `root`, `local` pixels from its top
/// left of `size`.
fn canvas_press(world: &mut World, root: Entity, local: Vec2, size: Vec2, button: PointerButton) {
    if button != PointerButton::Primary {
        return;
    }
    let Some((path, tileset, view, edit)) = context(world, root) else {
        return;
    };
    take_keyboard(world, root);
    let layer = view.collision.layer;
    let pixels = tileset.tile_size.as_vec2();
    let at = edit.snap.apply(in_tile(local, size), pixels);
    let polygons = polygons_of(&tileset, &edit, layer);
    if edit.tool == ShapeTool::Add {
        if let Some(first) = edit.draft.first() {
            let closes = edit.draft.len() >= 3 && on_canvas(*first, size).distance(local) <= HANDLE;
            if closes {
                close_draft(world, root);
                return;
            }
            if edit.draft.last() != Some(&at) {
                edit_view(world, root, |e| e.draft.push(at));
            }
            refresh(world, root);
            return;
        }
        if let Some((index, after, point)) = nearest_edge(&polygons, local, size) {
            let point = edit.snap.apply(point, pixels);
            let mut ring = polygons[index].clone();
            if !ring.contains(&point) {
                ring.insert(after + 1, point);
                set_polygon(
                    world,
                    root,
                    (&path, &edit, layer),
                    Some(index),
                    &ring,
                    |e| {
                        e.polygon = Some(index);
                    },
                );
                return;
            }
        }
    }
    let inside = polygons
        .iter()
        .rposition(|r| contains(r, in_tile(local, size)));
    let start = edit.tool == ShapeTool::Add && inside.is_none() && edit.polygon.is_none();
    edit_view(world, root, |e| {
        if start {
            e.draft = vec![at];
        } else {
            e.polygon = inside;
        }
    });
    refresh(world, root);
}

/// The polygon, the index of the edge's first point and the point on it in
/// tile fractions, of the edge nearest `local` within reach.
fn nearest_edge(polygons: &[Vec<Vec2>], local: Vec2, size: Vec2) -> Option<(usize, usize, Vec2)> {
    let mut best: Option<(f32, usize, usize, Vec2)> = None;
    for (index, ring) in polygons.iter().enumerate() {
        let next = ring.iter().cycle().skip(1);
        for (edge, (a, b)) in ring.iter().zip(next).enumerate() {
            let (a, b) = (on_canvas(*a, size), on_canvas(*b, size));
            let along = b - a;
            let t = ((local - a).dot(along) / along.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let on = a + along * t;
            let distance = on.distance(local);
            if distance <= EDGE_REACH && best.is_none_or(|(d, ..)| distance < d) {
                best = Some((distance, index, edge, in_tile(on, size)));
            }
        }
    }
    best.map(|(_, index, edge, at)| (index, edge, at))
}

/// Whether `at` is inside `ring`, by the even-odd rule.
fn contains(ring: &[Vec2], at: Vec2) -> bool {
    let next = ring.iter().cycle().skip(1);
    (ring.iter().zip(next))
        .filter(|(a, b)| (a.y > at.y) != (b.y > at.y))
        .filter(|(a, b)| at.x < a.x + (at.y - a.y) / (b.y - a.y) * (b.x - a.x))
        .count()
        % 2
        == 1
}

/// Move the dragged point to `local` pixels from the top left of its
/// canvas of `size`, and draw the result.
fn move_drag(world: &mut World, local: Vec2, size: Vec2) {
    let Some(mut drag) = world.resource_mut::<ShapeDrag>().0.take() else {
        return;
    };
    let at = drag.snap.apply(in_tile(local, size), drag.pixels);
    let slot = (drag.polygons.get_mut(drag.polygon)).and_then(|r| r.get_mut(drag.point));
    if let Some(point) = slot.filter(|p| **p != at) {
        *point = at;
        drag.moved = true;
        let rect = handle_rect(on_canvas(at, size));
        if let Some(mut node) = world.get_mut::<Node>(drag.handle) {
            node.left = px(rect.min.x);
            node.top = px(rect.min.y);
        }
        let lines = (world.query::<(Entity, &ShapeLines)>().iter(world))
            .find(|(_, l)| l.root == drag.root)
            .map(|(e, _)| e);
        if let Some(lines) = lines {
            let picked = Some(drag.polygon);
            draw_lines(world, lines, size, &drag.polygons, picked, &[], drag.color);
        }
    }
    world.resource_mut::<ShapeDrag>().0 = Some(drag);
}

/// Apply a finished drag as one edit, or pick the polygon of a handle
/// pressed without moving.
fn end_drag(world: &mut World) {
    let Some(drag) = world.resource_mut::<ShapeDrag>().0.take() else {
        return;
    };
    let (root, index) = (drag.root, drag.polygon);
    if !drag.moved {
        edit_view(world, root, |e| e.polygon = Some(index));
        refresh(world, root);
        return;
    }
    let Some((path, _, view, edit)) = context(world, root) else {
        return;
    };
    let Some(ring) = drag.polygons.get(index) else {
        return;
    };
    let layer = view.collision.layer;
    set_polygon(world, root, (&path, &edit, layer), Some(index), ring, |e| {
        e.polygon = Some(index);
    });
}

/// A pick of the snap combo snaps the editor's points to that grid.
fn on_snap_pick(
    change: On<ComboBoxChangeEvent>,
    picks: Query<&ShapeSnapPick>,
    mut commands: Commands,
) {
    let Ok(pick) = picks.get(change.entity).copied() else {
        return;
    };
    let Some(snap) = ShapeSnap::ALL.get(change.selected).copied() else {
        return;
    };
    commands.queue(move |world: &mut World| {
        edit_view(world, pick.root, |e| e.snap = snap);
        refresh(world, pick.root);
    });
}

/// A shape editor closed by Escape or a press outside it drops its edit.
fn on_dismiss(
    dismissed: On<PopoverDismissed>,
    popovers: Query<&ShapePopover>,
    mut commands: Commands,
) {
    let Ok(popover) = popovers.get(dismissed.entity).copied() else {
        return;
    };
    commands.queue(move |world: &mut World| {
        let Some(path) = (world.get::<TilesetDock>(popover.root)).and_then(|d| d.path.clone())
        else {
            return;
        };
        if let Some(view) = world.resource_mut::<TilesetDockViews>().0.get_mut(&path) {
            view.shape = None;
        }
        world.resource_mut::<ShapeDrag>().0 = None;
    });
}

fn on_popover_added(
    _: On<Add<ShapePopover>>,
    mut contexts: Query<&mut ContextActivity, With<ShapeEditorInputContext>>,
) {
    for mut activity in &mut contexts {
        activity.set_if_neq(ContextActivity(true));
    }
}

fn on_popover_removed(
    removed: On<Remove<ShapePopover>>,
    popovers: Query<Entity, With<ShapePopover>>,
    mut contexts: Query<&mut ContextActivity, With<ShapeEditorInputContext>>,
) {
    let open = popovers.iter().any(|e| e != removed.entity);
    for mut activity in &mut contexts {
        activity.set_if_neq(ContextActivity(open));
    }
}

/// A closed tileset tab takes its shape editor with it.
fn close_with_dock(
    removed: On<Remove<TilesetDock>>,
    popovers: Query<(Entity, &ShapePopover)>,
    mut commands: Commands,
) {
    for (entity, popover) in &popovers {
        if popover.root == removed.entity {
            commands.entity(entity).try_despawn();
        }
    }
}
