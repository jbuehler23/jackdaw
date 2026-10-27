//! Draw order on a 2D world scene: the page keys step a sprite's z, and
//! `sprite.y_sort` tags sprites that sort by world y without that sort ever
//! reaching the authored z.

use crate::sprite_gizmo::{
    drag, handle, history_len, near, run, select, settle, sprite, transform, undo, world_app,
};
use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::{prelude::*, sprite::Anchor};
use jackdaw::{
    active_tool::ActiveTool,
    project::{ProjectConfig, ProjectRoot},
    selection::Selection,
    sprite_gizmo::SpriteHandle,
};
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{YSort, y_sort_offset};

fn select_both(app: &mut App, first: Entity, second: Entity) {
    jackdaw::selection::select_only(app.world_mut(), first);
    app.world_mut()
        .resource_mut::<Selection>()
        .entities
        .push(second);
    settle(app);
}

fn y_sort(app: &mut App, on: bool) {
    y_sort_now(app, on);
    settle(app);
}

fn y_sort_now(app: &mut App, on: bool) {
    app.world_mut()
        .operator("sprite.y_sort")
        .param("on", on)
        .call()
        .expect("sprite.y_sort is registered")
        .assert_finished();
}

fn drawn_z(app: &App, entity: Entity) -> f32 {
    app.world()
        .get::<GlobalTransform>(entity)
        .expect("propagated")
        .translation()
        .z
}

fn pup_under(app: &mut App, parent: Entity) -> Entity {
    sprite(
        app,
        parent,
        "Pup",
        Sprite::from_color(Color::WHITE, Vec2::splat(4.0)),
        Anchor::CENTER,
        Vec3::new(5.0, 0.0, 2.0),
    )
}

fn z_of(app: &App, entity: Entity) -> f32 {
    transform(app, entity).translation.z
}

#[test]
fn the_page_keys_step_the_selected_sprites_z_by_one_entry_each() {
    let (mut app, scene) = world_app();
    select_both(&mut app, scene.tile, scene.other);
    let entries = history_len(&app);

    run(&mut app, "input.key key=PageUp");
    assert_eq!(z_of(&app, scene.tile), 1.0);
    assert_eq!(z_of(&app, scene.other), 1.0);
    assert_eq!(history_len(&app), entries + 1, "one step, one entry");
    assert!(near(
        transform(&app, scene.other).translation,
        Vec3::new(100.0, 0.0, 1.0)
    ));

    undo(&mut app);
    assert_eq!(z_of(&app, scene.tile), 0.0);
    assert_eq!(z_of(&app, scene.other), 0.0);

    run(&mut app, "input.key key=PageDown");
    assert_eq!(z_of(&app, scene.tile), -1.0);
    assert_eq!(z_of(&app, scene.other), -1.0);
    assert_eq!(history_len(&app), entries + 1);
}

#[test]
fn dragging_a_y_sorted_sprite_keeps_its_authored_z() {
    let (mut app, scene) = world_app();
    jackdaw::selection::select_only(app.world_mut(), scene.corner);
    y_sort(&mut app, true);
    let drawn = app
        .world()
        .get::<GlobalTransform>(scene.corner)
        .expect("propagated")
        .translation()
        .z;
    assert_eq!(drawn, -y_sort_offset(50.0), "the corner draws sorted");

    select(&mut app, Vec2::new(-80.0, 60.0), ActiveTool::Translate);
    drag(
        &mut app,
        Vec2::new(-80.0, 60.0),
        Vec2::new(-50.0, 100.0),
        "",
    );
    let moved = transform(&app, scene.corner);
    assert!(
        near(moved.translation, Vec3::new(-70.0, 90.0, 0.0)),
        "{moved:?}"
    );
    assert_eq!(moved.translation.z, 0.0, "no sorted z leaks into the move");

    select(&mut app, Vec2::new(-60.0, 100.0), ActiveTool::Rotate);
    let knob = handle(&mut app, SpriteHandle::Rotate);
    drag(&mut app, knob, knob + Vec2::new(-40.0, -10.0), "");
    assert_eq!(z_of(&app, scene.corner), 0.0, "nor into a turn");
}

#[test]
fn sprite_y_sort_tags_the_selection_and_undoes() {
    let (mut app, scene) = world_app();
    select_both(&mut app, scene.tile, scene.other);
    let entries = history_len(&app);
    let tagged = |app: &App| {
        [scene.tile, scene.other].map(|entity| app.world().get::<YSort>(entity).is_some())
    };

    y_sort(&mut app, true);
    assert_eq!(tagged(&app), [true, true]);
    assert_eq!(history_len(&app), entries + 1, "one toggle, one entry");

    y_sort(&mut app, false);
    assert_eq!(tagged(&app), [false, false]);
    assert_eq!(history_len(&app), entries + 2);

    undo(&mut app);
    assert_eq!(tagged(&app), [true, true]);
    undo(&mut app);
    assert_eq!(tagged(&app), [false, false]);
}

