//! Authoring an animation graph: the operators write a file the runtime loader
//! reads back, the canvas and the definition follow each other, and a
//! parameter written in the window reaches the rig previewing it.

use crate::util;

use bevy::prelude::*;
use jackdaw::animation::graph_doc::AnimationGraphDoc;
use jackdaw_animation_runtime::{
    AnimationConditionOp, AnimationGraphDef, AnimationGraphRef, AnimationMotion, AnimationParams,
    parse_animation_graph,
};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_node_graph::GraphNode;
use jackdaw_scene_types::PropertyValue;

/// An editor holding a project of its own, so the graphs these tests write go
/// to a directory nothing else reads.
fn editor_in_a_fresh_project() -> (App, tempfile::TempDir) {
    let project = tempfile::tempdir().expect("a project directory");
    std::fs::create_dir_all(project.path().join("assets")).expect("an assets directory");
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: project.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    (app, project)
}

#[track_caller]
fn call(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
    app.update();
}

/// Author the same small locomotion graph every test here starts from.
fn author_a_locomotion_graph(app: &mut App) {
    call(
        app,
        "animation.graph.new",
        &[("name", PropertyValue::String("locomotion".into()))],
    );
    call(
        app,
        "animation.graph.add_param",
        &[
            ("name", PropertyValue::String("speed".into())),
            ("kind", PropertyValue::String("float".into())),
        ],
    );
    call(
        app,
        "animation.graph.add_state",
        &[
            ("name", PropertyValue::String("idle".into())),
            ("clip", PropertyValue::String("rig.glb#Idle".into())),
        ],
    );
    call(
        app,
        "animation.graph.add_state",
        &[
            ("name", PropertyValue::String("run".into())),
            ("clip", PropertyValue::String("rig.glb#Run".into())),
            ("speed", PropertyValue::Float(1.5)),
        ],
    );
    call(
        app,
        "animation.graph.add_transition",
        &[
            ("from", PropertyValue::String("idle".into())),
            ("to", PropertyValue::String("run".into())),
            ("when", PropertyValue::String("speed > 0.1".into())),
            ("fade", PropertyValue::Float(0.2)),
        ],
    );
    call(
        app,
        "animation.graph.set_entry",
        &[("name", PropertyValue::String("idle".into()))],
    );
}

/// The graph on disk, read the way the runtime loader reads it.
fn saved_graph(app: &App, project: &tempfile::TempDir) -> AnimationGraphDef {
    let file = project
        .path()
        .join("assets/animation/locomotion.animgraph.bsn");
    let text = std::fs::read_to_string(&file).expect("the graph file was written");
    let registry = app.world().resource::<AppTypeRegistry>().read();
    parse_animation_graph(&text, &registry).expect("the graph file reads back")
}

#[test]
fn a_graph_authored_through_the_operators_reads_back_off_the_disk() {
    let (mut app, project) = editor_in_a_fresh_project();
    author_a_locomotion_graph(&mut app);
    call(&mut app, "animation.graph.save", &[]);

    let def = saved_graph(&app, &project);
    assert_eq!(def.entry, "idle");
    assert_eq!(
        def.states
            .iter()
            .map(|state| &state.name)
            .collect::<Vec<_>>(),
        vec!["idle", "run"]
    );
    assert_eq!(def.states[1].speed, 1.5);
    assert!(matches!(
        &def.states[0].motion,
        AnimationMotion::Clip(clip) if clip.source == "rig.glb" && clip.clip == "Idle"
    ));
    assert_eq!(def.parameters.len(), 1);
    assert_eq!(def.parameters[0].name, "speed");

    let transition = def.transitions.first().expect("the transition was written");
    assert_eq!(transition.from, "idle");
    assert_eq!(transition.to, "run");
    assert_eq!(transition.crossfade_secs, 0.2);
    assert_eq!(transition.conditions[0].parameter, "speed");
    assert_eq!(transition.conditions[0].op, AnimationConditionOp::Greater);
    assert_eq!(transition.conditions[0].value, 0.1);
}

#[test]
fn a_saved_graph_opens_again_with_a_node_for_every_state() {
    let (mut app, _project) = editor_in_a_fresh_project();
    author_a_locomotion_graph(&mut app);
    call(&mut app, "animation.graph.save", &[]);
    call(
        &mut app,
        "animation.graph.open",
        &[(
            "path",
            PropertyValue::String("animation/locomotion.animgraph.bsn".into()),
        )],
    );

    let states = app.world().resource::<AnimationGraphDoc>().def.states.len();
    assert_eq!(states, 2);
    let nodes = app
        .world_mut()
        .query::<&GraphNode>()
        .iter(app.world())
        .filter(|node| node.node_type == "anim.state")
        .count();
    assert_eq!(nodes, 2, "each state is drawn as a node");
}

