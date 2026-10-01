//! Verification for the GLB authoring model:
//! `GltfSource` is authored, `WorldAssetRoot` is derived.

use crate::util;

use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::world_serialization::WorldAssetRoot;
use jackdaw::remote::server::call_operator_handler;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_commands::CommandHistory;
use serde_json::json;

trait Finished {
    fn assert_finished(self);
}
impl Finished for OperatorResult {
    fn assert_finished(self) {
        assert_eq!(self, OperatorResult::Finished, "place_gltf did not finish");
    }
}

fn place(app: &mut App, path: &str) {
    app.world_mut()
        .operator("entity.place_gltf")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("path", path.to_string())
        .param("pos_x", 1.0f64)
        .param("pos_y", 0.0f64)
        .param("pos_z", 0.0f64)
        .call()
        .expect("dispatch")
        .assert_finished();
    // The model is handed to the scene spawner on the frame after its source
    // is set, so a whole document's worth never lands in one frame.
    app.update();
}

/// The skip list is matched by string, so a typo silently disables the skip.
#[test]
fn world_asset_root_skip_path_matches_real_type_path() {
    assert!(
        jackdaw::scene_io::should_skip_component(WorldAssetRoot::type_path()),
        "WorldAssetRoot type path is {:?}, which the skip list does not match",
        WorldAssetRoot::type_path()
    );
}

#[test]
fn gltf_source_derives_world_asset_root_and_stays_out_of_the_document() {
    let mut app = util::editor_test_app();
    place(&mut app, "models/dungeon.glb");

    let mut q = app
        .world_mut()
        .query::<(Entity, &jackdaw_scene_types::GltfSource)>();
    let (entity, source) = q.single(app.world()).expect("one GltfSource");
    assert_eq!(source.path, "models/dungeon.glb");

    assert!(
        app.world().get::<WorldAssetRoot>(entity).is_some(),
        "the observer should have derived WorldAssetRoot from GltfSource"
    );
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.ast_for(entity).expect("GLB must be in the document");
    assert!(
        ast.find_patch_by_type_path(node, WorldAssetRoot::type_path())
            .is_none(),
        "the derived handle must not be written into the document"
    );
    assert!(
        ast.find_patch_by_type_path(node, jackdaw_scene_types::GltfSource::type_path())
            .is_some(),
        "GltfSource is the authored truth and must be in the document"
    );
}

#[test]
fn undo_redo_restores_a_loadable_gltf() {
    let mut app = util::editor_test_app();
    place(&mut app, "models/dungeon.glb");

    let handle_before = current_handle(&mut app);

    app.world_mut()
        .resource_scope(|world, mut h: Mut<CommandHistory>| h.undo(world));
    let mut q = app.world_mut().query::<&jackdaw_scene_types::GltfSource>();
    assert_eq!(q.iter(app.world()).count(), 0, "undo removes the GLB");

    app.world_mut()
        .resource_scope(|world, mut h: Mut<CommandHistory>| h.redo(world));

    let handle_after = current_handle(&mut app);
    assert_eq!(
        handle_before, handle_after,
        "redo must restore a handle pointing at the same asset, not a default one"
    );
}

/// The asset path behind the derived handle: asserting the component merely
/// exists would pass with a defaulted handle.
fn current_handle(app: &mut App) -> String {
    app.update();
    let mut q = app
        .world_mut()
        .query::<(&jackdaw_scene_types::GltfSource, &WorldAssetRoot)>();
    let (_, root) = q.single(app.world()).expect("one GLB root");
    let server = app.world().resource::<AssetServer>();
    let path = server
        .get_path(root.0.id())
        .map(|p| p.to_string())
        .unwrap_or_else(|| panic!("derived handle has no asset path (defaulted handle?)"));
    assert!(
        path.contains("dungeon.glb"),
        "handle points at {path:?}, not the placed model"
    );
    path
}

