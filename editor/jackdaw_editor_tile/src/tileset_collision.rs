use std::collections::{BTreeSet, HashMap};

use bevy::picking::events::{
    PointerCancel, PointerDragEnd, PointerMove, PointerPress, PointerRelease,
};
use bevy::picking::pointer::{PointerButton, PointerId, PointerPressState};
use bevy::prelude::*;
use bevy::ui::UiTransform;
use bevy::window::WindowFocused;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_feathers::button::{ButtonProps, ButtonVariant, button};
use jackdaw_feathers::combobox::{ComboBoxChangeEvent, combobox_compact_with_selected};
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::tokens;
use jackdaw_tilemap::{RgbaImage, SheetInput, TileCollision, trace_art};
use jackdaw_tileset::{CollisionPreset, Tileset};

use crate::tileset_dock::{
    CollisionBrush, DockAction, DockControl, DockView, TilesetDockViews, dispatch, dock_path,
    dock_tileset, refill, sheet_image, spawn_icon_button, spawn_pick, spawn_row, spawn_text,
};
use crate::tileset_edit::{apply_edit, text, tile_list};
use crate::tileset_import::decode_sheet;
use crate::tileset_layout::GhostZones;
use crate::tileset_modes::{ModeId, ModePanel, ModeSheet, TilesetMode, TilesetModeAppExt as _};
use crate::zone_overlay::{
    ZoneLayout, ZoneOutline, ZoneOverlay, ZoneValues, redraw_zone_rows, spawn_zone_overlay,
};

/// Outline color of solid shapes, and of triggers.
const SOLID: Color = Color::srgb(0.35, 0.78, 1.0);
const TRIGGER: Color = Color::srgb(1.0, 0.72, 0.22);
/// On-screen size of a preset's glyph.
const GLYPH: f32 = 14.0;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<CollisionStroke>()
        .add_observer(on_layer_pick)
        .add_observer(on_press)
        .add_observer(on_move)
        .add_observer(
            |end: On<PointerRelease>, stroke: Res<CollisionStroke>, mut commands: Commands| {
                end_stroke(end.button, &stroke, &mut commands);
            },
        )
        .add_observer(
            |end: On<PointerDragEnd>, stroke: Res<CollisionStroke>, mut commands: Commands| {
                end_stroke(end.button, &stroke, &mut commands);
            },
        )
        .add_systems(
            Update,
            cancel_on_interruption.run_if(|stroke: Res<CollisionStroke>| stroke.0.is_some()),
        )
        .register_tileset_mode(TilesetMode {
            id: ModeId::COLLISION,
            label: "Collision",
            order: 20,
            panel: collision_panel,
            overlay: Some(collision_overlay),
        });
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TilesetCollisionSetOp>()
        .register_operator::<TilesetCollisionLayerOp>();
}

/// The physics layer combo of the dock `root`.
#[derive(Component, Clone, Copy, Debug)]
pub struct PhysicsLayerPick {
    pub root: Entity,
}

