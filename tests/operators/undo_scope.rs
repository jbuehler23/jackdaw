//! An undo or redo touches only what its edit changed: the other brushes keep
//! their meshes and their document patches, and the viewport mesh of everything
//! it did touch matches the brush.

use super::csg_ops::{call, cube, entity_of};
use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::ecs::change_detection::Tick;
use bevy::ecs::component::ComponentId;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use jackdaw::brush::{BrushHalfedge, BrushMeshCache, BrushMeshChunk, BrushSelection};
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::remote::server::entity_handler;
use jackdaw::scene_nodes::{name_scene_nodes, node_id_patch};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_bsn::{BsnPatch, BsnTupleStructData, SceneBsnAst};
use jackdaw_scene_types::{Brush, SceneNodeId};
use serde_json::json;

const BRUSH: &str = "jackdaw_scene_types::types::Brush";
const TRANSFORM: &str = "bevy_transform::components::transform::Transform";

/// An editor app in the running state, where brushes render and sync, on an
/// empty scene in the returned project folder.
fn editor_app() -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = editor_app_in(dir.path());
    (app, dir)
}

/// An editor app as [`editor_app`] makes it, on the project folder `dir`.
fn editor_app_in(dir: &std::path::Path) -> App {
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.to_path_buf(),
        ProjectConfig::default(),
    ));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    app.update();
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    app
}

fn undo(app: &mut App) {
    call(app, "history.undo", json!({}));
    settle(app);
}

fn redo(app: &mut App) {
    call(app, "history.redo", json!({}));
    settle(app);
}

fn settle(app: &mut App) {
    for _ in 0..3 {
        app.update();
    }
}

fn row(app: &mut App, count: usize) -> Vec<Entity> {
    let brushes = (0..count)
        .map(|index| {
            cube(
                app,
                &format!("Block{index}"),
                [1.0, 1.0, 1.0],
                [index as f32 * 3.0, 0.0, 0.0],
                [0.0, 0.0, 0.0],
            )
        })
        .collect();
    settle(app);
    brushes
}

fn extrude_top(app: &mut App, entity: Entity) {
    let selected = call(
        app,
        "brush.select_elements",
        json!({ "entity": entity.to_bits(), "kind": "face", "where": { "normal": "+y" } }),
    );
    assert_eq!(selected["result"], "finished", "{selected}");
    app.update();
    let extruded = call(app, "brush.mesh.extrude_region", json!({ "distance": 0.5 }));
    assert_eq!(extruded["result"], "finished", "{extruded}");
    settle(app);
}

/// Fail unless every brush's rendered mesh is the one its `Brush` describes: the
/// same corners, one ring per polygon, live chunks under the brush and no others,
/// and each chunk in its faces' material.
#[track_caller]
fn assert_views_match(app: &mut App) {
    let brushes: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .collect();
    let mut chunks = app.world_mut().query::<(Entity, &BrushMeshChunk)>();
    let chunks: Vec<(Entity, Entity, AssetId<StandardMaterial>, bool)> = chunks
        .iter(app.world())
        .map(|(entity, chunk)| {
            (
                entity,
                chunk.brush_entity,
                chunk.material.id(),
                chunk.uses_default_material,
            )
        })
        .collect();
    for entity in brushes {
        let world = app.world();
        let brush = world.get::<Brush>(entity).expect("a brush");
        let cache = world
            .get::<BrushMeshCache>(entity)
            .expect("a rendered brush holds a mesh cache");
        let name = world.get::<Name>(entity).map(Name::as_str).unwrap_or("");
        assert_eq!(
            cache.base_vertices.len(),
            brush.topology.vertices.len(),
            "{name}: the mesh has the brush's corners"
        );
        for (index, vertex) in brush.topology.vertices.iter().enumerate() {
            assert!(
                cache.base_vertices[index].abs_diff_eq(vertex.position, 1e-4),
                "{name}: corner {index} of the mesh sits where the brush says"
            );
        }
        assert_eq!(
            cache.face_polygons.len(),
            brush.topology.polygons.len(),
            "{name}: the mesh has the brush's faces"
        );
        if let Some(halfedge) = world.get::<BrushHalfedge>(entity) {
            let lifted = halfedge.mesh.flatten_to_topology();
            assert_eq!(
                lifted.vertices.len(),
                brush.topology.vertices.len(),
                "{name}: the edit mesh has the brush's corners"
            );
        }
        let held: Vec<Entity> = chunks
            .iter()
            .filter(|chunk| chunk.1 == entity)
            .map(|chunk| chunk.0)
            .collect();
        assert_eq!(
            held, cache.chunk_entities,
            "{name}: only the last rebuild's chunks are left"
        );
        for chunk in chunks.iter().filter(|chunk| chunk.1 == entity && !chunk.3) {
            assert!(
                brush.faces.iter().any(|face| face.material.id() == chunk.2),
                "{name}: a chunk wears a material none of the faces name"
            );
        }
    }
}

/// When each patch of `entity`'s document node last changed.
fn patch_ticks(app: &App, entity: Entity) -> Vec<Tick> {
    let live = app.world().resource::<SceneBsnAst>();
    let node = live.ast_for(entity).expect("a document node");
    live.get_patches(node)
        .expect("patches")
        .0
        .iter()
        .map(|&patch| {
            live.world
                .get_entity(patch)
                .expect("a patch entity")
                .get_ref::<BsnPatch>()
                .expect("a patch")
                .last_changed()
        })
        .collect()
}

/// The state of a brush that an undo of something else must leave alone.
#[derive(PartialEq, Debug)]
struct Untouched {
    chunks: Vec<Entity>,
    brush_changed: Tick,
    patches: Vec<Tick>,
}

fn untouched(app: &App, entity: Entity) -> Untouched {
    let world = app.world();
    Untouched {
        chunks: world
            .get::<BrushMeshCache>(entity)
            .expect("a mesh cache")
            .chunk_entities
            .clone(),
        brush_changed: world
            .get_entity(entity)
            .expect("a brush")
            .get_ref::<Brush>()
            .expect("a brush")
            .last_changed(),
        patches: patch_ticks(app, entity),
    }
}

/// Run `step` and fail if it touched any brush in `others`.
#[track_caller]
fn assert_leaves_alone(app: &mut App, others: &[Entity], step: impl FnOnce(&mut App)) {
    let before: Vec<Untouched> = others.iter().map(|&other| untouched(app, other)).collect();
    step(app);
    let after: Vec<Untouched> = others.iter().map(|&other| untouched(app, other)).collect();
    assert_eq!(before, after, "another brush was touched");
}

#[test]
fn an_undone_extrude_leaves_the_mesh_matching_the_brush() {
    let (mut app, _dir) = editor_app();
    let block = row(&mut app, 1)[0];
    extrude_top(&mut app, block);
    assert_eq!(
        app.world()
            .get::<BrushMeshCache>(block)
            .expect("a mesh cache")
            .base_vertices
            .len(),
        12,
        "the extrude shows"
    );
    assert_views_match(&mut app);

    undo(&mut app);
    assert_eq!(
        app.world()
            .get::<Brush>(block)
            .expect("a brush")
            .topology
            .vertices
            .len(),
        8,
        "undo restores the cube"
    );
    assert_views_match(&mut app);

    redo(&mut app);
    assert_views_match(&mut app);
    undo(&mut app);

    call(&mut app, "brush.exit_edit_mode", json!({}));
    settle(&mut app);
    app.world_mut().resource_mut::<BrushSelection>().clear();
    settle(&mut app);
    assert_views_match(&mut app);
}