/// A model's render root goes out a frame or more after its source is set, so
/// the placement can be taken back in between. Bringing the model on anyway
/// would leave an instance under an entity that names no model, which nothing
/// owns and nothing takes down again.
#[test]
fn a_model_whose_source_went_while_its_root_waited_never_comes_on() {
    let mut app = util::editor_test_app();

    let entity = app
        .world_mut()
        .spawn(jackdaw_scene_types::GltfSource {
            path: "models/cube.gltf".into(),
            scene_index: 0,
        })
        .id();
    app.world_mut()
        .entity_mut(entity)
        .remove::<jackdaw_scene_types::GltfSource>();
    app.update();
    app.update();

    assert!(
        app.world().get::<WorldAssetRoot>(entity).is_none(),
        "a model whose source went before its turn came was brought on anyway"
    );
}

/// Models wait their turn, so an undo can land while a crowd of them is still
/// queued. The undo respawns every entity in the document, which leaves those
/// entries waiting on entities that have gone: handing a render root to one of
/// those puts an instance under nothing, and the instance keeps the glTF's
/// meshes and materials alive after the scene that named them is gone.
#[test]
fn an_undo_that_respawns_the_scene_leaves_no_model_queued_for_an_entity_that_has_gone() {
    let mut app = util::editor_test_app();
    place(&mut app, "models/dungeon.glb");

    // A crowd still waiting its turn when the undo lands.
    for index in 0..8 {
        app.world_mut().spawn((
            Name::new(format!("Waiting{index}")),
            Transform::default(),
            jackdaw_scene_types::GltfSource {
                path: format!("models/waiting{index}.gltf"),
                scene_index: 0,
            },
        ));
    }
    app.world_mut().flush();
    assert!(
        !app.world()
            .resource::<jackdaw::entity_ops::PendingModelRoots>()
            .is_empty(),
        "the crowd is queued, so the undo has something to strand"
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));

    let named = app
        .world_mut()
        .query::<&jackdaw_scene_types::GltfSource>()
        .iter(app.world())
        .count();
    let queued = app
        .world()
        .resource::<jackdaw::entity_ops::PendingModelRoots>()
        .len();
    assert!(
        queued <= named,
        "{queued} models are queued for a scene that names {named}"
    );

    // The frames after the undo hand out whatever is left; a root reaching an
    // entity that names no model would be a command error, and every root that
    // did land belongs to an entity that is still there.
    for _ in 0..8 {
        app.update();
    }
    let stranded = app
        .world_mut()
        .query_filtered::<Entity, (
            With<WorldAssetRoot>,
            Without<jackdaw_scene_types::GltfSource>,
        )>()
        .iter(app.world())
        .count();
    assert_eq!(
        stranded, 0,
        "a model came on under an entity that names none"
    );
}

/// The thumbnail stage builds its subjects beside the open scene, under an
/// editor root a scene teardown leaves standing. Taking the scene down used to
/// empty the whole queue, so a thumbnail whose model was still waiting its turn
/// lost it with nothing left to ask for it again and drew an empty tile.
#[test]
fn a_model_queued_beside_the_scene_survives_the_scene_being_taken_down() {
    let mut app = util::editor_test_app();
    let stage = app
        .world_mut()
        .spawn((jackdaw::EditorEntity, Transform::default()))
        .id();
    let subject = app
        .world_mut()
        .spawn((
            ChildOf(stage),
            Transform::default(),
            jackdaw_scene_types::GltfSource {
                path: "models/lantern.glb".into(),
                scene_index: 0,
            },
        ))
        .id();
    app.world_mut().flush();

    jackdaw::scenes::operators::scene_new_system(app.world_mut());

    assert!(
        app.world().get_entity(subject).is_ok(),
        "the teardown took the thumbnail stage down with the scene"
    );
    for _ in 0..8 {
        app.update();
    }
    assert!(
        app.world().get::<WorldAssetRoot>(subject).is_some(),
        "a model queued beside the scene was forgotten when the scene went"
    );
}

