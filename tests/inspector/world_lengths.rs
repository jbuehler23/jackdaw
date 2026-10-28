//! The lengths of an entity in a 2D world scene (its Transform translation and
//! its sprite's custom size), shown and typed in the project's length unit
//! while the document keeps world units.

use crate::util;

use bevy::{prelude::*, ui_widgets::ValueChange};
use jackdaw::commands::CommandHistory;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_feathers::number_input::ScrubNumberInputValue;
use jackdaw_scene_types::PropertyValue;

const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
const SPRITE: &str = "bevy_sprite::sprite::Sprite";

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn call(app: &mut App, id: &'static str, key: &'static str, value: impl Into<PropertyValue>) {
    let result = app
        .world_mut()
        .operator(id)
        .param(key, value)
        .call()
        .unwrap_or_else(|err| panic!("{id} dispatches: {err}"));
    assert_eq!(result, OperatorResult::Finished, "{id}");
    settle(app);
}

/// A selected placeholder sprite at world (2, 5) in a 2D world scene, with
/// the inspector open and the project at 16 pixels per unit.
fn app_with_sprite() -> (App, Entity) {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    jackdaw::entity_ops::seed_2d_scene_root(app.world_mut());
    app.update();
    call(&mut app, "project.set_pixels_per_unit", "value", 16.0);
    let added = app
        .world_mut()
        .operator("sprite.add")
        .param("x", 2.0)
        .param("y", 5.0)
        .call()
        .expect("sprite.add dispatches");
    assert_eq!(added, OperatorResult::Finished);
    settle(&mut app);
    let sprite = app
        .world()
        .resource::<Selection>()
        .primary()
        .expect("the new sprite is selected");
    (app, sprite)
}

fn input(app: &mut App, type_path: &str, field_path: &str) -> Entity {
    let entities: Vec<Entity> = app
        .world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect();
    entities
        .into_iter()
        .find(|&entity| {
            jackdaw::inspector::field_edited_by(app.world(), entity)
                == Some((type_path, field_path))
        })
        .unwrap_or_else(|| panic!("an input for {field_path}"))
}

fn shown_in(app: &mut App, type_path: &str, field_path: &str) -> f64 {
    let input = input(app, type_path, field_path);
    match app.world().get::<ScrubNumberInputValue>(input) {
        Some(ScrubNumberInputValue::F64(value)) => *value,
        other => panic!("an f64 value, not {other:?}"),
    }
}

fn type_in(app: &mut App, type_path: &str, field_path: &str, value: f64) {
    let input = input(app, type_path, field_path);
    app.world_mut().trigger(ValueChange {
        source: input,
        value,
        is_final: true,
    });
    settle(app);
}

fn shown(app: &mut App, field_path: &str) -> f64 {
    shown_in(app, TRANSFORM, field_path)
}

fn type_value(app: &mut App, field_path: &str, value: f64) {
    type_in(app, TRANSFORM, field_path, value);
}

fn translation(app: &App, entity: Entity) -> Vec3 {
    app.world()
        .get::<Transform>(entity)
        .expect("a transform")
        .translation
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    settle(app);
}

#[test]
fn translation_shows_and_takes_pixels_at_the_project_ppu() {
    let (mut app, sprite) = app_with_sprite();
    assert_eq!(shown(&mut app, "translation.x"), 32.0);
    assert_eq!(shown(&mut app, "translation.y"), 80.0);

    let depth = history_len(&app);
    type_value(&mut app, "translation.x", 48.0);
    assert_eq!(translation(&app, sprite).x, 3.0);
    assert_eq!(history_len(&app) - depth, 1, "one history entry");
    assert_eq!(shown(&mut app, "translation.x"), 48.0);
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), &std::env::temp_dir())
        .expect("the scene emits");
    assert!(
        text.contains("x: 3.0") || text.contains("x: 3,"),
        "the document holds world units: {text}"
    );

    undo(&mut app);
    assert_eq!(translation(&app, sprite).x, 2.0);
    assert_eq!(
        shown(&mut app, "translation.x"),
        32.0,
        "an undo shows pixels"
    );
}

#[test]
fn the_unit_display_shows_world_units_and_leaves_z_rotation_and_scale_alone() {
    let (mut app, sprite) = app_with_sprite();
    call(&mut app, "project.set_length_unit", "unit", "units");
    assert_eq!(shown(&mut app, "translation.x"), 2.0);
    type_value(&mut app, "translation.y", 7.5);
    assert_eq!(translation(&app, sprite).y, 7.5);

    call(&mut app, "project.set_length_unit", "unit", "pixels");
    assert_eq!(shown(&mut app, "translation.y"), 120.0);
    let z = translation(&app, sprite).z as f64;
    assert_eq!(shown(&mut app, "translation.z"), z, "z is draw order");
    assert_eq!(shown(&mut app, "scale.x"), 1.0);
}

#[test]
fn a_3d_entity_keeps_world_units() {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    call(&mut app, "project.set_pixels_per_unit", "value", 16.0);
    let entity = app
        .world_mut()
        .spawn((Name::new("Box"), Transform::from_xyz(2.0, 0.0, 0.0)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    jackdaw::selection::select_only(app.world_mut(), entity);
    settle(&mut app);
    assert_eq!(shown(&mut app, "translation.x"), 2.0);
}

#[test]
fn a_sprites_custom_size_shows_and_takes_pixels() {
    let (mut app, sprite) = app_with_sprite();
    let size = |app: &App| {
        app.world()
            .get::<Sprite>(sprite)
            .and_then(|s| s.custom_size)
    };
    let placeholder = size(&app).expect("the placeholder has a custom size");
    assert_eq!(
        shown_in(&mut app, SPRITE, "custom_size.0.x"),
        f64::from(placeholder.x) * 16.0
    );
    type_in(&mut app, SPRITE, "custom_size.0.y", 24.0);
    assert_eq!(size(&app), Some(Vec2::new(placeholder.x, 1.5)));
    assert_eq!(shown_in(&mut app, SPRITE, "custom_size.0.y"), 24.0);
}

#[test]
fn a_sprite_moved_out_of_the_2d_scene_reads_world_units_again() {
    let (mut app, sprite) = app_with_sprite();
    assert_eq!(shown(&mut app, "translation.x"), 32.0);
    let group = app
        .world_mut()
        .spawn((Name::new("Group"), Transform::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), group);
    jackdaw::selection::select_only(app.world_mut(), sprite);
    settle(&mut app);
    let moved = app
        .world_mut()
        .operator("entity.reparent")
        .param("child", sprite)
        .param("parent", group)
        .call()
        .expect("entity.reparent dispatches");
    assert_eq!(moved, OperatorResult::Finished);
    settle(&mut app);
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(sprite),
        "still selected"
    );
    assert_eq!(shown(&mut app, "translation.x"), 2.0);
    type_value(&mut app, "translation.x", 5.0);
    assert_eq!(translation(&app, sprite).x, 5.0);
}