#[test]
fn undoing_one_brush_leaves_the_other_brushes_alone() {
    let (mut app, _dir) = editor_app();
    let brushes = row(&mut app, 4);
    extrude_top(&mut app, brushes[0]);
    let others = &brushes[1..];

    assert_leaves_alone(&mut app, others, undo);
    assert_views_match(&mut app);
    assert_leaves_alone(&mut app, others, redo);
    assert_views_match(&mut app);
}

#[test]
fn undoing_a_field_edit_leaves_the_other_brushes_alone() {
    let (mut app, _dir) = editor_app();
    let brushes = row(&mut app, 3);
    let moved = call(
        &mut app,
        "component.set",
        json!({
            "entity": brushes[0].to_bits(),
            "type_path": TRANSFORM,
            "field": "translation",
            "value": [0.0, 4.0, 0.0],
        }),
    );
    assert_eq!(moved["result"], "finished", "{moved}");
    settle(&mut app);
    let others = &brushes[1..];

    assert_leaves_alone(&mut app, others, undo);
    assert_eq!(
        app.world()
            .get::<Transform>(brushes[0])
            .expect("a transform")
            .translation,
        Vec3::ZERO,
        "undo puts the brush back"
    );
    assert_leaves_alone(&mut app, others, redo);
    assert_views_match(&mut app);
}

#[test]
fn undo_of_a_delete_and_a_duplicate_keeps_every_view_right() {
    let (mut app, _dir) = editor_app();
    let brushes = row(&mut app, 3);
    let count = |app: &mut App| {
        app.world_mut()
            .query_filtered::<(), With<Brush>>()
            .iter(app.world())
            .count()
    };

    app.world_mut().resource_mut::<Selection>().entities = vec![brushes[1]];
    call(&mut app, "entity.delete", json!({}));
    settle(&mut app);
    assert_eq!(count(&mut app), 2);
    undo(&mut app);
    assert_eq!(count(&mut app), 3, "undo brings the deleted brush back");
    assert_views_match(&mut app);
    redo(&mut app);
    assert_eq!(count(&mut app), 2);
    assert_views_match(&mut app);
    undo(&mut app);

    app.world_mut().resource_mut::<Selection>().entities = vec![brushes[0]];
    call(&mut app, "entity.duplicate", json!({}));
    settle(&mut app);
    assert_eq!(count(&mut app), 4);
    assert_views_match(&mut app);
    undo(&mut app);
    assert_eq!(count(&mut app), 3, "undo takes the copy away");
    assert_views_match(&mut app);
    redo(&mut app);
    assert_eq!(count(&mut app), 4);
    assert_views_match(&mut app);
}

#[test]
fn undo_of_a_reparent_keeps_every_view_right() {
    let (mut app, _dir) = editor_app();
    let brushes = row(&mut app, 2);
    let group =
        entity_of(&call(&mut app, "entity.add.group", json!({ "name": "Group" }))["entities"][0]);
    settle(&mut app);
    let moved = call(
        &mut app,
        "entity.reparent",
        json!({ "child": brushes[0].to_bits(), "parent": group.to_bits() }),
    );
    assert_eq!(moved["result"], "finished", "{moved}");
    settle(&mut app);
    let parent = |app: &App, entity| app.world().get::<ChildOf>(entity).map(ChildOf::parent);
    assert_eq!(parent(&app, brushes[0]), Some(group));

    assert_leaves_alone(&mut app, &brushes[1..], undo);
    assert_eq!(parent(&app, brushes[0]), None, "undo moves the brush out");
    assert_views_match(&mut app);
    assert_leaves_alone(&mut app, &brushes[1..], redo);
    assert_eq!(parent(&app, brushes[0]), Some(group), "redo moves it back");
    assert_views_match(&mut app);
}

#[test]
fn undo_of_a_material_change_rebuilds_the_mesh_in_the_old_material() {
    let (mut app, _dir) = editor_app();
    let brushes = row(&mut app, 2);
    let created = call(
        &mut app,
        "material.create",
        json!({ "preset": "thatch", "palette": "roof" }),
    );
    assert_eq!(created["outcome"], "changed", "{created}");
    settle(&mut app);
    let applied = call(
        &mut app,
        "material.apply",
        json!({ "material": "roof", "entities": "Block0" }),
    );
    assert_eq!(applied["result"], "finished", "{applied}");
    settle(&mut app);
    let dressed = |app: &App, entity| {
        !jackdaw_bsn::holds_no_asset(
            &app.world().get::<Brush>(entity).expect("a brush").faces[0].material,
        )
    };
    assert!(dressed(&app, brushes[0]), "the material landed");
    assert_views_match(&mut app);

    assert_leaves_alone(&mut app, &brushes[1..], undo);
    assert!(!dressed(&app, brushes[0]), "undo takes the material off");
    assert_views_match(&mut app);
    assert_leaves_alone(&mut app, &brushes[1..], redo);
    assert!(dressed(&app, brushes[0]), "redo puts it back");
    assert_views_match(&mut app);
}

#[test]
fn undo_of_packing_a_prefab_keeps_every_view_right() {
    let (mut app, _dir) = editor_app();
    row(&mut app, 1);
    let group =
        entity_of(&call(&mut app, "entity.add.group", json!({ "name": "Hut" }))["entities"][0]);
    let added = call(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Wall", "parent": group.to_bits() }),
    );
    assert_eq!(added["outcome"], "changed", "{added}");
    settle(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    let packed = call(
        &mut app,
        "prefab.pack",
        json!({ "path": "prefabs/hut.bsn" }),
    );
    assert_eq!(packed["result"], "finished", "{packed}");
    settle(&mut app);
    assert_views_match(&mut app);

    undo(&mut app);
    assert_eq!(brushes_of(&mut app).len(), 2, "undo keeps both brushes");
    assert_views_match(&mut app);
    redo(&mut app);
    assert_views_match(&mut app);
    assert_eq!(brushes_of(&mut app).len(), 2, "redo keeps both brushes");
}

/// A scene holding `Block0` and a `Hut` group around a `Wall`, with the group
/// selected and ready to pack, in the returned project folder.
fn scene_with_hut() -> (App, tempfile::TempDir) {
    let (mut app, dir) = editor_app();
    row(&mut app, 1);
    let group =
        entity_of(&call(&mut app, "entity.add.group", json!({ "name": "Hut" }))["entities"][0]);
    call(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Wall", "parent": group.to_bits() }),
    );
    settle(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    (app, dir)
}

fn saved_text(app: &mut App, dir: &std::path::Path) -> String {
    jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), dir).expect("the scene emits")
}