#[test]
fn a_state_renamed_in_the_window_carries_its_transitions_with_it() {
    let (mut app, project) = editor_in_a_fresh_project();
    author_a_locomotion_graph(&mut app);
    call(
        &mut app,
        "animation.graph.set_state",
        &[
            ("name", PropertyValue::String("run".into())),
            ("rename", PropertyValue::String("sprint".into())),
        ],
    );
    call(&mut app, "animation.graph.save", &[]);

    let def = saved_graph(&app, &project);
    assert!(def.states.iter().any(|state| state.name == "sprint"));
    assert_eq!(def.transitions[0].to, "sprint");
}

#[test]
fn a_node_taken_off_the_canvas_takes_its_state_and_wires_with_it() {
    let (mut app, _project) = editor_in_a_fresh_project();
    author_a_locomotion_graph(&mut app);

    let node = app
        .world_mut()
        .query::<(Entity, &GraphNode)>()
        .iter(app.world())
        .find(|(_, node)| node.node_type == "anim.state")
        .map(|(entity, _)| entity)
        .expect("a state is drawn on the canvas");
    app.world_mut().entity_mut(node).despawn();
    app.update();

    let doc = app.world().resource::<AnimationGraphDoc>();
    assert_eq!(doc.def.states.len(), 1, "the state left with its node");
    assert!(
        doc.def.transitions.is_empty(),
        "a transition to a state that is gone leaves with it"
    );
}

#[test]
fn undoing_an_added_state_takes_its_node_off_the_canvas() {
    let (mut app, _project) = editor_in_a_fresh_project();
    author_a_locomotion_graph(&mut app);
    call(&mut app, "history.undo", &[]);
    call(&mut app, "history.undo", &[]);

    let doc = app.world().resource::<AnimationGraphDoc>();
    assert!(
        doc.def.transitions.is_empty(),
        "the first undo took the transition back"
    );
    assert_eq!(
        doc.def
            .states
            .iter()
            .map(|state| &state.name)
            .collect::<Vec<_>>(),
        vec!["idle"],
        "the second undo took the state back"
    );
    let nodes = app
        .world_mut()
        .query::<&GraphNode>()
        .iter(app.world())
        .filter(|node| node.node_type == "anim.state")
        .count();
    assert_eq!(nodes, 1, "the canvas follows the definition undo restored");
}

#[test]
fn a_parameter_written_in_the_window_reaches_the_rig_being_previewed() {
    let (mut app, _project) = editor_in_a_fresh_project();
    author_a_locomotion_graph(&mut app);

    let rig = app
        .world_mut()
        .spawn((
            Name::new("Rig"),
            AnimationGraphRef {
                path: "animation/locomotion.animgraph.bsn".to_string(),
                ..default()
            },
        ))
        .id();
    app.update();

    call(
        &mut app,
        "animation.graph.set_param",
        &[
            ("name", PropertyValue::String("speed".into())),
            ("value", PropertyValue::Float(4.0)),
        ],
    );

    let written = app
        .world()
        .get::<AnimationParams>(rig)
        .expect("the preview target was written");
    assert_eq!(written.float("speed"), 4.0);
}

#[test]
fn a_graph_held_in_the_binary_form_is_titled_without_its_extension() {
    let (mut app, project) = editor_in_a_fresh_project();
    author_a_locomotion_graph(&mut app);
    call(&mut app, "animation.graph.save", &[]);
    jackdaw_bsn::convert_to_binary(
        &project
            .path()
            .join("assets/animation/locomotion.animgraph.bsn"),
    )
    .expect("the graph is written as binary");
    call(
        &mut app,
        "animation.graph.open",
        &[(
            "path",
            PropertyValue::String("animation/locomotion.animgraph.bsb".into()),
        )],
    );

    let graph = app
        .world()
        .resource::<AnimationGraphDoc>()
        .graph
        .expect("the canvas holds the open graph");
    let title = app
        .world()
        .get::<jackdaw_node_graph::NodeGraph>(graph)
        .map(|graph| graph.title.clone())
        .expect("the graph root is titled");
    assert_eq!(title, "locomotion");
}

#[test]
fn a_graph_opened_from_its_binary_file_is_saved_back_as_binary() {
    let (mut app, project) = editor_in_a_fresh_project();
    author_a_locomotion_graph(&mut app);
    call(&mut app, "animation.graph.save", &[]);
    let text_file = project
        .path()
        .join("assets/animation/locomotion.animgraph.bsn");
    jackdaw_bsn::convert_to_binary(&text_file).expect("the graph is written as binary");
    let binary_file = project
        .path()
        .join("assets/animation/locomotion.animgraph.bsb");
    call(
        &mut app,
        "animation.graph.open",
        &[(
            "path",
            PropertyValue::String("animation/locomotion.animgraph.bsb".into()),
        )],
    );

    call(&mut app, "animation.graph.save", &[]);

    let bytes = std::fs::read(&binary_file).expect("the graph file is there");
    assert!(
        jackdaw_bsn::is_binary(&bytes),
        "the file holds the form its name says"
    );
    let registry = app.world().resource::<AppTypeRegistry>().read();
    let text = jackdaw_bsn::read_document_text(&binary_file).expect("the graph reads back");
    parse_animation_graph(&text, &registry).expect("the graph reads back as a definition");
}
