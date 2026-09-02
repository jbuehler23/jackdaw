//! The `Default` of a list-shaped widget is a persisted contract.
//!
//! A component equal to its default emits as a bare type path, so every
//! document that ever saved one of these at rest carries the type path and
//! nothing else. What that path means on the way back in is the `Default`
//! impl, which makes changing one a silent reinterpretation of scenes already
//! on disk. This is the golden the change would have to break first.

use crate::util;

use bevy::prelude::*;
use jackdaw_widgets_runtime::{Dropdown, RadioOptions, TabStrip};

/// A screen holding the three widgets at rest: each one a bare type path,
/// which is what a save writes for a component equal to its default.
const AT_REST: &str = "\
#Screen
jackdaw_scene_types::UiSceneRoot
bevy_ui::ui_node::Node
bevy_ecs::hierarchy::Children [
    #Picker
    jackdaw_widgets_runtime::Dropdown
    bevy_ui::ui_node::Node
    ,
    #Choices
    jackdaw_widgets_runtime::RadioOptions
    bevy_ui::ui_node::Node
    ,
    #Tabs
    jackdaw_widgets_runtime::TabStrip
    bevy_ui::ui_node::Node
]
";

fn by_name(app: &mut App, name: &str) -> Entity {
    let world = app.world_mut();
    world
        .query::<(Entity, &Name)>()
        .iter(world)
        .find(|(_, entity_name)| entity_name.as_str() == name)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("the loaded document holds no entity named {name}"))
}

#[test]
fn a_document_holding_the_bare_paths_loads_the_documented_defaults() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("at-rest.bsn");
    std::fs::write(&path, AT_REST).expect("write the scene");

    let mut app = util::editor_test_app();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &path);
    for _ in 0..4 {
        app.update();
    }

    let picker = by_name(&mut app, "Picker");
    assert_eq!(
        app.world().get::<Dropdown>(picker),
        Some(&Dropdown {
            options: Vec::new(),
            selected: 0,
        }),
        "a bare `Dropdown` is no options with the first one chosen",
    );

    let choices = by_name(&mut app, "Choices");
    assert_eq!(
        app.world().get::<RadioOptions>(choices),
        Some(&RadioOptions {
            options: Vec::new(),
            selected: 0,
        }),
        "a bare `RadioOptions` is no choices with the first one taken",
    );

    let tabs = by_name(&mut app, "Tabs");
    assert_eq!(
        app.world().get::<TabStrip>(tabs),
        Some(&TabStrip {
            labels: Vec::new(),
            active: 0,
        }),
        "a bare `TabStrip` is no labels with the first tab in front",
    );
}

/// The other direction: the three at rest still emit as bare paths, so the
/// document above is the one a save keeps producing.
#[test]
fn the_defaults_still_emit_as_the_bare_paths_they_are_read_from() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("at-rest.bsn");
    std::fs::write(&path, AT_REST).expect("write the scene");

    let mut app = util::editor_test_app();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &path);
    for _ in 0..4 {
        app.update();
    }

    let text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(app.world_mut(), dir.path());
    for bare in [
        "jackdaw_widgets_runtime::Dropdown\n",
        "jackdaw_widgets_runtime::RadioOptions\n",
        "jackdaw_widgets_runtime::TabStrip\n",
    ] {
        assert!(
            text.contains(bare),
            "the widget at rest emits as its bare path:\n{text}",
        );
    }
}
