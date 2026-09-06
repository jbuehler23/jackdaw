use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use jackdaw_bind::{BindContext, BindPath, Binding, Bindings, JackdawBindPlugin};

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Health {
    current: f32,
    max: f32,
}

#[derive(EntityEvent, Reflect, Clone)]
#[reflect(Event, Default)]
struct OpenSettingsMenu {
    entity: Entity,
}

impl Default for OpenSettingsMenu {
    fn default() -> Self {
        Self {
            entity: Entity::PLACEHOLDER,
        }
    }
}

#[derive(Resource, Default)]
struct Opened(u32);

fn ratio(current: f32, max: f32) -> f32 {
    (current / max).clamp(0.0, 1.0)
}

fn is_zero(v: f32) -> bool {
    v == 0.0
}

#[test]
fn hud_binds_fill_text_veil_and_button() {
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
    app.register_type::<OpenSettingsMenu>();
    app.register_function(ratio);
    app.register_function(is_zero);
    app.init_resource::<Opened>();
    app.add_observer(|_: On<OpenSettingsMenu>, mut opened: ResMut<Opened>| opened.0 += 1);

    let player = app
        .world_mut()
        .spawn(Health {
            current: 87.0,
            max: 120.0,
        })
        .id();
    let root = app
        .world_mut()
        .spawn((Node::default(), BindContext(player)))
        .id();
    let fill = app
        .world_mut()
        .spawn((
            Node::default(),
            ChildOf(root),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Health.current"), BindPath::new("Health.max")],
                via: Some("ratio".into()),
                write: BindPath::new("Node.width"),
                as_percent: true,
            }]),
        ))
        .id();
    let label = app
        .world_mut()
        .spawn((
            Text::new(""),
            ChildOf(root),
            Bindings(vec![Binding::Text {
                format: "{} / {}".into(),
                args: vec![BindPath::new("Health.current"), BindPath::new("Health.max")],
            }]),
        ))
        .id();
    let veil = app
        .world_mut()
        .spawn((
            Node::default(),
            Visibility::Hidden,
            ChildOf(root),
            Bindings(vec![Binding::Visible {
                read: BindPath::new("Health.current"),
                via: Some("is_zero".into()),
            }]),
        ))
        .id();
    let button = app
        .world_mut()
        .spawn((
            Node::default(),
            ChildOf(root),
            Bindings(vec![Binding::Action {
                event: "OpenSettingsMenu".into(),
                fields: vec![],
            }]),
        ))
        .id();

    app.update();
    assert_eq!(
        app.world().get::<Node>(fill).unwrap().width,
        Val::Percent(72.5)
    );
    assert_eq!(app.world().get::<Text>(label).unwrap().0, "87 / 120");
    assert_eq!(
        *app.world().get::<Visibility>(veil).unwrap(),
        Visibility::Hidden
    );

    app.world_mut().get_mut::<Health>(player).unwrap().current = 0.0;
    app.update();
    assert_eq!(
        app.world().get::<Node>(fill).unwrap().width,
        Val::Percent(0.0)
    );
    assert_eq!(app.world().get::<Text>(label).unwrap().0, "0 / 120");
    assert_eq!(
        *app.world().get::<Visibility>(veil).unwrap(),
        Visibility::Inherited
    );

    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert_eq!(app.world().resource::<Opened>().0, 1);
}

/// BSN deserialization resolves every type by registration. Registering
/// `Bindings` already pulls `Binding` and `BindPath` in transitively, so the
/// explicit lines in the plugin record the BSN vocabulary rather than being
/// required for the dependent types to resolve.
#[test]
fn plugin_registers_the_binding_types() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(JackdawBindPlugin);
    let registry = app.world().resource::<AppTypeRegistry>().read();
    for type_path in [
        "jackdaw_bind::types::Bindings",
        "jackdaw_bind::types::Binding",
        "jackdaw_bind::types::BindPath",
        "jackdaw_bind::types::BindContext",
    ] {
        assert!(
            registry.get_with_type_path(type_path).is_some(),
            "{type_path} is not registered"
        );
    }
}
