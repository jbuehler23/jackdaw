//! Document edits made while playing reach the game as node component
//! updates, whichever path made them: an operator, undo, redo, or a gizmo
//! drag committed on release.

use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw::commands::SetTransform;
use jackdaw::play_sync::collect_node_changes;
use jackdaw_api::prelude::*;
use jackdaw_bsn::BsnSceneAssets;
use jackdaw_commands::{CommandHistory, EditorCommand};
use jackdaw_pie_protocol::event::{from_bytes, to_bytes};
use jackdaw_pie_protocol::{ControlEvent, LoopbackTransport, PieChannel, PieTransport};
use jackdaw_scene_types::{Brush, SceneNodeId};
use serde_json::Value;

use super::batch_undo::{editor, history, menu};
use crate::util::OperatorResultExt as _;

const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
const NAME: &str = "bevy_ecs::name::Name";

pub(super) fn cube(app: &mut App) -> SceneNodeId {
    let before = nodes(app);
    menu(app, "entity.add.cube", true);
    nodes(app)
        .into_iter()
        .find(|node| !before.contains(node))
        .expect("the menu added a node")
}

pub(super) fn nodes(app: &mut App) -> Vec<SceneNodeId> {
    app.world_mut()
        .query::<&SceneNodeId>()
        .iter(app.world())
        .copied()
        .collect()
}

pub(super) fn entity_of(app: &mut App, node: SceneNodeId) -> Entity {
    app.world_mut()
        .query::<(Entity, &SceneNodeId)>()
        .iter(app.world())
        .find(|(_, held)| **held == node)
        .map(|(entity, _)| entity)
        .expect("the node is spawned")
}

fn x_of(app: &mut App, node: SceneNodeId) -> f32 {
    let entity = entity_of(app, node);
    app.world()
        .get::<Transform>(entity)
        .expect("a transform")
        .translation
        .x
}

pub(super) fn move_to(app: &mut App, node: SceneNodeId, x: f64) {
    let entity = entity_of(app, node);
    app.world_mut()
        .operator("entity.set_transform")
        .param("entity", entity)
        .param("x", x)
        .call()
        .expect("entity.set_transform dispatches")
        .assert_finished();
    app.update();
}

fn rename(app: &mut App, node: SceneNodeId, name: &str) {
    let entity = entity_of(app, node);
    app.world_mut()
        .operator("entity.rename")
        .param("entity", entity)
        .param("name", name.to_string())
        .call()
        .expect("entity.rename dispatches")
        .assert_finished();
    app.update();
}

/// The one node event `events` holds, as `(node, set, removed)`.
fn only(events: Vec<ControlEvent>) -> (u64, Vec<(String, Value)>, Vec<String>) {
    match events.as_slice() {
        [ControlEvent::NodeComponents { node, set, removed }] => {
            (*node, set.clone(), removed.clone())
        }
        other => panic!("expected one node event, got {other:?}"),
    }
}

fn translation_x(set: &[(String, Value)]) -> f64 {
    let (_, value) = set
        .iter()
        .find(|(path, _)| path == TRANSFORM)
        .expect("the transform was sent");
    value["translation"][0].as_f64().expect("a translation")
}

/// A started session: the scene as it is now is what the game loaded.
fn playing() -> (App, tempfile::TempDir, SceneNodeId) {
    let (mut app, dir) = editor();
    let rock = cube(&mut app);
    assert!(collect_node_changes(app.world_mut()).is_empty());
    (app, dir, rock)
}

#[test]
fn pie_sync_sends_an_edit_and_its_undo_and_redo() {
    let (mut app, _dir, rock) = playing();
    let start = f64::from(x_of(&mut app, rock));

    move_to(&mut app, rock, 5.0);
    let (node, set, removed) = only(collect_node_changes(app.world_mut()));
    assert_eq!(node, rock.0);
    assert_eq!(
        set.iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        vec![TRANSFORM],
        "only the touched component goes"
    );
    assert_eq!(translation_x(&set), 5.0);
    assert!(removed.is_empty());
    assert!(
        collect_node_changes(app.world_mut()).is_empty(),
        "nothing more until the next edit"
    );

    history(&mut app, "history.undo");
    let (_, set, _) = only(collect_node_changes(app.world_mut()));
    assert_eq!(translation_x(&set), start);

    history(&mut app, "history.redo");
    let (_, set, _) = only(collect_node_changes(app.world_mut()));
    assert_eq!(translation_x(&set), 5.0);
}

