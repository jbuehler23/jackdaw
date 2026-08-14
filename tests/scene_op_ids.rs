//! `scene.open` is registered twice, by the single-scene ops in
//! `scene_ops.rs` and the tab ops in `scenes/operators.rs`. The
//! dispatcher indexes operators by id into a `HashMap`, so the last
//! registration wins and registration order decides which one a menu click or
//! keybind runs.

use std::collections::HashMap;

use bevy::prelude::*;
use jackdaw::scenes::Scenes;
use jackdaw_api::prelude::*;

mod util;

/// Ids knowingly registered by two subsystems. A new duplicate is an
/// accidental collision: one of the two operators becomes unreachable by id.
const KNOWN_DUPLICATE_IDS: &[&str] = &["scene.open"];

#[test]
fn scene_new_dispatch_resolves_to_the_tab_operator() {
    let mut app = util::editor_test_app();

    let before = app.world().resource::<Scenes>().tabs.len();
    let result = app
        .world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatch errored");
    app.update();
    let after = app.world().resource::<Scenes>().tabs.len();

    assert_eq!(
        after,
        before + 1,
        "scene.new should append a tab; dispatch returned {result:?}, tabs {before} -> {after}"
    );
}

/// The tab operators carry the user-facing labels, so a flipped lookup changes
/// the File menu wording with it.
#[test]
fn both_scene_op_registrations_are_present() {
    let mut app = util::editor_test_app();

    for (id, expected) in [
        ("scene.new", ["New Scene"].as_slice()),
        ("scene.open", ["Open", "Open Scene..."].as_slice()),
    ] {
        let mut labels = util::operator_labels(&mut app, id);
        labels.sort_unstable();
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        assert_eq!(
            labels, expected,
            "{id} registrations changed; update KNOWN_DUPLICATE_IDS and the \
             dispatch expectations in this file to match"
        );
    }
}

#[test]
fn duplicate_operator_ids_stay_limited_to_the_known_set() {
    let mut app = util::editor_test_app();

    let mut by_id: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, label) in util::operator_id_labels(&mut app) {
        by_id.entry(id).or_default().push(label);
    }

    let mut unexpected: Vec<String> = by_id
        .iter()
        .filter(|(id, labels)| labels.len() > 1 && !KNOWN_DUPLICATE_IDS.contains(id))
        .map(|(id, labels)| format!("{id}: {labels:?}"))
        .collect();
    unexpected.sort();
    assert!(
        unexpected.is_empty(),
        "these operator ids are registered by more than one subsystem, so only \
         the last registration is reachable by id: {unexpected:#?}"
    );

    // Keeps the allowlist from going stale.
    for id in KNOWN_DUPLICATE_IDS {
        assert!(
            by_id.get(id).is_some_and(|labels| labels.len() > 1),
            "{id} is no longer double-registered; drop it from KNOWN_DUPLICATE_IDS"
        );
    }
}
