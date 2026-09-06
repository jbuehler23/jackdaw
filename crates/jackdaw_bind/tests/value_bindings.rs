use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{SliderValue, ValueChange};
use jackdaw_bind::{BindFailures, BindPath, Binding, Bindings, JackdawBindPlugin, ValueTextTarget};

#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct AudioSettings {
    master: f32,
}

#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct Flags {
    enabled: bool,
}

#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct Profile {
    name: String,
}

/// A count, which is an integer and not a float. A binding reads one widened
/// to f32; a two-way binding has to be able to put one back.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct Roster {
    seats: u32,
}

/// Not named `Volume`: short-path lookup returns nothing for a name two
/// registered types share, and a workspace build auto-registers bevy's own
/// `Volume`, so the binding below would fail as an ambiguous short type path.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct BoundVolume(f32);

/// Stands in for `jackdaw_widgets_runtime::TextValue`, which this crate cannot
/// depend on. The seam it reaches through is the same one that crate's real
/// target is named by.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct BoundText(String);

/// Counts how many frames observed a flagged `SliderValue` or `AudioSettings`,
/// so a binding that keeps re-writing an unchanged value shows up as a rising
/// count rather than a silent cost.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
struct Touches {
    slider: usize,
    audio: usize,
    text: usize,
    profile: usize,
}

fn count_touches(
    sliders: Query<(), Changed<SliderValue>>,
    texts: Query<(), Changed<BoundText>>,
    audio: Res<AudioSettings>,
    profile: Res<Profile>,
    mut touches: ResMut<Touches>,
) {
    touches.slider += sliders.iter().count();
    touches.text += texts.iter().count();
    if audio.is_changed() {
        touches.audio += 1;
    }
    if profile.is_changed() {
        touches.profile += 1;
    }
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
    app.register_type::<AudioSettings>();
    app.register_type::<Flags>();
    app.register_type::<Profile>();
    app.register_type::<Roster>();
    app.register_type::<BoundVolume>();
    app.register_type::<BoundText>();
    app.insert_resource(ValueTextTarget(BindPath::new("BoundText.0")));
    app.init_resource::<AudioSettings>();
    app.init_resource::<Flags>();
    app.init_resource::<Profile>();
    app.init_resource::<Roster>();
    app.init_resource::<Touches>();
    app.add_systems(Update, count_touches);
    app
}

fn slider(app: &mut App, two_way: bool) -> Entity {
    app.world_mut()
        .spawn((
            Node::default(),
            SliderValue(0.0),
            Bindings(vec![Binding::Value {
                with: BindPath::new("Res(AudioSettings).master"),
                two_way,
            }]),
        ))
        .id()
}

#[test]
fn widget_change_writes_resource() {
    let mut app = app();
    let slider = slider(&mut app, true);
    app.update();
    app.world_mut().trigger(ValueChange {
        source: slider,
        value: 0.8_f32,
        is_final: true,
    });
    app.update();
    assert_eq!(app.world().resource::<AudioSettings>().master, 0.8);
}

#[test]
fn resource_change_syncs_widget() {
    let mut app = app();
    let slider = slider(&mut app, true);
    app.world_mut().resource_mut::<AudioSettings>().master = 0.3;
    app.update();
    assert_eq!(app.world().get::<SliderValue>(slider).unwrap().0, 0.3);
}

#[test]
fn one_way_value_binding_ignores_widget_changes() {
    let mut app = app();
    let slider = slider(&mut app, false);
    app.update();
    app.world_mut().trigger(ValueChange {
        source: slider,
        value: 0.8_f32,
        is_final: true,
    });
    app.update();
    assert_eq!(app.world().resource::<AudioSettings>().master, 0.0);
}

#[test]
fn two_way_value_binding_settles_without_a_feedback_loop() {
    let mut app = app();
    let slider = slider(&mut app, true);
    for _ in 0..3 {
        app.update();
    }
    let quiet = *app.world().resource::<Touches>();

    app.world_mut().trigger(ValueChange {
        source: slider,
        value: 0.8_f32,
        is_final: true,
    });
    for _ in 0..3 {
        app.update();
    }
    let settled = *app.world().resource::<Touches>();
    assert_eq!(
        settled.audio,
        quiet.audio + 1,
        "the widget write should flag the resource exactly once"
    );
    assert_eq!(
        settled.slider,
        quiet.slider + 1,
        "the sync back should flag the slider exactly once"
    );

    for _ in 0..3 {
        app.update();
    }
    assert_eq!(
        *app.world().resource::<Touches>(),
        settled,
        "bindings kept re-flagging after settling"
    );
    assert_eq!(app.world().resource::<AudioSettings>().master, 0.8);
    assert_eq!(app.world().get::<SliderValue>(slider).unwrap().0, 0.8);
}