/// A scene root is despawned with its descendants, and the models the
/// descendants name are queued under their own entities. Forgetting only the
/// root would leave those entries waiting on entities that have gone.
#[test]
fn a_despawned_parent_takes_the_models_its_children_queued_with_it() {
    let mut app = util::editor_test_app();
    let parent = app
        .world_mut()
        .spawn((Name::new("Group"), Transform::default()))
        .id();
    app.world_mut().spawn((
        ChildOf(parent),
        Transform::default(),
        jackdaw_scene_types::GltfSource {
            path: "models/lantern.glb".into(),
            scene_index: 0,
        },
    ));
    app.world_mut().flush();
    assert_eq!(
        app.world()
            .resource::<jackdaw::entity_ops::PendingModelRoots>()
            .len(),
        1,
        "the child's model is queued, so the despawn has something to forget"
    );

    app.world_mut().entity_mut(parent).despawn();

    assert!(
        app.world()
            .resource::<jackdaw::entity_ops::PendingModelRoots>()
            .is_empty(),
        "a child despawned with its parent left its model queued"
    );
}

/// The textured material a placed model's meshes wear, once it and its base
/// colour image have loaded.
fn loaded_base_colour(app: &App) -> Option<Handle<Image>> {
    let materials = app.world().resource::<Assets<StandardMaterial>>();
    let server = app.world().resource::<AssetServer>();
    let mut worn = app
        .world()
        .try_query::<&MeshMaterial3d<StandardMaterial>>()?;
    worn.iter(app.world()).find_map(|material| {
        let texture = materials.get(&material.0)?.base_color_texture.clone()?;
        server
            .is_loaded_with_dependencies(&texture)
            .then_some(texture)
    })
}

#[test]
fn a_placed_model_loads_its_meshes_with_the_textures_its_materials_name() {
    let mut app = util::editor_test_app();
    place(&mut app, "jan/jan.gltf");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while loaded_base_colour(&app).is_none() && std::time::Instant::now() < deadline {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let texture = loaded_base_colour(&app)
        .expect("the model's meshes wear a material whose base colour image loaded");
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&texture)
            .is_some(),
        "the image the material names is in the image assets"
    );
}

/// Entities whose `ChildOf` names a parent that does not list them among its
/// `Children`, so transforms and visibility never reach them.
fn unlisted_children(app: &mut App) -> Vec<Entity> {
    let mut children = app.world_mut().query::<(Entity, &ChildOf)>();
    children
        .iter(app.world())
        .filter(|(child, child_of)| {
            app.world()
                .get::<Children>(child_of.parent())
                .is_none_or(|listed| !listed.contains(child))
        })
        .map(|(child, _)| child)
        .collect()
}

fn mesh_count(app: &mut App) -> usize {
    app.world_mut().query::<&Mesh3d>().iter(app.world()).count()
}

