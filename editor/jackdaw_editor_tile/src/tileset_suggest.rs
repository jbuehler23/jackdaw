use std::collections::{HashMap, HashSet};

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_feathers::button::{ButtonProps, ButtonVariant, button};
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::tokens;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_tilemap::{
    LayoutTemplate, RgbaImage, SheetInput, Suggestion, SuggestionKind, TerrainId, suggest_blocks,
    taken_tiles,
};
use jackdaw_tileset::Tileset;

use crate::tileset_dock::{
    DockAction, DockControl, DockPane, DockView, Pane, TilesetDock, TilesetDockViews, block_rect,
    dispatch, dock_path, dock_tileset, frame_size, load, refill, sheet_image, spawn_heading,
    spawn_icon_button, spawn_row, spawn_text, take_keyboard, zoom,
};
use crate::tileset_edit::text;
use crate::tileset_layout::open_block;
use crate::tileset_modes::ModeId;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<DismissedSuggestions>();
}

/// Suggestions dismissed this session, as sheet, origin and size, by
/// tileset path.
#[derive(Resource, Default)]
pub struct DismissedSuggestions(pub HashMap<String, HashSet<(usize, UVec2, UVec2)>>);

/// The outline of the suggestion shown on the sheet of the dock `root`.
#[derive(Component, Clone, Copy, Debug)]
pub struct SuggestionOutline {
    pub root: Entity,
    pub origin: UVec2,
    pub size: UVec2,
}

/// The one-line text of a suggestion row.
#[derive(Component, Clone, Copy, Debug)]
pub struct SuggestionLabel;

/// List blocks and objects worth setting up in the Tileset tab.
#[operator(
    id = "tileset.suggest",
    label = "Suggest Blocks",
    description = "List blocks of the sheet that look like a terrain layout, with the \
                   terrains they seem to join, and groups of tiles that look like one object, \
                   in the Suggestions list of a Tileset tab. It changes nothing; each row is \
                   applied only by its own button.",
    allows_undo = false,
    params(path(
        String,
        doc = "Tileset file under the project's assets; defaults to the tab with the \
               keyboard, then the open tileset."
    ))
)]
pub fn tileset_suggest(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let path = text(&params, "path");
    commands.queue(move |world: &mut World| match suggest(world, &path) {
        Ok(message) => report_to_caller(world, message),
        Err(err) => warn_caller(world, format!("tileset.suggest: {err}")),
    });
    OperatorResult::Finished
}

fn suggest(world: &mut World, path: &str) -> Result<String, String> {
    let path = dock_path(world, path)?;
    let tileset = load(world, &path)?
        .1
        .ok_or("the tileset is still loading")?;
    let dismissed = (world.resource::<DismissedSuggestions>().0.get(&path))
        .cloned()
        .unwrap_or_default();
    let found: Vec<Suggestion> = (find(world, &tileset)?.into_iter())
        .filter(|s| !dismissed.contains(&key(s)))
        .collect();
    let lines: Vec<String> = (found.iter())
        .map(|s| format!("{} at {}", describe(&tileset, s), place(s)))
        .collect();
    let mut views = world.resource_mut::<TilesetDockViews>();
    let view = views.0.entry(path.clone()).or_default();
    (view.suggestions, view.shown) = (Some(found), None);
    refill(world, &path);
    Ok(format!("{} suggestions\n{}", lines.len(), lines.join("\n")))
}

/// The suggestions for `tileset`'s setup sheets as they are drawn now.
fn find(world: &mut World, tileset: &Tileset) -> Result<Vec<Suggestion>, String> {
    let setup = tileset.setup().map_err(|e| e.to_string())?;
    let mut handles = Vec::new();
    for index in &tileset.setup_sheets {
        let source = &(tileset.sheets.get(*index as usize)).ok_or("a sheet is missing")?;
        let handle = sheet_image(world, &source.source).ok_or("a sheet image is missing")?;
        handles.push((*index, handle));
    }
    let images = world.resource::<Assets<Image>>();
    let mut inputs = Vec::new();
    for (index, handle) in &handles {
        let image = images
            .get(handle)
            .ok_or("the sheet images are still loading")?;
        let pixels = image.data.as_deref().ok_or("a sheet image has no pixels")?;
        inputs.push(SheetInput {
            name: "",
            image: RgbaImage::new(image.width(), image.height(), pixels)
                .ok_or("a sheet image is not 8-bit RGBA")?,
            layout: tileset.sheet_layout(*index).ok_or("a sheet has no grid")?,
        });
    }
    let icons: Vec<_> = (tileset.terrains.iter())
        .filter_map(|t| {
            let (sheet, tile) = t.icon?;
            Some((t.id, tileset.setup_address(sheet, tile).ok()?))
        })
        .collect();
    suggest_blocks(&inputs, &setup, &icons).map_err(|e| e.to_string())
}

