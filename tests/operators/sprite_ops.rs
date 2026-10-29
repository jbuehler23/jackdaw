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
        [
            ("Sprite", wanted),
            ("Empty 2D", "op:entity.add.empty"),
            ("Camera", "op:entity.add.camera_2d"),
        ]
    );
    let cameras: Vec<(&str, &str)> = taxonomy
        .entries_of("cameras")
        .map(|entry| (entry.label.as_str(), entry.action.as_str()))
        .collect();
    assert_eq!(cameras.first(), Some(&("Camera", "op:entity.add.camera")));
    assert!(
        !rows
            .iter()
            .any(|(action, _)| action.contains("pixel_camera")),
        "no Pixel Camera entry: {rows:?}"
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

const SHEET: &str = "overworld.png";
const SHEET_SIZE: Vec2 = Vec2::new(432.0, 1040.0);
const OTHER_SHEET: &str = "overworld_spaced.png";
const OTHER_SHEET_SIZE: Vec2 = Vec2::new(458.0, 1104.0);

fn assets_of(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("assets")
}

/// A 2D scene of a project in a temp folder whose assets hold the overworld
/// sheet packed and spaced, with one sprite of the packed sheet whose image
/// has loaded.
fn sheet_app() -> (App, tempfile::TempDir, Entity) {
    let dir = tempfile::tempdir().expect("tempdir");
    let assets = assets_of(&dir);
    std::fs::create_dir_all(&assets).expect("assets");
    std::fs::copy(util::OVERWORLD_SHEET, assets.join(SHEET)).expect("copy the sheet");
    util::write_spaced_overworld_sheet(&assets.join(OTHER_SHEET));
    let mut app = util::editor_test_app_reading(&assets);
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: dir.path().to_path_buf(),
            config: default(),
        });
    jackdaw::entity_ops::seed_2d_scene_root(app.world_mut());
    jackdaw::selection::clear_selection_in_world(app.world_mut());
    app.update();
    add(&mut app, &[("path", SHEET.into())]);
    let sprite = only_sprite(&mut app);
    util::update_until(
        &mut app,
        "the sheet to load",
        std::time::Duration::from_secs(20),
        |app| {
            let image = &app.world().get::<Sprite>(sprite).expect("sprite").image;
            app.world().resource::<Assets<Image>>().get(image).is_some()
        },
    );
    (app, dir, sprite)
}

fn set_ppu(app: &mut App, sprite: Entity, ppu: Option<f64>) -> OperatorResult {
    let mut call = app
        .world_mut()
        .operator("sprite.set_pixels_per_unit")
        .param("entity", sprite);
    if let Some(ppu) = ppu {
        call = call.param("ppu", ppu);
    }
    let result = call.call().expect("sprite.set_pixels_per_unit dispatches");
    app.update();
    result
}

fn custom_size(app: &App, sprite: Entity) -> Option<Vec2> {
    app.world()
        .get::<Sprite>(sprite)
        .and_then(|sprite| sprite.custom_size)
}

fn warnings(app: &mut App) -> Vec<String> {
    std::mem::take(
        &mut app
            .world_mut()
            .get_resource_or_init::<jackdaw_api_internal::operator::OperatorWarnings>()
            .0,
    )
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
}

fn redo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    app.update();
}

#[test]
fn set_pixels_per_unit_is_offered_once_the_scene_has_a_sprite() {
    let (mut app, _root) = two_d_app();
    assert!(!available(&mut app, "sprite.set_pixels_per_unit"));
    add(&mut app, &[]);
    assert!(available(&mut app, "sprite.set_pixels_per_unit"));
}

#[test]
fn set_pixels_per_unit_measures_the_sprite_rect() {
    let (mut app, _dir, sprite) = sheet_app();
    app.world_mut()
        .get_mut::<Sprite>(sprite)
        .expect("sprite")
        .rect = Some(Rect::new(0.0, 0.0, 32.0, 48.0));
    set_ppu(&mut app, sprite, Some(16.0)).assert_finished();
    assert_eq!(custom_size(&app, sprite), Some(Vec2::new(2.0, 3.0)));
}

