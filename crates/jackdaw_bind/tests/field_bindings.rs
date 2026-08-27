use bevy::prelude::*;
use bevy::reflect::func::{DynamicFunction, Return, SignatureInfo};
use jackdaw_bind::{
    BindContext, BindError, BindFailures, BindPath, BindValue, Binding, Bindings,
    JackdawBindPlugin, WriteValue, apply_via, evaluate_bindings, write_path,
};

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Health {
    current: f32,
    max: f32,
}

#[derive(Component, Reflect, Default)]
#[component(immutable)]
#[reflect(Component)]
struct Locked(f32);

fn ratio(current: f32, max: f32) -> f32 {
    (current / max).clamp(0.0, 1.0)
}

/// A registered function that hands back a borrow instead of an owned value.
/// Rust's `IntoFunction` impls cannot express this from a plain fn item, so the
/// dynamic function is built by hand.
fn borrowed_function() -> DynamicFunction<'static> {
    static VALUE: f32 = 1.0;
    DynamicFunction::new(
        |_args| Ok(Return::Ref(&VALUE)),
        SignatureInfo::named("borrowed")
            .with_arg::<f32>("current")
            .with_return::<&f32>(),
    )
}

mod hud {
    pub fn scale(v: f32) -> f32 {
        v
    }
}

mod hotbar {
    pub fn scale(v: f32) -> f32 {
        v * 2.0
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
    app.register_type::<Health>();
    app.register_type::<Locked>();
    app.register_function(ratio);
    app
}

#[test]
fn field_binding_writes_percent_width() {
    let mut app = app();
    let subject = app
        .world_mut()
        .spawn(Health {
            current: 40.0,
            max: 100.0,
        })
        .id();
    let node = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Health.current"), BindPath::new("Health.max")],
                via: Some("ratio".into()),
                write: BindPath::new("Node.width"),
                as_percent: true,
            }]),
        ))
        .id();
    app.update();
    let width = app.world().get::<Node>(node).unwrap().width;
    assert_eq!(width, Val::Percent(40.0));

    app.world_mut().get_mut::<Health>(subject).unwrap().current = 10.0;
    app.update();
    let width = app.world().get::<Node>(node).unwrap().width;
    assert_eq!(width, Val::Percent(10.0));
}

#[test]
fn broken_path_warns_once_and_does_not_panic() {
    let mut app = app();
    let subject = app.world_mut().spawn_empty().id();
    app.world_mut().spawn((
        Node::default(),
        BindContext(subject),
        Bindings(vec![Binding::Field {
            read: vec![BindPath::new("Nonsense.field")],
            via: None,
            write: BindPath::new("Node.width"),
            as_percent: false,
        }]),
    ));
    app.update();
    app.update();
    let failures = app.world().resource::<jackdaw_bind::BindFailures>();
    assert_eq!(failures.0.len(), 1);
}

/// Several reads with no `via` to fold them is an error. Writing only the last
/// one would leave a binding that forgot its combiner looking wired up.
#[test]
fn several_reads_without_via_is_an_error_not_a_silent_last_read() {
    let mut app = app();
    let subject = app
        .world_mut()
        .spawn(Health {
            current: 40.0,
            max: 100.0,
        })
        .id();
    let node = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Health.current"), BindPath::new("Health.max")],
                via: None,
                write: BindPath::new("Node.width"),
                as_percent: false,
            }]),
        ))
        .id();
    app.update();
    let failures = app.world().resource::<jackdaw_bind::BindFailures>();
    assert_eq!(failures.0.len(), 1);
    assert_eq!(app.world().get::<Node>(node).unwrap().width, Val::Auto);
}

#[derive(Resource, Default)]
struct ChangedNodes(usize);

fn count_changed_nodes(nodes: Query<(), Changed<Node>>, mut count: ResMut<ChangedNodes>) {
    count.0 += nodes.iter().count();
}

#[test]
fn re_evaluating_an_unchanged_source_leaves_the_target_clean() {
    let mut app = app();
    app.init_resource::<ChangedNodes>();
    app.add_systems(PostUpdate, count_changed_nodes.after(evaluate_bindings));
    let subject = app
        .world_mut()
        .spawn(Health {
            current: 40.0,
            max: 100.0,
        })
        .id();
    let node = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Health.current"), BindPath::new("Health.max")],
                via: Some("ratio".into()),
                write: BindPath::new("Node.width"),
                as_percent: true,
            }]),
        ))
        .id();
    app.update();
    assert_eq!(app.world().resource::<ChangedNodes>().0, 1);

    app.world_mut().resource_mut::<ChangedNodes>().0 = 0;
    app.update();
    assert_eq!(app.world().resource::<ChangedNodes>().0, 0);
    let width = app.world().get::<Node>(node).unwrap().width;
    assert_eq!(width, Val::Percent(40.0));
}