/// Paint a collision shape on tiles, or clear it.
#[operator(
    id = "tileset.collision.set",
    writes_disk = true,
    label = "Set Tile Collision",
    description = "Put a collision shape on tiles on one physics layer, in place of the shape \
                   they had there, as one undo entry: a preset, or the tile's opaque pixels \
                   traced. No preset clears the layer's shape. A traced tile with no opaque \
                   pixel is cleared.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        tiles(
            String,
            doc = "Comma-separated tiles, each a packed layer or sheet:tile."
        ),
        preset(
            String,
            doc = "solid, top_half, bottom_half, slope_up, slope_down or traced; empty or \
                   none clears."
        ),
        layer(String, doc = "Physics layer index or name; layer 0 by default."),
        trigger(
            bool,
            default = false,
            doc = "Whether the shape overlaps without blocking."
        ),
    )
)]
pub fn tileset_collision_set(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (path, tiles) = (text(&params, "path"), text(&params, "tiles"));
    let (preset, layer) = (text(&params, "preset"), text(&params, "layer"));
    let trigger = params.as_bool("trigger").unwrap_or(false);
    commands.queue(move |world: &mut World| {
        let op = TilesetCollisionSetOp::ID;
        let set = apply_edit(world, op, &path, |world, tileset| {
            let tiles = tile_list(tileset, &tiles)?;
            let shape = match preset.as_str() {
                "" | "none" => None,
                name => {
                    let found = CollisionPreset::from_name(name).ok_or_else(|| {
                        format!(
                            "preset is solid, top_half, bottom_half, slope_up, slope_down, \
                             traced or none, not `{name}`"
                        )
                    })?;
                    Some((found, trigger))
                }
            };
            let layer = physics_layer(tileset, &layer)?;
            let sheets = traced_sheets(world, tileset, &tiles, shape)?;
            let trace = |sheet: u32, tile: u32| {
                let (pixels, layout) = sheets.get(&sheet)?;
                traced(pixels, *layout, tile)
            };
            let changed = paint_collision(tileset, &tiles, shape, layer, &trace)?;
            let done = if shape.is_some() { "set" } else { "cleared" };
            Ok(format!("{changed} of {} tiles {done}", tiles.len()))
        });
        match set {
            Ok(message) => report_to_caller(world, message),
            Err(err) => warn_caller(world, format!("{op}: {err}")),
        }
    });
    OperatorResult::Finished
}

/// Add, rename or show a physics layer.
#[operator(
    id = "tileset.collision.layer",
    writes_disk = true,
    label = "Tileset Physics Layer",
    description = "Add a physics layer to a tileset and show it, rename one, or pick the one \
                   the Tileset tab's collision mode shows and paints. Adding and renaming are \
                   undo entries.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the tab with the \
                   keyboard, then the open tileset."
        ),
        action(String, doc = "add, rename or select."),
        layer(String, doc = "Index or name of the layer to rename or select."),
        name(
            String,
            doc = "Name of the added or renamed layer; an added one is Layer n by default."
        ),
    )
)]
pub fn tileset_collision_layer(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let path = text(&params, "path");
    let (action, layer, name) = (
        text(&params, "action"),
        text(&params, "layer"),
        text(&params, "name"),
    );
    commands.queue(move |world: &mut World| {
        let op = TilesetCollisionLayerOp::ID;
        match layer_action(world, &path, &action, &layer, &name) {
            Ok(message) => report_to_caller(world, message),
            Err(err) => warn_caller(world, format!("{op}: {err}")),
        }
    });
    OperatorResult::Finished
}

fn layer_action(
    world: &mut World,
    path: &str,
    action: &str,
    layer: &str,
    name: &str,
) -> Result<String, String> {
    let op = TilesetCollisionLayerOp::ID;
    let path = dock_path(world, path)?;
    match action {
        "add" => {
            let mut added = 0;
            let message = apply_edit(world, op, &path, |_, tileset| {
                let layers = tileset.physics_layers_mut();
                let name = match name {
                    "" => (layers.len() + 1..)
                        .map(|n| format!("Layer {n}"))
                        .find(|n| !layers.contains(n))
                        .unwrap_or_default(),
                    name => free_layer_name(layers, name)?,
                };
                added = layers.len() as u32;
                layers.push(name.clone());
                Ok(format!("physics layer {added} \"{name}\" added"))
            })?;
            show_layer(world, &path, added);
            Ok(message)
        }
        "rename" => apply_edit(world, op, &path, |_, tileset| {
            let index = physics_layer(tileset, layer)?;
            let layers = tileset.physics_layers_mut();
            let others: Vec<String> = (layers.iter().enumerate())
                .filter(|(i, _)| *i as u32 != index)
                .map(|(_, n)| n.clone())
                .collect();
            let name = free_layer_name(&others, name)?;
            layers[index as usize] = name.clone();
            Ok(format!("physics layer {index} is \"{name}\""))
        }),
        "select" => {
            let (_, tileset) = crate::tileset_edit::open_tileset(world, &path)?;
            let index = physics_layer(&tileset, layer)?;
            if !world.resource::<TilesetDockViews>().0.contains_key(&path) {
                return Err(format!("{path} has no Tileset tab"));
            }
            show_layer(world, &path, index);
            let name = tileset.physics_layer_names()[index as usize].clone();
            Ok(format!("physics layer \"{name}\" shown"))
        }
        other => Err(format!("action is add, rename or select, not `{other}`")),
    }
}

