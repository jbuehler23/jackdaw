//! Verification for the GLB authoring model:
//! `GltfSource` is authored, `WorldAssetRoot` is derived.

use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::world_serialization::WorldAssetRoot;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_commands::CommandHistory;

mod util;

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
}

/// The skip list is matched by string, so a typo silently reverts the fix.
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

    // Derived, not authored.
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

/// The asset path behind the derived handle, which is what actually decides
/// whether anything renders. Asserting the component merely exists would pass
/// with a defaulted handle.
fn current_handle(app: &mut App) -> String {
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
