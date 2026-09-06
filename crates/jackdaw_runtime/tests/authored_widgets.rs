//! A game runtime renders the widgets a scene was authored with.
//!
//! Two things have to hold outside the editor: the widget markers survive the
//! load (they are built through `ReflectDefault`, which `bevy_ui_widgets` and
//! `bevy_feathers` do not register), and the value observers are attached, so
//! a checkbox the player clicks answers by checking itself.

use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{Checkbox, CheckboxPlugin, ToggleChecked};
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};

/// A checkbox beside a themed caption, the shape the editor's UI palette
/// writes: a widget marker plus the feathers styling components the
/// always-save list carries through a round trip.
const SCENE: &str = r#"
bevy_ecs::hierarchy::Children [
    #Checkbox
    bevy_ui::ui_node::Node
    bevy_ui_widgets::checkbox::Checkbox
    ,
    #Caption
    bevy_ui::ui_node::Node
    bevy_feathers::theme::ThemedText
    ,
    #Plain
    bevy_ui::ui_node::Node
]
"#;

#[test]
fn a_checkbox_loaded_by_the_game_runtime_keeps_its_widget_and_toggles() {
    let mut app = runtime_app();
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(SCENE.into(), ".".into()));
    app.world_mut().spawn(JackdawSceneRoot(scene));

    app.update();
    app.update();

    let checkbox = named_entity(app.world_mut(), "Checkbox");
    assert!(
        app.world().get::<Checkbox>(checkbox).is_some(),
        "the widget marker survives the load"
    );

    // `bevy_feathers` needs `bevy_render`, so the theme markers only register
    // in a rendering build; a headless game loads the widget without them.
    // `Plain` is the same node without the theme marker, so comparing
    // component counts stands in for naming a type that may not be compiled
    // in at all.
    let caption = named_entity(app.world_mut(), "Caption");
    let plain = named_entity(app.world_mut(), "Plain");
    let carried = |app: &App, entity| {
        app.world()
            .inspect_entity(entity)
            .expect("the entity is live")
            .count()
    };

    #[cfg(feature = "render")]
    {
        assert!(
            app.world()
                .get::<bevy::feathers::theme::ThemedText>(caption)
                .is_some(),
            "the authored theme marker survives the load"
        );
        assert!(
            carried(&app, caption) > carried(&app, plain),
            "the marker is what the caption carries over the plain node",
        );
    }
    #[cfg(not(feature = "render"))]
    assert_eq!(
        carried(&app, caption),
        carried(&app, plain),
        "a headless game drops the theme marker rather than loading it",
    );

    assert!(
        app.world().get::<Checked>(checkbox).is_none(),
        "a fresh checkbox loads unchecked"
    );

    app.world_mut().trigger(ToggleChecked { entity: checkbox });
    app.update();
    assert!(
        app.world().get::<Checked>(checkbox).is_some(),
        "clicking the checkbox checks it: the self-update observer is attached"
    );

    app.world_mut().trigger(ToggleChecked { entity: checkbox });
    app.update();
    assert!(
        app.world().get::<Checked>(checkbox).is_none(),
        "and clicking again clears it"
    );
}

fn runtime_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(CheckboxPlugin);
    app.add_plugins(JackdawPlugin);
    app
}

fn named_entity(world: &mut World, target: &str) -> Entity {
    let mut names = world.query::<(Entity, &Name)>();
    names
        .iter(world)
        .find_map(|(entity, name)| (name.as_str() == target).then_some(entity))
        .unwrap_or_else(|| panic!("expected entity named {target}"))
}