#[test]
fn pie_sync_sends_a_gizmo_drag_once_it_is_committed() {
    let (mut app, _dir, rock) = playing();
    let entity = entity_of(&mut app, rock);
    let old_transform = *app.world().get::<Transform>(entity).expect("a transform");
    let new_transform = old_transform.with_translation(Vec3::new(7.0, 0.0, 0.0));

    *app.world_mut()
        .get_mut::<Transform>(entity)
        .expect("a transform") = new_transform;
    app.update();
    assert!(
        collect_node_changes(app.world_mut()).is_empty(),
        "a drag in progress has not changed the document"
    );

    let command = SetTransform {
        entity,
        old_transform,
        new_transform,
    };
    command.sync_after_external_execute(app.world_mut());
    app.world_mut()
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(command));
    let (_, set, _) = only(collect_node_changes(app.world_mut()));
    assert_eq!(translation_x(&set), 7.0);
}

#[test]
fn pie_sync_names_a_removed_component() {
    let (mut app, _dir, rock) = playing();
    rename(&mut app, rock, "Boulder");
    let (_, set, removed) = only(collect_node_changes(app.world_mut()));
    assert_eq!(set, vec![(NAME.to_string(), Value::from("Boulder"))]);
    assert!(removed.is_empty());

    rename(&mut app, rock, "");
    let (_, set, removed) = only(collect_node_changes(app.world_mut()));
    assert!(set.is_empty());
    assert_eq!(removed, vec![NAME.to_string()]);
}

/// The game side of the link before its scene has spawned.
fn unspawned_game() -> App {
    let mut game = App::new();
    game.add_plugins(MinimalPlugins);
    game.register_type::<Transform>();
    game.register_type::<Name>();
    game.register_type::<SceneNodeId>();
    jackdaw_runtime::node_sync::install_node_index(&mut game);
    game
}

/// The game side of the link: a world whose entity came from the editor's node.
fn game(node: SceneNodeId) -> (App, Entity) {
    let mut game = unspawned_game();
    let entity = game.world_mut().spawn((node, Transform::default())).id();
    (game, entity)
}

fn deliver(
    editor: &mut App,
    link: &mut LoopbackTransport,
    game_end: &mut LoopbackTransport,
    game: &mut App,
) {
    let events = collect_node_changes(editor.world_mut());
    send(events, link, game_end, game);
}

fn send(
    events: Vec<ControlEvent>,
    link: &mut LoopbackTransport,
    game_end: &mut LoopbackTransport,
    game: &mut App,
) {
    for event in events {
        link.send(PieChannel::Reliable, &to_bytes(&event).expect("encode"));
    }
    for (_, bytes) in game_end.drain_received() {
        if let Ok(ControlEvent::NodeComponents { node, set, removed }) = from_bytes(&bytes) {
            jackdaw_runtime::node_sync::apply_node_components(
                game.world_mut(),
                node,
                &set,
                &removed,
            );
        }
    }
    game.update();
}

#[test]
fn pie_sync_an_edit_and_its_undo_land_in_the_game() {
    let (mut app, _dir, rock) = playing();
    let (mut game, entity) = game(rock);
    let (mut link, mut game_end) = LoopbackTransport::pair();
    let x = |game: &App| {
        game.world()
            .get::<Transform>(entity)
            .expect("a transform")
            .translation
            .x
    };

    move_to(&mut app, rock, 4.0);
    deliver(&mut app, &mut link, &mut game_end, &mut game);
    assert_eq!(x(&game), 4.0);

    move_to(&mut app, rock, 9.0);
    deliver(&mut app, &mut link, &mut game_end, &mut game);
    assert_eq!(x(&game), 9.0);

    history(&mut app, "history.undo");
    deliver(&mut app, &mut link, &mut game_end, &mut game);
    assert_eq!(x_of(&mut app, rock), 4.0);
    assert_eq!(x(&game), 4.0);
}

const VISIBILITY: &str = "bevy_camera::visibility::Visibility";
const BRUSH: &str = "jackdaw_scene_types::types::Brush";

fn set_field(app: &mut App, node: SceneNodeId, type_path: &str, field: &str, value: &str) {
    let entity = entity_of(app, node);
    app.world_mut()
        .operator("component.set")
        .param("entity", entity)
        .param("type_path", type_path.to_string())
        .param("field", field.to_string())
        .param("value", value.to_string())
        .call()
        .expect("component.set dispatches")
        .assert_finished();
    app.update();
}

fn sent<'a>(set: &'a [(String, Value)], type_path: &str) -> &'a Value {
    &set.iter()
        .find(|(path, _)| path == type_path)
        .unwrap_or_else(|| panic!("{type_path} was sent: {set:?}"))
        .1
}