#[test]
fn set_pixels_per_unit_refuses_while_the_image_loads_or_with_no_image() {
    let (mut app, _root) = two_d_app();
    add(&mut app, &[("path", IMAGE.into())]);
    let loading = only_sprite(&mut app);
    warnings(&mut app);
    set_ppu(&mut app, loading, Some(16.0)).assert_cancelled();
    assert!(
        warnings(&mut app)
            .iter()
            .any(|w| w.contains("still loading")),
        "the caller hears why"
    );
    assert_eq!(custom_size(&app, loading), None);

    add(&mut app, &[]);
    let square = sprites(&mut app)
        .into_iter()
        .find(|&entity| entity != loading)
        .expect("a placeholder");
    set_ppu(&mut app, square, Some(16.0)).assert_cancelled();
    assert!(warnings(&mut app).iter().any(|w| w.contains("no image")));
    assert_eq!(custom_size(&app, square), Some(Vec2::splat(32.0)));
}

#[test]
fn editing_another_sprite_field_keeps_its_image() {
    let (mut app, _root) = two_d_app();
    add(&mut app, &[("path", IMAGE.into())]);
    let sprite = only_sprite(&mut app);
    app.world_mut()
        .operator("field.set")
        .param("entity", sprite)
        .param("type_path", "bevy_sprite::sprite::Sprite")
        .param("field", "flip_x")
        .param("value", "true")
        .call()
        .expect("field.set dispatches")
        .assert_finished();
    app.update();
    assert_eq!(
        app.world().get::<Sprite>(sprite).map(|s| s.flip_x),
        Some(true)
    );
    assert_eq!(image_path(&app, sprite).as_deref(), Some(IMAGE));
}

#[test]
fn set_pixels_per_unit_measures_an_image_that_has_not_loaded_from_its_file() {
    let (mut app, _dir, _sprite) = sheet_app();
    add(&mut app, &[("path", OTHER_SHEET.into())]);
    let fresh = app
        .world()
        .resource::<jackdaw::selection::Selection>()
        .primary()
        .expect("the new sprite is selected");
    set_ppu(&mut app, fresh, Some(1.0)).assert_finished();
    assert_eq!(custom_size(&app, fresh), Some(OTHER_SHEET_SIZE));
}

