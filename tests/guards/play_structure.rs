//! Adding, deleting and reparenting document nodes while playing reaches the game
//! over the loopback, undo and redo included: the game ends up matching the
//! document after every step.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use jackdaw::boot_ops::run_op_clause;
use jackdaw::pie::InstanceKey;
use jackdaw::play_scene::{LaunchScene, PlayScene, serve_scene};
use jackdaw::play_sync::{collect_changes_for, collect_node_changes};
use jackdaw_api::prelude::*;
use jackdaw_pie_protocol::event::{from_bytes, to_bytes};
use jackdaw_pie_protocol::{ControlEvent, LoopbackTransport, PieChannel, PieTransport};
use jackdaw_runtime::node_structure::{StructureChange, apply_structure_change};
use jackdaw_runtime::node_sync::{apply_node_components, load_asset_files, load_scene_assets};
use jackdaw_runtime::play_overlay::load_prefab_files;
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};
use jackdaw_scene_types::{Brush, SceneNodeId};

use super::batch_undo::{editor, history, menu};
use super::play_sync::{cube, edit_brush, entity_of, move_to, nodes};
use crate::util::OperatorResultExt as _;

const SCENE: &str = "scenes/level.bsn";

fn key() -> InstanceKey {
    InstanceKey {
        config: "game".to_string(),
        instance: 1,
    }
}

struct Session {
    app: App,
    dir: tempfile::TempDir,
    game: App,
    root: Entity,
    link: LoopbackTransport,
    game_end: LoopbackTransport,
}

fn save(app: &mut App, dir: &tempfile::TempDir) {
    let path = dir.path().join("assets").join(SCENE);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("a scenes dir");
    app.world_mut()
        .operator("scene.save")
        .param("path", path.to_string_lossy().into_owned())
        .call()
        .expect("scene.save dispatches")
        .assert_finished();
    app.update();
}

fn game_app(dir: &tempfile::TempDir) -> (App, Entity) {
    let mut game = App::new();
    game.add_plugins(MinimalPlugins);
    game.add_plugins(bevy::transform::TransformPlugin);
    game.add_plugins(bevy::asset::AssetPlugin {
        file_path: dir.path().join("assets").to_string_lossy().into_owned(),
        ..Default::default()
    });
    game.add_plugins(bevy::image::ImagePlugin::default());
    game.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    game.init_asset::<Mesh>();
    game.init_asset::<StandardMaterial>();
    game.register_asset_reflect::<StandardMaterial>();
    jackdaw_runtime::node_sync::install_node_index(&mut game);
    game.add_plugins(JackdawPlugin);
    game.register_type::<Name>();
    let scene: Handle<JackdawScene> = game
        .world()
        .resource::<AssetServer>()
        .load(SCENE.to_string());
    let root = game.world_mut().spawn(JackdawSceneRoot(scene)).id();
    (game, root)
}

/// A game playing the scene `setup` built, spawned, with the editor's document
/// counted as sent.
fn session(setup: impl FnOnce(&mut App)) -> Session {
    let mut session = unspawned_session(setup);
    session.wait_for_scene();
    session
}

/// [`session`] with the game's scene still loading.
fn unspawned_session(setup: impl FnOnce(&mut App)) -> Session {
    let (mut app, dir) = editor();
    setup(&mut app);
    save(&mut app, &dir);
    assert!(collect_node_changes(app.world_mut()).is_empty());
    serve_scene(app.world_mut(), &key(), None, SCENE);
    let (game, root) = game_app(&dir);
    let (link, game_end) = LoopbackTransport::pair();
    Session {
        app,
        dir,
        game,
        root,
        link,
        game_end,
    }
}

