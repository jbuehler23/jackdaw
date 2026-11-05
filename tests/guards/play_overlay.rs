//! What a running game reads in place of files that lag the editor: the real
//! answer to a scene request, taken by the runtime's own loader, with Play's save
//! never having run.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::sprite_render::{TilemapChunkMaterial, TilemapChunkMeshCache};
use jackdaw::boot_ops::{run_op_clause, run_op_clause_as_user};
use jackdaw::pie::InstanceKey;
use jackdaw::play_scene::{LaunchScene, PlayScene, ServedScene, serve_scene};
use jackdaw::terrain_mode::TerrainDataStore;
use jackdaw_api::pie::PlayState;
use jackdaw_api::prelude::*;
use jackdaw_editor_tile::tilemap::{TilemapDataStore, store_key_of};
use jackdaw_pie_protocol::{ControlEvent, SceneAnswer};
use jackdaw_runtime::node_sync::{install_node_index, load_asset_files};
use jackdaw_runtime::play_overlay::{load_prefab_files, note_sidecar_overlay};
use jackdaw_runtime::{
    JackdawCatalogPath, JackdawPlugin, JackdawScene, JackdawSceneRoot, TerrainHeights,
    TilemapChunkOf,
};
use jackdaw_scene_types::{Terrain, Tilemap};
use jackdaw_terrain::sidecar::{self, RegionTerrainData, TerrainData};
use jackdaw_tilemap::{
    CellCoord, ChunkedLayer, LayerData, LayerKind, Placement, TileRef, TilemapData,
};

use crate::util::{self, OperatorResultExt as _};

const TERRAIN_FILE: &str = "field.terrain-0.jdterrain";
const TILES_FILE: &str = "field.tilemap-0.jdtiles";

const TILESET: &str = "\
#overworld
jackdaw_tileset::Tileset {
    image: \"overworld.png\",
    tile_size: glam::UVec2 { x: 16, y: 16 },
    grid: glam::UVec2 { x: 27, y: 65 },
    sheets: [
        jackdaw_tileset::TilesetSheet {
            source: \"overworld.png\",
            columns: 27,
            rows: 65,
            first_layer: 0,
        },
    ],
}
";

fn door(x: f32) -> String {
    format!(
        "#Door\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {{\n    \
         translation: glam::Vec3 {{ x: {x:?}, y: 0.0, z: 0.0 }},\n\
         }}\n"
    )
}

fn instance_of(source: &str) -> String {
    format!(
        "#Placed\n\
         jackdaw::prefab::components::IsA {{ source: \"{source}\", deleted: [] }}\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n"
    )
}

fn roots(entries: &[String]) -> String {
    format!(
        "bevy_ecs::hierarchy::Children [\n{}]\n",
        entries.join("--\n")
    )
}

fn terrain_entries() -> Vec<String> {
    vec![
        format!(
            "#Field\njackdaw_scene_types::types::Terrain {{ data_path: \"{TERRAIN_FILE}\" }}\n"
        ),
        format!("#Map\n{}", tilemap_component(TILES_FILE)),
    ]
}

fn terrain_scene() -> String {
    roots(&terrain_entries())
}

fn tilemap_component(data_path: &str) -> String {
    format!(
        "jackdaw_scene_types::tilemap::Tilemap {{\n    \
         data_path: \"{data_path}\",\n    \
         layers: [\n        \
         jackdaw_scene_types::tilemap::TilemapLayer {{ name: \"Ground\", tileset: \"overworld.tileset.bsn\", kind: jackdaw_scene_types::tilemap::TilemapLayerKind::Tile }},\n    \
         ],\n\
         }}\n"
    )
}

fn terrain_data(height: f32) -> RegionTerrainData {
    RegionTerrainData::from_legacy_v1(&TerrainData {
        resolution: 4,
        heights: vec![height; 16],
        channels: vec![],
    })
    .expect("a power-of-two resolution migrates")
}

