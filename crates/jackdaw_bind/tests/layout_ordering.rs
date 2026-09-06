//! Bindings must be evaluated before Bevy lays the UI out.
//!
//! `JackdawBindPlugin` declares `evaluate_bindings.before(UiSystems::Layout)`
//! in `PostUpdate`. Without that edge a binding that drives a `Node` field is
//! laid out a frame late: a dragged slider and the bar it feeds disagree by one
//! frame, on every bound layout value the viewport shows. The ordering is
//! pinned here rather than left to whichever order the two systems happen to be
//! added in.
//!
//! Pinned behaviourally rather than by walking the schedule graph: what matters
//! is that a write in frame N is measured in frame N, and a graph assertion
//! would still pass if the edge were declared against a set that did not
//! contain the layout pass.

use bevy::prelude::*;
use jackdaw_bind::{BindContext, BindPath, Binding, Bindings, JackdawBindPlugin};

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Panel {
    width: f32,
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::input::InputPlugin,
        bevy::asset::AssetPlugin::default(),
        bevy::text::TextPlugin,
        bevy::picking::PickingPlugin,
        bevy::picking::InteractionPlugin,
        bevy::ui::UiPlugin,
    ));
    app.init_asset::<Image>();
    app.init_asset::<TextureAtlasLayout>();
    app.add_plugins(JackdawBindPlugin);
    app.register_type::<Panel>();
    app
}

#[test]
fn a_binding_written_in_a_frame_is_laid_out_in_the_same_frame() {
    let mut app = app();

    let source = app.world_mut().spawn(Panel { width: 120.0 }).id();
    // An absolute width, so the assertion reads the binding rather than the
    // headless app's (empty) render target.
    let node = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(source),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Panel.width")],
                via: None,
                write: BindPath::new("Node.width"),
                as_percent: false,
            }]),
        ))
        .id();

    app.update();
    assert_eq!(
        app.world().get::<Node>(node).expect("bound node").width,
        Val::Px(120.0),
        "the binding wrote the authored width",
    );
    assert_eq!(
        app.world()
            .get::<ComputedNode>(node)
            .expect("bound node is laid out")
            .size()
            .x,
        120.0,
        "layout must see the binding's write in the frame it was written",
    );

    // A later change must not lag either.
    app.world_mut().get_mut::<Panel>(source).unwrap().width = 64.0;
    app.update();
    assert_eq!(
        app.world()
            .get::<ComputedNode>(node)
            .expect("bound node is laid out")
            .size()
            .x,
        64.0,
        "a binding change is laid out in the frame it changes",
    );
}