/// Fail unless each brush holds a node id of its own, and each node id the
/// saved text names is named once.
#[track_caller]
fn assert_one_brush_per_node(app: &mut App, dir: &std::path::Path) {
    let mut held: Vec<(String, SceneNodeId)> = app
        .world_mut()
        .query_filtered::<(&Name, &SceneNodeId), With<Brush>>()
        .iter(app.world())
        .map(|(name, id)| (name.to_string(), *id))
        .collect();
    held.sort_by_key(|(_, id)| id.0);
    let brushes = held.len();
    held.dedup_by_key(|(_, id)| *id);
    assert_eq!(held.len(), brushes, "a node id is held twice: {held:?}");

    let text = saved_text(app, dir);
    let mut named: Vec<&str> = text
        .split("SceneNodeId(")
        .skip(1)
        .filter_map(|rest| rest.split(')').next())
        .collect();
    assert!(!named.is_empty(), "saved text names no node:\n{text}");
    named.sort_unstable();
    let total = named.len();
    named.dedup();
    assert_eq!(named.len(), total, "saved text repeats a node id:\n{text}");
}

#[test]
fn redo_of_packing_a_prefab_leaves_one_brush_per_node() {
    let (mut app, dir) = scene_with_hut();
    let unpacked = saved_text(&mut app, dir.path());
    let packed = call(
        &mut app,
        "prefab.pack",
        json!({ "path": "prefabs/hut.bsn" }),
    );
    assert_eq!(packed["result"], "finished", "{packed}");
    settle(&mut app);
    let instanced = saved_text(&mut app, dir.path());
    assert_one_brush_per_node(&mut app, dir.path());
    assert_eq!(brushes_of(&mut app).len(), 2);

    for round in 0..2 {
        undo(&mut app);
        assert_eq!(brushes_of(&mut app).len(), 2, "round {round}: undo");
        assert_one_brush_per_node(&mut app, dir.path());
        assert_eq!(saved_text(&mut app, dir.path()), unpacked, "round {round}");
        assert_views_match(&mut app);

        redo(&mut app);
        assert_eq!(brushes_of(&mut app).len(), 2, "round {round}: redo");
        assert_one_brush_per_node(&mut app, dir.path());
        assert_eq!(saved_text(&mut app, dir.path()), instanced, "round {round}");
        assert_views_match(&mut app);
    }
}

fn scene_node(ast: &mut SceneBsnAst, id: u64, parent: Option<Entity>) -> Entity {
    let node = ast.create_entity_node(vec![node_id_patch(SceneNodeId(id))]);
    match parent {
        Some(parent) => ast.insert_child_in_ast(parent, node, 0),
        None => ast.add_to_roots(node),
    }
    node
}

/// A scene saved with a repeated id still holds both nodes, each with an id of
/// its own; the ids an instance's file gives its inner nodes stay as they are.
#[test]
fn naming_a_scene_replaces_a_repeated_id_and_keeps_those_inside_instances() {
    let mut ast = SceneBsnAst::default();
    let walls = [scene_node(&mut ast, 7, None), scene_node(&mut ast, 7, None)];
    let instances = [8, 9].map(|id| {
        let root = scene_node(&mut ast, id, None);
        let isa = BsnPatch::TupleStruct(BsnTupleStructData {
            type_path: "jackdaw::prefab::components::IsA".to_string(),
            values: Vec::new(),
        });
        let patch = ast.world.spawn(isa).id();
        ast.get_patches_mut(root).expect("a node").0.push(patch);
        let inner = scene_node(&mut ast, 7, Some(root));
        let peid = ast.world.spawn(prefab_entity_id_patch(1)).id();
        ast.get_patches_mut(inner).expect("a node").0.push(peid);
        inner
    });

    name_scene_nodes(&mut ast, &TypeRegistry::new());

    let [first, second] = walls.map(|wall| ast.stable_id_of(wall));
    assert_eq!(
        first,
        Some(7),
        "the first copy in the document keeps the id"
    );
    assert!(second.is_some());
    assert_ne!(first, second);
    for inner in instances {
        assert_eq!(ast.stable_id_of(inner), Some(7));
    }
}

fn prefab_entity_id_patch(id: i128) -> BsnPatch {
    BsnPatch::TupleStruct(BsnTupleStructData {
        type_path: "jackdaw::prefab::components::PrefabEntityId".to_string(),
        values: vec![jackdaw_bsn::BsnValue::Int(id)],
    })
}

fn hut_file(app: &App) -> std::path::PathBuf {
    let project = app.world().resource::<ProjectRoot>();
    project
        .root
        .join(project.assets_folder())
        .join("prefabs/hut.bsn")
}

fn pack_hut(app: &mut App) {
    let packed = call(app, "prefab.pack", json!({ "path": "prefabs/hut.bsn" }));
    assert_eq!(packed["result"], "finished", "{packed}");
    settle(app);
}

fn place_hut(app: &mut App) {
    call(
        app,
        "prefab.spawn_instance",
        json!({ "path": "prefabs/hut.bsn", "pos_x": 9.0, "pos_y": 0.0, "pos_z": 0.0 }),
    );
    settle(app);
}

fn holders_named(app: &mut App, name: &str) -> Vec<(Entity, SceneNodeId)> {
    app.world_mut()
        .query::<(Entity, &Name, &SceneNodeId)>()
        .iter(app.world())
        .filter(|(_, held, _)| held.as_str() == name)
        .map(|(entity, _, id)| (entity, *id))
        .collect()
}

fn below_an_instance(app: &App, entity: Entity) -> bool {
    let world = app.world();
    let mut at = world.get::<ChildOf>(entity).map(ChildOf::parent);
    while let Some(parent) = at {
        if world.get::<jackdaw::prefab::IsA>(parent).is_some() {
            return true;
        }
        at = world.get::<ChildOf>(parent).map(ChildOf::parent);
    }
    false
}

/// The holder of `name` that does, or does not, sit below an instance.
fn holder_placed(app: &mut App, name: &str, inner: bool) -> Entity {
    let holders = holders_named(app, name);
    holders
        .into_iter()
        .map(|(entity, _)| entity)
        .find(|&entity| below_an_instance(app, entity) == inner)
        .unwrap_or_else(|| panic!("no {name} with inner={inner}"))
}

fn ids_in_file(path: &std::path::Path) -> Vec<(String, u64)> {
    let text = std::fs::read_to_string(path).expect("the file reads");
    let ast = jackdaw_bsn::parse_bsn_text(&text).expect("the file parses");
    ast.roots
        .iter()
        .flat_map(|&root| std::iter::once(root).chain(ast.descendants_of(root)))
        .filter_map(|node| {
            let name = ast.get_name(node)?.to_string();
            Some((name, ast.stable_id_of(node)?))
        })
        .collect()
}

/// A scene holding the `Hut` group and an instance of a file that an older
/// pack wrote: the file's nodes repeat the ids of the group's. Returns the id
/// the scene's `Wall` and the instance's inner `Wall` share.
fn scene_beside_an_older_file() -> (App, tempfile::TempDir, SceneNodeId) {
    let (mut app, dir) = scene_with_hut();
    let wall = holders_named(&mut app, "Wall")[0].1;
    let hut = holders_named(&mut app, "Hut")[0].1;
    pack_hut(&mut app);
    undo(&mut app);
    let file = hut_file(&app);
    let mut text = std::fs::read_to_string(&file).expect("the file reads");
    for (name, id) in ids_in_file(&file) {
        let older = match name.as_str() {
            "Wall" => wall,
            "Hut" => hut,
            _ => continue,
        };
        text = text.replace(
            &format!("SceneNodeId({id})"),
            &format!("SceneNodeId({})", older.0),
        );
    }
    std::fs::write(&file, text).expect("the file writes");
    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .invalidate(&file);
    place_hut(&mut app);
    assert_eq!(holders_named(&mut app, "Wall").len(), 2);
    (app, dir, wall)
}

