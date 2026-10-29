//! The Asset Browser over tilesets: a tileset opens in its dock tab, an image
//! imports as one from its context menu, and the packed copy's array texture
//! is pictured as a plain sheet.

use crate::tileset_edit_ops::{FILE, farm};
use crate::tileset_ops::{SHEET, asset_file, project_app};
use crate::util::OperatorResultExt as _;

use std::path::Path;

use bevy::prelude::*;
use bevy::render::render_resource::TextureDimension;
use jackdaw::definition_assets::OpenDefinition;
use jackdaw::project_window::{IMPORT_TILESET_ACTION, MenuTarget, context_items};
use jackdaw::texture_files::{AssetPreviewState, FlatSheets};
use jackdaw::tileset_dock::TILESET_WINDOW_ID;
use jackdaw_api::prelude::*;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_panels::tree::DockTree;
use jackdaw_scene_types::PropertyValue;
use jackdaw_widgets::context_menu::ContextMenuAction;
use jackdaw_widgets::file_browser::FileItemDoubleClicked;

const PACKED: &str = "tilemap_packed.tiles.png";

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn tileset_tabs(app: &App) -> Vec<String> {
    (app.world().resource::<DockTree>().tabs())
        .filter(|(_, t)| t.window_id == TILESET_WINDOW_ID)
        .filter_map(|(_, t)| t.payload.clone())
        .collect()
}

#[track_caller]
fn opens_the_dock_only(app: &App) {
    assert_eq!(tileset_tabs(app), [FILE.to_string()]);
    assert_eq!(
        app.world().resource::<OpenDefinition>().0,
        None,
        "no generic definition card is opened"
    );
}

#[test]
fn double_clicking_a_tileset_opens_its_dock_tab() {
    let (mut app, dir) = farm();
    let path = asset_file(&dir, FILE).to_string_lossy().into_owned();
    app.world_mut().trigger(FileItemDoubleClicked {
        path,
        is_directory: false,
    });
    settle(&mut app);
    opens_the_dock_only(&app);
}

#[test]
fn project_open_on_a_tileset_opens_the_dock() {
    let (mut app, _dir) = farm();
    app.world_mut()
        .operator("project.open")
        .param("path", PropertyValue::from(FILE))
        .call()
        .expect("dispatches")
        .assert_finished();
    settle(&mut app);
    opens_the_dock_only(&app);
}

fn offers(path: &Path) -> bool {
    (context_items(path, false).iter()).any(|(action, _)| *action == IMPORT_TILESET_ACTION)
}

#[test]
fn array_textures_and_ktx2_do_not_offer_import_as_tileset() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sheet = dir.path().join("sheet.png");
    std::fs::write(&sheet, b"").expect("sheet");
    assert!(offers(&sheet), "a plain png is offered");
    let meta = jackdaw::image_sampling::tile_array_meta(None, UVec2::splat(16)).expect("meta");
    std::fs::write(jackdaw::texture_import::meta_path(&sheet), meta).expect("meta written");
    assert!(!offers(&sheet), "not once its meta loads it as an array");
    assert!(
        !offers(&dir.path().join("farm.tiles.png")),
        "nor a packed copy"
    );
    assert!(!offers(&dir.path().join("sky.ktx2")), "nor ktx2");
}

#[test]
fn only_images_offer_import_as_tileset() {
    assert!(offers(Path::new("sheets/farm.png")));
    assert!(offers(Path::new("sheets/farm.PNG")));
    assert!(!offers(Path::new("farm.tileset.bsn")));
    assert!(!offers(Path::new("town.bsn")));
    let in_folder = (context_items(Path::new("sheets"), true).iter())
        .any(|(action, _)| *action == IMPORT_TILESET_ACTION);
    assert!(!in_folder);
}

#[test]
fn import_as_tileset_imports_the_image_and_opens_it() {
    let (mut app, dir) = project_app();
    app.world_mut().insert_resource(MenuTarget {
        path: asset_file(&dir, SHEET),
    });
    app.world_mut().trigger(ContextMenuAction {
        action: IMPORT_TILESET_ACTION.to_string(),
        target_entity: None,
    });
    settle(&mut app);
    assert!(asset_file(&dir, FILE).is_file(), "the tileset is written");
    assert!(asset_file(&dir, PACKED).is_file(), "with its packed copy");
    let tileset = crate::tileset_ops::read_tileset(&asset_file(&dir, FILE));
    assert!(!tileset.terrains.is_empty(), "set up automatically");
    opens_the_dock_only(&app);
}

