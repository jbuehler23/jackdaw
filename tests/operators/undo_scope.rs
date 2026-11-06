//! An undo or redo touches only what its edit changed: the other brushes keep
//! their meshes and their document patches, and the viewport mesh of everything
//! it did touch matches the brush.

use super::csg_ops::{call, cube, entity_of};
use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::ecs::change_detection::Tick;
use bevy::prelude::*;
use jackdaw::brush::{BrushHalfedge, BrushMeshCache, BrushMeshChunk, BrushSelection};
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_bsn::{BsnPatch, SceneBsnAst};
use jackdaw_scene_types::Brush;
use serde_json::json;

const BRUSH: &str = "jackdaw_scene_types::types::Brush";
const TRANSFORM: &str = "bevy_transform::components::transform::Transform";

/// An editor app in the running state, where brushes render and sync, on an
/// empty scene in the returned project folder.
fn editor_app() -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
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
    (app, dir)
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
