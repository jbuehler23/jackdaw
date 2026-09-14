//! Verification for the GLB authoring model:
//! `GltfSource` is authored, `WorldAssetRoot` is derived.

use crate::util;

use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::world_serialization::WorldAssetRoot;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_commands::CommandHistory;

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
