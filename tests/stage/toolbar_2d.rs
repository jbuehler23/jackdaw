//! The 2D viewport toolbar: the object tools and Edit Tiles, the tile tools
//! that replace them in tile edit mode, and the Snap and View menus.

use crate::tile_palette::{brush, packed, tile_map};
use crate::tilemap_render::{grass_patch, ground_terrain, project_app, settle, spawn_map};

use crate::util::OperatorResultExt as _;
use bevy::feathers::controls::ButtonVariant;
use bevy::prelude::*;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use jackdaw::selection::Selection;
use jackdaw::tile_tools::{BrushSource, TileBrush, TileEditMode, TileTool};
use jackdaw::toolbar_2d::Toolbar2dGroup;
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_scene_types::PropertyValue;

const TILE_TOOLS: [(&str, TileTool); 7] = [
    ("tilemap.brush", TileTool::Brush),
    ("tilemap.eraser", TileTool::Eraser),
    ("tilemap.rectangle", TileTool::Rectangle),
    ("tilemap.line", TileTool::Line),
    ("tilemap.bucket", TileTool::Bucket),
    ("tilemap.picker", TileTool::Picker),
    ("tilemap.select", TileTool::Select),
];

const SELECTION_OPS: [&str; 3] = [
    "tilemap.selection.copy",
    "tilemap.selection.cut",
    "tilemap.selection.erase",
];

fn group(app: &mut App, which: Toolbar2dGroup) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &Toolbar2dGroup)>()
        .iter(app.world())
        .filter(|(_, group)| **group == which)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(found.len(), 1, "one {which:?} group in the panel");
    found[0]
}

fn shown(app: &mut App, which: Toolbar2dGroup) -> bool {
    let entity = group(app, which);
    app.world()
        .get::<Node>(entity)
        .is_some_and(|node| node.display != Display::None)
}

fn ids(app: &mut App, which: Toolbar2dGroup) -> Vec<String> {
    let entity = group(app, which);
    let children: Vec<Entity> = app
        .world()
        .get::<Children>(entity)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    children
        .into_iter()
        .filter_map(|child| app.world().get::<ButtonOperatorCall>(child))
        .map(|call| call.id.to_string())
        .collect()
}

fn button(app: &mut App, id: &str) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &ButtonOperatorCall, &ChildOf)>()
        .iter(app.world())
        .filter(|(_, call, _)| call.id == id)
        .filter(|(_, _, parent)| app.world().get::<Toolbar2dGroup>(parent.parent()).is_some())
        .map(|(entity, ..)| entity)
        .collect();
    assert_eq!(found.len(), 1, "one {id} button in the 2D toolbar");
    found[0]
}

fn disabled(app: &mut App, id: &str) -> bool {
    let entity = button(app, id);
    app.world().get::<InteractionDisabled>(entity).is_some()
}

fn highlighted(app: &mut App, id: &str) -> bool {
    let entity = button(app, id);
    app.world().get::<ButtonVariant>(entity) == Some(&ButtonVariant::Primary)
}

fn click(app: &mut App, id: &str) {
    let entity = button(app, id);
    app.world_mut().trigger(Activate { entity });
    settle(app);
}

fn mode(app: &App) -> TileEditMode {
    *app.world().resource::<TileEditMode>()
}

#[test]
fn the_2d_toolbar_carries_the_object_tools_and_edit_tiles() {
    let (mut app, _dir) = project_app();
    assert_eq!(
        ids(&mut app, Toolbar2dGroup::Object),
        vec![
            "tool.select",
            "tool.translate",
            "tool.rotate",
            "tool.scale",
            "tilemap.edit_mode"
        ],
    );
    assert!(shown(&mut app, Toolbar2dGroup::Object));
    assert!(!shown(&mut app, Toolbar2dGroup::Tiles));
}

#[test]
fn edit_tiles_waits_for_a_tilemap_and_enters_tile_edit_mode() {
    let (mut app, _dir) = project_app();
    assert!(
        disabled(&mut app, "tilemap.edit_mode"),
        "nothing to edit without a tilemap"
    );
    let edit = button(&mut app, "tilemap.edit_mode");
    let reason = app
        .world()
        .get::<Tooltip>(edit)
        .map(|tooltip| tooltip.description.clone());
    assert_eq!(
        reason.as_deref(),
        Some("Select a tilemap to edit its tiles")
    );

    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    app.world_mut().resource_mut::<Selection>().entities = vec![map];
    settle(&mut app);
    assert!(!disabled(&mut app, "tilemap.edit_mode"));

    click(&mut app, "tilemap.edit_mode");
    assert_eq!(mode(&app).map, Some(map));
}

