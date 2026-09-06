//! A progress bar authored in a document fills from game state.
//!
//! `Progress` is a reflected field, so a binding writes it like any other
//! component field; the fill child is what turns the number into something a
//! player can see, and that half lives in `jackdaw_widgets_runtime`. This
//! covers the composition, over the document the editor writes.

use bevy::prelude::*;
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};
use jackdaw_widgets_runtime::Progress;

#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct Vitals {
    health: f32,
}

const SCENE: &str = r#"
bevy_ecs::hierarchy::Children [
    #Bar
    bevy_ui::ui_node::Node { width: bevy_ui::geometry::Val::Px(180.0), height: bevy_ui::geometry::Val::Px(8.0) }
    jackdaw_widgets_runtime::Progress { value: 0.0 }
    jackdaw_bind::types::Bindings([
        jackdaw_bind::types::Binding::Field {
            read: [jackdaw_bind::types::BindPath { raw: "Res(progress_bindings::Vitals).health" }],
            via: None,
            write: jackdaw_bind::types::BindPath { raw: "jackdaw_widgets_runtime::Progress.value" },
            as_percent: false,
        },
    ])
    bevy_ecs::hierarchy::Children [
        #Fill
        bevy_ui::ui_node::Node { width: bevy_ui::geometry::Val::Percent(0.0), height: bevy_ui::geometry::Val::Percent(100.0) }
        jackdaw_widgets_runtime::ProgressFill
    ]
]
"#;

#[test]
fn a_loaded_progress_bar_fills_from_the_state_it_is_bound_to() {
    let mut app = runtime_app();
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(SCENE.into(), ".".into()));
    app.world_mut().spawn(JackdawSceneRoot(scene));
    app.update();
    app.update();

    let bar = named_entity(app.world_mut(), "Bar");
    let fill = named_entity(app.world_mut(), "Fill");
    assert!(
        app.world().get::<Progress>(bar).is_some(),
        "the authored progress value loaded"
    );

    app.world_mut().resource_mut::<Vitals>().health = 0.4;
    app.update();
    assert_eq!(
        app.world().get::<Progress>(bar).map(|p| p.value),
        Some(0.4),
        "the binding puts the source's number in the widget"
    );
    assert_eq!(
        app.world().get::<Node>(fill).map(|node| node.width),
        Some(Val::Percent(40.0)),
        "and the fill is redrawn from it in the same frame the binding wrote it"
    );

    app.world_mut().resource_mut::<Vitals>().health = 1.0;
    app.update();
    assert_eq!(
        app.world().get::<Node>(fill).map(|node| node.width),
        Some(Val::Percent(100.0)),
    );
}

/// No UI plugins: the fill is written by a system `JackdawPlugin` orders
/// against the evaluator by name, so the order holds in an app with no layout
/// in it at all.
fn runtime_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(JackdawPlugin);
    app.register_type::<Vitals>();
    app.init_resource::<Vitals>();
    app
}

fn named_entity(world: &mut World, target: &str) -> Entity {
    let mut names = world.query::<(Entity, &Name)>();
    names
        .iter(world)
        .find_map(|(entity, name)| (name.as_str() == target).then_some(entity))
        .unwrap_or_else(|| panic!("expected entity named {target}"))
}