/// `name` trimmed, refused when empty or taken by one of `others`.
fn free_layer_name(others: &[String], name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(format!("`{name}` cannot name a physics layer"));
    }
    if others.iter().any(|n| n == name) {
        return Err(format!("a physics layer is already named `{name}`"));
    }
    Ok(name.to_string())
}

/// Show physics layer `layer` in the tab of the tileset at `path`.
fn show_layer(world: &mut World, path: &str, layer: u32) {
    let mut views = world.resource_mut::<TilesetDockViews>();
    let Some(view) = views.0.get_mut(path) else {
        return;
    };
    view.collision.layer = layer;
    refill(world, path);
}

/// A physics layer by index or name; layer 0 when `text` is empty.
fn physics_layer(tileset: &Tileset, text: &str) -> Result<u32, String> {
    let names = tileset.physics_layer_names();
    if text.is_empty() {
        return Ok(0);
    }
    let found = (names.iter().zip(0u32..))
        .find(|(name, i)| text.parse::<u32>().is_ok_and(|n| n == *i) || *name == text);
    found
        .map(|(_, i)| i)
        .ok_or_else(|| format!("no physics layer is `{text}`"))
}

/// The pixels and grid of each sheet a traced shape is read from.
type TracedSheets = HashMap<u32, (Vec<u8>, (u32, u32, jackdaw_tilemap::GridLayout))>;

/// The sheets of `tiles` decoded from disk, when `shape` is traced.
fn traced_sheets(
    world: &World,
    tileset: &Tileset,
    tiles: &[(u32, u32)],
    shape: Option<(CollisionPreset, bool)>,
) -> Result<TracedSheets, String> {
    let mut sheets = TracedSheets::new();
    if !shape.is_some_and(|(p, _)| p == CollisionPreset::Traced) {
        return Ok(sheets);
    }
    let assets = jackdaw_editor_core::asset_index::assets_dir(world).ok_or("no project is open")?;
    for &(sheet, _) in tiles {
        if sheets.contains_key(&sheet) {
            continue;
        }
        let source = tileset
            .sheets
            .get(sheet as usize)
            .ok_or("a sheet is missing")?;
        let layout = tileset.sheet_layout(sheet).ok_or("a sheet has no grid")?;
        let decoded = decode_sheet(&assets, source.source.clone())?;
        let size = (decoded.width, decoded.height, layout);
        sheets.insert(sheet, (decoded.pixels, size));
    }
    Ok(sheets)
}

/// Tile `tile`'s opaque pixels traced, as polygons y up from the tile's
/// bottom-left.
fn traced(
    pixels: &[u8],
    (width, height, layout): (u32, u32, jackdaw_tilemap::GridLayout),
    tile: u32,
) -> Option<Vec<Vec<Vec2>>> {
    let input = SheetInput {
        name: "",
        image: RgbaImage::new(width, height, pixels)?,
        layout,
    };
    trace_art(&input, tile).map(|shape| shape.polygons)
}

/// Put `shape` on `tiles` on physics layer `layer`, or clear the layer from
/// them when `shape` is `None`; says how many tiles changed. `trace` reads
/// a tile's opaque pixels for a traced shape.
pub fn paint_collision(
    tileset: &mut Tileset,
    tiles: &[(u32, u32)],
    shape: Option<(CollisionPreset, bool)>,
    layer: u32,
    trace: &dyn Fn(u32, u32) -> Option<Vec<Vec<Vec2>>>,
) -> Result<usize, String> {
    let layers = tileset.physics_layer_names().len() as u32;
    if layer >= layers {
        return Err(format!("the tileset has no physics layer {layer}"));
    }
    let mut setup = tileset.setup().map_err(|e| e.to_string())?;
    let mut changed = 0;
    for &(sheet, tile) in tiles {
        let address = (tileset.setup_address(sheet, tile)).map_err(|e| e.to_string())?;
        let polygons =
            shape.and_then(|(preset, _)| preset.polygons().or_else(|| trace(sheet, tile)));
        let done = match (shape, polygons) {
            (Some((preset, trigger)), Some(polygons)) => {
                let collision = TileCollision {
                    layer,
                    trigger,
                    preset: preset.into(),
                    polygons,
                };
                setup.set_collision(address, collision)
            }
            _ => setup.clear_collision(address, layer),
        };
        changed += usize::from(done);
    }
    tileset.set_setup(&setup).map_err(|e| e.to_string())?;
    Ok(changed)
}