fn select(app: &mut App, entity: Entity) {
    jackdaw::selection::select_only(app.world_mut(), entity);
}

#[test]
fn a_pack_gives_the_file_node_ids_of_its_own() {
    let (mut app, dir) = scene_with_hut();
    let wall = holders_named(&mut app, "Wall")[0].1;
    pack_hut(&mut app);
    let file = hut_file(&app);
    assert!(
        ids_in_file(&file)
            .iter()
            .all(|(name, id)| name != "Wall" || *id != wall.0),
        "the file repeats the scene's id"
    );
    undo(&mut app);
    place_hut(&mut app);
    assert_eq!(holders_named(&mut app, "Wall").len(), 2);
    assert_one_brush_per_node(&mut app, dir.path());

    let path = dir.path().join("assets/placed.bsn");
    std::fs::write(
        &path,
        saved_text(&mut app, path.parent().expect("a folder")),
    )
    .expect("the scene writes");
    let mut fresh = editor_app_in(dir.path());
    jackdaw::scenes::operators::scene_open_system(fresh.world_mut(), &path);
    crate::util::settle_scene_load(&mut fresh);
    settle(&mut fresh);
    let walls: Vec<SceneNodeId> = holders_named(&mut fresh, "Wall")
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    assert_eq!(walls.len(), 2);
    assert_ne!(walls[0], walls[1], "a fresh session shares an id");
}

#[test]
fn undo_and_redo_of_a_pack_leave_the_instance_agreeing_with_the_file() {
    let (mut app, _dir) = scene_with_hut();
    pack_hut(&mut app);
    let file = hut_file(&app);
    let text = std::fs::read_to_string(&file).expect("the file reads");
    let named = ids_in_file(&file);
    let in_file = |name: &str| {
        named
            .iter()
            .find_map(|(held, id)| (held == name).then_some(*id))
    };
    let inner = holder_placed(&mut app, "Wall", true);
    assert_eq!(
        app.world().get::<SceneNodeId>(inner).map(|id| id.0),
        in_file("Wall")
    );

    undo(&mut app);
    assert_eq!(
        std::fs::read_to_string(&file).expect("the file reads"),
        text,
        "undo leaves the file"
    );
    redo(&mut app);
    assert_eq!(
        std::fs::read_to_string(&file).expect("the file reads"),
        text,
        "redo leaves the file"
    );
    let inner = holder_placed(&mut app, "Wall", true);
    assert_eq!(
        app.world().get::<SceneNodeId>(inner).map(|id| id.0),
        in_file("Wall"),
        "the instance holds the ids a game reads from the file"
    );
}

#[test]
fn an_override_survives_a_pack_writing_new_ids_over_its_file() {
    let (mut app, dir) = scene_with_hut();
    pack_hut(&mut app);
    let inner = holder_placed(&mut app, "Wall", true);
    let before = app.world().get::<SceneNodeId>(inner).copied();
    call(
        &mut app,
        "entity.set_transform",
        json!({ "entity": inner.to_bits(), "y": 5.0 }),
    );
    settle(&mut app);

    let group =
        entity_of(&call(&mut app, "entity.add.group", json!({ "name": "Hut" }))["entities"][0]);
    call(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Wall", "parent": group.to_bits() }),
    );
    settle(&mut app);
    let packed = call(
        &mut app,
        "prefab.pack",
        json!({ "entity": group.to_bits(), "path": "prefabs/hut.bsn", "overwrite": true }),
    );
    assert_eq!(packed["result"], "finished", "{packed}");
    settle(&mut app);

    let raised = holders_named(&mut app, "Wall")
        .into_iter()
        .map(|(entity, _)| entity)
        .find(|&entity| {
            app.world()
                .get::<Transform>(entity)
                .is_some_and(|transform| (transform.translation.y - 5.0).abs() < 1e-4)
        });
    assert!(raised.is_some(), "the override went with the old ids");
    let id = raised.and_then(|entity| app.world().get::<SceneNodeId>(entity).copied());
    assert_ne!(id, before, "the instance took the file's new ids");
    assert_eq!(
        id.map(|id| id.0),
        ids_in_file(&hut_file(&app))
            .iter()
            .find_map(|(name, id)| (name == "Wall").then_some(*id)),
        "the override entry keeps no id of its own"
    );
    let saved = saved_text(&mut app, dir.path());
    let stale = format!("SceneNodeId({})", before.expect("an id").0);
    assert!(
        !saved.contains(&stale),
        "the scene keeps the file's old id as an override:\n{saved}"
    );
}

/// The scene's `Hut` goes into another file, leaving the older instance's inner
/// `Wall` the only holder of its id. That holder is still not the unit.
#[test]
fn undo_does_not_take_a_lone_inner_node_for_the_scene_node() {
    let (mut app, _dir, id) = scene_beside_an_older_file();
    let hut = holder_placed(&mut app, "Hut", false);
    let packed = call(
        &mut app,
        "prefab.pack",
        json!({ "entity": hut.to_bits(), "path": "prefabs/other.bsn" }),
    );
    assert_eq!(packed["result"], "finished", "{packed}");
    settle(&mut app);
    let held = |app: &mut App| {
        holders_named(app, "Wall")
            .iter()
            .filter(|(_, held)| *held == id)
            .count()
    };
    assert_eq!(held(&mut app), 1, "the pack took the scene's Wall");

    for round in 0..2 {
        undo(&mut app);
        assert_eq!(held(&mut app), 2, "round {round}: undo brings it back");
        assert_views_match(&mut app);
        let unit = holder_placed(&mut app, "Wall", false);
        assert_eq!(app.world().get::<SceneNodeId>(unit), Some(&id));
        redo(&mut app);
        assert_eq!(held(&mut app), 1, "round {round}: redo takes it again");
        assert_views_match(&mut app);
    }
}

/// An edit inside the instance respawns it whole. The scene's node with the same
/// id is still the one its own edit lands on, on either side of that respawn.
#[test]
fn undo_finds_a_scene_node_after_an_instance_with_its_id_respawned() {
    let (mut app, _dir, id) = scene_beside_an_older_file();
    let wall = holder_placed(&mut app, "Wall", false);
    let inner = holder_placed(&mut app, "Wall", true);
    extrude_top(&mut app, inner);
    settle(&mut app);
    extrude_top(&mut app, wall);
    settle(&mut app);
    let corners = |app: &App, entity: Entity| {
        app.world()
            .get::<Brush>(entity)
            .map(|brush| brush.topology.vertices.len())
    };
    let extruded = corners(&app, wall);
    assert_ne!(extruded, Some(8), "the extrude changed the brush");

    for round in 0..2 {
        undo(&mut app);
        assert_eq!(
            corners(&app, wall),
            Some(8),
            "round {round}: the scene's Wall"
        );
        undo(&mut app);
        assert_eq!(holders_named(&mut app, "Wall").len(), 2, "round {round}");
        let unit = holder_placed(&mut app, "Wall", false);
        assert_eq!(
            unit, wall,
            "round {round}: the scene's Wall kept its entity"
        );
        let inner = holder_placed(&mut app, "Wall", true);
        assert_eq!(
            corners(&app, inner),
            Some(8),
            "round {round}: the inner Wall went back"
        );
        redo(&mut app);
        redo(&mut app);
        assert_eq!(
            corners(&app, wall),
            extruded,
            "round {round}: the scene's Wall"
        );
        assert_eq!(holders_named(&mut app, "Wall").len(), 2, "round {round}");
        assert_views_match(&mut app);
    }
    assert_eq!(app.world().get::<SceneNodeId>(wall), Some(&id));
}