#[test]
fn tile_edit_mode_swaps_the_tool_group_and_leaving_restores_it() {
    let (mut app, _dir) = project_app();
    let map = tile_map(&mut app, &[]);
    assert_eq!(mode(&app).map, Some(map));
    assert!(!shown(&mut app, Toolbar2dGroup::Object));
    assert!(shown(&mut app, Toolbar2dGroup::Tiles));
    assert_eq!(
        ids(&mut app, Toolbar2dGroup::Tiles),
        vec![
            "tilemap.edit_mode.exit",
            "tilemap.brush",
            "tilemap.eraser",
            "tilemap.rectangle",
            "tilemap.line",
            "tilemap.bucket",
            "tilemap.picker",
            "tilemap.select",
            "tilemap.brush.flip_x",
            "tilemap.brush.flip_y",
            "tilemap.brush.rotate",
            "tilemap.selection.copy",
            "tilemap.selection.cut",
            "tilemap.stamp.paste",
            "tilemap.selection.erase",
        ],
    );

    click(&mut app, "tilemap.edit_mode.exit");
    assert_eq!(mode(&app).map, None);
    assert!(shown(&mut app, Toolbar2dGroup::Object));
    assert!(!shown(&mut app, Toolbar2dGroup::Tiles));

    tile_map(&mut app, &[]);
    app.world_mut().resource_mut::<Selection>().entities.clear();
    settle(&mut app);
    assert_eq!(mode(&app).map, None, "deselecting the map leaves the mode");
    assert!(
        shown(&mut app, Toolbar2dGroup::Object),
        "and the object tools come back without an operator call",
    );
}

#[test]
fn each_tile_tool_button_chooses_and_highlights_its_tool() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    for (id, tool) in TILE_TOOLS {
        click(&mut app, id);
        assert_eq!(mode(&app).tool, tool, "{id}");
        for (other, _) in TILE_TOOLS {
            assert_eq!(
                highlighted(&mut app, other),
                other == id,
                "{other} after clicking {id}"
            );
        }
    }
}

#[test]
fn flip_and_rotate_buttons_turn_the_brush_and_show_it() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    let plain = brush(&app).tile;
    assert!(!highlighted(&mut app, "tilemap.brush.flip_x"));

    click(&mut app, "tilemap.brush.flip_x");
    assert_eq!(brush(&app).tile, plain.with_flip_x(true));
    assert!(highlighted(&mut app, "tilemap.brush.flip_x"));
    assert!(!highlighted(&mut app, "tilemap.brush.flip_y"));

    click(&mut app, "tilemap.brush.flip_x");
    click(&mut app, "tilemap.brush.flip_y");
    assert_eq!(brush(&app).tile, plain.with_flip_y(true));
    assert!(highlighted(&mut app, "tilemap.brush.flip_y"));

    click(&mut app, "tilemap.brush.flip_y");
    click(&mut app, "tilemap.brush.rotate");
    assert!(brush(&app).tile.transpose());
    assert!(highlighted(&mut app, "tilemap.brush.rotate"));
}

#[test]
fn selection_buttons_wait_for_a_selection_and_a_stamp() {
    let (mut app, _dir) = project_app();
    let tile = packed(&app, 0, 3);
    tile_map(&mut app, &[(0, 0, tile), (1, 0, tile)]);
    for id in SELECTION_OPS.into_iter().chain(["tilemap.stamp.paste"]) {
        assert!(disabled(&mut app, id), "{id} without a selection");
        let entity = button(&mut app, id);
        let reason = app
            .world()
            .get::<Tooltip>(entity)
            .map(|tooltip| tooltip.description.clone())
            .unwrap_or_default();
        assert!(
            reason.contains("first"),
            "{id} says what it waits for: {reason}"
        );
    }

    let result = app
        .world_mut()
        .operator("tilemap.selection.set")
        .param("rect", PropertyValue::from("0,0,1,0".to_string()))
        .call()
        .expect("tilemap.selection.set dispatches");
    assert_eq!(result, OperatorResult::Finished);
    settle(&mut app);
    for id in SELECTION_OPS {
        assert!(!disabled(&mut app, id), "{id} with a selection");
    }
    assert!(
        disabled(&mut app, "tilemap.stamp.paste"),
        "nothing copied yet"
    );

    click(&mut app, "tilemap.selection.copy");
    assert!(!disabled(&mut app, "tilemap.stamp.paste"));
    click(&mut app, "tilemap.stamp.paste");
    assert_eq!(
        app.world().resource::<TileBrush>().source,
        BrushSource::Stamp
    );
}