impl Session {
    fn wait_for_scene(&mut self) {
        let start = Instant::now();
        while self
            .game
            .world()
            .get::<jackdaw_scene_types::SceneRootTag>(self.root)
            .is_none()
        {
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "the scene spawned"
            );
            self.game.update();
            std::thread::sleep(Duration::from_millis(2));
        }
        self.game.update();
    }

    /// Everything the document sends this frame, read by the game in order.
    fn deliver(&mut self) -> Vec<ControlEvent> {
        let events = self.read_without_updating();
        self.game.update();
        self.game.update();
        events
    }

    /// [`Self::deliver`] with the game's frame still to come, so a scene that has
    /// not spawned cannot spawn meanwhile.
    fn read_without_updating(&mut self) -> Vec<ControlEvent> {
        let events: Vec<ControlEvent> = collect_changes_for(self.app.world_mut(), &[key()])
            .into_iter()
            .map(|(_, event)| event)
            .collect();
        for event in &events {
            self.link
                .send(PieChannel::Reliable, &to_bytes(event).expect("encode"));
        }
        for (_, bytes) in self.game_end.drain_received() {
            read_in_game(self.game.world_mut(), from_bytes(&bytes).expect("decode"));
        }
        events
    }

    #[track_caller]
    fn assert_matches(&mut self) {
        self.deliver();
        assert_eq!(
            document_of(self.game.world_mut()),
            document_of(self.app.world_mut()),
            "the game matches the document"
        );
    }

    fn holders(&mut self, node: SceneNodeId) -> Vec<Entity> {
        holders(self.game.world_mut(), node)
    }

    fn only_holder(&mut self, node: SceneNodeId) -> Entity {
        let held = self.holders(node);
        assert_eq!(held.len(), 1, "one entity of {node:?}");
        held[0]
    }
}

fn read_in_game(world: &mut World, event: ControlEvent) {
    match event {
        ControlEvent::NodeComponents { node, set, removed } => {
            apply_node_components(world, node, &set, &removed);
        }
        ControlEvent::SceneAssets { text } => load_scene_assets(world, &text),
        ControlEvent::AssetFiles { files } => load_asset_files(world, &files),
        ControlEvent::PrefabFiles { files } => load_prefab_files(world, &files),
        ControlEvent::SpawnNode {
            node,
            parent,
            follows,
            scene,
            dir,
            bsn,
        } => apply_structure_change(
            world,
            StructureChange::Spawn {
                node,
                parent,
                follows,
                scene,
                dir,
                bsn,
            },
        ),
        ControlEvent::DespawnNode { node } => {
            apply_structure_change(world, StructureChange::Despawn { node });
        }
        ControlEvent::ReparentNode {
            node,
            parent,
            follows,
        } => apply_structure_change(
            world,
            StructureChange::Reparent {
                node,
                parent,
                follows,
            },
        ),
        other => panic!("not document sync: {other:?}"),
    }
}

fn holders(world: &mut World, node: SceneNodeId) -> Vec<Entity> {
    world
        .query::<(Entity, &SceneNodeId)>()
        .iter(world)
        .filter(|(_, held)| **held == node)
        .map(|(entity, _)| entity)
        .collect()
}

/// What each node's entities are: their parent nodes, their place among the
/// parent's nodes, their translation in thousandths and whether they hold a brush.
/// Only the game has an entity above the top of the scene, so a place there is left
/// out.
fn document_of(world: &mut World) -> BTreeMap<u64, Vec<(Option<u64>, usize, [i64; 3], bool)>> {
    let mut query = world.query::<(Entity, &SceneNodeId, &Transform, Option<&Brush>)>();
    let mut held: BTreeMap<u64, Vec<(Option<u64>, usize, [i64; 3], bool)>> = BTreeMap::new();
    for (entity, node, transform, brush) in query.iter(world) {
        let parent = world.get::<ChildOf>(entity).map(ChildOf::parent);
        let parent_node = parent
            .and_then(|parent| world.get::<SceneNodeId>(parent))
            .map(|node| node.0);
        let translation = transform
            .translation
            .to_array()
            .map(|x| (x * 1000.0) as i64);
        let place = parent
            .filter(|_| parent_node.is_some())
            .and_then(|parent| world.get::<Children>(parent))
            .map_or(0, |children| {
                children
                    .iter()
                    .filter(|child| world.get::<SceneNodeId>(*child).is_some())
                    .position(|child| child == entity)
                    .unwrap_or(0)
            });
        held.entry(node.0)
            .or_default()
            .push((parent_node, place, translation, brush.is_some()));
    }
    for entities in held.values_mut() {
        entities.sort();
    }
    held
}

