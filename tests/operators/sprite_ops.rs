//! `sprite.add` and the 2D group of the Add menu: availability by scene kind,
//! parameters, the parent rule, history, and a save round trip.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::{prelude::*, sprite::Anchor};
use jackdaw::commands::CommandHistory;
use jackdaw::entity_ops::SCENE_2D_ROOT_NAME;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::{PropertyValue, Scene2dRoot, SceneNodeId, UiSceneRoot};
use path_slash::PathExt as _;

const IMAGE: &str = "textures/hero.png";

fn two_d_app() -> (App, Entity) {
    let mut app = util::editor_test_app();
    let root = jackdaw::entity_ops::seed_2d_scene_root(app.world_mut());
    jackdaw::selection::clear_selection_in_world(app.world_mut());
    app.update();
    (app, root)
}

fn available(app: &mut App, id: &'static str) -> bool {
    app.world_mut()
        .operator(id)
        .is_available()
        .unwrap_or_else(|err| panic!("{id}: is_available errored: {err}"))
}

fn add(app: &mut App, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator("sprite.add");
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    call.settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    })
    .call()
    .expect("sprite.add dispatches")
    .assert_finished();
    app.update();
}

fn sprites(app: &mut App) -> Vec<Entity> {
    let mut query = app
        .world_mut()
        .query_filtered::<Entity, (With<Sprite>, Without<jackdaw::EditorEntity>)>();
    let mut found: Vec<Entity> = query.iter(app.world()).collect();
    found.sort();
    found
}

fn only_sprite(app: &mut App) -> Entity {
    let found = sprites(app);
    assert_eq!(found.len(), 1, "one sprite was added: {found:?}");
    found[0]
}

fn image_path(app: &App, entity: Entity) -> Option<String> {
    let sprite = app.world().get::<Sprite>(entity)?;
    Some(sprite.image.path()?.path().to_slash_lossy().into_owned())
}

fn parent_of(app: &App, entity: Entity) -> Option<Entity> {
    app.world().get::<ChildOf>(entity).map(ChildOf::parent)
}

