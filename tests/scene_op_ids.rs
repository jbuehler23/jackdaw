//! Operator ids must stay unique: the dispatcher indexes them into a
//! `HashMap`, so a duplicate means one of the two operators is unreachable
//! by id.

use std::collections::HashMap;

use bevy::prelude::*;
use jackdaw::scenes::Scenes;
use jackdaw_api::prelude::*;

mod util;

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

#[test]
fn scene_file_operators_use_the_tab_labels() {
    let mut app = util::editor_test_app();

    for (id, expected) in [("scene.new", "New Scene"), ("scene.open", "Open Scene...")] {
        let labels = util::operator_labels(&mut app, id);
        assert_eq!(
            labels,
            [expected],
            "{id} should have a single tab-operator registration"
        );
    }
}

#[test]
fn no_duplicate_operator_ids() {
    let mut app = util::editor_test_app();

    let mut by_id: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, label) in util::operator_id_labels(&mut app) {
        by_id.entry(id).or_default().push(label);
    }

    let mut unexpected: Vec<String> = by_id
        .iter()
        .filter(|(_id, labels)| labels.len() > 1)
        .map(|(id, labels)| format!("{id}: {labels:?}"))
        .collect();
    unexpected.sort();
    assert!(
        unexpected.is_empty(),
        "these operator ids are registered by more than one subsystem, so only \
         the last registration is reachable by id: {unexpected:#?}"
    );
}
