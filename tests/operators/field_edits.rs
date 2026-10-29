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
const SHEET: &str = "overworld.png";

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
    std::fs::copy(util::OVERWORLD_SHEET, dir.path().join(SHEET)).expect("copy the sheet");
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

const PROJECTION: &str = "bevy_camera::projection::Projection";

fn field_set(
    app: &mut App,
    entity: Entity,
    type_path: &str,
    field: &str,
    value: &str,
) -> OperatorResult {
    jackdaw::selection::select_only(app.world_mut(), entity);
    let result = app
        .world_mut()
        .operator("field.set")
        .param("entity", entity)
        .param("type_path", type_path.to_owned())
        .param("field", field.to_owned())
        .param("value", value.to_owned())
        .call()
        .expect("field.set dispatches");
    app.update();
    result
}

fn warnings(app: &mut App) -> Vec<String> {
    std::mem::take(
        &mut app
            .world_mut()
            .get_resource_or_init::<jackdaw_api_internal::operator::OperatorWarnings>()
            .0,
    )
}

fn authored_sprite(app: &mut App) -> Entity {
    let entity = app
        .world_mut()
        .spawn((Name::new("tile"), Sprite::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.update();
    warnings(app);
    entity
}

fn rect(app: &App, entity: Entity) -> Option<Rect> {
    app.world()
        .get::<Sprite>(entity)
        .and_then(|sprite| sprite.rect)
}

#[test]
fn field_set_refuses_a_value_of_the_wrong_shape_and_names_the_shape() {
    let mut app = util::editor_test_app();
    let sprite = authored_sprite(&mut app);
    let depth = history_len(&app);

    let result = field_set(
        &mut app,
        sprite,
        SPRITE,
        "rect",
        r#"{"x":0,"y":0,"w":16,"h":16}"#,
    );

    assert_eq!(result, OperatorResult::Cancelled);
    let said = warnings(&mut app).join("\n");
    assert!(
        said.contains("`rect`"),
        "the warning names the field: {said}"
    );
    assert!(said.contains("\"min\""), "and the shape it takes: {said}");
    assert_eq!(rect(&app, sprite), None);
    assert_eq!(history_len(&app), depth, "a refusal records nothing");
}

#[test]
fn field_set_refuses_a_path_the_component_does_not_have() {
    let mut app = util::editor_test_app();
    let sprite = authored_sprite(&mut app);
    let depth = history_len(&app);

    let result = field_set(
        &mut app,
        sprite,
        SPRITE,
        "rekt",
        r#"{"min":[0,0],"max":[16,16]}"#,
    );

    assert_eq!(result, OperatorResult::Cancelled);
    let said = warnings(&mut app).join("\n");
    assert!(
        said.contains("`rekt`"),
        "the warning names the path: {said}"
    );
    assert!(
        said.contains("rect"),
        "and lists the fields there are: {said}"
    );
    assert_eq!(history_len(&app), depth);
}

#[test]
fn field_set_on_an_option_takes_the_bare_value_and_refuses_some() {
    let mut app = util::editor_test_app();
    let sprite = authored_sprite(&mut app);
    let depth = history_len(&app);

    let wrapped = field_set(
        &mut app,
        sprite,
        SPRITE,
        "rect",
        r#"{"Some":{"min":[0,0],"max":[16,16]}}"#,
    );
    assert_eq!(wrapped, OperatorResult::Cancelled);
    let said = warnings(&mut app).join("\n");
    assert!(
        said.contains("bare"),
        "the warning says how an Option is spelled: {said}"
    );
    assert_eq!(history_len(&app), depth);

    field_set(
        &mut app,
        sprite,
        SPRITE,
        "rect",
        r#"{"min":[0,0],"max":[16,16]}"#,
    )
    .assert_finished();
    assert_eq!(rect(&app, sprite), Some(Rect::new(0.0, 0.0, 16.0, 16.0)));
    assert_eq!(history_len(&app), depth + 1);
}

#[test]
fn field_set_writes_into_an_enum_variant_at_its_default() {
    let mut app = util::editor_test_app();
    let camera = app.world_mut().spawn((Name::new("cam"), Camera2d)).id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), camera);
    let text = app
        .world_mut()
        .spawn((Name::new("label"), Text::new("hi"), TextFont::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), text);
    app.update();
    let depth = history_len(&app);

    field_set(&mut app, camera, PROJECTION, "0.scale", "0.0625").assert_finished();
    let scale = match app.world().get::<Projection>(camera) {
        Some(Projection::Orthographic(ortho)) => Some(ortho.scale),
        _ => None,
    };
    assert_eq!(scale, Some(0.0625));

    field_set(
        &mut app,
        text,
        "bevy_text::text::TextFont",
        "font_size.0",
        "1.5",
    )
    .assert_finished();
    assert!(matches!(
        app.world().get::<TextFont>(text).map(|font| font.font_size),
        Some(bevy::text::FontSize::Rem(size)) if size == 1.5
    ));
    assert_eq!(history_len(&app), depth + 2);
}