#[test]
fn import_as_tileset_takes_a_path_with_a_comma_whole() {
    let (mut app, dir) = project_app();
    let named = "grass, dirt.png";
    std::fs::copy(asset_file(&dir, SHEET), asset_file(&dir, named)).expect("copy");
    app.world_mut().insert_resource(MenuTarget {
        path: asset_file(&dir, named),
    });
    app.world_mut().trigger(ContextMenuAction {
        action: IMPORT_TILESET_ACTION.to_string(),
        target_entity: None,
    });
    settle(&mut app);
    assert!(asset_file(&dir, "grass, dirt.tileset.bsn").is_file());
    assert_eq!(tileset_tabs(&app), ["grass, dirt.tileset.bsn".to_string()]);
}

#[test]
fn opening_a_packed_copy_opens_the_tileset_that_owns_it() {
    let (mut app, _dir) = farm();
    app.world_mut()
        .operator("project.open")
        .param("path", PropertyValue::from(PACKED))
        .call()
        .expect("dispatches")
        .assert_finished();
    settle(&mut app);
    opens_the_dock_only(&app);
}

fn plain_2d(app: &App, handle: &Handle<Image>) -> bool {
    app.world()
        .resource::<Assets<Image>>()
        .get(handle)
        .is_some_and(|image| {
            image.texture_descriptor.dimension == TextureDimension::D2
                && image.texture_descriptor.size.depth_or_array_layers == 1
        })
}

#[test]
fn the_packed_copy_and_its_tileset_are_pictured_as_a_plain_sheet() {
    let (mut app, dir) = farm();
    settle(&mut app);

    let packed = asset_file(&dir, PACKED);
    let sheet = (app.world().resource::<FlatSheets>().0.iter())
        .find(|(path, _)| path.ends_with(PACKED))
        .and_then(|(_, sheet)| sheet.handle.clone())
        .expect("the packed copy has a flat sheet");
    assert!(plain_2d(&app, &sheet), "the sheet is one 2D layer");
    let size = app
        .world()
        .resource::<Assets<Image>>()
        .get(&sheet)
        .map(Image::size);
    assert_eq!(size, Some(UVec2::new(192, 176)), "the whole packed sheet");
    let mut nodes = app.world_mut().query::<&ImageNode>();
    let pictured = (nodes.iter(app.world()))
        .filter(|node| node.image == sheet)
        .count();
    assert_eq!(pictured, 2, "the packed copy's tile and the tileset's tile");
    let tileset_tile = (app
        .world_mut()
        .query::<(&Tooltip, &Children)>()
        .iter(app.world()))
    .filter(|(tooltip, _)| tooltip.title == "Tileset")
    .any(|(_, children)| {
        children.iter().any(|child| {
            app.world()
                .get::<ImageNode>(child)
                .is_some_and(|node| node.image == sheet)
        })
    });
    assert!(
        tileset_tile,
        "the pictured tileset tile keeps its kind tooltip"
    );

    app.world_mut()
        .operator("project.select")
        .param(
            "path",
            PropertyValue::from(packed.to_string_lossy().into_owned()),
        )
        .call()
        .expect("dispatches")
        .assert_finished();
    settle(&mut app);
    let preview = app.world().resource::<AssetPreviewState>();
    let info = preview.selected_info.clone().expect("the preview shows it");
    assert!(info.is_array, "read as an array texture");
    assert_eq!(info.layer_count, 12 * 11);
    assert_eq!(info.image_handle.as_ref(), Some(&sheet), "pictured flat");
    assert!(preview.layer_images.is_empty(), "not cut into strips");

    std::fs::remove_file(&packed).expect("the packed copy goes");
    app.world_mut()
        .operator("project.filter")
        .param("kind", PropertyValue::from("all"))
        .call()
        .expect("dispatches")
        .assert_finished();
    settle(&mut app);
    let held = app.world().resource::<FlatSheets>().0.contains_key(&packed);
    assert!(!held, "no copy is held of a file that is gone");
}
