//! Smoke test: dispatch every registered operator with empty params
//! and assert that:
//!  - the dispatcher resolves the id (no `UnknownId`),
//!  - the call doesn't panic,
//!  - the result is one of the well-formed variants
//!    (`Finished` / `Cancelled` / `Running`).
//!
//! This is the cheapest possible regression net for the operator surface.
//! Behavioural correctness lives in `operator_modals.rs`,
//! `operator_undo.rs`, and `operator_params.rs`; this file just ensures
//! every id is reachable from a clean editor app.

use jackdaw::asset_browser::AssetSelectFolderOp;
use jackdaw::material_browser::MaterialSelectFolderOp;
use jackdaw::navmesh::save_load::{NavmeshLoadOp, NavmeshSaveOp};
use jackdaw::scene_ops::{SceneOpenOp, SceneSaveAsOp, SceneSaveOp};
use jackdaw_api::prelude::*;

mod util;

/// One operator the smoke loop should not actually call, paired with
/// a human-readable reason. The id is sourced from the typed
/// `Operator::ID` constant so a rename of the underlying op breaks
/// the build instead of silently leaving the entry stale.
struct SkipOp {
    id: &'static str,
    /// Documentation only: surfaces *why* the entry exists so future
    /// readers don't have to git-blame to find out. Not consumed by
    /// the test logic.
    #[expect(dead_code, reason = "carried for inline documentation")]
    reason: &'static str,
}

impl SkipOp {
    const fn new<O: Operator>(reason: &'static str) -> Self {
        Self { id: O::ID, reason }
    }
}

/// Operators that genuinely cannot run from a clean headless app
/// without test fixtures we don't have here.
///
/// Native file/folder dialogs are spawned through `rfd::AsyncFileDialog`
/// on a background task pool. The dialog is opened **immediately** when
/// the operator's invoke system runs. The operator returns `Finished`
/// synchronously, but the OS picker is already on screen and the task
/// survives test shutdown, so without these skips a smoke run stacks
/// one new file picker per dispatch (16 stuck folder dialogs after a
/// single `cargo test` was the reproducer).
const SMOKE_SKIP_LIST: &[SkipOp] = &[
    SkipOp::new::<SceneOpenOp>("spawns native file-open dialog"),
    SkipOp::new::<SceneSaveAsOp>("spawns native file-save dialog"),
    SkipOp::new::<SceneSaveOp>(
        "falls through to scene.save_as (native dialog) when no SceneFilePath is set",
    ),
    SkipOp::new::<AssetSelectFolderOp>("spawns native folder picker"),
    SkipOp::new::<MaterialSelectFolderOp>("spawns native folder picker"),
    SkipOp::new::<NavmeshSaveOp>("spawns native file-save dialog"),
    SkipOp::new::<NavmeshLoadOp>("spawns native file-open dialog"),
];

#[test]
fn smoke_dispatch_every_operator() {
    let mut app = util::editor_test_app();
    let ids = util::iter_operator_ids(&mut app);
    // Floor catches "we forgot to register a whole module" regressions.
    // The exact count grows over time as new operators land; bump this
    // up if it ever feels stale, but keep it as a regression guard.
    assert!(
        ids.len() >= 60,
        "expected at least 60 registered operators after editor_test_app() startup, got {}",
        ids.len()
    );

    let mut failures: Vec<String> = Vec::new();
    for id in ids {
        if SMOKE_SKIP_LIST.iter().any(|skip| skip.id == id.as_ref()) {
            continue;
        }
        // Cancel any modal that a prior iteration left running so the
        // next dispatch isn't refused with `ModalAlreadyActive`. The
        // built-in `modal.cancel` operator is a no-op when nothing is
        // active.
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
    // Leave the world clean for any subsequent test sharing the binary.
    let _ = app.world_mut().operator("modal.cancel").call();

    assert!(
        failures.is_empty(),
        "{} operators failed smoke dispatch:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