#[test]
fn the_status_bar_names_the_tile_tool() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    click(&mut app, "tilemap.bucket");
    let text = app
        .world_mut()
        .query_filtered::<&Text, With<jackdaw_feathers::status_bar::StatusBarRight>>()
        .iter(app.world())
        .map(|text| text.0.clone())
        .next()
        .unwrap_or_default();
    assert!(text.starts_with("Bucket"), "status bar reads {text:?}");
}

fn menu_rows(app: &mut App, name: &str) -> Vec<(String, String)> {
    app.world_mut()
        .operator("menu.open")
        .param("name", name.to_string())
        .call()
        .expect("menu.open dispatches")
        .assert_finished();
    app.update();
    let rows = app
        .world_mut()
        .query::<&jackdaw_widgets::menu_bar::MenuBarItem>()
        .iter(app.world())
        .find(|item| item.label == name)
        .map(|item| item.actions.clone())
        .unwrap_or_else(|| panic!("the 2D toolbar carries a {name} menu"));
    app.world_mut()
        .resource_mut::<jackdaw_widgets::menu_bar::MenuBarState>()
        .open_menu = None;
    app.update();
    rows
}

fn labels(rows: &[(String, String)]) -> Vec<&str> {
    rows.iter()
        .map(|(_, label)| label.as_str())
        .filter(|label| !label.is_empty())
        .collect()
}

#[test]
fn snap_and_view_menus_split_snapping_from_display() {
    let (mut app, _scene) = crate::sprite_gizmo::world_app();
    let snap = menu_rows(&mut app, "Snap");
    let snap_labels = labels(&snap);
    for label in ["Use Snap", "Use Pixel Snap", "Grid"] {
        assert!(snap_labels.contains(&label), "Snap offers {label}");
    }
    for label in [
        "Show Rulers",
        "Show Grid",
        "Finer",
        "Zoom In",
        "Fit",
        "Linear",
    ] {
        assert!(!snap_labels.contains(&label), "Snap leaves {label} to View");
    }

    let view = menu_rows(&mut app, jackdaw::toolbar_2d::CANVAS_VIEW_MENU);
    let view_labels = labels(&view);
    for label in [
        "Finer",
        "Coarser",
        "Zoom In",
        "Zoom Out",
        "Actual Size",
        "Fit",
        "Show Rulers",
        "Show Guides",
        "Show Game Frame",
        "Show Grid",
        "Linear",
        "Nearest (Pixel Art)",
        "16",
        "Pixels",
        "World Units",
    ] {
        assert!(view_labels.contains(&label), "View offers {label}");
    }
    for (action, label) in &view {
        if ["Linear", "16", "Pixels"].contains(&label.as_str()) {
            assert!(
                action.starts_with("(o)") || action.starts_with("( )"),
                "{label} is a radio choice: {action}"
            );
        }
    }
}

fn span(app: &App, entity: Entity) -> (f32, f32) {
    let node = app.world().get::<ComputedNode>(entity).expect("laid out");
    let at = app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(entity)
        .expect("placed");
    let scale = node.inverse_scale_factor();
    let half = node.size().x * scale / 2.0;
    (
        at.translation.x * scale - half,
        at.translation.x * scale + half,
    )
}

fn descends_from(app: &App, entity: Entity, ancestor: Entity) -> bool {
    let mut cursor = entity;
    while let Some(parent) = app.world().get::<ChildOf>(cursor).map(ChildOf::parent) {
        if parent == ancestor {
            return true;
        }
        cursor = parent;
    }
    false
}