fn reparent(app: &mut App, child: SceneNodeId, parent: SceneNodeId) {
    let child = entity_of(app, child);
    let parent = entity_of(app, parent);
    app.world_mut()
        .operator("entity.reparent")
        .param("child", child)
        .param("parent", parent)
        .call()
        .expect("entity.reparent dispatches")
        .assert_finished();
    app.update();
}

fn delete(app: &mut App, node: SceneNodeId) {
    let entity = entity_of(app, node);
    app.world_mut()
        .operator("entity.delete")
        .param("entity", entity)
        .call()
        .expect("entity.delete dispatches")
        .assert_finished();
    app.update();
}

fn face_meshes(game: &mut App, brush: Entity) -> usize {
    let children: Vec<Entity> = game
        .world()
        .get::<Children>(brush)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    children
        .into_iter()
        .filter(|child| game.world().get::<Mesh3d>(*child).is_some())
        .count()
}

#[test]
fn pie_structure_a_brush_added_reaches_the_game_with_its_mesh_and_undoes() {
    let mut s = session(|_| {});
    let added = cube(&mut s.app);
    s.assert_matches();
    let brush = s.only_holder(added);
    s.game.update();
    assert!(face_meshes(&mut s.game, brush) > 0, "the brush has a mesh");

    history(&mut s.app, "history.undo");
    s.assert_matches();
    assert!(s.holders(added).is_empty());

    history(&mut s.app, "history.redo");
    s.assert_matches();
    let brush = s.only_holder(added);
    s.game.update();
    assert!(face_meshes(&mut s.game, brush) > 0, "the brush has a mesh");
}

#[test]
fn pie_structure_a_prop_deleted_leaves_the_game_and_undo_brings_it_back() {
    let mut prop = None;
    let mut s = session(|app| {
        let rock = cube(app);
        move_to(app, rock, 4.0);
        prop = Some(rock);
    });
    let prop = prop.expect("a prop");
    s.assert_matches();
    assert_eq!(s.holders(prop).len(), 1);

    delete(&mut s.app, prop);
    s.assert_matches();
    assert!(s.holders(prop).is_empty());

    history(&mut s.app, "history.undo");
    s.assert_matches();
    let back = s.only_holder(prop);
    assert_eq!(
        s.game
            .world()
            .get::<Transform>(back)
            .expect("a transform")
            .translation
            .x,
        4.0
    );

    history(&mut s.app, "history.redo");
    s.assert_matches();
    assert!(s.holders(prop).is_empty());
}

#[test]
fn pie_structure_a_reparent_moves_the_entity_keeps_it_and_undoes() {
    let mut made = Vec::new();
    let mut s = session(|app| {
        made.push(cube(app));
        made.push(cube(app));
    });
    let (child, parent) = (made[0], made[1]);
    let entity = s.only_holder(child);
    let parent_entity = s.only_holder(parent);

    reparent(&mut s.app, child, parent);
    s.assert_matches();
    assert_eq!(
        s.game.world().get::<ChildOf>(entity).map(ChildOf::parent),
        Some(parent_entity),
        "the same entity moved"
    );

    history(&mut s.app, "history.undo");
    s.assert_matches();
    assert_eq!(
        s.game.world().get::<ChildOf>(entity).map(ChildOf::parent),
        Some(s.root)
    );
    assert_eq!(s.only_holder(child), entity);

    history(&mut s.app, "history.redo");
    s.assert_matches();
    assert_eq!(
        s.game.world().get::<ChildOf>(entity).map(ChildOf::parent),
        Some(parent_entity)
    );
}

#[test]
fn pie_structure_a_subtree_goes_and_comes_back_as_one() {
    let mut made = Vec::new();
    let mut s = session(|app| {
        made.push(cube(app));
        made.push(cube(app));
        made.push(cube(app));
        reparent(app, made[1], made[0]);
        reparent(app, made[2], made[1]);
    });
    let (top, middle, leaf) = (made[0], made[1], made[2]);
    s.assert_matches();

    delete(&mut s.app, top);
    let events = s.deliver();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ControlEvent::DespawnNode { .. }))
            .count(),
        1,
        "only the top of the subtree: {events:?}"
    );
    assert!(s.holders(leaf).is_empty());

    history(&mut s.app, "history.undo");
    s.assert_matches();
    assert_eq!(s.holders(middle).len(), 1);
    assert_eq!(s.holders(leaf).len(), 1);
}