fn tiles(cells: &[(i32, i32)]) -> TilemapData {
    let mut layer = ChunkedLayer::new();
    for &(x, y) in cells {
        layer.set(CellCoord::new(x, y), TileRef::new(3));
    }
    TilemapData {
        layers: vec![LayerData {
            kind: LayerKind::Tile,
            placement: Placement::OnCells,
            tiles: layer,
            authoring: None,
        }],
    }
}

/// An editor on a project holding these files under `assets`.
fn editor(files: &[(&str, &str)]) -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let assets = tmp.path().join("assets");
    std::fs::create_dir_all(&assets).expect("assets");
    std::fs::write(assets.join("catalog.bsn"), "// no catalog entries\n").expect("catalog");
    std::fs::write(assets.join("overworld.tileset.bsn"), TILESET).expect("tileset");
    for (path, text) in files {
        write_in(&assets, path, text.as_bytes());
    }
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    (app, tmp)
}

fn write_in(assets: &Path, path: &str, bytes: &[u8]) {
    let path = assets.join(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the folder");
    }
    std::fs::write(path, bytes).expect("the file");
}

fn open(app: &mut App, path: &str) {
    run_op_clause(app.world_mut(), &format!("scene.open path={path}"))
        .expect("the document opens")
        .assert_finished();
    app.update();
}

fn key() -> InstanceKey {
    InstanceKey {
        config: "game".to_string(),
        instance: 1,
    }
}

fn serve(app: &mut App, path: &str) -> ServedScene {
    serve_scene(app.world_mut(), &key(), None, path)
}

fn asset_file(tmp: &tempfile::TempDir, path: &str) -> Vec<u8> {
    std::fs::read(tmp.path().join("assets").join(path)).expect("the file")
}

fn play_dir(tmp: &tempfile::TempDir) -> PathBuf {
    tmp.path().join(".jackdaw/play")
}

/// Edit a prefab the way an author does: in its own tab, then back to `scene`.
/// `x` lands in the prefab's cache entry as the tab goes behind.
fn edit_door_in_its_tab(app: &mut App, door_path: &str, x: f64, scene: &str) {
    open(app, door_path);
    let root = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Door")
        .map(|(entity, _)| entity)
        .expect("the prefab is in front");
    app.world_mut()
        .operator("entity.set_transform")
        .param("entity", root)
        .param("x", x)
        .call()
        .expect("entity.set_transform dispatches")
        .assert_finished();
    app.update();
    open(app, scene);
}

fn set_cached_prefab(app: &mut App, tmp: &tempfile::TempDir, path: &str, text: &str) {
    let ast = jackdaw_bsn::parse_bsn_text(text).expect("the text parses");
    let mut ast = ast;
    let assets = tmp.path().join("assets");
    let full = assets.join(path);
    jackdaw_prefab::prepare_prefab_document(&mut ast, &full, &assets);
    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .insert(&full, ast);
    app.update();
}

/// A game on the runtime's own loader, reading the project at `tmp`, with no
/// scene yet.
fn empty_game(tmp: &tempfile::TempDir) -> App {
    let assets = tmp.path().join("assets");
    let mut game = App::new();
    game.add_plugins(MinimalPlugins);
    game.add_plugins(bevy::transform::TransformPlugin);
    game.add_plugins(bevy::asset::AssetPlugin {
        file_path: assets.to_string_lossy().into_owned(),
        ..default()
    });
    game.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    game.init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_asset::<TilemapChunkMaterial>()
        .init_resource::<TilemapChunkMeshCache>();
    game.init_asset::<StandardMaterial>();
    game.register_asset_reflect::<StandardMaterial>();
    game.insert_resource(JackdawCatalogPath(assets.join("catalog.bsn")));
    install_node_index(&mut game);
    game.add_plugins(JackdawPlugin);
    game
}