#[test]
fn bool_value_binding_syncs_both_ways() {
    let mut app = app();
    let checkbox = app
        .world_mut()
        .spawn((
            Node::default(),
            Bindings(vec![Binding::Value {
                with: BindPath::new("Res(Flags).enabled"),
                two_way: true,
            }]),
        ))
        .id();
    app.world_mut().resource_mut::<Flags>().enabled = true;
    app.update();
    assert!(app.world().get::<Checked>(checkbox).is_some());

    app.world_mut().trigger(ValueChange {
        source: checkbox,
        value: false,
        is_final: true,
    });
    app.update();
    assert!(!app.world().resource::<Flags>().enabled);
    assert!(app.world().get::<Checked>(checkbox).is_none());
}

#[test]
fn string_value_binding_writes_resource() {
    let mut app = app();
    let field = app
        .world_mut()
        .spawn((
            Node::default(),
            Bindings(vec![Binding::Value {
                with: BindPath::new("Res(Profile).name"),
                two_way: true,
            }]),
        ))
        .id();
    app.update();
    app.world_mut().trigger(ValueChange {
        source: field,
        value: "aldermoor".to_string(),
        is_final: true,
    });
    app.update();
    assert_eq!(app.world().resource::<Profile>().name, "aldermoor");
}

#[test]
fn numeric_value_binding_without_a_slider_is_reported_not_ignored() {
    let mut app = app();
    let orphan = app
        .world_mut()
        .spawn((
            Node::default(),
            Bindings(vec![Binding::Value {
                with: BindPath::new("Res(AudioSettings).master"),
                two_way: true,
            }]),
        ))
        .id();
    app.update();
    assert!(
        app.world()
            .resource::<BindFailures>()
            .0
            .contains(&(orphan, 0)),
        "a Value binding with no widget target should report, not sit silent"
    );
}

#[test]
fn string_value_binding_reports_that_no_widget_target_exists() {
    let mut app = app();
    let field = app
        .world_mut()
        .spawn((
            Node::default(),
            Bindings(vec![Binding::Value {
                with: BindPath::new("Res(Profile).name"),
                two_way: true,
            }]),
        ))
        .id();
    app.update();
    assert!(
        app.world()
            .resource::<BindFailures>()
            .0
            .contains(&(field, 0)),
        "a string Value binding has no widget target and should say so"
    );
}

fn text_input(app: &mut App, two_way: bool) -> Entity {
    app.world_mut()
        .spawn((
            Node::default(),
            BoundText::default(),
            Bindings(vec![Binding::Value {
                with: BindPath::new("Res(Profile).name"),
                two_way,
            }]),
        ))
        .id()
}

#[test]
fn a_string_value_binding_fills_the_widgets_text() {
    let mut app = app();
    let field = text_input(&mut app, true);
    app.world_mut().resource_mut::<Profile>().name = "Ada".to_string();
    app.update();
    assert_eq!(app.world().get::<BoundText>(field).unwrap().0, "Ada");
}

#[test]
fn a_typed_string_reaches_the_source_the_widget_is_bound_to() {
    let mut app = app();
    let field = text_input(&mut app, true);
    app.update();
    app.world_mut().trigger(ValueChange {
        source: field,
        value: "Ada".to_string(),
        is_final: false,
    });
    app.update();
    assert_eq!(app.world().resource::<Profile>().name, "Ada");
    assert_eq!(app.world().get::<BoundText>(field).unwrap().0, "Ada");
}

#[test]
fn a_two_way_string_binding_settles_after_an_edit() {
    let mut app = app();
    let field = text_input(&mut app, true);
    for _ in 0..3 {
        app.update();
    }
    let quiet = *app.world().resource::<Touches>();

    app.world_mut().trigger(ValueChange {
        source: field,
        value: "Ada".to_string(),
        is_final: true,
    });
    for _ in 0..3 {
        app.update();
    }
    let settled = *app.world().resource::<Touches>();
    assert_eq!(settled.profile, quiet.profile + 1);
    assert_eq!(settled.text, quiet.text + 1);

    for _ in 0..3 {
        app.update();
    }
    assert_eq!(
        *app.world().resource::<Touches>(),
        settled,
        "the text and its source kept re-flagging after settling",
    );
    assert_eq!(app.world().resource::<Profile>().name, "Ada");
    assert_eq!(app.world().get::<BoundText>(field).unwrap().0, "Ada");
}