#[test]
fn pie_structure_a_prefab_instance_spawns_resolved_and_undoes() {
    let mut s = session(|_| {});
    let before = nodes(&mut s.app);
    s.app
        .world_mut()
        .operator("prefab.spawn_instance")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("path", "prefabs/rock.bsn")
        .param("pos_x", 2.0)
        .param("pos_y", 0.0)
        .param("pos_z", 0.0)
        .call()
        .expect("dispatch")
        .assert_finished();
    s.app.update();
    let instance = nodes(&mut s.app)
        .into_iter()
        .find(|node| !before.contains(node))
        .expect("an instance node");
    s.assert_matches();
    let entity = s.only_holder(instance);
    let in_editor = entity_of(&mut s.app, instance);
    assert_eq!(
        s.game.world().get::<Name>(entity).map(ToString::to_string),
        s.app
            .world()
            .get::<Name>(in_editor)
            .map(ToString::to_string),
    );
    assert!(
        s.game.world().get::<jackdaw_prefab::IsA>(entity).is_some(),
        "the game knows it as an instance"
    );

    history(&mut s.app, "history.undo");
    s.assert_matches();
    assert!(s.holders(instance).is_empty());

    history(&mut s.app, "history.redo");
    s.assert_matches();
    assert_eq!(s.holders(instance).len(), 1);
}

#[test]
fn pie_structure_an_entity_added_with_a_material_brings_the_material() {
    let mut s = session(|_| {});
    let added = cube(&mut s.app);
    let material = s
        .app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            perceptual_roughness: 0.25,
            ..Default::default()
        });
    edit_brush(&mut s.app, added, |brush| {
        brush.faces[0].material = material;
    });

    let events = s.deliver();
    let first_spawn = events
        .iter()
        .position(|event| matches!(event, ControlEvent::SpawnNode { .. }))
        .expect("a spawn");
    let (ControlEvent::SpawnNode { bsn, .. }, Some(ControlEvent::SceneAssets { text })) = (
        &events[first_spawn],
        events[..first_spawn]
            .iter()
            .find(|event| matches!(event, ControlEvent::SceneAssets { .. })),
    ) else {
        panic!("the material goes ahead of the spawn: {events:?}");
    };
    assert!(bsn.contains("#Play"), "a session name: {bsn}");
    assert!(text.contains("#Play") || text.contains("Play"), "{text}");

    let brush = s.only_holder(added);
    let handle = s.game.world().get::<Brush>(brush).expect("a brush").faces[0]
        .material
        .clone();
    let materials = s.game.world().resource::<Assets<StandardMaterial>>();
    assert_eq!(
        materials
            .get(&handle)
            .expect("the game holds the material")
            .perceptual_roughness,
        0.25
    );

    let again = cube(&mut s.app);
    let earlier = entity_of(&mut s.app, added);
    let same = s.app.world().get::<Brush>(earlier).expect("a brush").faces[0]
        .material
        .clone();
    edit_brush(&mut s.app, again, |brush| {
        brush.faces[0].material = same;
    });
    let events = s.deliver();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ControlEvent::SceneAssets { .. })),
        "a material the game holds does not go again: {events:?}"
    );
}

#[test]
fn pie_structure_an_edit_in_the_frame_of_the_spawn_lands_once() {
    let mut s = session(|_| {});
    let added = cube(&mut s.app);
    move_to(&mut s.app, added, 7.0);
    let events = s.deliver();
    assert!(
        !events.iter().any(
            |event| matches!(event, ControlEvent::NodeComponents { node, .. } if *node == added.0)
        ),
        "the text holds the edit: {events:?}"
    );
    s.assert_matches();
    let entity = s.only_holder(added);
    assert_eq!(
        s.game
            .world()
            .get::<Transform>(entity)
            .expect("a transform")
            .translation
            .x,
        7.0
    );

    move_to(&mut s.app, added, 9.0);
    s.assert_matches();
}