#[test]
fn pie_sync_undoing_a_derived_component_edit_sends_its_value() {
    let (mut app, _dir, rock) = playing();
    set_field(&mut app, rock, VISIBILITY, "", "\"Hidden\"");
    let (_, set, removed) = only(collect_node_changes(app.world_mut()));
    assert_eq!(sent(&set, VISIBILITY), &Value::from("Hidden"));
    assert!(removed.is_empty());

    history(&mut app, "history.undo");
    let (_, set, removed) = only(collect_node_changes(app.world_mut()));
    assert!(
        removed.is_empty(),
        "the component stays on the entity, so the game keeps it too"
    );
    assert_eq!(sent(&set, VISIBILITY), &Value::from("Inherited"));
}

/// A material the open scene embeds as `name`, as a loaded scene file's named
/// asset entries are.
pub(super) fn embed_as(world: &mut World, name: &str) -> Handle<StandardMaterial> {
    let stone = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let mut assets = world.get_resource_or_init::<BsnSceneAssets>();
    assets.0.insert(name.to_string(), stone.clone().untyped());
    stone
}

fn embed_stone(world: &mut World) -> Handle<StandardMaterial> {
    embed_as(world, "#Stone")
}

#[test]
fn pie_sync_a_brush_face_edit_reaches_the_game_with_its_material() {
    let (mut app, _dir, rock) = playing();
    embed_stone(app.world_mut());
    set_field(&mut app, rock, BRUSH, "faces[0].material", "\"#Stone\"");
    let (_, set, _) = only(collect_node_changes(app.world_mut()));
    let name = sent(&set, BRUSH)["faces"][0]["material"]
        .as_str()
        .expect("the material is named")
        .to_string();
    assert!(name.starts_with("#Play"), "a session name: {name}");

    let (mut game, entity) = game(rock);
    game.add_plugins(bevy::asset::AssetPlugin::default());
    game.init_asset::<StandardMaterial>();
    game.register_asset_reflect::<StandardMaterial>();
    game.register_type::<Brush>();
    let game_stone = embed_as(game.world_mut(), &name);
    game.world_mut()
        .entity_mut(entity)
        .insert(Brush::cuboid(0.5, 0.5, 0.5));
    let (mut link, mut game_end) = LoopbackTransport::pair();

    set_field(&mut app, rock, BRUSH, "faces[0].uv_offset", "[0.5, 0.25]");
    deliver(&mut app, &mut link, &mut game_end, &mut game);
    let brush = game.world().get::<Brush>(entity).expect("a brush");
    assert_eq!(brush.faces[0].uv_offset, Vec2::new(0.5, 0.25));
    assert_eq!(brush.faces[0].material, game_stone);
}

/// A handle whose asset is gone from its store has no definition to send.
#[test]
fn pie_sync_a_material_with_no_definition_is_held_back_until_it_can_be_sent() {
    let (mut app, _dir, rock) = playing();
    let stone = embed_stone(app.world_mut());
    set_field(&mut app, rock, BRUSH, "faces[0].material", "\"#Stone\"");
    app.world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .remove(&stone);
    assert!(
        collect_node_changes(app.world_mut()).is_empty(),
        "a material that cannot be embedded is not sent"
    );

    app.world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .insert(stone.id(), StandardMaterial::default())
        .expect("a live handle");
    move_to(&mut app, rock, 2.0);
    let (_, set, _) = only(collect_node_changes(app.world_mut()));
    assert_eq!(translation_x(&set), 2.0);
    assert!(
        sent(&set, BRUSH)["faces"][0]["material"]
            .as_str()
            .is_some_and(|name| name.starts_with("#Play")),
        "the held-back component goes with the node's next edit"
    );
}

/// A component holding a list, registered in both the editor and the game.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
struct Waypoints {
    points: Vec<f32>,
}

/// An immutable component, registered in both the editor and the game.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[component(immutable)]
#[reflect(Component, Default)]
struct Tint {
    level: u32,
}

fn add_component(app: &mut App, node: SceneNodeId, type_path: &str) {
    let entity = entity_of(app, node);
    app.world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param("type_path", type_path.to_string())
        .call()
        .expect("component.add dispatches")
        .assert_finished();
    app.update();
}