#[test]
fn a_snapshot_brings_the_selection_back_on_the_scene_node() {
    use jackdaw_api_internal::snapshot::SceneSnapshotter as _;
    let (mut app, _dir, _) = scene_beside_an_older_file();
    let wall = holder_placed(&mut app, "Wall", false);
    select(&mut app, wall);
    let snapshot = jackdaw::undo_snapshot::BsnDocumentSnapshotter.capture(app.world_mut());
    snapshot.restore_selection(app.world_mut());
    let selected = app.world().resource::<Selection>().entities.clone();
    assert_eq!(selected, vec![wall], "the selection came back elsewhere");
}

#[test]
fn a_tab_switch_brings_the_selection_back_on_the_scene_node() {
    let (mut app, _dir, _) = scene_beside_an_older_file();
    let wall = holder_placed(&mut app, "Wall", false);
    select(&mut app, wall);
    let tab = app.world().resource::<jackdaw::scenes::Scenes>().active;
    call(&mut app, "scene.new", json!({}));
    settle(&mut app);
    assert_ne!(
        app.world().resource::<jackdaw::scenes::Scenes>().active,
        tab
    );
    assert!(jackdaw::scenes::swap::swap_active_tab(app.world_mut(), tab));
    settle(&mut app);
    let wall = holder_placed(&mut app, "Wall", false);
    let selected = app.world().resource::<Selection>().entities.clone();
    assert_eq!(selected, vec![wall], "the selection came back elsewhere");
}

#[test]
fn a_live_edit_lands_on_the_scene_node() {
    let (mut app, _dir, id) = scene_beside_an_older_file();
    let wall = holder_placed(&mut app, "Wall", false);
    let projection = jackdaw::pie_projection::PieProjection::default();
    let key = jackdaw::live_edits::LiveEditKey {
        bits: 1,
        type_path: TRANSFORM.to_string(),
        field_path: "translation".to_string(),
    };
    let entry = jackdaw::live_edits::LiveEditEntry {
        node_id: Some(id.0),
        baseline: None,
        live_value: json!(null),
        label: String::new(),
    };
    let ast = app.world().resource::<SceneBsnAst>();
    assert_eq!(
        jackdaw::live_edits::resolve_entry_entity(ast, &projection, &key, &entry),
        Some(wall),
        "the save would land on the instance's inner node"
    );
}

fn dead_entity_of(app: &mut App, id: SceneNodeId) -> Entity {
    let dead = app.world_mut().spawn(id).id();
    app.world_mut().despawn(dead);
    dead
}

#[test]
fn a_recorded_entity_resolves_to_the_scene_node_over_an_inner_node() {
    let (mut app, _dir, id) = scene_beside_an_older_file();
    let wall = holder_placed(&mut app, "Wall", false);
    let dead = dead_entity_of(&mut app, id);
    assert_eq!(jackdaw::scene_nodes::live_entity(app.world(), dead), wall);
}

#[test]
fn a_recorded_entity_resolves_to_one_instances_inner_node() {
    let (mut app, _dir) = scene_with_hut();
    pack_hut(&mut app);
    let inner = holder_placed(&mut app, "Wall", true);
    let id = *app.world().get::<SceneNodeId>(inner).expect("an id");
    let dead = dead_entity_of(&mut app, id);
    assert_eq!(
        jackdaw::scene_nodes::live_entity(app.world(), dead),
        inner,
        "one instance's inner node still resolves"
    );

    place_hut(&mut app);
    assert_eq!(
        jackdaw::scene_nodes::live_entity(app.world(), dead),
        dead,
        "two instances' inner nodes name no one entity"
    );
}

fn deep_and_wide(depth: usize, width: usize) -> SceneBsnAst {
    let mut ast = SceneBsnAst::default();
    let named = |ast: &mut SceneBsnAst, name: String, id: u64, parent: Option<Entity>| {
        let node = scene_node(ast, id, parent);
        for patch in [BsnPatch::Name(name), BsnPatch::Type(TRANSFORM.to_string())] {
            let patch = ast.world.spawn(patch).id();
            ast.get_patches_mut(node).expect("a node").0.push(patch);
        }
        node
    };
    let mut at = None;
    for level in 0..depth {
        at = Some(named(
            &mut ast,
            format!("Level{level}"),
            1_000 + level as u64,
            at,
        ));
    }
    for leaf in 0..width {
        named(&mut ast, format!("Leaf{leaf}"), 10_000 + leaf as u64, at);
    }
    ast
}

#[test]
fn naming_a_scene_searches_for_no_parents() {
    let mut ast = deep_and_wide(40, 200);
    name_scene_nodes(&mut ast, &TypeRegistry::new());
    assert_eq!(ast.parent_walks(), 0);
}

#[test]
fn undo_in_a_deep_scene_searches_for_no_parents_per_step() {
    const DEPTH: usize = 40;
    let (mut app, dir) = editor_app();
    let path = dir.path().join("deep.bsn");
    std::fs::write(&path, jackdaw_bsn::emit_scene(&deep_and_wide(DEPTH, 100)))
        .expect("the scene writes");
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &path);
    crate::util::settle_scene_load(&mut app);
    settle(&mut app);
    let deepest = holders_named(&mut app, &format!("Level{}", DEPTH - 1))[0].0;
    let created = call(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Late", "parent": deepest.to_bits() }),
    );
    let late = entity_of(&created["entities"][0]);
    settle(&mut app);
    let corners = |app: &App| {
        app.world()
            .get::<Brush>(late)
            .map(|brush| brush.topology.vertices.len())
    };
    let plain = corners(&app);
    extrude_top(&mut app, late);
    let extruded = corners(&app);
    assert_ne!(plain, extruded, "the extrude changed the brush");

    let walks = |app: &App| app.world().resource::<SceneBsnAst>().parent_walks();
    let before = walks(&app);
    undo(&mut app);
    let after_undo = walks(&app);
    assert_eq!(corners(&app), plain, "undo ran in place");
    redo(&mut app);
    let after_redo = walks(&app);
    assert_eq!(corners(&app), extruded, "redo ran in place");

    let (undo_walks, redo_walks) = (after_undo - before, after_redo - after_undo);
    assert!(
        undo_walks < DEPTH as u64 && redo_walks < DEPTH as u64,
        "undo walked {undo_walks} times, redo {redo_walks}"
    );
}

const HUT_PREFAB: &str = "\
#hut
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
bevy_transform::components::transform::Transform
bevy_ecs::hierarchy::Children [
    #Wall
    jackdaw::prefab::components::PrefabEntityId(1)
    jackdaw_scene_types::node_id::SceneNodeId(77)
    bevy_transform::components::transform::Transform
]
";