/// Run frames until the scene holds `expected` meshes, or a deadline passes.
fn settle_meshes(app: &mut App, expected: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while mesh_count(app) < expected && std::time::Instant::now() < deadline {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for _ in 0..4 {
        app.update();
    }
}

fn translations(app: &mut App) -> Vec<[f32; 3]> {
    let mut placed: Vec<[f32; 3]> = app
        .world_mut()
        .query_filtered::<&Transform, With<jackdaw_scene_types::GltfSource>>()
        .iter(app.world())
        .map(|transform| transform.translation.to_array())
        .collect();
    placed.sort_by(|a, b| a.partial_cmp(b).expect("finite translations"));
    placed
}

fn models(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<jackdaw_scene_types::GltfSource>>()
        .iter(app.world())
        .collect()
}

fn delete_then_undo(app: &mut App, targets: &[Entity]) {
    let ids = targets
        .iter()
        .map(|entity| entity.to_bits().to_string())
        .collect::<Vec<_>>()
        .join(",");
    app.world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": "entity.delete", "params": { "entities": ids } })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("entity.delete refused: {}", err.message));
    app.update();
    assert!(models(app).is_empty(), "the delete takes every model down");
    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("dispatch")
        .assert_finished();
    app.update();
}

fn assert_undo_restores_models_whole(count: usize) {
    let mut app = util::editor_test_app();
    let mut meshes_per_model = 0;
    for _ in 0..count {
        place(&mut app, "models/dungeon.glb");
        if meshes_per_model == 0 {
            settle_meshes(&mut app, 1);
            meshes_per_model = mesh_count(&mut app);
        }
    }
    let placed = meshes_per_model * count;
    settle_meshes(&mut app, placed);
    assert!(meshes_per_model > 0, "the model spawned meshes");
    assert_eq!(mesh_count(&mut app), placed, "every model came on");
    assert!(unlisted_children(&mut app).is_empty());

    let targets = models(&mut app);
    let placed_at = translations(&mut app);
    delete_then_undo(&mut app, &targets);
    settle_meshes(&mut app, placed);
    assert_eq!(
        translations(&mut app),
        placed_at,
        "each model comes back where it was"
    );

    assert_eq!(
        models(&mut app).len(),
        count,
        "undo brings every model back"
    );
    assert_eq!(
        mesh_count(&mut app),
        placed,
        "undo brings each model's meshes back once"
    );
    let unlisted = unlisted_children(&mut app);
    assert!(
        unlisted.is_empty(),
        "{} restored entities are missing from their parent's children",
        unlisted.len()
    );
}

#[test]
fn undoing_a_model_delete_restores_its_hierarchy() {
    assert_undo_restores_models_whole(1);
}

#[test]
fn undoing_a_bulk_model_delete_restores_every_hierarchy() {
    assert_undo_restores_models_whole(12);
}

#[test]
fn a_delete_is_one_history_entry() {
    let mut app = util::editor_test_app();
    place(&mut app, "models/dungeon.glb");
    let targets = models(&mut app);
    let before = app.world().resource::<CommandHistory>().undo_stack.len();

    app.world_mut()
        .operator("entity.delete")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("entity", targets[0])
        .call()
        .expect("dispatch")
        .assert_finished();
    app.update();

    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1,
        "one undo takes the delete back"
    );
}

fn assert_group_delete_restores_its_models(with_a_model: bool) {
    let mut app = util::editor_test_app();
    let group = app
        .world_mut()
        .spawn((Name::new("Grove"), Transform::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), group);
    let mut models_in_group = Vec::new();
    for (index, x) in [0.0, 1.0, 2.0].into_iter().enumerate() {
        let model = app
            .world_mut()
            .spawn((
                Name::new(format!("Tree{index}")),
                Transform::from_xyz(x, 0.0, 0.0),
                jackdaw_scene_types::GltfSource {
                    path: "models/dungeon.glb".into(),
                    scene_index: 0,
                },
                ChildOf(group),
            ))
            .id();
        jackdaw::scene_io::register_entity_in_ast(app.world_mut(), model);
        models_in_group.push(model);
    }
    settle_meshes(&mut app, 3);
    let placed = mesh_count(&mut app);
    assert!(placed > 0, "the models spawned meshes");

    let targets = if with_a_model {
        vec![group, models_in_group[1]]
    } else {
        vec![group]
    };
    delete_then_undo(&mut app, &targets);
    settle_meshes(&mut app, placed);

    let mut groves = app
        .world_mut()
        .query_filtered::<Entity, Without<jackdaw_scene_types::GltfSource>>();
    let restored = groves
        .iter(app.world())
        .find(|&entity| {
            app.world()
                .get::<Name>(entity)
                .is_some_and(|name| name.as_str() == "Grove")
        })
        .expect("undo brings the group back");
    let names: Vec<String> = app
        .world()
        .get::<Children>(restored)
        .expect("the group lists its children")
        .iter()
        .filter_map(|child| app.world().get::<Name>(child).map(ToString::to_string))
        .collect();
    assert_eq!(
        names,
        ["Tree0", "Tree1", "Tree2"],
        "in their authored order"
    );
    assert_eq!(
        mesh_count(&mut app),
        placed,
        "each model's meshes come back once"
    );
    assert!(unlisted_children(&mut app).is_empty());
}

#[test]
fn undoing_a_group_delete_puts_its_models_back_under_it() {
    assert_group_delete_restores_its_models(false);
}

#[test]
fn undoing_a_delete_of_a_group_and_one_of_its_models_restores_that_model_once() {
    assert_group_delete_restores_its_models(true);
}