fn key(s: &Suggestion) -> (usize, UVec2, UVec2) {
    (s.sheet, s.origin, s.size)
}

fn short(template: LayoutTemplate) -> &'static str {
    match template {
        LayoutTemplate::Corner16 => "16 corner",
        LayoutTemplate::Side16 => "16 side",
        LayoutTemplate::Minimal3x3 => "3x3",
        LayoutTemplate::Corner2x2 => "2x2 corners",
        LayoutTemplate::Blob47 => "47 blob",
        LayoutTemplate::Minimal3x3Inner => "3x3 + corners",
    }
}

/// "Dirt on grass, 3x3 + corners", or "5x3 block, 3x3 + corners" when it
/// names none of the user's terrains.
fn describe(tileset: &Tileset, s: &Suggestion) -> String {
    let size = format!("{}x{}", s.size.x, s.size.y);
    match &s.kind {
        SuggestionKind::Block {
            layouts,
            inner,
            outer,
        } => {
            let layout = layouts.first().map_or("a layout", |l| short(l.template));
            let name = |id: Option<TerrainId>| match id {
                Some(0) => "empty".to_string(),
                Some(id) => tileset.terrain_name(id),
                None => "another".to_string(),
            };
            if !s.named() {
                return format!("{size} block, {layout}");
            }
            let line = format!("{} on {}, {layout}", name(*inner), name(*outer));
            let mut chars = line.chars();
            chars
                .next()
                .map_or(line.clone(), |c| c.to_uppercase().chain(chars).collect())
        }
        SuggestionKind::Object { cells } => format!("{size} object, {} tiles", cells.len()),
    }
}

/// "column 10, row 0", the top-left tile of `s`.
fn place(s: &Suggestion) -> String {
    format!("column {}, row {}", s.origin.x, s.origin.y)
}

/// Drop suggestions whose sheet is gone or whose tiles have been set up, and
/// the outline of one dropped or folded away.
pub(crate) fn prune(view: &mut DockView, tileset: &Tileset) {
    let (Some(found), Ok(setup)) = (view.suggestions.take(), tileset.setup()) else {
        return;
    };
    let taken = taken_tiles(&setup);
    let columns = |s: &Suggestion| {
        let index = tileset.setup_sheets.get(s.sheet)?;
        Some(tileset.sheets.get(*index as usize)?.columns)
    };
    let shown = view.shown.and_then(|i| found.get(i)).cloned();
    let kept: Vec<Suggestion> = (found.into_iter())
        .filter(|s| columns(s).is_some_and(|c| s.free_in(&taken, c)))
        .collect();
    let folded = |s: &Suggestion| {
        let object = matches!(s.kind, SuggestionKind::Object { .. });
        (object && !view.objects_open) || (!object && view.mode == ModeId::OBJECTS)
    };
    view.shown = (shown.filter(|s| !folded(s))).and_then(|s| kept.iter().position(|k| *k == s));
    view.suggestions = Some(kept);
}