fn set_anchor(
    app: &mut App,
    sprite: Entity,
    params: &[(&'static str, PropertyValue)],
) -> OperatorResult {
    let mut call = app
        .world_mut()
        .operator("sprite.set_anchor")
        .param("entity", sprite);
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call.call().expect("sprite.set_anchor dispatches");
    app.update();
    result
}

#[test]
fn set_anchor_takes_a_preset_name_or_an_offset_in_one_entry_each() {
    let (mut app, _root) = two_d_app();
    assert!(!available(&mut app, "sprite.set_anchor"));
    add(&mut app, &[]);
    let sprite = only_sprite(&mut app);
    assert!(available(&mut app, "sprite.set_anchor"));
    let depth = history_len(&app);

    set_anchor(&mut app, sprite, &[("preset", "bottom_left".into())]).assert_finished();
    assert_eq!(
        app.world().get::<Anchor>(sprite),
        Some(&Anchor::BOTTOM_LEFT)
    );
    set_anchor(
        &mut app,
        sprite,
        &[("x", 0.25_f64.into()), ("y", (-0.1_f64).into())],
    )
    .assert_finished();
    assert_eq!(
        app.world().get::<Anchor>(sprite),
        Some(&Anchor(Vec2::new(0.25, -0.1)))
    );
    assert_eq!(history_len(&app) - depth, 2);

    undo(&mut app);
    assert_eq!(
        app.world().get::<Anchor>(sprite),
        Some(&Anchor::BOTTOM_LEFT)
    );
    undo(&mut app);
    assert_eq!(app.world().get::<Anchor>(sprite), Some(&Anchor::CENTER));

    warnings(&mut app);
    set_anchor(&mut app, sprite, &[("preset", "middle".into())]).assert_cancelled();
    assert!(warnings(&mut app).iter().any(|w| w.contains("bottom_left")));
    set_anchor(&mut app, sprite, &[("x", 0.1_f64.into())]).assert_cancelled();
}

#[test]
fn set_pixels_per_unit_sizes_the_sprite_in_one_undoable_entry_and_stores_nothing_else() {
    let (mut app, dir, sprite) = sheet_app();
    let depth = history_len(&app);
    set_ppu(&mut app, sprite, Some(16.0)).assert_finished();
    assert_eq!(history_len(&app) - depth, 1, "one history entry");
    assert_eq!(custom_size(&app, sprite), Some(SHEET_SIZE / 16.0));
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), &assets_of(&dir))
        .expect("the scene emits");
    assert!(
        !text.contains("PixelsPerUnit"),
        "only the size is saved: {text}"
    );

    set_ppu(&mut app, sprite, Some(8.0)).assert_finished();
    assert_eq!(custom_size(&app, sprite), Some(SHEET_SIZE / 8.0));
    undo(&mut app);
    assert_eq!(custom_size(&app, sprite), Some(SHEET_SIZE / 16.0));
    undo(&mut app);
    assert_eq!(custom_size(&app, sprite), None);
    redo(&mut app);
    assert_eq!(custom_size(&app, sprite), Some(SHEET_SIZE / 16.0));
}

#[test]
fn set_pixels_per_unit_defaults_to_the_project_ppu() {
    let (mut app, _dir, sprite) = sheet_app();
    set_ppu(&mut app, sprite, None).assert_finished();
    assert_eq!(
        custom_size(&app, sprite),
        Some(SHEET_SIZE),
        "ppu 1 by default"
    );
    app.world_mut()
        .operator("project.set_pixels_per_unit")
        .param("value", 16.0)
        .call()
        .expect("project.set_pixels_per_unit dispatches")
        .assert_finished();
    set_ppu(&mut app, sprite, None).assert_finished();
    assert_eq!(custom_size(&app, sprite), Some(SHEET_SIZE / 16.0));
}

#[test]
fn set_pixels_per_unit_refuses_a_zero_negative_or_infinite_ppu() {
    let (mut app, _dir, sprite) = sheet_app();
    let depth = history_len(&app);
    for ppu in [Some(0.0), Some(-4.0), Some(f64::INFINITY)] {
        warnings(&mut app);
        set_ppu(&mut app, sprite, ppu).assert_cancelled();
        assert!(
            warnings(&mut app).iter().any(|w| w.contains("above 0")),
            "the caller hears why for {ppu:?}"
        );
    }
    assert_eq!(history_len(&app), depth);
    assert_eq!(custom_size(&app, sprite), None);
}

#[test]
fn set_pixels_per_unit_takes_any_finite_value_above_zero() {
    let (mut app, _dir, sprite) = sheet_app();
    for ppu in [0.001, 50_000.0] {
        set_ppu(&mut app, sprite, Some(ppu)).assert_finished();
        assert_eq!(custom_size(&app, sprite), Some(SHEET_SIZE / ppu as f32));
    }
}

