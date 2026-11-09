//! Modes of the Tileset window: an extension registers its own mode, which
//! the mode bar lists and `tileset.mode` switches to.

use crate::operators::tileset_dock::{mode_bar, opened, press, settle, texts, view};
use crate::operators::tileset_edit_ops::call;
use crate::operators::tileset_ops::{SHEET, import, project_app_with, warnings};

use std::collections::HashMap;

use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::{
    Extension, ExtensionAppExt as _, enable_extension, unload_extension,
};
use jackdaw_editor_tile::tileset_dock::{DockAction, spawn_text};
use jackdaw_editor_tile::tileset_modes::{
    ModeId, ModePanel, ModeSheet, TilesetMode, TilesetModeAppExt as _, TilesetModeExtensionExt as _,
};
use jackdaw_editor_tile::zone_overlay::{ZoneLayout, ZoneOverlay, ZoneValues, spawn_zone_overlay};
use jackdaw_scene_types::PropertyValue;

const MARKS: ModeId = ModeId("marks");

fn marks_panel(world: &mut World, panel: &ModePanel) {
    spawn_text(world, panel.list, "Marked tiles", Color::WHITE);
}

fn marks_overlay(world: &mut World, sheet: &ModeSheet) {
    let values = ZoneValues {
        layout: ZoneLayout::Whole,
        tiles: HashMap::from([(3, [0, 0, 0, 0, 1, 0, 0, 0, 0])]),
        picked: Some(1),
        colors: HashMap::from([(1, Color::WHITE)]),
        dim: true,
        hover: Some(Color::WHITE),
        outlines: HashMap::new(),
    };
    spawn_zone_overlay(
        world,
        sheet.frame,
        sheet.root,
        (sheet.tileset, sheet.sheet),
        values,
    );
}

fn marks() -> TilesetMode {
    TilesetMode {
        id: MARKS,
        label: "Marks",
        order: 50,
        panel: marks_panel,
        overlay: Some(marks_overlay),
    }
}

fn marks_mode(app: &mut App) {
    app.register_tileset_mode(marks());
}

#[derive(Default)]
struct MarksExtension;

impl JackdawExtension for MarksExtension {
    fn id(&self) -> String {
        "test.tileset_marks".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_tileset_mode(marks());
    }
}

fn overlaid(app: &mut App, root: Entity) -> Vec<u32> {
    (app.world_mut().query::<&ZoneOverlay>().iter(app.world()))
        .filter(|o| o.root == root)
        .flat_map(|o| o.values.zones().map(|(tile, ..)| tile))
        .collect()
}

#[test]
fn a_mode_registered_by_a_plugin_shows_in_the_bar_and_tileset_mode_selects_it() {
    let (mut app, _dir) = project_app_with(|app| {
        app.add_plugins(marks_mode);
    });
    import(&mut app, &[("paths", SHEET)]);
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    settle(&mut app);
    let (_, root) = opened(&mut app);
    assert_eq!(
        mode_bar(&mut app, root),
        [
            ModeId::TILES,
            ModeId::TERRAINS,
            ModeId::COLLISION,
            MARKS,
            ModeId::OBJECTS
        ],
        "listed by order"
    );
    assert!(overlaid(&mut app, root).is_empty());

    call(&mut app, "tileset.mode", &[("mode", "Marks")]);
    settle(&mut app);
    assert_eq!(view(&app).mode, MARKS);
    let shown = texts(&mut app, root);
    assert!(shown.iter().any(|t| t == "Marked tiles"), "{shown:?}");
    assert_eq!(overlaid(&mut app, root), [3], "its overlay draws");

    press(&mut app, root, DockAction::Mode(ModeId::TILES));
    assert_eq!(view(&app).mode, ModeId::TILES);
    press(&mut app, root, DockAction::Mode(MARKS));
    assert_eq!(view(&app).mode, MARKS, "its button calls tileset.mode");

    call(&mut app, "tileset.mode", &[]);
    settle(&mut app);
    assert_eq!(
        view(&app).mode,
        ModeId::OBJECTS,
        "no mode steps to the next"
    );
}

#[test]
fn an_unknown_mode_is_refused_and_keeps_the_mode() {
    let (mut app, _dir) = project_app_with(|_| {});
    import(&mut app, &[("paths", SHEET)]);
    settle(&mut app);
    opened(&mut app);
    call(&mut app, "tileset.mode", &[("mode", "painting")]);
    settle(&mut app);
    assert_eq!(view(&app).mode, ModeId::TILES);
    let said = warnings(&app).join("\n");
    assert!(
        said.contains("tiles, terrains, collision, objects"),
        "{said}"
    );
}

#[test]
fn an_extensions_mode_goes_when_it_unloads_and_its_tab_falls_back() {
    let (mut app, _dir) = project_app_with(|app| {
        app.register_extension::<MarksExtension>();
    });
    let id = "test.tileset_marks";
    let extension = enable_extension(app.world_mut(), id).unwrap_or_else(|| {
        (app.world_mut()
            .query::<(Entity, &Extension)>()
            .iter(app.world()))
        .find(|(_, e)| e.id == id)
        .map(|(e, _)| e)
        .expect("the extension is loaded")
    });
    import(&mut app, &[("paths", SHEET)]);
    settle(&mut app);
    let (_, root) = opened(&mut app);
    call(&mut app, "tileset.mode", &[("mode", "marks")]);
    settle(&mut app);
    assert_eq!(view(&app).mode, MARKS);
    assert!(mode_bar(&mut app, root).contains(&MARKS));

    unload_extension(app.world_mut(), extension);
    settle(&mut app);
    assert_eq!(
        mode_bar(&mut app, root),
        [
            ModeId::TILES,
            ModeId::TERRAINS,
            ModeId::COLLISION,
            ModeId::OBJECTS
        ]
    );
    assert_eq!(view(&app).mode, ModeId::TILES, "the tab falls back");
}

#[test]
fn tileset_mode_without_a_path_switches_the_tab_in_front() {
    let (mut app, _dir) = project_app_with(|_| {});
    import(&mut app, &[("paths", SHEET)]);
    settle(&mut app);
    opened(&mut app);
    app.world_mut()
        .resource_mut::<jackdaw_editor_core::definition_assets::OpenDefinition>()
        .0 = None;
    app.world_mut()
        .resource_mut::<bevy::input_focus::InputFocus>()
        .clear();
    app.world_mut()
        .operator("tileset.mode")
        .param("mode", PropertyValue::from("Terrains".to_string()))
        .call()
        .expect("dispatches")
        .assert_finished();
    settle(&mut app);
    let said = warnings(&app);
    assert!(!said.iter().any(|w| w.contains("tileset.mode")), "{said:?}");
    assert_eq!(view(&app).mode, ModeId::TERRAINS);
}
