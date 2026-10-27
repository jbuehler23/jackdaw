//! The shared field commit: an edit that leaves the document as it was records
//! nothing but still puts the document's value back on a drifted component,
//! and a registered entity's asset fields name their files in the document.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw_api::prelude::*;
use path_slash::PathExt as _;

const SPRITE: &str = "bevy_sprite::sprite::Sprite";
const IMAGE_NODE: &str = "bevy_ui::widget::image::ImageNode";
const SHEET: &str = "tilemap_packed.png";

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn set_field(app: &mut App, entity: Entity, type_path: &str, field: &str, value: &str) {
    app.world_mut()
        .operator("component.set")
        .param("entity", entity)
        .param("type_path", type_path.to_owned())
        .param("field", field.to_owned())
        .param("value", value.to_owned())
        .call()
        .expect("component.set dispatches")
        .assert_finished();
    app.update();
}

#[test]
fn an_unchanged_field_write_puts_the_document_value_back_on_a_drifted_component() {
    let mut app = util::editor_test_app();
    let entity = app
        .world_mut()
        .spawn((Name::new("tile"), Sprite::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    set_field(&mut app, entity, SPRITE, "flip_x", "true");
    let depth = history_len(&app);
    app.world_mut()
        .get_mut::<Sprite>(entity)
        .expect("sprite")
        .flip_x = false;
    set_field(&mut app, entity, SPRITE, "flip_x", "true");
    assert_eq!(history_len(&app), depth, "nothing new to undo");
    assert_eq!(
        app.world()
            .get::<Sprite>(entity)
            .map(|sprite| sprite.flip_x),
        Some(true),
        "the live component follows the document again"
    );
}

fn image_path(handle: &Handle<Image>) -> Option<String> {
    handle
        .path()
        .map(|path| path.path().to_slash_lossy().into_owned())
}

#[test]
fn a_registered_entity_records_its_image_paths_and_keeps_them_through_an_edit() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/crates/jackdaw_tilemap/tests/data/tiny_farm/tilemap_packed.png"
        ),
        dir.path().join(SHEET),
    )
    .expect("copy the sheet");
    let mut app = util::editor_test_app_reading(dir.path());
    let image = app.world().resource::<AssetServer>().load::<Image>(SHEET);
    let sprite = app
        .world_mut()
        .spawn((Name::new("direct"), Sprite::from_image(image.clone())))
        .id();
    let node = app
        .world_mut()
        .spawn((Name::new("picture"), Node::default(), ImageNode::new(image)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), sprite);
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), node);
    for (entity, type_path) in [(sprite, SPRITE), (node, IMAGE_NODE)] {
        let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
        let held = ast
            .ast_for(entity)
            .and_then(|node| ast.find_patch_by_type_path(node, type_path))
            .and_then(|patch| ast.get_patch(patch))
            .map(|patch| format!("{patch:?}"))
            .unwrap_or_default();
        assert!(
            held.contains(&format!("String(\"{SHEET}\")")),
            "{type_path} records its image path: {held}"
        );
    }

    set_field(&mut app, sprite, SPRITE, "flip_x", "true");
    set_field(&mut app, node, IMAGE_NODE, "flip_x", "true");
    let sprite_image = app
        .world()
        .get::<Sprite>(sprite)
        .and_then(|sprite| image_path(&sprite.image));
    let node_image = app
        .world()
        .get::<ImageNode>(node)
        .and_then(|image| image_path(&image.image));
    assert_eq!(sprite_image.as_deref(), Some(SHEET));
    assert_eq!(node_image.as_deref(), Some(SHEET));
}