#[test]
fn set_pixels_per_unit_sizes_every_selected_sprite_in_one_entry() {
    let (mut app, _dir, first) = sheet_app();
    add(&mut app, &[("path", OTHER_SHEET.into())]);
    let second = app
        .world()
        .resource::<jackdaw::selection::Selection>()
        .primary()
        .expect("the new sprite is selected");
    jackdaw::selection::select_only(app.world_mut(), first);
    app.world_mut()
        .resource_mut::<jackdaw::selection::Selection>()
        .entities
        .push(second);
    let depth = history_len(&app);
    let result = app
        .world_mut()
        .operator("sprite.set_pixels_per_unit")
        .param("ppu", 2.0_f64)
        .call()
        .expect("dispatches");
    assert_eq!(result, OperatorResult::Finished);
    app.update();
    assert_eq!(history_len(&app) - depth, 1, "one entry for both");
    assert_eq!(custom_size(&app, first), Some(SHEET_SIZE / 2.0));
    assert_eq!(custom_size(&app, second), Some(OTHER_SHEET_SIZE / 2.0));
    undo(&mut app);
    assert_eq!(custom_size(&app, first), None);
    assert_eq!(custom_size(&app, second), None);
}

#[test]
fn a_read_only_remote_sprite_is_refused() {
    let (mut app, _root) = two_d_app();
    add(&mut app, &[]);
    let sprite = only_sprite(&mut app);
    app.world_mut()
        .entity_mut(sprite)
        .insert(jackdaw::remote::entity_browser::RemoteEntityProxy { remote_bits: 7 });
    warnings(&mut app);
    set_anchor(&mut app, sprite, &[("preset", "bottom_left".into())]).assert_cancelled();
    assert_eq!(app.world().get::<Anchor>(sprite), Some(&Anchor::CENTER));
    set_ppu(&mut app, sprite, Some(2.0)).assert_cancelled();
    assert!(warnings(&mut app).iter().all(|w| w.contains("read-only")));
}

const ANCHOR_PATH: &str = "bevy_sprite::sprite::Anchor";

fn anchor_patch(app: &App, entity: Entity) -> bool {
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    ast.ast_for(entity)
        .and_then(|node| ast.find_patch_by_type_path(node, ANCHOR_PATH))
        .is_some()
}

#[test]
fn undoing_a_preset_on_a_never_authored_anchor_leaves_no_anchor_in_the_document() {
    let (mut app, _root) = two_d_app();
    add(&mut app, &[]);
    let sprite = only_sprite(&mut app);
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
        let node = ast.ast_for(sprite).expect("registered");
        ast.remove_component_patch(node, ANCHOR_PATH);
    }
    assert!(!anchor_patch(&app, sprite));
    set_anchor(&mut app, sprite, &[("preset", "bottom_left".into())]).assert_finished();
    assert_eq!(
        app.world().get::<Anchor>(sprite),
        Some(&Anchor::BOTTOM_LEFT)
    );
    undo(&mut app);
    assert_eq!(app.world().get::<Anchor>(sprite), Some(&Anchor::CENTER));
    assert!(
        !anchor_patch(&app, sprite),
        "undo leaves the anchor unauthored, as it was"
    );
}

#[test]
fn set_anchor_rejects_an_offset_that_is_not_finite() {
    let (mut app, _root) = two_d_app();
    add(&mut app, &[]);
    let sprite = only_sprite(&mut app);
    for (x, y) in [(f64::NAN, 0.0), (0.0, f64::INFINITY)] {
        warnings(&mut app);
        set_anchor(&mut app, sprite, &[("x", x.into()), ("y", y.into())]).assert_cancelled();
        assert!(warnings(&mut app).iter().any(|w| w.contains("finite")));
    }
    assert_eq!(app.world().get::<Anchor>(sprite), Some(&Anchor::CENTER));
}

#[test]
fn set_anchor_without_an_entity_anchors_every_selected_sprite_in_one_entry() {
    let (mut app, _root) = two_d_app();
    for _ in 0..3 {
        add(&mut app, &[]);
    }
    let all = sprites(&mut app);
    assert_eq!(all.len(), 3);
    app.world_mut()
        .resource_mut::<jackdaw::selection::Selection>()
        .entities = all.clone();
    let depth = history_len(&app);
    let result = app
        .world_mut()
        .operator("sprite.set_anchor")
        .param("preset", "top_right")
        .call()
        .expect("dispatches");
    assert_eq!(result, OperatorResult::Finished);
    app.update();
    assert_eq!(history_len(&app) - depth, 1, "one entry for all three");
    for &sprite in &all {
        assert_eq!(app.world().get::<Anchor>(sprite), Some(&Anchor::TOP_RIGHT));
    }
    undo(&mut app);
    for &sprite in &all {
        assert_eq!(app.world().get::<Anchor>(sprite), Some(&Anchor::CENTER));
    }
}

