//! The resolution a binding keeps beside it.
//!
//! Splitting a binding's paths and looking up its types happens once and lands
//! in a `ResolvedBindings` sibling component, which the evaluator reads instead
//! of parsing the paths again on every frame.
//!
//! Keeping a lookup means keeping it true. What is pinned here:
//!
//! 1. A widget moved under a different `BindContext` reads the new subject,
//!    not the one it was resolved against.
//! 2. A frame in which no source moved reads nothing at all.
//! 3. A widget moved out of its context, or one whose context is taken away,
//!    stops reading the subject it was resolved against.
//! 4. A despawned widget takes its resolution with it.
//! 5. The resolution cannot reach a document: it is not a reflected type, so
//!    nothing that writes a scene can see it.
//! 6. A widget whose target component arrives late still binds, rather than
//!    failing once and staying failed.
//! 7. A `Value` binding keeps its widget in step every frame, gate or no gate.
//! 8. What one binding writes reaches a binding that reads it.

use bevy::prelude::*;
use jackdaw_bind::{
    BindContext, BindPath, BindReads, Binding, Bindings, JackdawBindPlugin, ResolvedBindings,
};

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Health {
    current: f32,
    alive: bool,
}

/// A widget component one binding writes and another reads, which is how a
/// chain of two bindings is built.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Relay {
    value: f32,
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
    app.register_type::<Health>();
    app.register_type::<Relay>();
    app
}

/// Whether the evaluator runs this frame, standing in for the editor's
/// preview toggle.
#[derive(Resource, Default)]
struct Evaluating(bool);

/// The editor's registration: the evaluator alone, behind a run condition that
/// can park it. `JackdawBindPlugin` runs it every frame, so a parked frame
/// cannot be shown against an app built with the plugin.
fn parked_app() -> App {
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
    app.init_resource::<Evaluating>();
    app.register_type::<Health>();
    app.add_systems(
        PostUpdate,
        jackdaw_bind::evaluate_bindings
            .before(bevy::ui::UiSystems::Layout)
            .run_if(|gate: Res<Evaluating>| gate.0),
    );
    app
}

fn set_evaluating(app: &mut App, on: bool) {
    app.world_mut().resource_mut::<Evaluating>().0 = on;
}

/// A widget that leaves its context while the evaluator is parked is the one
/// case nothing else catches. Reparenting is reported through a removal queue
/// bevy clears every frame, so the removal is gone before the evaluator runs
/// again, and only the reader's missed-message cursor still knows it happened.
#[test]
fn a_widget_that_left_its_context_while_parked_is_looked_up_again_on_resume() {
    let mut app = parked_app();
    let hero = subject(&mut app, 10.0);
    let panel = app.world_mut().spawn(BindContext(hero)).id();
    let widget = widget_under(&mut app, panel);

    set_evaluating(&mut app, true);
    app.update();
    assert_eq!(width(&app, widget), Val::Px(10.0));

    set_evaluating(&mut app, false);
    app.world_mut().entity_mut(widget).remove::<ChildOf>();
    // Long enough for bevy to drop the removal nothing read.
    for _ in 0..4 {
        app.update();
    }
    app.world_mut()
        .get_mut::<Health>(hero)
        .expect("health")
        .current = 90.0;

    set_evaluating(&mut app, true);
    app.update();

    assert_eq!(
        width(&app, widget),
        Val::Px(10.0),
        "resuming has to look the widget up again: it is under no context now, \
         so the binding has nothing to read and must leave the value alone",
    );
}

fn subject(app: &mut App, current: f32) -> Entity {
    app.world_mut()
        .spawn(Health {
            current,
            alive: true,
        })
        .id()
}

/// A widget whose width follows `Health.current` on whatever context it
/// inherits.
fn widget_under(app: &mut App, parent: Entity) -> Entity {
    app.world_mut()
        .spawn((
            Node::default(),
            ChildOf(parent),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Health.current")],
                via: None,
                write: BindPath::new("Node.width"),
                as_percent: false,
            }]),
        ))
        .id()
}

fn width(app: &App, entity: Entity) -> Val {
    app.world().get::<Node>(entity).expect("a node").width
}

fn reads(app: &App) -> u64 {
    app.world().resource::<BindReads>().0
}