/// Take what the editor sent, in send order, the way the link does.
fn take(game: &mut App, served: ServedScene) {
    if let Some(ControlEvent::AssetFiles { files }) = served.files {
        load_asset_files(game.world_mut(), &files);
    }
    if let Some(ControlEvent::PrefabFiles { files }) = served.prefabs {
        load_prefab_files(game.world_mut(), &files);
    }
    let handle = match served.answer {
        SceneAnswer::Document {
            text,
            dir,
            sidecars,
            ..
        } => {
            if let Some(sidecars) = sidecars {
                note_sidecar_overlay(game.world_mut(), &sidecars);
            }
            game.world_mut()
                .resource_mut::<Assets<JackdawScene>>()
                .add(JackdawScene::new(text, PathBuf::from(dir)))
        }
        SceneAnswer::UseFile { path } => game.world().resource::<AssetServer>().load(path),
    };
    game.world_mut().spawn(JackdawSceneRoot(handle));
}

/// Run the game until `done` holds.
fn update_until(game: &mut App, what: &str, done: impl Fn(&mut App) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !done(game) {
        assert!(std::time::Instant::now() < deadline, "never: {what}");
        game.update();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn game_serving(app: &mut App, tmp: &tempfile::TempDir, path: &str) -> App {
    let served = serve(app, path);
    let mut game = empty_game(tmp);
    game.update();
    take(&mut game, served);
    game
}

fn door_x(game: &mut App) -> f32 {
    x_of(game, "Placed")
}

fn x_of(game: &mut App, entity: &str) -> f32 {
    update_until(game, "the instance spawned", |game| {
        game.world_mut()
            .query::<&Name>()
            .iter(game.world())
            .any(|name| name.as_str() == entity)
    });
    game.world_mut()
        .query::<(&Name, &Transform)>()
        .iter(game.world())
        .find(|(name, _)| name.as_str() == entity)
        .map(|(_, transform)| transform.translation.x)
        .expect("the instance is in the game")
}

fn terrain_height(game: &mut App) -> f32 {
    update_until(game, "the terrain has heights", |game| {
        game.world_mut()
            .query::<&TerrainHeights>()
            .iter(game.world())
            .next()
            .is_some()
    });
    game.world_mut()
        .query::<&TerrainHeights>()
        .iter(game.world())
        .next()
        .and_then(|heights| heights.0.heights.first().copied())
        .expect("a height")
}

/// The chunks the game drew for its one map.
fn tile_chunks(game: &mut App) -> usize {
    update_until(game, "the map drew", |game| {
        game.update();
        game.world_mut()
            .query::<&TilemapChunkOf>()
            .iter(game.world())
            .next()
            .is_some()
    });
    game.world_mut()
        .query::<&TilemapChunkOf>()
        .iter(game.world())
        .count()
}

fn prefab_files(served: &ServedScene) -> Vec<(String, Option<String>)> {
    match &served.prefabs {
        Some(ControlEvent::PrefabFiles { files }) => files.clone(),
        Some(other) => panic!("not prefabs: {other:?}"),
        None => Vec::new(),
    }
}

fn sidecars_of(served: &ServedScene) -> Option<String> {
    match &served.answer {
        SceneAnswer::Document { sidecars, .. } => sidecars.clone(),
        SceneAnswer::UseFile { .. } => None,
    }
}

#[test]
fn an_unsaved_prefab_edit_shows_in_an_instance_and_the_file_stays() {
    let scene = instance_of("door.bsn");
    let (mut app, tmp) = editor(&[("door.bsn", &door(1.0)), ("zone.bsn", &scene)]);
    open(&mut app, "assets/zone.bsn");
    let saved = asset_file(&tmp, "door.bsn");

    edit_door_in_its_tab(&mut app, "assets/door.bsn", 9.0, "assets/zone.bsn");

    let mut game = game_serving(&mut app, &tmp, "zone.bsn");
    assert_eq!(door_x(&mut game), 9.0);
    assert_eq!(asset_file(&tmp, "door.bsn"), saved, "the file is untouched");
}

#[test]
fn an_unsaved_edit_of_a_prefab_inside_a_prefab_reaches_the_instance() {
    let inner = door(1.0);
    let outer = "#Gate\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform\n\
         bevy_ecs::hierarchy::Children [\n\
         #Leaf\n\
         jackdaw::prefab::components::PrefabEntityId(1)\n\
         jackdaw::prefab::components::IsA { source: \"props/door.bsn\", deleted: [] }\n\
         ]\n"
    .to_string();
    let scene = instance_of("gate.bsn");
    let (mut app, tmp) = editor(&[
        ("props/door.bsn", &inner),
        ("gate.bsn", &outer),
        ("zone.bsn", &scene),
    ]);
    open(&mut app, "assets/zone.bsn");
    let saved = (
        asset_file(&tmp, "props/door.bsn"),
        asset_file(&tmp, "gate.bsn"),
    );

    edit_door_in_its_tab(&mut app, "assets/props/door.bsn", 7.0, "assets/zone.bsn");

    let served = serve(&mut app, "zone.bsn");
    let paths: Vec<String> = prefab_files(&served)
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert_eq!(paths, vec!["props/door.bsn"], "only the edited prefab goes");
    let mut game = empty_game(&tmp);
    game.update();
    take(&mut game, served);
    assert_eq!(x_of(&mut game, "Leaf"), 7.0);
    assert_eq!(
        (
            asset_file(&tmp, "props/door.bsn"),
            asset_file(&tmp, "gate.bsn")
        ),
        saved
    );
}

/// The prefab's own tab in front: its live document goes, with the scene behind
/// it answering.
#[test]
fn an_edit_in_a_prefab_tab_reaches_a_scene_behind_it() {
    let scene = instance_of("door.bsn");
    let (mut app, tmp) = editor(&[("door.bsn", &door(1.0)), ("zone.bsn", &scene)]);
    open(&mut app, "assets/zone.bsn");
    let saved = asset_file(&tmp, "door.bsn");
    open(&mut app, "assets/door.bsn");
    let root = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Door")
        .map(|(entity, _)| entity)
        .expect("the prefab is in front");
    app.world_mut()
        .operator("entity.set_transform")
        .param("entity", root)
        .param("x", 5.0)
        .call()
        .expect("entity.set_transform dispatches")
        .assert_finished();
    app.update();

    let mut game = game_serving(&mut app, &tmp, "zone.bsn");
    assert_eq!(door_x(&mut game), 5.0);
    assert_eq!(asset_file(&tmp, "door.bsn"), saved);
    assert!(
        jackdaw::scenes::operators::active_tab_modified(app.world()),
        "the tab stays unsaved"
    );
}

/// A scene no tab holds loads from its file, and its instances still get the
/// unsaved prefab.
#[test]
fn a_scene_loaded_from_its_file_gets_the_unsaved_prefab_too() {
    let scene = instance_of("door.bsn");
    let opened = instance_of("door.bsn");
    let (mut app, tmp) = editor(&[
        ("door.bsn", &door(1.0)),
        ("zone.bsn", &opened),
        ("other.bsn", &scene),
    ]);
    open(&mut app, "assets/zone.bsn");
    set_cached_prefab(&mut app, &tmp, "door.bsn", &door(4.0));

    let mut game = game_serving(&mut app, &tmp, "other.bsn");
    assert_eq!(door_x(&mut game), 4.0);
}

#[test]
fn an_embedded_asset_of_an_unsaved_prefab_goes_by_its_session_name() {
    let scene = instance_of("door.bsn");
    let (mut app, tmp) = editor(&[
        ("door.bsn", &door(1.0)),
        ("zone.bsn", &scene),
        ("other.bsn", &scene),
    ]);
    open(&mut app, "assets/zone.bsn");
    let with_material = roots(&[
        door(2.0),
        "#Stone\nbevy_pbr::pbr_material::StandardMaterial { perceptual_roughness: 0.25 }\n"
            .to_string(),
    ]);
    set_cached_prefab(&mut app, &tmp, "door.bsn", &with_material);

    let served = serve(&mut app, "other.bsn");
    let files = prefab_files(&served);
    let text = files[0].1.as_deref().expect("the text");
    assert!(!text.contains("#Stone"), "{text}");
    let name = text
        .lines()
        .map(str::trim_start)
        .find(|line| line.starts_with("#Play"))
        .expect("a session name")
        .to_string();

    let mut game = empty_game(&tmp);
    game.update();
    take(&mut game, served);
    let handle = game
        .world()
        .resource::<jackdaw_bsn::BsnSceneAssets>()
        .0
        .get(&name)
        .cloned()
        .expect("loaded when the text arrived");
    let material = game
        .world()
        .resource::<Assets<StandardMaterial>>()
        .get(handle.id().typed::<StandardMaterial>())
        .expect("the material");
    assert_eq!(material.perceptual_roughness, 0.25);
    assert_eq!(door_x(&mut game), 2.0);
}

/// A second answer repeats nothing the game holds, and a prefab that has since
/// been saved sends the game back to its file.
#[test]
fn a_game_is_sent_only_what_it_does_not_hold() {
    let scene = instance_of("door.bsn");
    let (mut app, tmp) = editor(&[("door.bsn", &door(1.0)), ("zone.bsn", &scene)]);
    open(&mut app, "assets/zone.bsn");
    set_cached_prefab(&mut app, &tmp, "door.bsn", &door(9.0));

    let first = prefab_files(&serve(&mut app, "zone.bsn"));
    assert_eq!(first.len(), 1);
    assert!(first[0].1.is_some());
    assert!(prefab_files(&serve(&mut app, "zone.bsn")).is_empty());

    set_cached_prefab(&mut app, &tmp, "door.bsn", &door(8.0));
    let changed = prefab_files(&serve(&mut app, "zone.bsn"));
    assert!(
        changed[0]
            .1
            .as_deref()
            .is_some_and(|text| text.contains("8.0"))
    );

    let cached = tmp.path().join("assets/door.bsn");
    jackdaw::prefab::operators::save_prefab_to_disk(app.world_mut(), &cached)
        .expect("the prefab saves");
    let back = prefab_files(&serve(&mut app, "zone.bsn"));
    assert_eq!(back, vec![("door.bsn".to_string(), None)]);
    assert!(prefab_files(&serve(&mut app, "zone.bsn")).is_empty());
}

fn field_with_sidecars() -> (App, tempfile::TempDir) {
    field_holding(terrain_scene())
}

fn field_holding(scene: String) -> (App, tempfile::TempDir) {
    let terrain = sidecar::save(&terrain_data(1.0)).expect("encodes");
    let map = jackdaw_tilemap::encode(&tiles(&[(0, 0)]));
    let (mut app, tmp) = editor(&[("levels/field.bsn", &scene), ("door.bsn", &door(1.0))]);
    let assets = tmp.path().join("assets");
    write_in(&assets, &format!("levels/{TERRAIN_FILE}"), &terrain);
    write_in(&assets, &format!("levels/{TILES_FILE}"), &map);
    open(&mut app, "assets/levels/field.bsn");
    (app, tmp)
}

fn edit_sidecars(app: &mut App) {
    let map = app
        .world_mut()
        .query_filtered::<Entity, With<Tilemap>>()
        .iter(app.world())
        .next()
        .expect("the map");
    let key = store_key_of(app.world_mut(), map).expect("a key");
    app.world_mut()
        .resource_mut::<TilemapDataStore>()
        .insert(key, tiles(&[(0, 0), (40, 40), (-40, -40)]));
    app.world_mut()
        .resource_mut::<TerrainDataStore>()
        .insert(TERRAIN_FILE, terrain_data(6.0));
}

#[test]
fn unsaved_sidecars_reach_the_game_and_leave_with_stop() {
    let (mut app, tmp) = field_with_sidecars();
    let saved = (
        asset_file(&tmp, &format!("levels/{TERRAIN_FILE}")),
        asset_file(&tmp, &format!("levels/{TILES_FILE}")),
    );
    edit_sidecars(&mut app);

    let mut game = game_serving(&mut app, &tmp, "levels/field.bsn");
    assert_eq!(terrain_height(&mut game), 6.0);
    assert_eq!(tile_chunks(&mut game), 3);

    assert_eq!(
        (
            asset_file(&tmp, &format!("levels/{TERRAIN_FILE}")),
            asset_file(&tmp, &format!("levels/{TILES_FILE}")),
        ),
        saved,
        "the user's files stay"
    );
    assert!(play_dir(&tmp).join("levels").join(TILES_FILE).is_file());
    let map = app
        .world_mut()
        .query_filtered::<Entity, With<Tilemap>>()
        .iter(app.world())
        .next()
        .expect("the map");
    let key = store_key_of(app.world_mut(), map).expect("a key");
    assert!(
        app.world()
            .resource::<TilemapDataStore>()
            .has_unsaved_edits(&key),
        "a copy for the game saves nothing"
    );

    app.world_mut()
        .resource_mut::<NextState<PlayState>>()
        .set(PlayState::Playing);
    app.update();
    assert!(!play_dir(&tmp).exists(), "cleared as Play starts");
    let served = serve(&mut app, "levels/field.bsn");
    assert!(sidecars_of(&served).is_some());
    assert!(play_dir(&tmp).exists());
    app.world_mut()
        .resource_mut::<NextState<PlayState>>()
        .set(PlayState::Stopped);
    app.update();
    assert!(!play_dir(&tmp).exists(), "gone after Stop");
}

/// A tab behind the one in front answers from its own stores.
#[test]
fn a_tab_behind_answers_with_its_unsaved_sidecars() {
    let (mut app, tmp) = field_with_sidecars();
    edit_sidecars(&mut app);
    let other = instance_of("nothing.bsn");
    write_in(&tmp.path().join("assets"), "other.bsn", other.as_bytes());
    open(&mut app, "assets/other.bsn");

    let mut game = game_serving(&mut app, &tmp, "levels/field.bsn");
    assert_eq!(terrain_height(&mut game), 6.0);
    assert_eq!(tile_chunks(&mut game), 3);
}

#[test]
fn an_untitled_scene_plays_with_its_terrain_and_tilemap() {
    let (mut app, tmp) = editor(&[]);
    run_op_clause(app.world_mut(), "scene.new")
        .expect("a new scene opens")
        .assert_finished();
    app.update();
    run_op_clause_as_user(app.world_mut(), "entity.add.terrain")
        .expect("the terrain dispatched")
        .assert_finished();
    app.update();
    add_map_node(&mut app);
    edit_untitled_sidecars(&mut app);

    let launch = LaunchScene {
        replaces: "scene.bsn".to_string(),
        scene: PlayScene::Untitled(
            app.world().resource::<jackdaw::scenes::Scenes>().tabs
                [app.world().resource::<jackdaw::scenes::Scenes>().active]
                .display_name
                .clone(),
        ),
    };
    let served = serve_scene(app.world_mut(), &key(), Some(&launch), "scene.bsn");
    assert!(sidecars_of(&served).is_some());
    let mut game = empty_game(&tmp);
    game.update();
    take(&mut game, served);
    assert_eq!(terrain_height(&mut game), 3.0);
    assert_eq!(tile_chunks(&mut game), 2);
}

/// A map in the document in front, with the data path its tab gives it.
fn add_map_node(app: &mut App) {
    let map = Tilemap {
        layers: vec![jackdaw_scene_types::TilemapLayer {
            name: "Ground".to_string(),
            tileset: "overworld.tileset.bsn".to_string(),
            kind: jackdaw_scene_types::TilemapLayerKind::Tile,
            ..default()
        }],
        ..default()
    };
    let world = app.world_mut();
    let entity = world.spawn(map.clone()).id();
    let registry = world.resource::<AppTypeRegistry>().clone();
    let patch = jackdaw_bsn::component_to_bsn_patch(&map, &registry.read());
    let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.create_entity_node(vec![patch]);
    ast.add_to_roots(node);
    ast.link(entity, node);
    world
        .run_system_cached(jackdaw_editor_tile::tilemap::ensure_tilemap_data_path)
        .expect("it runs");
    app.update();
}

fn edit_untitled_sidecars(app: &mut App) {
    jackdaw_editor_terrain::ensure_terrain_data_path(app.world_mut());
    let map = app
        .world_mut()
        .query_filtered::<Entity, With<Tilemap>>()
        .iter(app.world())
        .next()
        .expect("the map");
    let key = store_key_of(app.world_mut(), map).expect("a key");
    app.world_mut()
        .resource_mut::<TilemapDataStore>()
        .insert(key, tiles(&[(0, 0), (40, 40)]));
    let data_path = app
        .world_mut()
        .query::<&Terrain>()
        .iter(app.world())
        .next()
        .map(|terrain| terrain.data_path.clone())
        .expect("the terrain");
    assert!(!data_path.is_empty());
    app.world_mut()
        .resource_mut::<TerrainDataStore>()
        .insert(data_path, terrain_data(3.0));
}

#[test]
fn nothing_goes_for_data_with_no_unsaved_change() {
    let mut entries = terrain_entries();
    entries.push(instance_of("door.bsn"));
    let (mut app, tmp) = field_holding(roots(&entries));
    let served = serve(&mut app, "levels/field.bsn");
    assert!(prefab_files(&served).is_empty(), "no unsaved prefab");
    assert_eq!(sidecars_of(&served), None, "no unsaved sidecar");
    assert!(!play_dir(&tmp).join("levels").join(TILES_FILE).exists());
    assert!(!play_dir(&tmp).join("levels").join(TERRAIN_FILE).exists());

    let mut game = empty_game(&tmp);
    game.update();
    take(&mut game, served);
    assert_eq!(terrain_height(&mut game), 1.0, "the normal file");
    assert_eq!(tile_chunks(&mut game), 1, "the normal file");
    assert_eq!(door_x(&mut game), 1.0, "the normal file");
}

/// Undoing an edit takes the copy away again, so the game reads the file.
#[test]
fn a_copy_goes_once_the_data_matches_the_file() {
    let (mut app, tmp) = field_with_sidecars();
    edit_sidecars(&mut app);
    serve(&mut app, "levels/field.bsn");
    let copy = play_dir(&tmp).join("levels").join(TERRAIN_FILE);
    assert!(copy.is_file());
    app.world_mut()
        .resource_mut::<TerrainDataStore>()
        .insert(TERRAIN_FILE, terrain_data(1.0));
    serve(&mut app, "levels/field.bsn");
    assert!(!copy.exists());
}

/// A game with no link never hears of the overlay, whatever lies in the folder.
#[test]
fn a_plain_run_never_reads_the_overlay() {
    let scene = instance_of("door.bsn");
    let (_app, tmp) = editor(&[("door.bsn", &door(1.0)), ("zone.bsn", &scene)]);
    let assets = tmp.path().join("assets");
    let map_scene = terrain_scene();
    write_in(&assets, "field.bsn", map_scene.as_bytes());
    write_in(
        &assets,
        TERRAIN_FILE,
        &sidecar::save(&terrain_data(1.0)).expect("encodes"),
    );
    write_in(
        &assets,
        TILES_FILE,
        &jackdaw_tilemap::encode(&tiles(&[(0, 0)])),
    );
    let play = play_dir(&tmp);
    write_in(
        &play,
        TERRAIN_FILE,
        &sidecar::save(&terrain_data(9.0)).expect("encodes"),
    );
    write_in(
        &play,
        TILES_FILE,
        &jackdaw_tilemap::encode(&tiles(&[(0, 0), (40, 40)])),
    );
    write_in(&play, "door.bsn", door(9.0).as_bytes());

    let mut game = empty_game(&tmp);
    game.update();
    let handle = game.world().resource::<AssetServer>().load("field.bsn");
    game.world_mut().spawn(JackdawSceneRoot(handle));
    assert_eq!(terrain_height(&mut game), 1.0);
    assert_eq!(tile_chunks(&mut game), 1);

    let mut game = empty_game(&tmp);
    game.update();
    let handle = game.world().resource::<AssetServer>().load("zone.bsn");
    game.world_mut().spawn(JackdawSceneRoot(handle));
    assert_eq!(door_x(&mut game), 1.0);
}