/// The overlay value of `preset`, solid or a trigger.
fn value(preset: CollisionPreset, trigger: bool) -> u32 {
    let index = (CollisionPreset::ALL.iter().position(|p| *p == preset)).unwrap_or_default();
    index as u32 + 1 + if trigger { 16 } else { 0 }
}

fn kind_color(trigger: bool) -> Color {
    if trigger { TRIGGER } else { SOLID }
}

/// The shapes of sheet `sheet` on the view's physics layer, the brush's
/// preset picked.
fn collision_values(tileset: &Tileset, sheet: u32, view: &DockView) -> ZoneValues {
    let brush = view.collision;
    let outlines = (tileset.tiles.iter())
        .filter(|t| t.sheet == sheet)
        .filter_map(|t| {
            let shape = t.collision.iter().find(|c| c.layer == brush.layer)?;
            let rings = (shape.polygons.iter())
                .map(|ring| ring.iter().map(|p| Vec2::new(p.x, 1.0 - p.y)).collect())
                .collect();
            let value = value(shape.preset, shape.trigger);
            Some((t.tile, ZoneOutline { value, rings }))
        })
        .collect();
    let colors = (CollisionPreset::ALL.into_iter())
        .flat_map(|p| [false, true].map(|trigger| (value(p, trigger), kind_color(trigger))))
        .collect();
    ZoneValues {
        layout: ZoneLayout::Whole,
        tiles: HashMap::new(),
        picked: Some(value(brush.preset, brush.trigger)),
        colors,
        dim: true,
        hover: Some(kind_color(brush.trigger)),
        outlines,
    }
}

/// Each tile's shape on the shown physics layer over a sheet of the
/// collision mode.
pub(crate) fn collision_overlay(world: &mut World, sheet: &ModeSheet) {
    let values = collision_values(sheet.tileset, sheet.sheet, sheet.view);
    spawn_zone_overlay(
        world,
        sheet.frame,
        sheet.root,
        (sheet.tileset, sheet.sheet),
        values,
    );
}

/// The side panel of the collision mode: the physics layer, the presets
/// and the trigger toggle.
pub(crate) fn collision_panel(world: &mut World, panel: &ModePanel) {
    let ModePanel {
        root,
        list,
        tileset,
        view,
        ..
    } = *panel;
    let brush = view.collision;
    let row = spawn_row(world, list, tokens::SPACING_SM);
    let names = tileset.physics_layer_names().to_vec();
    let combo = world
        .spawn((
            combobox_compact_with_selected(names, brush.layer as usize),
            PhysicsLayerPick { root },
            ChildOf(row),
        ))
        .id();
    if let Some(mut node) = world.get_mut::<Node>(combo) {
        node.flex_grow = 1.0;
    }
    let add = DockAction::AddPhysicsLayer;
    spawn_icon_button(world, row, root, Icon::Plus, "Add physics layer", add);
    for preset in CollisionPreset::ALL {
        let action = DockAction::CollisionPreset(preset);
        let picked = brush.preset == preset;
        let row = spawn_pick(world, list, root, action, picked);
        spawn_glyph(world, row, preset);
        spawn_text(world, row, preset_label(preset), tokens::TEXT_PRIMARY);
    }
    world.spawn((
        button(
            ButtonProps::new("Trigger")
                .with_variant(ButtonVariant::Ghost)
                .align_left()
                .with_left_checkbox(brush.trigger),
        ),
        DockControl {
            root,
            action: DockAction::CollisionTrigger,
        },
        ChildOf(list),
    ));
    let hint = "Click or drag over tiles to paint, right-drag to clear.";
    spawn_text(world, list, hint, tokens::TEXT_SECONDARY);
}