/// Put the editor in Live view with `sprite` projected from game entity `bits`.
fn project_live(app: &mut App, sprite: Entity, bits: u64) {
    *app.world_mut()
        .resource_mut::<jackdaw::pie_mirror::PieViewMode>() =
        jackdaw::pie_mirror::PieViewMode::Live;
    app.world_mut()
        .resource_mut::<jackdaw::pie_projection::PieProjection>()
        .by_bits
        .insert(bits, sprite);
}

fn live_edits(app: &App) -> usize {
    app.world()
        .resource::<jackdaw::live_edits::LiveEditLog>()
        .entries
        .len()
}

#[test]
fn set_anchor_in_live_view_goes_to_the_running_game() {
    let (mut app, _root) = two_d_app();
    add(&mut app, &[]);
    let sprite = only_sprite(&mut app);
    project_live(&mut app, sprite, 77);
    let depth = history_len(&app);
    set_anchor(&mut app, sprite, &[("preset", "bottom_left".into())]).assert_finished();
    assert_eq!(live_edits(&app), 1, "the game is told");
    assert_eq!(history_len(&app), depth, "the scene is not edited");
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Vitals {
    ratio: f32,
}

/// Move `sprites` under a new UI scene root whose bindings drive `write` on
/// each, and start the preview.
fn preview_drives(app: &mut App, sprites: &[Entity], write: &str) {
    app.register_type::<Vitals>();
    let root = app
        .world_mut()
        .spawn((Name::new("UiRoot"), UiSceneRoot::default(), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), root);
    for &sprite in sprites {
        app.world_mut().entity_mut(sprite).insert((
            ChildOf(root),
            jackdaw_bind::Bindings(vec![jackdaw_bind::Binding::Field {
                read: vec![jackdaw_bind::BindPath::new("Vitals.ratio")],
                via: None,
                write: jackdaw_bind::BindPath::new(write),
                as_percent: false,
            }]),
        ));
    }
    app.update();
    jackdaw::preview_context::set_preview(app.world_mut(), true);
    app.update();
}

#[test]
fn set_anchor_is_refused_while_the_preview_drives_the_anchor() {
    let (mut app, _root) = two_d_app();
    add(&mut app, &[]);
    let sprite = only_sprite(&mut app);
    preview_drives(&mut app, &[sprite], "Anchor.0");
    let depth = history_len(&app);
    warnings(&mut app);
    set_anchor(&mut app, sprite, &[("preset", "bottom_left".into())]).assert_cancelled();
    assert!(warnings(&mut app).iter().any(|w| w.contains("preview")));
    assert_eq!(history_len(&app), depth);
}

#[test]
fn set_pixels_per_unit_refuses_all_when_one_target_is_previewed() {
    let (mut app, _dir, projected) = sheet_app();
    add(&mut app, &[("path", SHEET.into())]);
    let previewed = sprites(&mut app)
        .into_iter()
        .find(|&entity| entity != projected)
        .expect("a second sprite");
    util::update_until(
        &mut app,
        "the second sheet to load",
        std::time::Duration::from_secs(20),
        |app| {
            let image = &app.world().get::<Sprite>(previewed).expect("sprite").image;
            app.world().resource::<Assets<Image>>().get(image).is_some()
        },
    );
    preview_drives(&mut app, &[previewed], "Sprite.custom_size");
    project_live(&mut app, projected, 88);
    app.world_mut()
        .resource_mut::<jackdaw::selection::Selection>()
        .entities = vec![projected, previewed];
    let depth = history_len(&app);
    warnings(&mut app);
    let result = app
        .world_mut()
        .operator("sprite.set_pixels_per_unit")
        .param("ppu", 4.0_f64)
        .call()
        .expect("dispatches");
    app.update();
    assert_eq!(result, OperatorResult::Cancelled);
    assert!(warnings(&mut app).iter().any(|w| w.contains("preview")));
    assert_eq!(live_edits(&app), 0, "nothing reached the game");
    assert_eq!(history_len(&app), depth, "nothing was written");
    assert_eq!(custom_size(&app, previewed), None);
}

#[test]
fn set_pixels_per_unit_does_not_read_a_file_of_another_asset_source() {
    let (mut app, _dir, _sprite) = sheet_app();
    let elsewhere = app
        .world()
        .resource::<AssetServer>()
        .load::<Image>(format!("elsewhere://{OTHER_SHEET}"));
    let sprite = app
        .world_mut()
        .spawn((Name::new("remote art"), Sprite::from_image(elsewhere)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), sprite);
    warnings(&mut app);
    set_ppu(&mut app, sprite, Some(1.0)).assert_cancelled();
    assert!(
        warnings(&mut app)
            .iter()
            .any(|w| w.contains("still loading")),
        "the default source's file of the same name is not measured"
    );
    assert_eq!(custom_size(&app, sprite), None);
}

#[test]
fn set_pixels_per_unit_refuses_a_malformed_ppu() {
    let (mut app, _dir, sprite) = sheet_app();
    for ppu in [
        PropertyValue::String("many".into()),
        PropertyValue::Bool(true),
    ] {
        warnings(&mut app);
        let result = app
            .world_mut()
            .operator("sprite.set_pixels_per_unit")
            .param("entity", sprite)
            .param("ppu", ppu.clone())
            .call()
            .expect("sprite.set_pixels_per_unit dispatches");
        app.update();
        result.assert_cancelled();
        assert!(
            warnings(&mut app).iter().any(|w| w.contains("above 0")),
            "the caller hears why for {ppu:?}"
        );
    }
    assert_eq!(custom_size(&app, sprite), None);
}

/// A 2D scene of a project whose assets hold a 16x16 `tile.png`, at `ppu`.
fn tile_app(ppu: f64) -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let assets = assets_of(&dir);
    std::fs::create_dir_all(&assets).expect("assets");
    image::RgbaImage::new(16, 16)
        .save(assets.join("tile.png"))
        .expect("a 16x16 image");
    let mut app = util::editor_test_app_reading(&assets);
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: dir.path().to_path_buf(),
            config: default(),
        });
    jackdaw::entity_ops::seed_2d_scene_root(app.world_mut());
    jackdaw::selection::clear_selection_in_world(app.world_mut());
    app.update();
    app.world_mut()
        .operator("project.set_pixels_per_unit")
        .param("value", ppu)
        .call()
        .expect("project.set_pixels_per_unit dispatches")
        .assert_finished();
    app.update();
    (app, dir)
}

#[test]
fn a_new_sprite_comes_in_at_the_project_ppu() {
    let (mut app, _dir) = tile_app(16.0);
    add(&mut app, &[("path", "tile.png".into())]);
    let sprite = only_sprite(&mut app);
    assert_eq!(
        custom_size(&app, sprite),
        Some(Vec2::ONE),
        "16 px at 16 to the unit"
    );
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), &std::env::temp_dir())
        .expect("the scene emits");
    assert!(text.contains("custom_size"), "the size is authored: {text}");

    let (mut app, _dir) = tile_app(1.0);
    add(&mut app, &[("path", "tile.png".into())]);
    let sprite = only_sprite(&mut app);
    assert_eq!(
        custom_size(&app, sprite),
        None,
        "at ppu 1 the image sizes it"
    );
}