#[test]
fn ambiguous_short_function_name_is_an_error() {
    let mut app = app();
    app.register_function(hud::scale);
    app.register_function(hotbar::scale);
    let err = apply_via(app.world(), "scale", vec![BindValue::F32(1.0)]).unwrap_err();
    assert!(
        matches!(err, BindError::AmbiguousFunction { .. }),
        "wrong branch: {err}"
    );
    let message = err.to_string();
    assert!(
        message.starts_with("ambiguous function 'scale'"),
        "{message}"
    );
    assert!(message.contains("hud::scale"), "{message}");
    assert!(message.contains("hotbar::scale"), "{message}");
}

#[test]
fn immutable_component_write_is_an_error_not_a_panic() {
    let mut app = app();
    let entity = app.world_mut().spawn(Locked(1.0)).id();
    let err = write_path(
        app.world_mut(),
        entity,
        &BindPath::new("Locked.0"),
        &WriteValue::F32(2.0),
    )
    .unwrap_err();
    assert!(
        matches!(err, BindError::ImmutableComponent { .. }),
        "wrong branch: {err}"
    );
    assert!(err.to_string().contains("immutable"), "{err}");
    assert_eq!(app.world().get::<Locked>(entity).unwrap().0, 1.0);
}

/// A NaN is refused by the float targets, not only the integer ones.
///
/// NaN compares unequal to itself, so a stored NaN makes the equality guard
/// report a change on every evaluation: the component is flagged every frame
/// forever, and a NaN `Val` goes on to reach layout. Refusing the write keeps
/// an unchanged source from costing anything.
#[test]
fn a_non_finite_number_is_refused_by_every_float_target() {
    let mut app = app();
    let entity = app
        .world_mut()
        .spawn((
            Health {
                current: 1.0,
                max: 2.0,
            },
            Node {
                width: Val::Px(10.0),
                ..default()
            },
        ))
        .id();

    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let err = write_path(
            app.world_mut(),
            entity,
            &BindPath::new("Node.width"),
            &WriteValue::F32(value),
        )
        .unwrap_err();
        assert!(
            matches!(err, BindError::WriteOutOfRange { target: "Val", .. }),
            "wrong branch for {value}: {err}"
        );

        let err = write_path(
            app.world_mut(),
            entity,
            &BindPath::new("Health.current"),
            &WriteValue::F32(value),
        )
        .unwrap_err();
        assert!(
            matches!(err, BindError::WriteOutOfRange { target: "f32", .. }),
            "wrong branch for {value}: {err}"
        );
    }

    assert_eq!(
        app.world().get::<Node>(entity).unwrap().width,
        Val::Px(10.0),
        "the refused write left the authored value alone",
    );
    assert_eq!(
        app.world().get::<Health>(entity).unwrap().current,
        1.0,
        "the refused write left the authored value alone",
    );
}

/// A NaN that got in would never stop flagging its component, because it
/// compares unequal to itself. Pins that the write never reports a change it
/// cannot stop reporting.
#[test]
fn a_refused_nan_cannot_churn_change_ticks() {
    let mut app = app();
    let entity = app
        .world_mut()
        .spawn(Node {
            width: Val::Px(10.0),
            ..default()
        })
        .id();

    // The write is refused, so it reports no change, twice over, which is what
    // a stored NaN could never do.
    for _ in 0..2 {
        assert!(
            write_path(
                app.world_mut(),
                entity,
                &BindPath::new("Node.width"),
                &WriteValue::F32(f32::NAN),
            )
            .is_err(),
            "a NaN write is refused rather than reporting a change",
        );
    }

    // Not vacuous: a real number still reports a change once, then settles.
    assert!(
        write_path(
            app.world_mut(),
            entity,
            &BindPath::new("Node.width"),
            &WriteValue::F32(20.0),
        )
        .expect("a finite write lands"),
        "the first write of a new value is a change",
    );
    assert!(
        !write_path(
            app.world_mut(),
            entity,
            &BindPath::new("Node.width"),
            &WriteValue::F32(20.0),
        )
        .expect("a finite write lands"),
        "and writing it again is not -- the guard settles, which NaN never would",
    );
}

/// The type is never spawned, inserted or queried in this world, so it holds no
/// `ComponentId` until the write forces one.
#[test]
fn unspawned_immutable_component_write_is_an_error_not_a_panic() {
    let mut app = app();
    let entity = app.world_mut().spawn_empty().id();
    let err = write_path(
        app.world_mut(),
        entity,
        &BindPath::new("Locked.0"),
        &WriteValue::F32(2.0),
    )
    .unwrap_err();
    assert!(
        matches!(err, BindError::ImmutableComponent { .. }),
        "wrong branch: {err}"
    );
    assert!(err.to_string().contains("immutable"), "{err}");
}

