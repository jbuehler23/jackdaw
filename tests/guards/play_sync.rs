//! Document edits made while playing reach the game as node component
//! updates, whichever path made them: an operator, undo, redo, or a gizmo
//! drag committed on release.

use bevy::prelude::*;
use jackdaw::commands::SetTransform;
use jackdaw::play_sync::collect_node_changes;
use jackdaw_api::prelude::*;
use jackdaw_commands::{CommandHistory, EditorCommand};
use jackdaw_pie_protocol::event::{from_bytes, to_bytes};
use jackdaw_pie_protocol::{ControlEvent, LoopbackTransport, PieChannel, PieTransport};
use jackdaw_scene_types::SceneNodeId;
use serde_json::Value;

use super::batch_undo::{editor, history, menu};
use crate::util::OperatorResultExt as _;

const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
const NAME: &str = "bevy_ecs::name::Name";

fn cube(app: &mut App) -> SceneNodeId {
    let before = nodes(app);
    menu(app, "entity.add.cube", true);
    nodes(app)
        .into_iter()
        .find(|node| !before.contains(node))
        .expect("the menu added a node")
}

fn nodes(app: &mut App) -> Vec<SceneNodeId> {
    app.world_mut()
        .query::<&SceneNodeId>()
        .iter(app.world())
        .copied()
        .collect()
}

fn entity_of(app: &mut App, node: SceneNodeId) -> Entity {
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

fn move_to(app: &mut App, node: SceneNodeId, x: f64) {
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

/// The game side of the link: a world whose entity came from the editor's node.
fn game(node: SceneNodeId) -> (App, Entity) {
    let mut game = App::new();
    game.add_plugins(MinimalPlugins);
    game.register_type::<Transform>();
    game.register_type::<Name>();
    game.register_type::<SceneNodeId>();
    jackdaw_runtime::node_sync::install_node_index(&mut game);
    let entity = game.world_mut().spawn((node, Transform::default())).id();
    (game, entity)
}

fn deliver(
    editor: &mut App,
    link: &mut LoopbackTransport,
    game_end: &mut LoopbackTransport,
    game: &mut App,
) {
    for event in collect_node_changes(editor.world_mut()) {
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