/// The open Suggestions list: blocks, then the objects folded into one row.
/// The Objects page lists only objects.
pub(crate) fn spawn_suggestions(
    world: &mut World,
    parent: Entity,
    root: Entity,
    tileset: &Tileset,
    view: &DockView,
) {
    let Some(found) = &view.suggestions else {
        return;
    };
    let heading = spawn_row(world, parent, tokens::SPACING_SM);
    spawn_heading(world, heading, "Suggestions");
    let grow = Node {
        flex_grow: 1.0,
        ..default()
    };
    world.spawn((grow, ChildOf(heading)));
    world.spawn((
        button(ButtonProps::new("Close").with_variant(ButtonVariant::Ghost)),
        DockControl {
            root,
            action: DockAction::SuggestionsClose,
        },
        ChildOf(heading),
    ));
    let (objects, blocks): (Vec<_>, Vec<_>) = (found.iter().enumerate())
        .partition(|(_, s)| matches!(s.kind, SuggestionKind::Object { .. }));
    if view.mode != ModeId::OBJECTS {
        for (i, s) in &blocks {
            spawn_suggestion(world, parent, root, (tileset, s), *i);
        }
    }
    if !objects.is_empty() {
        let row = spawn_row(world, parent, tokens::SPACING_XS);
        let line = spawn_text(
            world,
            row,
            format!("{} objects", objects.len()),
            tokens::TEXT_PRIMARY,
        );
        world.entity_mut(line).insert(Node {
            flex_grow: 1.0,
            ..default()
        });
        let (icon, tip) = if view.objects_open {
            (Icon::ChevronDown, "Hide objects")
        } else {
            (Icon::ChevronRight, "Show objects")
        };
        spawn_icon_button(world, row, root, icon, tip, DockAction::SuggestionObjects);
    }
    if view.objects_open {
        for (i, s) in &objects {
            spawn_suggestion(world, parent, root, (tileset, s), *i);
        }
    }
    if found.is_empty() {
        let row = spawn_row(world, parent, tokens::SPACING_SM);
        spawn_text(
            world,
            row,
            "Nothing left to suggest",
            tokens::TEXT_SECONDARY,
        );
    }
}

/// One row: a line that never wraps, its place in the tooltip, then Show,
/// Lay out or Add object, and Dismiss.
fn spawn_suggestion(
    world: &mut World,
    parent: Entity,
    root: Entity,
    (tileset, s): (&Tileset, &Suggestion),
    i: usize,
) {
    let row = spawn_row(world, parent, tokens::SPACING_XS);
    world
        .entity_mut(row)
        .insert((Tooltip::title(place(s)), Hovered::default()));
    let line = spawn_text(world, row, describe(tileset, s), tokens::TEXT_PRIMARY);
    world.entity_mut(line).insert((
        SuggestionLabel,
        TextLayout::default().with_no_wrap(),
        Node {
            flex_grow: 1.0,
            min_width: px(0.0),
            overflow: Overflow::clip_x(),
            ..default()
        },
    ));
    spawn_icon_button(
        world,
        row,
        root,
        Icon::Eye,
        "Show",
        DockAction::SuggestionShow(i),
    );
    let (icon, tip, act) = match s.kind {
        SuggestionKind::Object { .. } => (
            Icon::PackagePlus,
            "Add object",
            DockAction::SuggestionAddObject(i),
        ),
        SuggestionKind::Block { .. } => (
            Icon::LayoutTemplate,
            "Lay out",
            DockAction::SuggestionLayOut(i),
        ),
    };
    spawn_icon_button(world, row, root, icon, tip, act);
    spawn_icon_button(
        world,
        row,
        root,
        Icon::X,
        "Dismiss",
        DockAction::SuggestionDismiss(i),
    );
}

/// The outline of the shown suggestion, when it is on sheet `index`.
pub(crate) fn spawn_outline(
    world: &mut World,
    frame: Entity,
    root: Entity,
    tileset: &Tileset,
    index: u32,
    view: &DockView,
) {
    let Some(s) = (view.shown).and_then(|i| view.suggestions.as_ref()?.get(i)) else {
        return;
    };
    if tileset.setup_sheets.get(s.sheet) != Some(&index) {
        return;
    }
    let Some((min, span)) = block_rect(tileset, index, s.origin, s.size) else {
        return;
    };
    world.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(min.x),
            top: px(min.y),
            width: px(span.x),
            height: px(span.y),
            border: UiRect::all(px(2.0)),
            ..default()
        },
        BorderColor::all(tokens::SELECTED_BORDER),
        SuggestionOutline {
            root,
            origin: s.origin,
            size: s.size,
        },
        Pickable::IGNORE,
        ChildOf(frame),
    ));
}