#[test]
fn a_saved_and_reopened_scene_keeps_its_y_sort() {
    let (mut app, scene) = world_app();
    jackdaw::selection::select_only(app.world_mut(), scene.tile);
    y_sort(&mut app, true);

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("sorted.bsn");
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path())
        .expect("the scene emits");
    assert!(text.contains("YSort"), "the tag is saved:\n{text}");
    std::fs::write(&path, text).expect("write the scene");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    settle(&mut app);
    let sorted: Vec<String> = app
        .world_mut()
        .query_filtered::<&Name, With<YSort>>()
        .iter(app.world())
        .map(|name| name.as_str().to_owned())
        .collect();
    assert_eq!(sorted, vec!["Tile".to_owned()]);
}

#[test]
fn a_world_space_set_in_the_frame_the_tag_lands_keeps_the_authored_z() {
    let (mut app, scene) = world_app();
    jackdaw::selection::select_only(app.world_mut(), scene.corner);
    y_sort_now(&mut app, true);
    app.world_mut()
        .operator("entity.set_transform")
        .param("entity", scene.corner)
        .param("world", true)
        .param("x", -90.0)
        .call()
        .expect("entity.set_transform is registered")
        .assert_finished();
    settle(&mut app);
    assert_eq!(
        transform(&app, scene.corner).translation,
        Vec3::new(-90.0, 50.0, 0.0)
    );
}

#[test]
fn turning_y_sort_off_or_undoing_it_restores_the_drawn_z() {
    let (mut app, scene) = world_app();
    let pup = pup_under(&mut app, scene.corner);
    settle(&mut app);
    jackdaw::selection::select_only(app.world_mut(), scene.corner);

    y_sort(&mut app, true);
    assert_eq!(drawn_z(&app, scene.corner), -y_sort_offset(50.0));
    assert_eq!(drawn_z(&app, pup), 2.0 - y_sort_offset(50.0));
    y_sort(&mut app, false);
    assert_eq!(drawn_z(&app, scene.corner), 0.0);
    assert_eq!(drawn_z(&app, pup), 2.0);

    undo(&mut app);
    assert_eq!(drawn_z(&app, pup), 2.0 - y_sort_offset(50.0));
    undo(&mut app);
    assert_eq!(drawn_z(&app, scene.corner), 0.0);
    assert_eq!(drawn_z(&app, pup), 2.0);
}

#[test]
fn a_prefab_placed_under_a_y_sorted_parent_keeps_the_authored_z() {
    let (mut app, scene) = world_app();
    let dir = tempfile::tempdir().expect("tempdir");
    let prefab = dir.path().join("assets/prefabs/rock.bsn");
    std::fs::create_dir_all(prefab.parent().expect("a parent")).expect("a prefabs dir");
    std::fs::write(
        &prefab,
        "#Rock\nbevy_transform::components::transform::Transform\n",
    )
    .expect("the prefab");
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    jackdaw::selection::select_only(app.world_mut(), scene.corner);
    y_sort(&mut app, true);

    app.world_mut()
        .operator("prefab.spawn_instance")
        .param("path", "prefabs/rock.bsn")
        .param("pos_x", -100.0)
        .param("pos_y", 60.0)
        .param("pos_z", 0.0)
        .param("parent", scene.corner)
        .call()
        .expect("prefab.spawn_instance is registered")
        .assert_finished();
    settle(&mut app);
    let rock = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Rock")
        .map(|(entity, _)| entity)
        .expect("the rock was placed");
    assert_eq!(transform(&app, rock).translation, Vec3::new(0.0, 10.0, 0.0));
}

#[test]
fn the_page_keys_leave_a_sprite_outside_the_open_scene_at_its_z() {
    let (mut app, _scene) = world_app();
    let stray = app
        .world_mut()
        .spawn((
            Name::new("Stray"),
            Sprite::from_color(Color::WHITE, Vec2::splat(8.0)),
            Transform::default(),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), stray);
    jackdaw::selection::select_only(app.world_mut(), stray);
    settle(&mut app);
    let offered = app
        .world_mut()
        .operator("sprite.z_up")
        .is_available()
        .expect("sprite.z_up is registered");
    assert!(!offered, "z steps only sprites the canvas moves");
    run(&mut app, "input.key key=PageUp");
    assert_eq!(z_of(&app, stray), 0.0);
}