#[test]
fn pie_structure_a_new_parent_arrives_ahead_of_the_reparent_under_it() {
    let mut made = Vec::new();
    let mut s = session(|app| {
        made.push(cube(app));
    });
    let child = made[0];
    let entity = s.only_holder(child);
    let parent = cube(&mut s.app);
    reparent(&mut s.app, child, parent);
    let events = s.deliver();
    let spawn = events
        .iter()
        .position(
            |event| matches!(event, ControlEvent::SpawnNode { node, .. } if *node == parent.0),
        )
        .expect("the parent spawns");
    let moved = events
        .iter()
        .position(
            |event| matches!(event, ControlEvent::ReparentNode { node, .. } if *node == child.0),
        )
        .expect("the child moves");
    assert!(spawn < moved, "{events:?}");
    if let ControlEvent::SpawnNode { bsn, .. } = &events[spawn] {
        assert!(
            !bsn.contains(&child.0.to_string()),
            "the text leaves out a node that exists: {bsn}"
        );
    }
    s.assert_matches();
    assert_eq!(s.only_holder(child), entity, "the entity was kept");
}

#[test]
fn pie_structure_a_child_moves_out_before_its_parent_goes() {
    let mut made = Vec::new();
    let mut s = session(|app| {
        made.push(cube(app));
        made.push(cube(app));
        made.push(cube(app));
        reparent(app, made[1], made[0]);
    });
    let (parent, child, other) = (made[0], made[1], made[2]);
    let entity = s.only_holder(child);
    reparent(&mut s.app, child, other);
    delete(&mut s.app, parent);
    s.assert_matches();
    assert_eq!(
        s.only_holder(child),
        entity,
        "the entity outlived its parent"
    );
    assert!(s.holders(parent).is_empty());
}

#[test]
fn pie_structure_a_delete_takes_what_the_game_put_under_the_node() {
    let mut made = Vec::new();
    let mut s = session(|app| {
        made.push(cube(app));
    });
    let prop = made[0];
    let entity = s.only_holder(prop);
    let extra = s.game.world_mut().spawn(ChildOf(entity)).id();

    delete(&mut s.app, prop);
    s.assert_matches();
    assert!(s.game.world().get_entity(extra).is_err());
    s.game.update();
}

#[test]
fn pie_structure_a_spawn_before_the_scene_spawned_waits_for_it() {
    let mut s = unspawned_session(|_| {});
    let added = cube(&mut s.app);
    s.read_without_updating();
    assert!(s.holders(added).is_empty(), "no root to spawn it under yet");
    s.wait_for_scene();
    s.assert_matches();
    assert_eq!(s.holders(added).len(), 1);
}

#[test]
fn pie_structure_a_spawn_the_scene_already_holds_does_not_double() {
    let mut s = session(|_| {});
    let (mut game, root) = game_app(&s.dir);
    let added = cube(&mut s.app);
    save(&mut s.app, &s.dir);
    let events: Vec<ControlEvent> = collect_changes_for(s.app.world_mut(), &[key()])
        .into_iter()
        .map(|(_, event)| event)
        .collect();
    assert!(!events.is_empty());
    for event in events {
        read_in_game(game.world_mut(), event);
    }
    s.game = game;
    s.root = root;
    s.wait_for_scene();
    assert_eq!(s.holders(added).len(), 1);
}

#[test]
fn pie_structure_edits_held_for_a_spawn_keep_their_order_with_it() {
    let mut made = Vec::new();
    let mut s = unspawned_session(|app| {
        made.push(cube(app));
    });
    let rock = made[0];
    move_to(&mut s.app, rock, 3.0);
    s.read_without_updating();
    delete(&mut s.app, rock);
    s.read_without_updating();
    history(&mut s.app, "history.undo");
    s.read_without_updating();
    move_to(&mut s.app, rock, 5.0);
    s.read_without_updating();
    s.wait_for_scene();
    s.assert_matches();
    let entity = s.only_holder(rock);
    assert_eq!(
        s.game
            .world()
            .get::<Transform>(entity)
            .expect("a transform")
            .translation
            .x,
        5.0
    );
}

