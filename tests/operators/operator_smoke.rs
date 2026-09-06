//! Smoke test: dispatch every registered operator with empty params and assert
//! the dispatcher resolves the id, the call does not panic, and the result is
//! `Finished`, `Cancelled` or `Running`. Behaviour lives in `operator_modals.rs`,
//! `operator_undo.rs` and `operator_params.rs`.

use crate::util;

use jackdaw::asset_browser::AssetSelectFolderOp;
use jackdaw::entity_ops::{EntityAddImageOp, EntityAddPrefabOp};
use jackdaw::material_browser::MaterialSelectFolderOp;
use jackdaw::scene_ops::{SceneSaveAsOp, SceneSaveOp};
use jackdaw::scenes::operators::SceneOpenOp;
use jackdaw_api::prelude::*;

/// One operator the smoke loop should not call, with a reason. The id comes from
/// the typed `Operator::ID`, so a rename breaks the build.
struct SkipOp {
    id: &'static str,
    /// Why the entry exists. Not consumed by the test logic.
    #[expect(dead_code, reason = "carried for inline documentation")]
    reason: &'static str,
}

impl SkipOp {
    const fn new<O: Operator>(reason: &'static str) -> Self {
        Self { id: O::ID, reason }
    }
}

/// Operators that cannot run from a clean headless app. Native file dialogs are
/// opened immediately by the invoke system and survive test shutdown, so without
/// these skips a smoke run stacks one stuck picker per dispatch.
const SMOKE_SKIP_LIST: &[SkipOp] = &[
    SkipOp::new::<SceneOpenOp>("spawns native file-open dialog"),
    SkipOp::new::<SceneSaveAsOp>("spawns native file-save dialog"),
    SkipOp::new::<SceneSaveOp>(
        "falls through to scene.save_as (native dialog) when no SceneFilePath is set",
    ),
    SkipOp::new::<AssetSelectFolderOp>("spawns native folder picker"),
    SkipOp::new::<MaterialSelectFolderOp>("spawns native folder picker"),
    SkipOp::new::<EntityAddImageOp>("spawns native image-file picker"),
    SkipOp::new::<EntityAddPrefabOp>("spawns native prefab-file picker"),
];

#[test]
fn smoke_dispatch_every_operator() {
    let mut app = util::editor_test_app();

    // The prefab-save and document operators write files under the project root,
    // falling back to the process working directory (the repo) without one.
    let project_dir = tempfile::tempdir().expect("tempdir");
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot::new(
            project_dir.path().to_path_buf(),
            jackdaw::project::ProjectConfig::default(),
        ));

    let ids = util::iter_operator_ids(&mut app);
    // Floor catches "a whole module went unregistered" regressions; bump it as
    // new operators land.
    assert!(
        ids.len() >= 60,
        "expected at least 60 registered operators after editor_test_app() startup, got {}",
        ids.len()
    );

    // A skip entry naming an operator that no longer exists would silently stop
    // covering anything.
    for skip in SMOKE_SKIP_LIST {
        assert!(
            ids.iter().any(|id| id.as_ref() == skip.id),
            "skip list names `{}`, which is not a registered operator",
            skip.id
        );
    }

    let mut failures: Vec<String> = Vec::new();
    for id in ids {
        if SMOKE_SKIP_LIST.iter().any(|skip| skip.id == id.as_ref()) {
            continue;
        }
        // Cancel any modal a prior iteration left running, or the next dispatch
        // is refused with `ModalAlreadyActive`.
        let _ = app.world_mut().operator("modal.cancel").call();
        match app.world_mut().operator(id.clone()).call() {
            Ok(OperatorResult::Finished | OperatorResult::Cancelled | OperatorResult::Running) => {}
            Err(CallOperatorError::UnknownId(missing)) => {
                failures.push(format!("UnknownId for {id} (resolver returned {missing})"));
            }
            Err(other) => {
                failures.push(format!("{id} -> {other}"));
            }
        }
    }
    let _ = app.world_mut().operator("modal.cancel").call();

    assert!(
        failures.is_empty(),
        "{} operators failed smoke dispatch:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