fn name_of(app: &App, entity: Entity) -> String {
    app.world()
        .get::<Name>(entity)
        .map(|name| name.as_str().to_owned())
        .unwrap_or_default()
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn absolute(relative: &str) -> String {
    jackdaw::entity_ops::get_assets_base_dir()
        .expect("the editor resolves an assets directory")
        .join(relative)
        .to_slash_lossy()
        .into_owned()
}

fn group_under(app: &mut App, parent: Entity, name: &str) -> Entity {
    let group = app
        .world_mut()
        .spawn((
            Name::new(name.to_owned()),
            Transform::default(),
            Visibility::default(),
            ChildOf(parent),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), group);
    group
}

#[test]
fn sprite_add_is_offered_on_a_2d_world_scene_only() {
    let mut app = util::editor_test_app();
    app.update();
    assert!(!available(&mut app, "sprite.add"), "refused in a 3D scene");

    let ui = app
        .world_mut()
        .spawn((
            Name::new("UiRoot"),
            UiSceneRoot {
                reference_size: UVec2::new(1920, 1080),
            },
            Node::default(),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), ui);
    app.update();
    assert!(!available(&mut app, "sprite.add"), "refused in a UI scene");

    let (mut app, _root) = two_d_app();
    assert!(available(&mut app, "sprite.add"), "offered in a 2D scene");
}

#[test]
fn sprite_add_stores_the_relative_path_position_and_stem_name() {
    let (mut app, root) = two_d_app();
    add(
        &mut app,
        &[
            ("path", absolute(IMAGE).into()),
            ("x", 12.5_f64.into()),
            ("y", (-4.0_f64).into()),
        ],
    );

    let sprite = only_sprite(&mut app);
    assert_eq!(image_path(&app, sprite).as_deref(), Some(IMAGE));
    assert_eq!(name_of(&app, sprite), "hero");
    assert_eq!(parent_of(&app, sprite), Some(root));
    assert_eq!(
        app.world().get::<Transform>(sprite).map(|t| t.translation),
        Some(Vec3::new(12.5, -4.0, 0.0))
    );
    assert_eq!(app.world().get::<Anchor>(sprite), Some(&Anchor::CENTER));
    assert_eq!(
        app.world()
            .resource::<jackdaw::selection::Selection>()
            .primary(),
        Some(sprite),
        "the new sprite is selected"
    );

    add(&mut app, &[("path", IMAGE.into())]);
    let second = sprites(&mut app)
        .into_iter()
        .find(|&entity| entity != sprite)
        .expect("a second sprite");
    assert_eq!(name_of(&app, second), "hero2", "the stem name is deduped");
    assert_eq!(image_path(&app, second).as_deref(), Some(IMAGE));
}

#[test]
fn sprite_add_takes_an_explicit_name_and_parent() {
    let (mut app, root) = two_d_app();
    let group = group_under(&mut app, root, "Props");
    add(
        &mut app,
        &[("name", "Tree".into()), ("parent", group.into())],
    );
    let sprite = only_sprite(&mut app);
    assert_eq!(name_of(&app, sprite), "Tree");
    assert_eq!(parent_of(&app, sprite), Some(group));
}

#[test]
fn sprite_add_goes_beside_a_selected_entity_of_the_2d_scene() {
    let (mut app, root) = two_d_app();
    let group = group_under(&mut app, root, "Props");
    let kid = group_under(&mut app, group, "Kid");
    jackdaw::selection::select_only(app.world_mut(), kid);
    add(&mut app, &[]);
    let sprite = only_sprite(&mut app);
    assert_eq!(
        parent_of(&app, sprite),
        Some(group),
        "a sibling of the selection"
    );

    jackdaw::selection::select_only(app.world_mut(), root);
    add(&mut app, &[]);
    let under_root = sprites(&mut app)
        .into_iter()
        .find(|&entity| entity != sprite)
        .expect("a second sprite");
    assert_eq!(parent_of(&app, under_root), Some(root), "the root adopts");
}

#[test]
fn sprite_add_without_a_path_adds_a_placeholder_square() {
    let (mut app, _root) = two_d_app();
    add(&mut app, &[]);
    let sprite = only_sprite(&mut app);
    assert_eq!(name_of(&app, sprite), "Sprite");
    let made = app.world().get::<Sprite>(sprite).expect("a sprite");
    assert_eq!(made.custom_size, Some(Vec2::splat(32.0)));
    assert_eq!(image_path(&app, sprite), None);
}

#[test]
fn sprite_add_is_one_undo_entry_and_redo_brings_the_same_node_back() {
    let (mut app, root) = two_d_app();
    let depth = history_len(&app);
    add(&mut app, &[("path", IMAGE.into())]);
    assert_eq!(
        history_len(&app) - depth,
        1,
        "one entry, no snapshot beside it"
    );
    let sprite = only_sprite(&mut app);
    let node = *app
        .world()
        .get::<SceneNodeId>(sprite)
        .expect("registered in the document");

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
    assert!(sprites(&mut app).is_empty(), "undo removes the sprite");

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    app.update();
    let back = only_sprite(&mut app);
    assert_eq!(
        app.world().get::<SceneNodeId>(back),
        Some(&node),
        "redo restores the same document node"
    );
    assert_eq!(parent_of(&app, back), Some(root));
    assert_eq!(image_path(&app, back).as_deref(), Some(IMAGE));
}

#[test]
fn the_add_menu_shows_the_2d_group_only_with_a_2d_scene_open() {
    let wanted = "op:sprite.add";
    let mut app = util::editor_test_app();
    app.update();
    let rows = jackdaw::add_entity_picker::add_menu_rows(app.world_mut());
    assert!(
        !rows.iter().any(|(action, _)| action == wanted),
        "no 2D rows in a 3D scene: {rows:?}"
    );

    let (mut app, _root) = two_d_app();
    let rows = jackdaw::add_entity_picker::add_menu_rows(app.world_mut());
    assert!(
        rows.iter().any(|(action, _)| action == wanted),
        "the 2D group offers Sprite: {rows:?}"
    );
    let taxonomy = jackdaw::creation_taxonomy::CreationTaxonomy::collect(app.world_mut());
    let labels: Vec<(&str, &str)> = taxonomy
        .entries_of("2d")
        .map(|entry| (entry.label.as_str(), entry.action.as_str()))
        .collect();
    assert_eq!(
        labels,
        [("Sprite", wanted), ("Empty 2D", "op:entity.add.empty")]
    );
}

#[test]
fn a_new_2d_scene_flags_the_menu_bar_for_a_rebuild() {
    let mut app = util::editor_test_app();
    app.update();
    app.world_mut().resource_mut::<jackdaw::MenuBarDirty>().0 = false;
    let root = jackdaw::entity_ops::seed_2d_scene_root(app.world_mut());
    assert!(app.world().resource::<jackdaw::MenuBarDirty>().0);

    app.update();
    app.world_mut().resource_mut::<jackdaw::MenuBarDirty>().0 = false;
    app.world_mut().entity_mut(root).despawn();
    assert!(app.world().resource::<jackdaw::MenuBarDirty>().0);
}

#[test]
fn empty_2d_lands_under_the_scene_root_and_stays_listed() {
    let (mut app, root) = two_d_app();
    app.world_mut()
        .operator("entity.add.empty")
        .call()
        .expect("dispatch")
        .assert_finished();
    for _ in 0..3 {
        app.update();
    }
    let empty = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::entity_ops::EmptyEntity>>()
        .single(app.world())
        .expect("one empty");
    assert_eq!(parent_of(&app, empty), Some(root));
    assert!(
        app.world().get::<jackdaw::EditorHidden>(empty).is_none(),
        "the outliner still lists it"
    );
}

#[test]
fn an_empty_in_a_3d_scene_stays_top_level() {
    let mut app = util::editor_test_app();
    app.update();
    app.world_mut()
        .operator("entity.add.empty")
        .call()
        .expect("dispatch")
        .assert_finished();
    app.update();
    let empty = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::entity_ops::EmptyEntity>>()
        .single(app.world())
        .expect("one empty");
    assert_eq!(parent_of(&app, empty), None);
}

#[test]
fn a_saved_and_reopened_sprite_keeps_its_image_name_and_listing() {
    let (mut app, _root) = two_d_app();
    add(&mut app, &[("path", IMAGE.into()), ("x", 8.0_f64.into())]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("sprites.bsn");
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path())
        .expect("the scene emits");
    std::fs::write(&path, text).expect("write the scene");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    for _ in 0..8 {
        app.update();
    }
    let sprite = app
        .world_mut()
        .query_filtered::<(Entity, &Name), With<Sprite>>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "hero")
        .map(|(entity, _)| entity)
        .expect("the reopened scene has the sprite, by name");
    assert_eq!(image_path(&app, sprite).as_deref(), Some(IMAGE));
    let parent = parent_of(&app, sprite).expect("still parented");
    assert!(app.world().get::<Scene2dRoot>(parent).is_some());
    assert_eq!(name_of(&app, parent), SCENE_2D_ROOT_NAME);
    assert!(
        app.world().get::<jackdaw::EditorHidden>(sprite).is_none(),
        "the outliner lists it after the reopen"
    );
}