const OUTER_PREFAB: &str = "\
#outer
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
bevy_transform::components::transform::Transform
bevy_ecs::hierarchy::Children [
    #Nested
    jackdaw::prefab::components::IsA { source: \"hut.bsn\", deleted: [] }
    jackdaw::prefab::components::PrefabEntityId(1)
    jackdaw_scene_types::node_id::SceneNodeId(66)
    bevy_transform::components::transform::Transform
]
";

/// A saved scene with an instance of `outer.bsn`, whose nested instance's
/// inner node holds 77, beside scene nodes repeating 66 and 77 and a repeat
/// of 88 deep in a group.
const GHOST_SCENE: &str = "\
bevy_ecs::hierarchy::Children [
    #Holder
    jackdaw::prefab::components::IsA { source: \"outer.bsn\", deleted: [] }
    jackdaw::prefab::components::PrefabEntityId(0)
    jackdaw_scene_types::node_id::SceneNodeId(5)
    bevy_transform::components::transform::Transform
    --
    #Ghost
    jackdaw_scene_types::node_id::SceneNodeId(77)
    bevy_transform::components::transform::Transform
    --
    #GhostOfNested
    jackdaw_scene_types::node_id::SceneNodeId(66)
    bevy_transform::components::transform::Transform
    --
    #Top
    jackdaw_scene_types::node_id::SceneNodeId(88)
    bevy_transform::components::transform::Transform
    --
    #Group
    jackdaw_scene_types::node_id::SceneNodeId(9)
    bevy_transform::components::transform::Transform
    bevy_ecs::hierarchy::Children [
        #Deep
        jackdaw_scene_types::node_id::SceneNodeId(88)
        bevy_transform::components::transform::Transform
    ]
]
";

fn ghost_project(dir: &std::path::Path) -> std::path::PathBuf {
    let assets = dir.join("assets");
    std::fs::create_dir_all(&assets).expect("the folder is made");
    std::fs::write(assets.join("hut.bsn"), HUT_PREFAB).expect("the prefab writes");
    std::fs::write(assets.join("outer.bsn"), OUTER_PREFAB).expect("the prefab writes");
    let scene = assets.join("ghost.bsn");
    std::fs::write(&scene, GHOST_SCENE).expect("the scene writes");
    scene
}

#[track_caller]
fn assert_ghosts_healed(app: &mut App) {
    let id_of = |app: &mut App, name: &str| {
        let holders = holders_named(app, name);
        assert_eq!(holders.len(), 1, "{name} spawned once");
        holders[0].1.0
    };
    assert_eq!(id_of(app, "Wall"), 77, "the file's inner node keeps its id");
    assert_eq!(
        id_of(app, "Nested"),
        66,
        "the file's nested root keeps its id"
    );
    assert_ne!(id_of(app, "Ghost"), 77, "a ghost shares an inner node's id");
    assert_ne!(
        id_of(app, "GhostOfNested"),
        66,
        "a ghost shares a nested root's id"
    );
    assert_eq!(
        id_of(app, "Top"),
        88,
        "the first copy in the document keeps the id"
    );
    assert_ne!(id_of(app, "Deep"), 88, "a deep repeat keeps the id");
    assert_eq!(id_of(app, "Holder"), 5);
    assert!(
        !jackdaw::scene_io::is_scene_dirty(app.world()),
        "the load dirtied the scene"
    );
    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    assert!(
        !scenes.tabs[scenes.active].dirty,
        "the load dirtied the tab"
    );
}

#[test]
fn opening_a_saved_ghost_gives_it_an_id_of_its_own() {
    let (mut app, dir) = editor_app();
    let scene = ghost_project(dir.path());
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    crate::util::settle_scene_load(&mut app);
    settle(&mut app);
    assert_ghosts_healed(&mut app);
}

#[test]
fn loading_a_saved_ghost_gives_it_an_id_of_its_own() {
    let (mut app, dir) = editor_app();
    let scene = ghost_project(dir.path());
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene);
    crate::util::settle_scene_load(&mut app);
    settle(&mut app);
    assert_ghosts_healed(&mut app);
}

/// A prefab with a nested instance, whose inner node holds 77, and a node of
/// its own repeating 77.
const NESTING_PREFAB: &str = "\
#hut
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
jackdaw_scene_types::node_id::SceneNodeId(60)
bevy_ecs::hierarchy::Children [
    #Nested
    jackdaw::prefab::components::IsA { source: \"inner.bsn\", deleted: [] }
    jackdaw::prefab::components::PrefabEntityId(1)
    jackdaw_scene_types::node_id::SceneNodeId(66)
    bevy_transform::components::transform::Transform
    --
    #Twin
    jackdaw::prefab::components::PrefabEntityId(2)
    jackdaw_scene_types::node_id::SceneNodeId(77)
    bevy_transform::components::transform::Transform
]
";

#[test]
fn a_prefab_tab_keeps_its_ids_and_undoes_its_own_nodes() {
    let (mut app, dir) = editor_app();
    let assets = dir.path().join("assets");
    std::fs::create_dir_all(&assets).expect("the folder is made");
    std::fs::write(assets.join("inner.bsn"), HUT_PREFAB).expect("the prefab writes");
    let file = assets.join("hut.bsn");
    std::fs::write(&file, NESTING_PREFAB).expect("the prefab writes");
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &file);
    crate::util::settle_scene_load(&mut app);
    settle(&mut app);
    assert_eq!(holders_named(&mut app, "Wall")[0].1.0, 77);
    assert_eq!(
        holders_named(&mut app, "Twin")[0].1.0,
        77,
        "a prefab tab is not healed"
    );
    assert_eq!(holders_named(&mut app, "Nested")[0].1.0, 66);

    let root = holders_named(&mut app, "hut");
    let parent = root.first().map(|(entity, _)| entity.to_bits());
    call(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Late", "parent": parent }),
    );
    settle(&mut app);
    assert_eq!(holders_named(&mut app, "Late").len(), 1);
    undo(&mut app);
    assert_eq!(holders_named(&mut app, "Late").len(), 0, "undo removes it");
    redo(&mut app);
    assert_eq!(holders_named(&mut app, "Late").len(), 1, "redo adds it");
}

/// The patch holding the id is found the way the id is read, whatever path the
/// document spells the type by.
#[test]
fn naming_a_scene_replaces_a_repeat_spelled_by_a_short_path() {
    let mut ast = SceneBsnAst::default();
    let short = |ast: &mut SceneBsnAst| {
        ast.create_entity_node(vec![BsnPatch::TupleStruct(BsnTupleStructData {
            type_path: "SceneNodeId".to_string(),
            values: vec![jackdaw_bsn::BsnValue::Int(7)],
        })])
    };
    let first = short(&mut ast);
    let second = short(&mut ast);
    ast.add_to_roots(first);
    ast.add_to_roots(second);
    name_scene_nodes(&mut ast, &TypeRegistry::new());
    assert_eq!(ast.stable_id_of(first), Some(7));
    assert_ne!(ast.stable_id_of(second), Some(7));
}

/// A scene file of `count` cubes, written by the editor itself.
fn scene_of_cubes(dir: &std::path::Path, count: usize) -> std::path::PathBuf {
    let (mut app, _dir) = editor_app();
    row(&mut app, count);
    let text =
        jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), dir).expect("the scene emits");
    let path = dir.join("cubes.bsn");
    std::fs::write(&path, text).expect("the scene writes");
    path
}