/// An app that never said where a widget's text lives. The binding cannot
/// guess, so it reports rather than sitting silent, and picks the target up
/// once one arrives, without the widget being touched.
#[test]
fn a_string_binding_with_no_named_target_reports_and_then_recovers() {
    let mut app = app();
    app.world_mut().remove_resource::<ValueTextTarget>();
    let field = text_input(&mut app, true);
    app.world_mut().resource_mut::<Profile>().name = "Ada".to_string();
    app.update();

    assert!(
        app.world()
            .resource::<BindFailures>()
            .0
            .contains(&(field, 0)),
        "a string binding with nowhere to write should say so",
    );
    assert_eq!(app.world().get::<BoundText>(field).unwrap().0, "");

    app.world_mut()
        .insert_resource(ValueTextTarget(BindPath::new("BoundText.0")));
    // A failing binding is looked up again on a cadence rather than every
    // frame, so recovery takes as long as the cadence and not one frame.
    for _ in 0..40 {
        app.update();
    }

    assert_eq!(
        app.world().get::<BoundText>(field).unwrap().0,
        "Ada",
        "a target named late is picked up when the failing binding is resolved again",
    );
}

/// A number cannot be put in a text widget by guessing: the arm dispatches on
/// what it read, so without a check this would look for a slider on a text
/// input and report the wrong thing missing.
#[test]
fn a_number_read_into_a_text_widget_is_refused() {
    let mut app = app();
    let field = app
        .world_mut()
        .spawn((
            Node::default(),
            BoundText("untouched".to_string()),
            Bindings(vec![Binding::Value {
                with: BindPath::new("Res(AudioSettings).master"),
                two_way: true,
            }]),
        ))
        .id();
    app.update();
    assert!(
        app.world()
            .resource::<BindFailures>()
            .0
            .contains(&(field, 0)),
    );
    assert_eq!(app.world().get::<BoundText>(field).unwrap().0, "untouched");
}

#[test]
fn value_binding_writes_a_component_on_the_context_entity() {
    let mut app = app();
    let subject = app.world_mut().spawn(BoundVolume(0.0)).id();
    let slider = app
        .world_mut()
        .spawn((
            Node::default(),
            SliderValue(0.0),
            jackdaw_bind::BindContext(subject),
            Bindings(vec![Binding::Value {
                with: BindPath::new("BoundVolume.0"),
                two_way: true,
            }]),
        ))
        .id();
    app.update();
    app.world_mut().trigger(ValueChange {
        source: slider,
        value: 0.5_f32,
        is_final: true,
    });
    app.update();
    assert_eq!(app.world().get::<BoundVolume>(subject).unwrap().0, 0.5);
}

/// A binding reads every integer widened to f32. Writing one back is the same
/// step in reverse, and without it a two-way binding on a count is a widget
/// that follows its source and can never move it.
#[test]
fn a_two_way_binding_writes_an_integer_source_back() {
    let mut app = app();
    let slider = app
        .world_mut()
        .spawn((
            Node::default(),
            SliderValue(0.0),
            Bindings(vec![Binding::Value {
                with: BindPath::new("Res(Roster).seats"),
                two_way: true,
            }]),
        ))
        .id();
    app.update();
    app.world_mut().trigger(ValueChange {
        source: slider,
        value: 4.0_f32,
        is_final: true,
    });
    app.update();
    assert_eq!(app.world().resource::<Roster>().seats, 4);
}

/// The same write with a number no `u32` holds. `as` would have stored
/// `u32::MAX`, which is not the number anyone computed.
#[test]
fn an_integer_source_refuses_a_number_it_cannot_hold() {
    let mut app = app();
    let slider = app
        .world_mut()
        .spawn((
            Node::default(),
            SliderValue(0.0),
            Bindings(vec![Binding::Value {
                with: BindPath::new("Res(Roster).seats"),
                two_way: true,
            }]),
        ))
        .id();
    app.update();
    app.world_mut().trigger(ValueChange {
        source: slider,
        value: -3.0_f32,
        is_final: true,
    });
    app.update();
    assert_eq!(
        app.world().resource::<Roster>().seats,
        0,
        "a number outside the field is refused, not wrapped or saturated",
    );
}