#[test]
fn pie_sync_a_shorter_list_and_an_immutable_edit_land_in_the_game() {
    let (mut app, _dir, rock) = playing();
    app.register_type::<Waypoints>().register_type::<Tint>();
    let waypoints = Waypoints::type_path();
    let tint = Tint::type_path();
    let (mut game, entity) = game(rock);
    game.register_type::<Waypoints>().register_type::<Tint>();
    let (mut link, mut game_end) = LoopbackTransport::pair();

    add_component(&mut app, rock, waypoints);
    set_field(&mut app, rock, waypoints, "points", "[1.0, 2.0, 3.0]");
    add_component(&mut app, rock, tint);
    deliver(&mut app, &mut link, &mut game_end, &mut game);
    assert_eq!(
        game.world()
            .get::<Waypoints>(entity)
            .map(|w| w.points.clone()),
        Some(vec![1.0, 2.0, 3.0])
    );

    set_field(&mut app, rock, waypoints, "", "{\"points\": [7.0]}");
    set_field(&mut app, rock, tint, "level", "5");
    deliver(&mut app, &mut link, &mut game_end, &mut game);
    assert_eq!(
        game.world()
            .get::<Waypoints>(entity)
            .map(|w| w.points.clone()),
        Some(vec![7.0])
    );
    assert_eq!(game.world().get::<Tint>(entity), Some(&Tint { level: 5 }));
}

fn game_x(game: &App, entity: Entity) -> f32 {
    game.world()
        .get::<Transform>(entity)
        .expect("a transform")
        .translation
        .x
}

/// An edit reaching the game while a scene root waits to spawn lands on the
/// entity that root brings, over the older value in the text it spawned from.
#[test]
fn pie_sync_an_edit_made_before_the_scene_spawns_lands_once_it_spawns() {
    use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};

    let (mut app, _dir, rock) = playing();
    let mut game = App::new();
    game.add_plugins(MinimalPlugins);
    game.add_plugins(bevy::transform::TransformPlugin);
    game.add_plugins(bevy::asset::AssetPlugin::default());
    game.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    jackdaw_runtime::node_sync::install_node_index(&mut game);
    game.add_plugins(JackdawPlugin);
    game.register_type::<Name>();
    let scene = game
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .reserve_handle();
    game.world_mut().spawn(JackdawSceneRoot(scene.clone()));
    let (mut link, mut game_end) = LoopbackTransport::pair();

    move_to(&mut app, rock, 6.0);
    rename(&mut app, rock, "Boulder");
    deliver(&mut app, &mut link, &mut game_end, &mut game);

    let text = format!(
        "#Rock\njackdaw_scene_types::node_id::SceneNodeId({})\n{TRANSFORM}\n",
        rock.0
    );
    game.world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .insert(
            scene.id(),
            JackdawScene::new(text, std::path::PathBuf::new()),
        )
        .expect("a live handle");
    game.update();
    let entity = game
        .world_mut()
        .query::<(Entity, &SceneNodeId)>()
        .iter(game.world())
        .find(|(_, node)| **node == rock)
        .map(|(entity, _)| entity)
        .expect("the scene spawned the node");
    assert_eq!(game_x(&game, entity), 6.0);
    assert_eq!(
        game.world().get::<Name>(entity).map(Name::as_str),
        Some("Boulder")
    );

    move_to(&mut app, rock, 2.0);
    deliver(&mut app, &mut link, &mut game_end, &mut game);
    assert_eq!(game_x(&game, entity), 2.0, "edits keep flowing after it");
}

/// Change the brush the way the brush tools do: on the entity, which the
/// editor mirrors into the document.
pub(super) fn edit_brush(app: &mut App, rock: SceneNodeId, edit: impl FnOnce(&mut Brush)) {
    let entity = entity_of(app, rock);
    let world = app.world_mut();
    let mut brush = world.get::<Brush>(entity).expect("a brush").clone();
    edit(&mut brush);
    world.entity_mut(entity).insert(brush.clone());
    jackdaw::brush::sync_brush_to_ast(world, entity, &brush);
    app.update();
}

#[test]
fn pie_sync_a_saved_material_goes_by_a_session_name_no_file_holds() {
    let (mut app, dir, rock) = playing();
    let material = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    edit_brush(&mut app, rock, |brush| brush.faces[0].material = material);
    let path = dir.path().join("assets/scenes/level.bsn");
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("a scenes dir");
    app.world_mut()
        .operator("scene.save")
        .param("path", path.to_string_lossy().into_owned())
        .call()
        .expect("scene.save dispatches")
        .assert_finished();
    app.update();
    let saved = std::fs::read_to_string(&path).expect("the saved scene");

    edit_brush(&mut app, rock, |brush| {
        brush.faces[0].uv_offset = Vec2::new(0.5, 0.25);
    });
    let (_, set, _) = only(collect_node_changes(app.world_mut()));
    let brush = sent(&set, BRUSH);
    assert_eq!(brush["faces"][0]["uv_offset"][0], 0.5);
    let name = brush["faces"][0]["material"]
        .as_str()
        .expect("the material is named");
    assert!(
        name.starts_with("#Play") && !saved.contains(name),
        "the saved file's name is not reused: {name}"
    );
}