/// A frame's worth of document ticks later, every patch newer than `since`.
fn newer_patches(app: &App, entities: &[Entity], since: Tick) -> usize {
    let now = app
        .world()
        .resource::<SceneBsnAst>()
        .world
        .read_change_tick();
    entities
        .iter()
        .flat_map(|&entity| patch_ticks(app, entity))
        .filter(|tick| tick.is_newer_than(since, now))
        .count()
}

/// Move the document's tick on, so a later write reads as newer.
fn mark(app: &mut App) -> Tick {
    let mut live = app.world_mut().resource_mut::<SceneBsnAst>();
    let document = &mut live.bypass_change_detection().world;
    let tick = document.change_tick();
    document.increment_change_tick();
    tick
}

fn brushes_of(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .collect()
}

#[test]
fn a_load_does_not_write_its_brushes_back_into_the_document() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let scene = scene_of_cubes(tmp.path(), 3);
    let (mut app, _dir) = editor_app();
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    let since = mark(&mut app);
    crate::util::settle_scene_load(&mut app);
    settle(&mut app);
    let brushes = brushes_of(&mut app);
    assert_eq!(brushes.len(), 3, "the scene opened");
    assert_eq!(
        newer_patches(&app, &brushes, since),
        0,
        "a value that came from the document was written back"
    );
    assert_views_match(&mut app);
}

#[test]
fn a_legacy_brush_gets_its_topology_written_into_the_document() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let scene = scene_of_cubes(tmp.path(), 1);
    let text = std::fs::read_to_string(&scene).expect("the scene reads");
    let mut ast = jackdaw_bsn::parse_bsn_text(&text).expect("the scene parses");
    let patch = ast
        .roots
        .iter()
        .find_map(|&root| ast.find_patch_by_type_path(root, BRUSH))
        .expect("a brush patch");
    let mut stripped = ast.get_patch(patch).expect("a patch").clone();
    let BsnPatch::Struct(ref mut data) = stripped else {
        panic!("a brush patch is a struct");
    };
    data.fields.0.retain(|field| field.name != "topology");
    ast.set_patch(patch, stripped);
    std::fs::write(&scene, jackdaw_bsn::emit_scene(&ast)).expect("the scene writes");

    let (mut app, _dir) = editor_app();
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    crate::util::settle_scene_load(&mut app);
    settle(&mut app);
    let brush = brushes_of(&mut app)[0];
    assert_eq!(
        app.world()
            .get::<Brush>(brush)
            .expect("a brush")
            .topology
            .polygons
            .len(),
        6,
        "the brush migrated"
    );
    assert!(
        jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path())
            .expect("the scene emits")
            .contains("topology"),
        "the document holds the migrated topology"
    );
}

fn stack_patch_present(app: &App, entity: Entity) -> bool {
    let live = app.world().resource::<SceneBsnAst>();
    live.ast_for(entity).is_some_and(|node| {
        live.find_patch_by_type_path(
            node,
            <jackdaw_geometry::ModifierStack as bevy::reflect::TypePath>::type_path(),
        )
        .is_some()
    })
}

fn mirrored_brush(app: &mut App) -> Entity {
    let brush = row(app, 1)[0];
    app.world_mut()
        .entity_mut(brush)
        .insert(jackdaw_geometry::ModifierStack {
            modifiers: vec![jackdaw_geometry::ModifierEntry::new(
                jackdaw_geometry::Modifier::Mirror(jackdaw_geometry::MeshMirror::default()),
            )],
        });
    settle(app);
    assert!(
        stack_patch_present(app, brush),
        "the stack reaches the document"
    );
    brush
}

#[test]
fn undoing_a_move_keeps_a_brushs_modifier_stack_in_the_document() {
    let (mut app, dir) = editor_app();
    let brush = mirrored_brush(&mut app);
    let moved = call(
        &mut app,
        "component.set",
        json!({
            "entity": brush.to_bits(),
            "type_path": TRANSFORM,
            "field": "translation",
            "value": [0.0, 4.0, 0.0],
        }),
    );
    assert_eq!(moved["result"], "finished", "{moved}");
    settle(&mut app);
    undo(&mut app);

    assert!(
        app.world()
            .get::<jackdaw_geometry::ModifierStack>(brush)
            .is_some(),
        "the live stack survives"
    );
    assert!(
        stack_patch_present(&app, brush),
        "the document keeps the stack"
    );
    assert!(
        jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), dir.path())
            .expect("the scene emits")
            .contains("ModifierStack"),
        "the saved scene keeps the stack"
    );
}

#[test]
fn a_load_does_not_write_a_modifier_stack_back_into_the_document() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let scene = {
        let (mut app, _dir) = editor_app();
        mirrored_brush(&mut app);
        let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path())
            .expect("the scene emits");
        let path = tmp.path().join("mirrored.bsn");
        std::fs::write(&path, text).expect("the scene writes");
        path
    };
    let (mut app, _dir) = editor_app();
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    let since = mark(&mut app);
    crate::util::settle_scene_load(&mut app);
    settle(&mut app);
    let brushes = brushes_of(&mut app);
    assert_eq!(brushes.len(), 1, "the scene opened");
    assert!(
        app.world()
            .get::<jackdaw_geometry::ModifierStack>(brushes[0])
            .is_some(),
        "the stack loaded"
    );
    assert_eq!(
        newer_patches(&app, &brushes, since),
        0,
        "a stack that came from the document was written back"
    );
}

#[test]
fn removing_a_modifier_stack_removes_its_document_patch() {
    let (mut app, _dir) = editor_app();
    let brush = mirrored_brush(&mut app);
    app.world_mut()
        .entity_mut(brush)
        .remove::<jackdaw_geometry::ModifierStack>();
    settle(&mut app);
    assert!(!stack_patch_present(&app, brush), "the patch is gone");
}

#[test]
fn undoing_the_add_of_a_modifier_stack_removes_its_document_patch() {
    let (mut app, _dir) = editor_app();
    let brush = row(&mut app, 1)[0];
    app.world_mut().resource_mut::<Selection>().entities = vec![brush];
    app.update();
    app.world_mut()
        .operator("mesh.mirror.add")
        .call()
        .expect("mesh.mirror.add dispatches")
        .assert_finished();
    settle(&mut app);
    assert!(stack_patch_present(&app, brush), "the stack was added");
    undo(&mut app);
    assert!(
        app.world()
            .get::<jackdaw_geometry::ModifierStack>(brush)
            .is_none(),
        "the live stack is gone"
    );
    assert!(!stack_patch_present(&app, brush), "the patch is gone");
}

/// Read `entity` the way `jackdaw/entity` answers a caller, and hand back the text.
fn read_over_remote(app: &mut App, entity: Entity) -> String {
    let answer = app
        .world_mut()
        .run_system_cached_with(entity_handler, Some(json!({ "entity": entity.to_bits() })))
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("the read refused: {}", err.message));
    answer["bsn"].as_str().expect("bsn text").to_string()
}

/// The corners and the face count of a brush.
fn shape_of(app: &App, entity: Entity) -> (Vec<Vec3>, usize) {
    let brush = app.world().get::<Brush>(entity).expect("a brush");
    (
        brush
            .topology
            .vertices
            .iter()
            .map(|vertex| vertex.position)
            .collect(),
        brush.topology.polygons.len(),
    )
}