fn preset_label(preset: CollisionPreset) -> &'static str {
    match preset {
        CollisionPreset::Solid => "Solid",
        CollisionPreset::TopHalf => "Top half",
        CollisionPreset::BottomHalf => "Bottom half",
        CollisionPreset::SlopeUp => "Slope up",
        CollisionPreset::SlopeDown => "Slope down",
        CollisionPreset::Traced => "Traced from the art",
    }
}

/// A small drawing of `preset`: a rectangle filled, any other shape as
/// lines; traced as an uneven outline.
fn spawn_glyph(world: &mut World, parent: Entity, preset: CollisionPreset) {
    let glyph = world
        .spawn((
            Node {
                width: px(GLYPH),
                height: px(GLYPH),
                flex_shrink: 0.0,
                ..default()
            },
            Pickable::IGNORE,
            ChildOf(parent),
        ))
        .id();
    let uneven = [
        (0.0, 0.0),
        (1.0, 0.0),
        (1.0, 0.6),
        (0.55, 0.6),
        (0.55, 1.0),
        (0.0, 1.0),
    ];
    let ring: Vec<Vec2> = match preset.polygons() {
        Some(polygons) => polygons.into_iter().next().unwrap_or_default(),
        None => uneven.iter().map(|&(x, y)| Vec2::new(x, y)).collect(),
    };
    let at = |p: Vec2| Vec2::new(p.x, 1.0 - p.y) * GLYPH;
    let lo = ring.iter().copied().reduce(Vec2::min).unwrap_or_default();
    let hi = ring.iter().copied().reduce(Vec2::max).unwrap_or_default();
    let rectangle = ring.len() == 4 && (hi - lo).cmpgt(Vec2::ZERO).all();
    if rectangle {
        let rect = Rect::from_corners(at(lo), at(hi));
        world.spawn((
            absolute(rect),
            BackgroundColor(tokens::TEXT_SECONDARY),
            Pickable::IGNORE,
            ChildOf(glyph),
        ));
        return;
    }
    let next = ring.iter().cycle().skip(1);
    for (a, b) in ring.iter().zip(next) {
        let (a, b) = (at(*a), at(*b));
        let side = Rect::from_center_size((a + b) / 2.0, Vec2::new(a.distance(b) + 1.5, 1.5));
        world.spawn((
            absolute(side),
            BackgroundColor(tokens::TEXT_SECONDARY),
            UiTransform {
                rotation: Rot2::radians((b - a).to_angle()),
                ..UiTransform::IDENTITY
            },
            Pickable::IGNORE,
            ChildOf(glyph),
        ));
    }
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

/// A pick of the physics layer combo shows that layer.
fn on_layer_pick(
    change: On<ComboBoxChangeEvent>,
    picks: Query<&PhysicsLayerPick>,
    mut commands: Commands,
) {
    let Ok(pick) = picks.get(change.entity).copied() else {
        return;
    };
    let selected = change.selected;
    commands.queue(move |world: &mut World| {
        let Some((path, _)) = dock_tileset(world, pick.root) else {
            return;
        };
        let called = world
            .operator(TilesetCollisionLayerOp::ID)
            .param("path", path)
            .param("action", "select")
            .param("layer", selected.to_string())
            .call();
        if let Err(err) = called {
            warn!("{}: {err}", TilesetCollisionLayerOp::ID);
        }
    });
}

/// The stroke a pointer is painting on a sheet, applied when it lifts.
#[derive(Resource, Default)]
struct CollisionStroke(Option<Stroke>);

struct Stroke {
    root: Entity,
    path: String,
    sheet: u32,
    button: PointerButton,
    brush: CollisionBrush,
    tileset: Tileset,
    /// The tileset with the stroke painted so far.
    shown: Tileset,
    tiles: BTreeSet<u32>,
    /// The last point painted, in sheet pixels.
    last: Vec2,
}

fn end_stroke(button: PointerButton, stroke: &CollisionStroke, commands: &mut Commands) {
    if stroke.0.as_ref().is_some_and(|s| s.button == button) {
        commands.queue(move |world: &mut World| finish(world, button));
    }
}

/// A cancelled pointer or a window losing focus drops the stroke unapplied.
fn cancel_on_interruption(
    mut focus: MessageReader<WindowFocused>,
    mut cancels: MessageReader<PointerCancel>,
    mut commands: Commands,
) {
    let lost = focus.read().any(|f| !f.focused);
    let cancelled = cancels.read().count() > 0;
    if lost || cancelled {
        commands.queue(|world: &mut World| {
            if let Some(stroke) = world.resource_mut::<CollisionStroke>().0.take() {
                refill(world, &stroke.path);
            }
        });
    }
}

/// The dock, frame, sheet tile and point in it, from (0, 0) at its top left
/// to (1, 1), of a pointer event on a sheet tile.
type TileHit = (Entity, Entity, u32, u32, Vec2);

fn tile_hit(
    entity: Entity,
    position: Option<Vec3>,
    controls: &Query<(&DockControl, &ChildOf)>,
) -> Option<TileHit> {
    let (control, parent) = controls.get(entity).ok()?;
    let DockAction::Tile(sheet, tile) = control.action else {
        return None;
    };
    let inside = position?.truncate() + 0.5;
    Some((control.root, parent.parent(), sheet, tile, inside))
}

fn on_press(
    press: On<PointerPress>,
    controls: Query<(&DockControl, &ChildOf)>,
    stroke: Res<CollisionStroke>,
    mut commands: Commands,
) {
    if stroke.0.is_some() {
        return;
    }
    if let Some(hit) = tile_hit(press.entity, press.hit.position, &controls) {
        let button = press.button;
        commands.queue(move |world: &mut World| begin(world, hit, button));
    }
}

fn on_move(
    moved: On<PointerMove>,
    controls: Query<(&DockControl, &ChildOf)>,
    pointers: Query<(&PointerId, &PointerPressState)>,
    stroke: Res<CollisionStroke>,
    mut commands: Commands,
) {
    let Some(button) = stroke.0.as_ref().map(|s| s.button) else {
        return;
    };
    let held = (pointers.iter())
        .find(|(id, _)| **id == moved.pointer.id)
        .is_some_and(|(_, state)| match button {
            PointerButton::Primary => state.is_primary_pressed(),
            PointerButton::Secondary => state.is_secondary_pressed(),
            PointerButton::Middle => state.is_middle_pressed(),
        });
    if !held {
        return;
    }
    if let Some(hit) = tile_hit(moved.entity, moved.hit.position, &controls) {
        commands.queue(move |world: &mut World| extend(world, hit));
    }
}

/// Start a stroke in a dock in the collision mode: left paints the brush,
/// right clears.
fn begin(world: &mut World, hit: TileHit, button: PointerButton) {
    let (root, _, sheet, tile, inside) = hit;
    if button == PointerButton::Middle || world.resource::<CollisionStroke>().0.is_some() {
        return;
    }
    let Some((path, tileset)) = dock_tileset(world, root) else {
        return;
    };
    let Some(view) = world.resource::<TilesetDockViews>().0.get(&path).cloned() else {
        return;
    };
    if view.mode != ModeId::COLLISION {
        return;
    }
    let Some((origin, size)) = tileset.sheet_rect(sheet, tile) else {
        return;
    };
    let stroke = Stroke {
        root,
        path,
        sheet,
        button,
        brush: view.collision,
        shown: tileset.clone(),
        tileset,
        tiles: BTreeSet::new(),
        last: origin.as_vec2() + inside * size.as_vec2(),
    };
    world.resource_mut::<CollisionStroke>().0 = Some(stroke);
    extend(world, hit);
}

/// Paint every tile on the line from the stroke's last point to the hit.
fn extend(world: &mut World, (root, frame, sheet, tile, inside): TileHit) {
    let Some(mut stroke) = world.resource_mut::<CollisionStroke>().0.take() else {
        return;
    };
    let layout = (stroke.tileset.sheet_layout(sheet))
        .filter(|_| stroke.root == root && stroke.sheet == sheet);
    if let Some((layout, (origin, size))) = layout.zip(stroke.tileset.sheet_rect(sheet, tile)) {
        let at = origin.as_vec2() + inside * size.as_vec2();
        let step = size.as_vec2().min_element() / 4.0;
        let steps = (stroke.last.distance(at) / step.max(0.5)).ceil().max(1.0) as u32;
        let mut fresh = Vec::new();
        for i in 0..=steps {
            let point = stroke.last.lerp(at, i as f32 / steps as f32);
            if let Some((found, _)) = layout
                .tile_at(point)
                .filter(|(t, _)| stroke.tiles.insert(*t))
            {
                fresh.push(found);
            }
        }
        stroke.last = at;
        show(world, &mut stroke, frame, &fresh);
    }
    world.resource_mut::<CollisionStroke>().0 = Some(stroke);
}

/// Paint the new tiles into the stroke's tileset and draw their rows again.
fn show(world: &mut World, stroke: &mut Stroke, frame: Entity, fresh: &[u32]) {
    if fresh.is_empty() || world.get_entity(frame).is_err() {
        return;
    }
    let brush = stroke.brush;
    let shape = (stroke.button == PointerButton::Primary).then_some((brush.preset, brush.trigger));
    let pixels = (shape.is_some_and(|(p, _)| p == CollisionPreset::Traced))
        .then(|| sheet_pixels(world, &stroke.tileset, stroke.sheet))
        .flatten();
    let trace = |_: u32, tile: u32| {
        let (bytes, size) = pixels.as_ref()?;
        traced(bytes, *size, tile)
    };
    let tiles: Vec<(u32, u32)> = fresh.iter().map(|t| (stroke.sheet, *t)).collect();
    if paint_collision(&mut stroke.shown, &tiles, shape, brush.layer, &trace).is_err() {
        return;
    }
    let Some(view) = (world.resource::<TilesetDockViews>().0.get(&stroke.path)).cloned() else {
        return;
    };
    let overlay = (world.query_filtered::<(Entity, &ZoneOverlay, &ChildOf), Without<GhostZones>>())
        .iter(world)
        .find(|(_, o, parent)| o.root == stroke.root && parent.parent() == frame)
        .map(|(e, ..)| e);
    let values = collision_values(&stroke.shown, stroke.sheet, &view);
    let Some(overlay) = overlay else {
        spawn_zone_overlay(
            world,
            frame,
            stroke.root,
            (&stroke.shown, stroke.sheet),
            values,
        );
        return;
    };
    let columns =
        (stroke.tileset.sheets.get(stroke.sheet as usize)).map_or(1, |s| s.columns.max(1));
    let rows: BTreeSet<u32> = fresh.iter().map(|tile| tile / columns).collect();
    redraw_zone_rows(world, overlay, values, rows);
}

/// The pixels of the drawn image of sheet `sheet`, with its size and grid.
fn sheet_pixels(
    world: &mut World,
    tileset: &Tileset,
    sheet: u32,
) -> Option<(Vec<u8>, (u32, u32, jackdaw_tilemap::GridLayout))> {
    let source = &tileset.sheets.get(sheet as usize)?.source;
    let handle = sheet_image(world, source)?;
    let image = world.resource::<Assets<Image>>().get(&handle)?;
    let pixels = image.data.clone()?;
    let layout = tileset.sheet_layout(sheet)?;
    Some((pixels, (image.width(), image.height(), layout)))
}

/// Apply the stroke `button` paints as one edit.
fn finish(world: &mut World, button: PointerButton) {
    let mut strokes = world.resource_mut::<CollisionStroke>();
    let Some(stroke) = strokes.0.take_if(|s| s.button == button) else {
        return;
    };
    let tiles: Vec<String> = (stroke.tiles.iter())
        .map(|tile| format!("{}:{tile}", stroke.sheet))
        .collect();
    let brush = stroke.brush;
    let preset = match button {
        PointerButton::Primary => brush.preset.name(),
        _ => "none",
    };
    let call = (!tiles.is_empty()).then(|| {
        let params = vec![
            ("tiles", tiles.join(",")),
            ("preset", preset.to_string()),
            ("layer", brush.layer.to_string()),
            ("trigger", brush.trigger.to_string()),
        ];
        (TilesetCollisionSetOp::ID, params)
    });
    dispatch(world, stroke.root, &stroke.path, &stroke.tileset, call);
}