/// A starter scene: a root holding a camera and a light, none of them with a
/// node id.
const STARTER_SCENE: &str = "\
bevy_ecs::hierarchy::Children [
    #Root
    bevy_transform::components::transform::Transform
    bevy_ecs::hierarchy::Children [
        #Camera
        bevy_transform::components::transform::Transform
        --
        #Sun
        bevy_transform::components::transform::Transform
    ]
]
";

fn named(app: &mut App, name: &str) -> SceneNodeId {
    app.world_mut()
        .query::<(&Name, &SceneNodeId)>()
        .iter(app.world())
        .find(|(held, _)| held.as_str() == name)
        .map(|(_, node)| *node)
        .unwrap_or_else(|| panic!("{name} holds a node id"))
}

#[test]
fn pie_sync_an_edit_to_a_nested_node_of_an_opened_scene_lands_in_the_game() {
    let (mut app, dir) = editor();
    let path = dir.path().join("assets/scenes/start.bsn");
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("a scenes dir");
    std::fs::write(&path, STARTER_SCENE).expect("the scene");
    app.world_mut()
        .operator("scene.open")
        .param("path", path.to_string_lossy().into_owned())
        .call()
        .expect("scene.open dispatches")
        .assert_finished();
    app.update();
    app.update();
    app.world_mut()
        .operator("scene.save")
        .param("path", path.to_string_lossy().into_owned())
        .call()
        .expect("scene.save dispatches")
        .assert_finished();
    app.update();
    let sun = named(&mut app, "Sun");

    let mut game = unspawned_game();
    let saved = std::fs::read_to_string(&path).expect("the saved scene");
    jackdaw_bsn::load_bsn_scene(game.world_mut(), &saved).expect("the game loads the scene");
    game.update();
    let entity = game
        .world_mut()
        .query::<(Entity, &SceneNodeId)>()
        .iter(game.world())
        .find(|(_, node)| **node == sun)
        .map(|(entity, _)| entity)
        .expect("the game's light carries the editor's node id");
    let (mut link, mut game_end) = LoopbackTransport::pair();
    assert!(collect_node_changes(app.world_mut()).is_empty());

    move_to(&mut app, sun, 3.0);
    deliver(&mut app, &mut link, &mut game_end, &mut game);
    assert_eq!(game_x(&game, entity), 3.0);
}

#[test]
fn a_list_field_set_shorter_by_path_loses_its_tail_and_undo_restores_it() {
    let (mut app, _dir, rock) = playing();
    app.register_type::<Waypoints>();
    let waypoints = Waypoints::type_path();
    add_component(&mut app, rock, waypoints);
    set_field(&mut app, rock, waypoints, "points", "[1.0, 2.0, 3.0]");

    set_field(&mut app, rock, waypoints, "points", "[7.0]");
    let entity = entity_of(&mut app, rock);
    let points = |app: &App| {
        app.world()
            .get::<Waypoints>(entity)
            .map(|w| w.points.clone())
    };
    assert_eq!(points(&app), Some(vec![7.0]));
    let (_, set, _) = only(collect_node_changes(app.world_mut()));
    assert_eq!(sent(&set, waypoints)["points"], serde_json::json!([7.0]));

    history(&mut app, "history.undo");
    assert_eq!(points(&app), Some(vec![1.0, 2.0, 3.0]));
}

#[test]
fn a_brush_field_set_by_path_keeps_the_face_materials() {
    let (mut app, _dir, rock) = playing();
    let stone = embed_stone(app.world_mut());
    set_field(&mut app, rock, BRUSH, "faces[0].material", "\"#Stone\"");
    let entity = entity_of(&mut app, rock);
    let material = |app: &App| {
        app.world()
            .get::<Brush>(entity)
            .map(|b| b.faces[0].material.clone())
    };
    assert_eq!(material(&app), Some(stone.clone()));

    set_field(&mut app, rock, BRUSH, "faces[0].uv_scale", "[2.0, 2.0]");
    assert_eq!(material(&app), Some(stone.clone()));
    set_field(&mut app, rock, BRUSH, "faces[1].uv_offset", "[0.5, 0.5]");
    assert_eq!(material(&app), Some(stone));
    let (_, set, _) = only(collect_node_changes(app.world_mut()));
    let sent_name = sent(&set, BRUSH)["faces"][0]["material"]
        .as_str()
        .unwrap_or_default();
    assert!(sent_name.starts_with("#Play"), "still named: {sent_name}");
}