#[test]
fn a_component_added_after_a_read_undoes() {
    let (mut app, _dir) = editor_app();
    let block = row(&mut app, 1)[0];
    read_over_remote(&mut app, block);

    let added = call(
        &mut app,
        "component.add",
        json!({ "entity": block.to_bits(), "type_path": "bevy_light::NotShadowCaster" }),
    );
    assert_eq!(added["result"], "finished", "{added}");
    settle(&mut app);
    assert!(
        app.world().get::<NotShadowCaster>(block).is_some(),
        "the add shows"
    );

    undo(&mut app);
    assert!(
        app.world().get::<NotShadowCaster>(block).is_none(),
        "undo removes the component a read came before"
    );
}

/// Select by `rule` on a read brush, run `op`, and undo: the brush is what it was.
#[track_caller]
fn assert_undoes_after_a_read(
    kind: &str,
    rule: serde_json::Value,
    op: &str,
    params: serde_json::Value,
) {
    let (mut app, _dir) = editor_app();
    let block = row(&mut app, 1)[0];
    read_over_remote(&mut app, block);
    let before = shape_of(&app, block);

    let selected = call(
        &mut app,
        "brush.select_elements",
        json!({ "entity": block.to_bits(), "kind": kind, "where": rule }),
    );
    assert_eq!(selected["result"], "finished", "{selected}");
    app.update();
    let outcome = call(&mut app, op, params);
    assert_eq!(outcome["result"], "finished", "{op}: {outcome}");
    settle(&mut app);
    assert_ne!(shape_of(&app, block), before, "{op} changes the brush");

    undo(&mut app);
    assert_eq!(shape_of(&app, block), before, "{op} undoes after a read");
    assert_views_match(&mut app);
}

#[test]
fn an_extrude_after_a_read_undoes() {
    assert_undoes_after_a_read(
        "face",
        json!({ "normal": "+y" }),
        "brush.mesh.extrude_region",
        json!({ "distance": 0.5 }),
    );
}

#[test]
fn a_deleted_face_after_a_read_undoes() {
    assert_undoes_after_a_read(
        "face",
        json!({ "normal": "+y" }),
        "brush.delete_element",
        json!({}),
    );
}

#[test]
fn a_dissolve_after_a_read_undoes() {
    assert_undoes_after_a_read(
        "face",
        json!({ "normal": "+y" }),
        "brush.mesh.dissolve_faces",
        json!({}),
    );
}

#[test]
fn a_vertex_move_after_a_read_undoes() {
    assert_undoes_after_a_read(
        "vertex",
        json!({ "extreme": "max_y" }),
        "brush.elements.transform",
        json!({ "translate": [0.0, 0.5, 0.0] }),
    );
}

/// Every component of `root` and its descendants, with when each was added and
/// last changed.
fn component_ticks(app: &App, root: Entity) -> Vec<(Entity, ComponentId, Tick, Tick)> {
    let world = app.world();
    let mut ticks = Vec::new();
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        let entity_ref = world.entity(entity);
        for &id in entity_ref.archetype().components() {
            let of = entity_ref.get_change_ticks_by_id(id).expect("ticks");
            ticks.push((entity, id, of.added, of.changed));
        }
        if let Some(children) = entity_ref.get::<Children>() {
            stack.extend(children.iter());
        }
    }
    ticks
}

#[test]
fn a_read_changes_nothing() {
    let (mut app, _dir) = editor_app();
    let block = row(&mut app, 1)[0];
    let child = call(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Child", "parent": block.to_bits() }),
    );
    let child = entity_of(&child["entities"][0]);
    settle(&mut app);
    assert!(
        app.world().get::<ChildOf>(child).is_some(),
        "the child is parented"
    );

    let components = component_ticks(&app, block);
    let patches = [patch_ticks(&app, block), patch_ticks(&app, child)];
    let node = app.world().resource::<SceneBsnAst>().ast_for(block);
    let resource = app
        .world()
        .get_resource_change_ticks::<SceneBsnAst>()
        .expect("a document")
        .changed;

    let text = read_over_remote(&mut app, block);
    assert!(
        text.contains("#Child"),
        "the read covers the subtree: {text}"
    );

    assert_eq!(component_ticks(&app, block), components);
    assert_eq!(
        [patch_ticks(&app, block), patch_ticks(&app, child)],
        patches
    );
    let live = app.world().resource::<SceneBsnAst>();
    assert_eq!(live.ast_for(block), node);
    assert_eq!(
        app.world()
            .get_resource_change_ticks::<SceneBsnAst>()
            .expect("a document")
            .changed,
        resource,
    );
}

#[test]
fn a_read_leaves_out_the_visibility_class() {
    let (mut app, _dir) = editor_app();
    let block = row(&mut app, 1)[0];
    let text = read_over_remote(&mut app, block);
    assert!(!text.contains("VisibilityClass"), "{text}");
}

#[test]
fn a_healed_scene_saves_and_reopens_with_the_same_ids() {
    let (mut app, dir) = editor_app();
    let scene = ghost_project(dir.path());
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    crate::util::settle_scene_load(&mut app);
    settle(&mut app);
    assert_ghosts_healed(&mut app);

    const NAMES: [&str; 8] = [
        "Holder",
        "Wall",
        "Nested",
        "Ghost",
        "GhostOfNested",
        "Top",
        "Group",
        "Deep",
    ];
    let ids_by_name = |app: &mut App| -> Vec<(&'static str, u64)> {
        NAMES
            .iter()
            .map(|&name| {
                let holders = holders_named(app, name);
                assert_eq!(holders.len(), 1, "{name} spawned once");
                (name, holders[0].1.0)
            })
            .collect()
    };
    let healed = ids_by_name(&mut app);
    let assets = dir.path().join("assets");
    let text = saved_text(&mut app, &assets);
    let saved = assets.join("saved.bsn");
    std::fs::write(&saved, &text).expect("the scene writes");

    let mut reopened = editor_app_in(dir.path());
    jackdaw::scenes::operators::scene_open_system(reopened.world_mut(), &saved);
    crate::util::settle_scene_load(&mut reopened);
    settle(&mut reopened);
    assert_eq!(ids_by_name(&mut reopened), healed, "saved:\n{text}");
    assert_eq!(saved_text(&mut reopened, &assets), text);
}

#[test]
fn pasting_a_copy_of_an_inner_node_adds_no_override_of_the_original() {
    let (mut app, dir) = scene_with_hut();
    app.world_mut()
        .remove_resource::<jackdaw::entity_ops::SystemClipboard>();
    pack_hut(&mut app);
    let inner = holder_placed(&mut app, "Wall", true);
    let overrides = |app: &mut App| {
        saved_text(app, dir.path())
            .matches("PrefabEntityId(2)")
            .count()
    };
    let before = overrides(&mut app);
    assert_eq!(before, 1, "the instance has one entry for the inner node");

    let brushes = brushes_of(&mut app).len();
    select(&mut app, inner);
    call(&mut app, "entity.copy", json!({}));
    call(&mut app, "entity.paste", json!({}));
    settle(&mut app);
    assert_eq!(brushes_of(&mut app).len(), brushes + 1, "the paste landed");
    assert_eq!(
        overrides(&mut app),
        before,
        "the copy carries the original's file id into the scene"
    );
}