#[test]
fn a_default_width_viewport_shows_every_paint_tool() {
    use jackdaw::toolbar_overflow::ToolbarOverflow;
    use jackdaw_scene_types::{Tilemap, TilemapLayerKind};

    let width = 536.0 - jackdaw::viewport_2d::RULER_SIZE;
    let (mut app, scene) = crate::sprite_gizmo::world_app_sized(width);
    let layer = crate::tilemap_render::layer(TilemapLayerKind::Tile, default());
    let map = app
        .world_mut()
        .spawn(Tilemap {
            layers: vec![layer],
            ..default()
        })
        .id();
    app.world_mut().resource_mut::<Selection>().entities = vec![map];
    settle(&mut app);
    app.world_mut()
        .operator("tilemap.edit_mode")
        .call()
        .expect("tilemap.edit_mode dispatches")
        .assert_finished();
    for _ in 0..8 {
        app.update();
    }

    let buttons: Vec<(Entity, String)> = app
        .world_mut()
        .query::<(Entity, &ButtonOperatorCall)>()
        .iter(app.world())
        .map(|(entity, call)| (entity, call.id.to_string()))
        .collect();
    let in_panel = |app: &App, id: &str| -> Entity {
        buttons
            .iter()
            .find(|(entity, call)| call == id && descends_from(app, *entity, scene.panel))
            .map(|(entity, _)| *entity)
            .unwrap_or_else(|| panic!("the panel's toolbar has {id}"))
    };
    let hidden = |app: &App, id: &str| {
        app.world().get::<Visibility>(in_panel(app, id)) == Some(&Visibility::Hidden)
    };

    let mut shown_end = 0.0_f32;
    for id in std::iter::once("tilemap.edit_mode.exit").chain(TILE_TOOLS.map(|(id, _)| id)) {
        assert!(!hidden(&app, id), "{id} is shown at the default width");
        shown_end = shown_end.max(span(&app, in_panel(&app, id)).1);
    }
    let panel_end = span(&app, scene.panel).1;
    assert!(shown_end <= panel_end, "and inside the toolbar");

    let flips_cut = ["tilemap.brush.flip_x", "tilemap.brush.flip_y"]
        .iter()
        .any(|id| hidden(&app, id));
    if flips_cut {
        for id in SELECTION_OPS.into_iter().chain(["tilemap.stamp.paste"]) {
            assert!(hidden(&app, id), "the clipboard goes before the flips");
        }
    }

    let overflow = app
        .world_mut()
        .query::<(Entity, &ToolbarOverflow, &Node)>()
        .iter(app.world())
        .find(|(entity, _, node)| {
            node.display != Display::None && descends_from(&app, *entity, scene.panel)
        })
        .map(|(entity, ..)| entity);
    if let Some(overflow) = overflow {
        let last_shown = buttons
            .iter()
            .filter(|(entity, _)| descends_from(&app, *entity, scene.panel))
            .filter(|(entity, _)| {
                app.world().get::<Visibility>(*entity) != Some(&Visibility::Hidden)
            })
            .map(|(entity, _)| span(&app, *entity).1)
            .filter(|right| *right <= span(&app, overflow).0)
            .fold(0.0, f32::max);
        let gap = span(&app, overflow).0 - last_shown;
        assert!(
            gap <= jackdaw_feathers::tokens::TOOLBAR_GAP + 1.0,
            "the overflow button sits right after the last shown tool: {gap}"
        );
    }
}

/// The rows the menu item `item` would open with, read from the item itself.
fn rows_of(app: &App, item: Entity) -> Vec<(String, String)> {
    let rows = app
        .world()
        .get::<jackdaw_feathers::menu_bar::DynamicMenuRows>(item)
        .expect("a menu built from the world");
    (rows.0)(app.world())
}

fn panel_overflow(app: &mut App, panel: Entity) -> Entity {
    use jackdaw::toolbar_overflow::ToolbarOverflow;
    let found: Vec<(Entity, Entity)> = app
        .world_mut()
        .query::<(Entity, &ToolbarOverflow)>()
        .iter(app.world())
        .map(|(entity, overflow)| (entity, overflow.tools))
        .collect();
    found
        .into_iter()
        .find(|(entity, tools)| {
            descends_from(app, *entity, panel)
                && app.world().get::<Toolbar2dGroup>(*tools) == Some(&Toolbar2dGroup::Object)
        })
        .map(|(entity, _)| entity)
        .expect("the panel's object tools have an overflow menu")
}

#[test]
fn a_toolbar_too_narrow_for_any_tool_stays_cut_and_lists_every_tool() {
    let (mut app, scene) = crate::sprite_gizmo::world_app_sized(60.0);
    let overflow = panel_overflow(&mut app, scene.panel);
    let tools: Vec<String> = [
        "tool.select",
        "tool.translate",
        "tool.rotate",
        "tool.scale",
        "tilemap.edit_mode",
    ]
    .map(|id| format!("op:{id}"))
    .to_vec();
    for frame in 0..6 {
        app.update();
        assert_eq!(
            app.world().get::<Node>(overflow).map(|node| node.display),
            Some(Display::Flex),
            "the overflow menu stays up on frame {frame}"
        );
        let listed: Vec<String> = rows_of(&app, overflow)
            .into_iter()
            .map(|(action, _)| action)
            .collect();
        assert_eq!(listed, tools, "and lists every tool on frame {frame}");
    }
}