/// Handle a suggestion control of the dock; false for any other control.
pub(crate) fn act(world: &mut World, control: DockControl) -> bool {
    let root = control.root;
    let index = match control.action {
        DockAction::SuggestionsClose => {
            edit_list(world, root, |view| {
                (view.suggestions, view.shown) = (None, None);
            });
            return true;
        }
        DockAction::SuggestionObjects => {
            edit_list(world, root, |view| view.objects_open = !view.objects_open);
            return true;
        }
        DockAction::SuggestionShow(i)
        | DockAction::SuggestionDismiss(i)
        | DockAction::SuggestionLayOut(i)
        | DockAction::SuggestionAddObject(i) => i,
        _ => return false,
    };
    take_keyboard(world, root);
    let Some((path, tileset)) = dock_tileset(world, root) else {
        return true;
    };
    let views = world.resource::<TilesetDockViews>();
    let Some(s) = (views.0.get(&path))
        .and_then(|v| v.suggestions.as_ref()?.get(index))
        .cloned()
    else {
        return true;
    };
    let Some(sheet) = tileset.setup_sheets.get(s.sheet).copied() else {
        return true;
    };
    match (control.action, &s.kind) {
        (DockAction::SuggestionShow(_), _) => {
            edit_list(world, root, |view| view.shown = Some(index));
            scroll_to(world, root, &tileset, sheet, &s);
        }
        (DockAction::SuggestionDismiss(_), _) => {
            (world.resource_mut::<DismissedSuggestions>().0)
                .entry(path.clone())
                .or_default()
                .insert(key(&s));
            edit_list(world, root, |view| {
                if let Some(list) = &mut view.suggestions {
                    list.remove(index);
                }
                view.shown = view.shown.and_then(|at| match at.cmp(&index) {
                    std::cmp::Ordering::Less => Some(at),
                    std::cmp::Ordering::Equal => None,
                    std::cmp::Ordering::Greater => Some(at - 1),
                });
            });
        }
        (
            DockAction::SuggestionLayOut(_),
            SuggestionKind::Block {
                layouts,
                inner,
                outer,
            },
        ) => {
            let pick = layouts.first().map(|l| (l.template, l.placement));
            let block = (sheet, s.origin, s.size);
            let guess = Some((pick, (*inner, *outer)));
            open_block(world, root, (&path, &tileset), block, guess);
            scroll_to(world, root, &tileset, sheet, &s);
        }
        (DockAction::SuggestionAddObject(_), SuggestionKind::Object { .. }) => {
            let params = vec![
                ("sheet", sheet.to_string()),
                ("origin", format!("{},{}", s.origin.x, s.origin.y)),
                ("size", format!("{},{}", s.size.x, s.size.y)),
            ];
            let call = Some(("tileset.object.add", params));
            dispatch(world, root, &path, &tileset, call);
        }
        _ => {}
    }
    true
}

/// The tileset path of the dock `root`.
fn dock_of(world: &World, root: Entity) -> Option<String> {
    world.get::<TilesetDock>(root)?.path.clone()
}

/// Change the suggestion state of the view of `root` and build it again.
fn edit_list(world: &mut World, root: Entity, edit: impl FnOnce(&mut DockView)) {
    let Some(path) = dock_of(world, root) else {
        return;
    };
    if let Some(view) = world.resource_mut::<TilesetDockViews>().0.get_mut(&path) {
        edit(view);
    }
    refill(world, &path);
}

/// Scroll the sheet pane of `root` so the suggestion's block is in view,
/// a tile in from its top-left corner.
fn scroll_to(world: &mut World, root: Entity, tileset: &Tileset, sheet: u32, s: &Suggestion) {
    let Some((corner, _)) = block_rect(tileset, sheet, s.origin, s.size) else {
        return;
    };
    let mut above = 0.0;
    for index in tileset.setup_sheets.iter().take_while(|i| **i != sheet) {
        let source = tileset
            .sheets
            .get(*index as usize)
            .map(|s| s.source.clone());
        let pixels = (source.and_then(|source| sheet_image(world, &source)))
            .and_then(|h| world.resource::<Assets<Image>>().get(&h).map(Image::size))
            .map_or(Vec2::ZERO, UVec2::as_vec2);
        above += frame_size(tileset, *index, pixels).y + tokens::SPACING_MD;
    }
    let margin = tileset.tile_size.as_vec2() * zoom(tileset.tile_size.as_vec2());
    let target = (corner + Vec2::new(0.0, above) - margin).max(Vec2::ZERO);
    let mut panes = world.query::<(&DockPane, &mut ScrollPosition)>();
    for (pane, mut scroll) in panes.iter_mut(world) {
        if pane.root == root && pane.pane == Pane::Sheet {
            scroll.0 = target;
        }
    }
}