#[test]
fn a_widget_moved_under_another_context_reads_the_new_subject() {
    let mut app = app();
    let first = subject(&mut app, 10.0);
    let second = subject(&mut app, 90.0);
    let panel = app.world_mut().spawn(BindContext(first)).id();
    let other = app.world_mut().spawn(BindContext(second)).id();
    let widget = widget_under(&mut app, panel);

    app.update();
    assert_eq!(width(&app, widget), Val::Px(10.0));

    app.world_mut().entity_mut(widget).insert(ChildOf(other));
    app.update();
    assert_eq!(
        width(&app, widget),
        Val::Px(90.0),
        "the binding still reads the context it was first resolved against",
    );
}

#[test]
fn a_frame_where_nothing_moved_reads_nothing() {
    let mut app = app();
    let hero = subject(&mut app, 10.0);
    let panel = app.world_mut().spawn(BindContext(hero)).id();
    let widget = widget_under(&mut app, panel);

    app.update();
    let after_first = reads(&app);
    assert!(
        after_first >= 1,
        "the first frame has to read: nothing has been evaluated yet",
    );

    for _ in 0..5 {
        app.update();
    }
    assert_eq!(
        reads(&app),
        after_first,
        "five idle frames took the read path anyway",
    );

    app.world_mut()
        .get_mut::<Health>(hero)
        .expect("health")
        .current = 70.0;
    app.update();
    assert_eq!(
        reads(&app),
        after_first + 1,
        "a moved source has to be read again",
    );
    assert_eq!(width(&app, widget), Val::Px(70.0));
}

#[test]
fn a_despawned_widget_leaves_no_resolution_behind() {
    let mut app = app();
    let hero = subject(&mut app, 10.0);
    let panel = app.world_mut().spawn(BindContext(hero)).id();
    let widget = widget_under(&mut app, panel);

    app.update();
    assert_eq!(resolutions(&mut app), 1);

    app.world_mut().entity_mut(widget).despawn();
    app.update();
    assert_eq!(
        resolutions(&mut app),
        0,
        "the resolution outlived the widget it belonged to",
    );
}

#[test]
fn dropping_the_bindings_drops_the_resolution() {
    let mut app = app();
    let hero = subject(&mut app, 10.0);
    let panel = app.world_mut().spawn(BindContext(hero)).id();
    let widget = widget_under(&mut app, panel);

    app.update();
    assert_eq!(resolutions(&mut app), 1);

    app.world_mut().entity_mut(widget).remove::<Bindings>();
    app.update();
    assert_eq!(
        resolutions(&mut app),
        0,
        "a widget with no bindings left kept its resolution",
    );
}

fn resolutions(app: &mut App) -> usize {
    app.world_mut()
        .query::<&ResolvedBindings>()
        .iter(app.world())
        .count()
}

/// The resolution holds entity ids, component ids and reflect handles, none of
/// which mean anything in a saved scene. It stays out of documents by not being
/// a reflected type at all: this build turns on `reflect_auto_register`, so a
/// `Reflect` derive on it would put it in the registry, and every writer the
/// editor has works from the registry.
#[test]
fn the_resolution_cannot_reach_a_document() {
    let mut app = app();
    let hero = subject(&mut app, 10.0);
    let panel = app.world_mut().spawn(BindContext(hero)).id();
    let widget = widget_under(&mut app, panel);
    app.update();

    assert!(
        app.world().get::<ResolvedBindings>(widget).is_some(),
        "the widget should carry a resolution to begin with",
    );
    let registry = app.world().resource::<AppTypeRegistry>().read();
    let named: Vec<&str> = registry
        .iter()
        .map(|reg| reg.type_info().type_path())
        .filter(|path| path.ends_with("ResolvedBindings"))
        .collect();
    assert!(
        named.is_empty(),
        "a document writer can see the resolution: {named:?}",
    );
}

// ---------------------------------------------------------------------------
// A context that goes away
// ---------------------------------------------------------------------------

#[test]
fn a_widget_moved_out_of_its_context_stops_reading_the_old_subject() {
    let mut app = app();
    let hero = subject(&mut app, 10.0);
    let panel = app.world_mut().spawn(BindContext(hero)).id();
    let widget = widget_under(&mut app, panel);

    app.update();
    assert_eq!(width(&app, widget), Val::Px(10.0));

    // What the outliner does when a widget is dragged to the scene root.
    app.world_mut().entity_mut(widget).remove::<ChildOf>();
    app.world_mut()
        .get_mut::<Health>(hero)
        .expect("health")
        .current = 55.0;
    app.update();
    assert_eq!(
        width(&app, widget),
        Val::Px(10.0),
        "the widget has no context any more and must not keep reading the old subject",
    );
}