#[test]
fn redo_puts_an_empty_2d_back_under_the_group_it_was_added_to() {
    let (mut app, root) = two_d_app();
    let group = group_under(&mut app, root, "Props");
    let kid = group_under(&mut app, group, "Kid");
    jackdaw::selection::select_only(app.world_mut(), kid);
    app.world_mut()
        .operator("entity.add.empty")
        .call()
        .expect("dispatch")
        .assert_finished();
    app.update();
    let empty = |app: &mut App| {
        app.world_mut()
            .query_filtered::<Entity, With<jackdaw::entity_ops::EmptyEntity>>()
            .single(app.world())
            .expect("one empty")
    };
    let first = empty(&mut app);
    assert_eq!(parent_of(&app, first), Some(group), "a sibling of the kid");

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
    jackdaw::selection::select_only(app.world_mut(), root);
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    app.update();
    let back = empty(&mut app);
    assert_eq!(
        parent_of(&app, back),
        Some(group),
        "redo keeps the parent the add chose, whatever is selected now"
    );
}

#[test]
fn a_new_sprite_draws_above_the_scene_sprites() {
    let (mut app, root) = two_d_app();
    let group = group_under(&mut app, root, "Props");
    app.world_mut()
        .entity_mut(group)
        .insert(Transform::from_xyz(0.0, 0.0, 2.0));
    let below = app
        .world_mut()
        .spawn((
            Name::new("Below"),
            Sprite::default(),
            Transform::from_xyz(0.0, 0.0, 3.0),
            ChildOf(group),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), below);
    app.update();

    add(&mut app, &[]);
    let added = sprites(&mut app)
        .into_iter()
        .find(|&entity| entity != below)
        .expect("a new sprite");
    assert_eq!(parent_of(&app, added), Some(root));
    assert_eq!(
        app.world().get::<Transform>(added).map(|t| t.translation.z),
        Some(6.0),
        "one above the highest sprite, which sits at 2 + 3"
    );
}