#[test]
fn borrowed_return_is_an_error_not_a_panic() {
    let mut app = app();
    app.register_function(borrowed_function());
    let err = apply_via(app.world(), "borrowed", vec![BindValue::F32(1.0)]).unwrap_err();
    assert!(
        matches!(err, BindError::NonOwnedReturn { .. }),
        "wrong branch: {err}"
    );
    assert!(
        err.to_string().contains("must return an owned value"),
        "{err}"
    );
}

#[test]
fn text_binding_formats_values() {
    let mut app = app();
    let subject = app
        .world_mut()
        .spawn(Health {
            current: 87.0,
            max: 120.0,
        })
        .id();
    let node = app
        .world_mut()
        .spawn((
            Text::new(""),
            BindContext(subject),
            Bindings(vec![Binding::Text {
                format: "{} / {}".into(),
                args: vec![BindPath::new("Health.current"), BindPath::new("Health.max")],
            }]),
        ))
        .id();
    app.update();
    assert_eq!(app.world().get::<Text>(node).unwrap().0, "87 / 120");
}

#[test]
fn visible_binding_toggles_visibility() {
    let mut app = app();
    app.register_function_with_name("is_zero", |v: f32| v == 0.0);
    let subject = app
        .world_mut()
        .spawn(Health {
            current: 0.0,
            max: 100.0,
        })
        .id();
    let veil = app
        .world_mut()
        .spawn((
            Node::default(),
            Visibility::Hidden,
            BindContext(subject),
            Bindings(vec![Binding::Visible {
                read: BindPath::new("Health.current"),
                via: Some("is_zero".into()),
            }]),
        ))
        .id();
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(veil).unwrap(),
        Visibility::Inherited
    );
    app.world_mut().get_mut::<Health>(subject).unwrap().current = 50.0;
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(veil).unwrap(),
        Visibility::Hidden
    );
}

/// A widget that is its own subject can be authored to read the very field it
/// writes. The binding would then feed itself, so it is refused when it is
/// resolved rather than run.
#[test]
fn a_binding_that_reads_what_it_writes_is_refused() {
    let mut app = app();
    let widget = app.world_mut().spawn_empty().id();
    app.world_mut().entity_mut(widget).insert((
        Node::default(),
        Health {
            current: 40.0,
            max: 100.0,
        },
        BindContext(widget),
        Bindings(vec![Binding::Field {
            read: vec![BindPath::new("Health.current")],
            via: None,
            write: BindPath::new("Health.current"),
            as_percent: false,
        }]),
    ));
    app.update();
    assert!(
        app.world()
            .resource::<BindFailures>()
            .0
            .contains(&(widget, 0)),
        "a binding reading its own write target should be reported",
    );
    assert_eq!(app.world().get::<Health>(widget).unwrap().current, 40.0);
}

/// The same shape with the read and the write on different fields is an
/// ordinary binding, so the refusal above is about the cycle and nothing else.
#[test]
fn reading_a_neighbouring_field_of_the_written_component_is_fine() {
    let mut app = app();
    let widget = app.world_mut().spawn_empty().id();
    app.world_mut().entity_mut(widget).insert((
        Node::default(),
        Health {
            current: 40.0,
            max: 100.0,
        },
        BindContext(widget),
        Bindings(vec![Binding::Field {
            read: vec![BindPath::new("Health.max")],
            via: None,
            write: BindPath::new("Health.current"),
            as_percent: false,
        }]),
    ));
    app.update();
    assert!(app.world().resource::<BindFailures>().0.is_empty());
    assert_eq!(app.world().get::<Health>(widget).unwrap().current, 100.0);
}

/// A binding that failed once and then started working has nothing left to
/// report, so its entry leaves the ledger and a second failure is worth a
/// second line.
#[test]
fn a_binding_that_starts_working_leaves_the_failure_ledger() {
    let mut app = app();
    let subject = app
        .world_mut()
        .spawn(Health {
            current: 3.0,
            max: 10.0,
        })
        .id();
    let label = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Text {
                format: "{}".into(),
                args: vec![BindPath::new("Health.current")],
            }]),
        ))
        .id();

    app.update();
    assert!(
        app.world()
            .resource::<BindFailures>()
            .0
            .contains(&(label, 0)),
        "a text binding with no Text to write should report",
    );

    app.world_mut().entity_mut(label).insert(Text::default());
    app.update();
    assert!(
        !app.world()
            .resource::<BindFailures>()
            .0
            .contains(&(label, 0)),
        "the binding works now and should no longer be a standing failure",
    );
    assert_eq!(app.world().get::<Text>(label).unwrap().0, "3");
}