#[test]
fn a_context_taken_away_above_a_widget_is_noticed() {
    let mut app = app();
    let hero = subject(&mut app, 10.0);
    let panel = app.world_mut().spawn(BindContext(hero)).id();
    let widget = widget_under(&mut app, panel);

    app.update();
    assert_eq!(width(&app, widget), Val::Px(10.0));

    app.world_mut().entity_mut(panel).remove::<BindContext>();
    app.world_mut()
        .get_mut::<Health>(hero)
        .expect("health")
        .current = 55.0;
    app.update();
    assert_eq!(
        width(&app, widget),
        Val::Px(10.0),
        "nothing names a subject above the widget any more",
    );
}

// ---------------------------------------------------------------------------
// Failures that fix themselves
// ---------------------------------------------------------------------------

/// A widget hydrated over two frames, its bindings landing before the component
/// they write. The first frame has nothing to write to and says so; the second
/// has to bind rather than stay dead, because nothing about the source moves.
#[test]
fn a_widget_whose_target_arrives_late_still_binds() {
    let mut app = app();
    let hero = subject(&mut app, 3.0);
    let panel = app.world_mut().spawn(BindContext(hero)).id();
    let widget = app
        .world_mut()
        .spawn((
            Node::default(),
            ChildOf(panel),
            Bindings(vec![Binding::Text {
                format: "{}".into(),
                args: vec![BindPath::new("Health.current")],
            }]),
        ))
        .id();

    app.update();
    assert!(app.world().get::<Text>(widget).is_none());

    app.world_mut().entity_mut(widget).insert(Text::default());
    app.update();
    assert_eq!(
        app.world().get::<Text>(widget).expect("text").0,
        "3",
        "the binding failed once and never tried again",
    );
}

// ---------------------------------------------------------------------------
// Value bindings
// ---------------------------------------------------------------------------

/// A one-way `Value` binding says the widget follows the value. A click that
/// moves the widget and not the value has to be put back, which means the arm
/// runs whether or not the source moved.
#[test]
fn a_one_way_value_binding_puts_the_widget_back() {
    let mut app = app();
    let hero = subject(&mut app, 10.0);
    let panel = app.world_mut().spawn(BindContext(hero)).id();
    let checkbox = app
        .world_mut()
        .spawn((
            Node::default(),
            ChildOf(panel),
            Bindings(vec![Binding::Value {
                with: BindPath::new("Health.alive"),
                two_way: false,
            }]),
        ))
        .id();

    app.update();
    assert!(app.world().get::<bevy::ui::Checked>(checkbox).is_some());

    app.world_mut()
        .entity_mut(checkbox)
        .remove::<bevy::ui::Checked>();
    app.update();
    assert!(
        app.world().get::<bevy::ui::Checked>(checkbox).is_some(),
        "the widget kept the state the user left it in",
    );
}

// ---------------------------------------------------------------------------
// One binding reading another's target
// ---------------------------------------------------------------------------

/// The evaluator decides what is due before it writes anything, so a chain
/// advances one link per frame. What it must never do is stall: a write has to
/// land on a later tick than the one the gate just read, or the binding
/// downstream of it is skipped for good.
#[test]
fn a_binding_reading_what_another_wrote_follows_it() {
    let mut app = app();
    let hero = subject(&mut app, 10.0);
    let panel = app.world_mut().spawn(BindContext(hero)).id();
    let relay = app
        .world_mut()
        .spawn((
            Node::default(),
            Relay::default(),
            ChildOf(panel),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Health.current")],
                via: None,
                write: BindPath::new("Relay.value"),
                as_percent: false,
            }]),
        ))
        .id();
    let follower = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(relay),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Relay.value")],
                via: None,
                write: BindPath::new("Node.width"),
                as_percent: false,
            }]),
        ))
        .id();

    for _ in 0..3 {
        app.update();
    }
    assert_eq!(width(&app, follower), Val::Px(10.0));

    app.world_mut()
        .get_mut::<Health>(hero)
        .expect("health")
        .current = 42.0;
    app.update();
    app.update();
    assert_eq!(
        width(&app, follower),
        Val::Px(42.0),
        "the relay moved and nothing downstream noticed",
    );
}
