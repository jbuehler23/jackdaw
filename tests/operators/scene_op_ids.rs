//! Operator ids must stay unique: the dispatcher indexes them into a
//! `HashMap`, so a duplicate means one of the two operators is unreachable
//! by id.

use crate::util;

use std::collections::HashMap;

use bevy::prelude::*;
use jackdaw::scenes::Scenes;
use jackdaw_api::prelude::*;

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

/// The scatter ops a caller with no pointer needs are registered under the
/// ids the panel's buttons, the book and the MCP tools all spell.
#[test]
fn the_scatter_group_operators_are_registered_under_their_ids() {
    let mut app = util::editor_test_app();

    for (id, expected) in [
        ("terrain.scatter.adopt", "Adopt Scatter Group"),
        ("terrain.scatter.group.select", "Select Scatter Group"),
    ] {
        let labels = util::operator_labels(&mut app, id);
        assert_eq!(labels, [expected], "{id} is not registered exactly once");
    }
}

/// The tint ops a caller with no pointer needs, under the ids the options
/// bar, the Textures tab and the MCP tools all spell.
#[test]
fn the_tint_operators_are_registered_under_their_ids() {
    let mut app = util::editor_test_app();

    for (id, expected) in [
        ("terrain.paint.tint", "Tint Colour"),
        ("terrain.tint.stamp", "Tint Stamp"),
        ("terrain.tint.variation", "Tint Variation"),
        ("terrain.tint.strength", "Tint Strength"),
        (
            "terrain.material.blend_sharpness",
            "Terrain Blend Sharpness",
        ),
    ] {
        let labels = util::operator_labels(&mut app, id);
        assert_eq!(labels, [expected], "{id} is not registered exactly once");
    }
}

/// The prefab ops a caller with no pointer needs, under the ids the outliner,
/// the book and the MCP tools all spell.
#[test]
fn the_prefab_pack_operators_are_registered_under_their_ids() {
    let mut app = util::editor_test_app();

    for (id, expected) in [
        ("prefab.pack", "Pack Group as Prefab"),
        ("prefab.pack_matching", "Pack Matching Groups as Prefab"),
    ] {
        let labels = util::operator_labels(&mut app, id);
        assert_eq!(labels, [expected], "{id} is not registered exactly once");
    }
}