#[test]
fn pie_structure_only_the_document_the_game_plays_syncs_across_a_tab_switch() {
    let mut made = Vec::new();
    let mut s = session(|app| {
        made.push(cube(app));
    });
    let rock = made[0];
    let before = document_of(s.game.world_mut());

    run_op_clause(s.app.world_mut(), "scene.new")
        .expect("scene.new dispatches")
        .assert_finished();
    s.app.update();
    let events = s.deliver();
    assert!(events.is_empty(), "a switch is no edit: {events:?}");
    let elsewhere = cube(&mut s.app);
    let events = s.deliver();
    assert!(
        !events.iter().any(|event| matches!(
            event,
            ControlEvent::SpawnNode { .. }
                | ControlEvent::DespawnNode { .. }
                | ControlEvent::ReparentNode { .. }
        )),
        "the other document stays out of the game: {events:?}"
    );
    assert!(s.holders(elsewhere).is_empty());
    assert_eq!(document_of(s.game.world_mut()), before);

    s.app
        .world_mut()
        .operator("scene.switch")
        .param("tab", 0_i64)
        .call()
        .expect("scene.switch dispatches")
        .assert_finished();
    s.app.update();
    s.app.update();
    let events = s.deliver();
    assert!(
        !events.iter().any(|event| matches!(
            event,
            ControlEvent::SpawnNode { .. }
                | ControlEvent::DespawnNode { .. }
                | ControlEvent::ReparentNode { .. }
        )),
        "coming back is no edit: {events:?}"
    );
    assert_eq!(document_of(s.game.world_mut()), before);

    let added = cube(&mut s.app);
    s.assert_matches();
    assert_eq!(s.holders(added).len(), 1);
    assert_eq!(s.holders(rock).len(), 1);
}

#[test]
fn pie_structure_a_game_that_never_asked_for_the_document_gets_no_structure() {
    let (mut app, _dir) = editor();
    assert!(collect_node_changes(app.world_mut()).is_empty());
    cube(&mut app);
    let sends = collect_changes_for(app.world_mut(), &[key()]);
    assert!(sends.is_empty(), "{sends:?}");
}

#[test]
fn pie_structure_a_large_undo_lands_in_one_frame() {
    let mut made = Vec::new();
    let mut s = session(|app| {
        for _ in 0..24 {
            made.push(cube(app));
        }
    });
    s.assert_matches();
    let all = document_of(s.game.world_mut()).len();
    let entities: Vec<Entity> = made
        .iter()
        .map(|node| entity_of(&mut s.app, *node))
        .collect();
    jackdaw::selection::select_many(s.app.world_mut(), &entities);
    s.app.update();
    menu(&mut s.app, "entity.delete", true);
    s.assert_matches();
    assert_eq!(document_of(s.game.world_mut()).len(), all - 24);

    history(&mut s.app, "history.undo");
    s.assert_matches();
    assert_eq!(document_of(s.game.world_mut()).len(), all);
}

#[test]
fn pie_structure_an_unsaved_tab_played_in_place_of_a_scene_goes_by_the_path_asked_for() {
    let (mut app, _dir) = editor();
    cube(&mut app);
    assert!(collect_node_changes(app.world_mut()).is_empty());
    let name = app
        .world()
        .resource::<jackdaw::scenes::Scenes>()
        .tabs
        .last()
        .expect("a tab")
        .display_name
        .clone();
    let launch = LaunchScene {
        replaces: "levels/main.bsn".to_string(),
        scene: PlayScene::Untitled(name),
    };
    serve_scene(app.world_mut(), &key(), Some(&launch), "levels/main.bsn");

    let added = cube(&mut app);
    let sends = collect_changes_for(app.world_mut(), &[key()]);
    let spawn = sends
        .iter()
        .find_map(|(_, event)| match event {
            ControlEvent::SpawnNode {
                node, scene, dir, ..
            } if *node == added.0 => Some((scene.clone(), dir.clone())),
            _ => None,
        })
        .expect("the node goes to the game");
    assert_eq!(spawn, ("levels/main.bsn".to_string(), "levels".to_string()));
}