#[test]
fn opening_the_canvas_menu_by_name_picks_the_focused_panel() {
    let (mut app, scene) = crate::sprite_gizmo::world_app();
    let second = crate::sprite_gizmo::add_panel(&mut app, 600.0);
    settle(&mut app);
    for panel in [scene.panel, second] {
        app.world_mut()
            .resource_mut::<jackdaw::viewport::ActiveViewport>()
            .host = Some(panel);
        app.world_mut()
            .operator("menu.open")
            .param("name", jackdaw::toolbar_2d::CANVAS_VIEW_MENU)
            .call()
            .expect("menu.open dispatches")
            .assert_finished();
        let open = app
            .world()
            .resource::<jackdaw_widgets::menu_bar::MenuBarState>()
            .open_menu
            .expect("a menu opens");
        assert!(
            descends_from(&app, open, panel),
            "the canvas view menu of the panel in focus opens"
        );
        app.world_mut()
            .resource_mut::<jackdaw_widgets::menu_bar::MenuBarState>()
            .open_menu = None;
    }
}

#[test]
fn the_fit_row_frames_only_its_own_panel() {
    use jackdaw::viewport_2d::Viewport2dPanelHost;

    let (mut app, scene) = crate::sprite_gizmo::world_app();
    let second = crate::sprite_gizmo::add_panel(&mut app, 600.0);
    settle(&mut app);
    for panel in [scene.panel, second] {
        app.world_mut()
            .get_mut::<Viewport2dPanelHost>(panel)
            .expect("a 2D panel")
            .fit_pending = false;
    }
    let fit = jackdaw::viewport_2d::view_menu_rows(app.world(), second)
        .into_iter()
        .find(|(_, label)| label == "Fit")
        .map(|(action, _)| action)
        .expect("the canvas view menu fits the view");
    let (area, camera) = app
        .world()
        .get::<Viewport2dPanelHost>(second)
        .map(|host| (host.area, host.camera))
        .expect("a 2D panel");
    app.world_mut()
        .resource_mut::<bevy::picking::hover::HoverMap>()
        .insert(
            bevy::picking::pointer::PointerId::Mouse,
            [(
                area,
                bevy::picking::backend::HitData::new(camera, 0.0, None, None),
            )]
            .into_iter()
            .collect(),
        );
    let call = ButtonOperatorCall::try_from(fit.as_str()).expect("the row calls an operator");
    let mut dispatch = app.world_mut().operator(call.id.to_string());
    for (key, value) in &call.params {
        dispatch = dispatch.param(key.to_string(), value.clone());
    }
    dispatch
        .call()
        .expect("the row dispatches")
        .assert_finished();
    let pending = |app: &App, panel: Entity| {
        app.world()
            .get::<Viewport2dPanelHost>(panel)
            .is_some_and(|host| host.fit_pending)
    };
    assert!(pending(&app, second), "the row's own panel is framed");
    assert!(
        !pending(&app, scene.panel),
        "and the other panel is left alone"
    );
}

#[test]
fn a_running_tool_is_the_reason_the_selection_buttons_wait() {
    let (mut app, _dir) = project_app();
    let tile = packed(&app, 0, 3);
    tile_map(&mut app, &[(0, 0, tile), (1, 0, tile)]);
    app.world_mut()
        .operator("tilemap.selection.set")
        .param("rect", PropertyValue::from("0,0,1,0".to_string()))
        .call()
        .expect("tilemap.selection.set dispatches")
        .assert_finished();
    settle(&mut app);
    assert!(!disabled(&mut app, "tilemap.selection.copy"));

    crate::tile_tools::hold(&mut app, "tilemap.brush", MouseButton::Left, &[(3, 3)]);
    settle(&mut app);
    assert!(disabled(&mut app, "tilemap.selection.copy"));
    let entity = button(&mut app, "tilemap.selection.copy");
    let reason = app
        .world()
        .get::<Tooltip>(entity)
        .map(|tooltip| tooltip.description.clone())
        .unwrap_or_default();
    assert_eq!(reason, "Finish the running tool first");
}
