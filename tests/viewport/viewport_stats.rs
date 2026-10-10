//! The Stats overlay: shown with the setting, counting what the scene draws,
//! and placed in the viewport's corner.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::fps_overlay::SceneReadout;
use jackdaw::viewport_settings::ViewportSettingsFile;

fn run(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .unwrap_or_else(|err| panic!("{clause}: {err}"))
        .assert_finished();
    app.update();
}

fn readout(app: &mut App) -> (Display, String) {
    let mut readouts = app
        .world_mut()
        .query_filtered::<(&Node, &Text), With<SceneReadout>>();
    let (node, text) = readouts
        .single(app.world())
        .expect("the overlay carries one scene readout");
    (node.display, text.0.clone())
}

#[test]
fn stats_show_a_readout_that_counts_the_scene() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = util::editor_test_app();
    app.world_mut().resource_mut::<ViewportSettingsFile>().path =
        Some(dir.path().join("viewport.json"));
    let root = app.world_mut().spawn(Node::default()).id();
    jackdaw::viewport::build_viewport_panel(app.world_mut(), root);
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::default());
    app.world_mut().spawn((Mesh3d(mesh), Transform::default()));
    run(&mut app, "entity.add.point_light");
    assert_eq!(readout(&mut app).0, Display::None, "hidden until asked for");

    run(&mut app, "viewport.stats.toggle on=true");
    util::update_until(
        &mut app,
        "the stats readout",
        std::time::Duration::from_secs(10),
        |app| !readout(app).1.is_empty(),
    );
    let (display, text) = readout(&mut app);
    assert_eq!(display, Display::Flex);
    assert!(
        text.starts_with("meshes ") && !text.contains("drawn of 0\n"),
        "the mesh is counted: {text:?}"
    );
    assert!(text.contains("lights 1,"), "and one light: {text:?}");

    run(&mut app, "viewport.stats.toggle on=false");
    assert_eq!(readout(&mut app).0, Display::None);
}

fn graph_display(app: &mut App) -> Display {
    let mut graphs = app.world_mut().query_filtered::<&Node, With<
        MaterialNode<bevy::dev_tools::frame_time_graph::FrametimeGraphMaterial>,
    >>();
    graphs
        .single(app.world())
        .expect("the overlay carries one frame time graph")
        .display
}

#[test]
fn the_frame_time_graph_shows_only_when_asked_for_under_the_stats() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = util::editor_test_app();
    app.world_mut().resource_mut::<ViewportSettingsFile>().path =
        Some(dir.path().join("viewport.json"));
    let root = app.world_mut().spawn(Node::default()).id();
    jackdaw::viewport::build_viewport_panel(app.world_mut(), root);

    run(&mut app, "viewport.stats.toggle on=true");
    app.update();
    assert_eq!(readout(&mut app).0, Display::Flex);
    assert_eq!(
        graph_display(&mut app),
        Display::None,
        "the stats start as text alone"
    );

    run(&mut app, "viewport.stats.graph.toggle on=true");
    app.update();
    assert_eq!(
        graph_display(&mut app),
        Display::Flex,
        "the graph is opt-in"
    );

    run(&mut app, "viewport.stats.toggle on=false");
    app.update();
    assert_eq!(
        graph_display(&mut app),
        Display::None,
        "and goes with the readout"
    );
}
