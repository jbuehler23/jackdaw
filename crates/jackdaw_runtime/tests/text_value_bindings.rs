//! A text input authored in a document keeps a game resource in step both ways.
//!
//! The two halves live in different crates (`jackdaw_bind` evaluates the
//! binding, `jackdaw_widgets_runtime` owns the text) and neither depends on
//! the other. This covers the composition that joins them, over the document
//! the editor writes.

use bevy::prelude::*;
use bevy::text::EditableText;
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};
use jackdaw_widgets_runtime::TextValue;

#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct Form {
    name: String,
}

const SCENE: &str = r#"
bevy_ecs::hierarchy::Children [
    #Name
    jackdaw_widgets_runtime::TextValue("")
    jackdaw_bind::types::Bindings([
        jackdaw_bind::types::Binding::Value { with: jackdaw_bind::types::BindPath { raw: "Res(text_value_bindings::Form).name" }, two_way: true },
    ])
]
"#;

#[test]
fn a_loaded_text_input_follows_its_source_and_is_followed_by_it() {
    let mut app = runtime_app();
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(SCENE.into(), ".".into()));
    app.world_mut().spawn(JackdawSceneRoot(scene));
    app.update();
    app.update();

    let input = named_entity(app.world_mut(), "Name");
    assert!(
        app.world().get::<TextValue>(input).is_some(),
        "the authored text value loaded"
    );

    app.world_mut().resource_mut::<Form>().name = "Ada".to_string();
    app.update();
    app.update();
    assert_eq!(
        app.world().get::<TextValue>(input).map(|v| v.0.clone()),
        Some("Ada".to_string()),
        "the binding puts the source's text in the widget"
    );
    assert_eq!(editor_text(app.world(), input), "Ada");

    set_editor_text(&mut app, input, "Bee");
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(
        app.world().resource::<Form>().name,
        "Bee",
        "a typed edit reaches the source the widget is bound to"
    );
    assert_eq!(
        app.world().get::<TextValue>(input).map(|v| v.0.clone()),
        Some("Bee".to_string()),
        "and the widget is left holding what the user typed"
    );
    assert_eq!(editor_text(app.world(), input), "Bee");
}

/// A keystroke survives whichever way round the two are composed:
/// `evaluate_bindings` takes `&mut World`, so bevy applies every pending
/// command before it runs, including the `ValueChange` the edit raised, the
/// observer that answers it, and the write-back that observer queues. The
/// evaluator never reads the source as it stood before the edit, whichever
/// side of it the text systems sit on.
///
/// `JackdawPlugin` declares an order between the two; the pair does not depend
/// on that declaration, and an evaluator turned into an ordinary system fails
/// here.
#[test]
fn a_keystroke_survives_whichever_order_the_two_are_composed_in() {
    for order in [Order::TextFirst, Order::EvaluatorFirst] {
        let mut app = hand_composed_app(order);
        let input = bound_input(&mut app);
        for _ in 0..3 {
            app.update();
        }

        set_editor_text(&mut app, input, "Bee");
        for _ in 0..3 {
            app.update();
        }

        assert_eq!(
            app.world().get::<TextValue>(input).map(|v| v.0.clone()),
            Some("Bee".to_string()),
            "{order:?}",
        );
        assert_eq!(app.world().resource::<Form>().name, "Bee", "{order:?}");
        assert_eq!(editor_text(app.world(), input), "Bee", "{order:?}");
    }
}

#[derive(Debug, Clone, Copy)]
enum Order {
    TextFirst,
    EvaluatorFirst,
}

/// The two crates joined by hand rather than through [`JackdawPlugin`], so the
/// order between them can be set either way.
fn hand_composed_app(order: Order) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(jackdaw_bind::JackdawBindPlugin);
    app.add_plugins(jackdaw_widgets_runtime::AuthoredWidgetPlugin);
    app.insert_resource(jackdaw_bind::ValueTextTarget(jackdaw_bind::BindPath::new(
        jackdaw_widgets_runtime::text_value_write_path(),
    )));
    let set = jackdaw_widgets_runtime::AuthoredTextSystems;
    match order {
        Order::TextFirst => {
            app.configure_sets(PostUpdate, set.before(jackdaw_bind::BindEvaluationSystems));
        }
        Order::EvaluatorFirst => {
            app.configure_sets(PostUpdate, set.after(jackdaw_bind::BindEvaluationSystems));
        }
    }
    app.register_type::<Form>();
    app.init_resource::<Form>();
    app
}

fn bound_input(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            TextValue::default(),
            jackdaw_bind::Bindings(vec![jackdaw_bind::Binding::Value {
                with: jackdaw_bind::BindPath::new("Res(text_value_bindings::Form).name"),
                two_way: true,
            }]),
        ))
        .id()
}

/// No UI plugins: `JackdawPlugin` puts the text systems after the evaluator by
/// naming both, so the order this test leans on holds in an app with no layout
/// in it at all.
fn runtime_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(JackdawPlugin);
    app.register_type::<Form>();
    app.init_resource::<Form>();
    app
}

fn editor_text(world: &World, entity: Entity) -> String {
    world
        .get::<EditableText>(entity)
        .expect("the input keeps its editor")
        .value()
        .into_iter()
        .collect()
}

fn set_editor_text(app: &mut App, entity: Entity, text: &str) {
    app.world_mut()
        .get_mut::<EditableText>(entity)
        .expect("the input keeps its editor")
        .editor_mut()
        .set_text(text);
}

fn named_entity(world: &mut World, target: &str) -> Entity {
    let mut names = world.query::<(Entity, &Name)>();
    names
        .iter(world)
        .find_map(|(entity, name)| (name.as_str() == target).then_some(entity))
        .unwrap_or_else(|| panic!("expected entity named {target}"))
}
