use bevy::{
    feathers::controls::{
        ButtonVariant, FeathersButton, FeathersCheckbox, FeathersSlider, FeathersTextInput,
        FeathersToggleSwitch,
    },
    picking::hover::Hovered,
    prelude::*,
    text::EditableText,
    ui::{Checked, InteractionDisabled, widget::Text},
    ui_widgets::{Button, Checkbox, Slider, SliderRange, SliderValue},
};
use jackdaw_ui::{
    JackdawUiPlugin, JackdawUiTheme, UiButton, UiButtonPalette, UiCanvas, UiCheckbox,
    UiGeneratedPart, UiSlider, UiStyleOverride, UiTextInput, UiThemeScope, UiToggle,
};

#[test]
fn authored_button_materializes_and_refreshes_without_duplicate_children() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        JackdawUiPlugin::default(),
    ));

    let button = app
        .world_mut()
        .spawn(UiButton {
            label: "Create".into(),
            variant: ButtonVariant::Primary,
            disabled: false,
        })
        .id();

    app.update();

    assert!(app.world().entity(button).contains::<Button>());
    assert!(app.world().entity(button).contains::<FeathersButton>());
    assert_eq!(
        app.world().get::<ButtonVariant>(button),
        Some(&ButtonVariant::Primary)
    );

    let label_entity = generated_label(app.world(), button);
    assert_eq!(
        app.world().get::<Text>(label_entity),
        Some(&Text::new("Create"))
    );

    app.world_mut().get_mut::<UiButton>(button).unwrap().label = "Save".into();
    app.update();
    app.update();

    let refreshed_label = generated_label(app.world(), button);
    assert_eq!(refreshed_label, label_entity);
    assert_eq!(
        app.world().get::<Text>(refreshed_label),
        Some(&Text::new("Save"))
    );
}

#[test]
fn canvas_themes_are_scoped_and_react_to_widget_state() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        JackdawUiPlugin::default(),
    ));

    let blue = app
        .world_mut()
        .resource_mut::<Assets<JackdawUiTheme>>()
        .add(JackdawUiTheme {
            primary: UiButtonPalette {
                normal: Color::srgb(0.0, 0.0, 0.8),
                hovered: Color::srgb(0.0, 0.0, 1.0),
                ..Default::default()
            },
            ..Default::default()
        });
    let green = app
        .world_mut()
        .resource_mut::<Assets<JackdawUiTheme>>()
        .add(JackdawUiTheme {
            primary: UiButtonPalette {
                normal: Color::srgb(0.0, 0.8, 0.0),
                hovered: Color::srgb(0.0, 1.0, 0.0),
                ..Default::default()
            },
            ..Default::default()
        });

    let blue_button = spawn_themed_button(&mut app, blue);
    let green_button = spawn_themed_button(&mut app, green);
    app.update();
    app.update();

    assert_eq!(
        app.world().get::<BackgroundColor>(blue_button),
        Some(&BackgroundColor(Color::srgb(0.0, 0.0, 0.8)))
    );
    assert_eq!(
        app.world().get::<BackgroundColor>(green_button),
        Some(&BackgroundColor(Color::srgb(0.0, 0.8, 0.0)))
    );

    app.world_mut()
        .entity_mut(blue_button)
        .insert(Hovered(true));
    app.update();

    assert_eq!(
        app.world().get::<BackgroundColor>(blue_button),
        Some(&BackgroundColor(Color::srgb(0.0, 0.0, 1.0)))
    );
    assert_eq!(
        app.world().get::<BackgroundColor>(green_button),
        Some(&BackgroundColor(Color::srgb(0.0, 0.8, 0.0)))
    );

    app.world_mut()
        .entity_mut(green_button)
        .insert(UiStyleOverride {
            background: Some(Color::srgb(0.8, 0.0, 0.0)),
            text: None,
        });
    app.update();
    assert_eq!(
        app.world().get::<BackgroundColor>(green_button),
        Some(&BackgroundColor(Color::srgb(0.8, 0.0, 0.0))),
        "an instance override wins only for that widget"
    );
}

#[test]
fn authored_feathers_controls_materialize_from_reflected_facades() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        JackdawUiPlugin::default(),
    ));

    let checkbox = app
        .world_mut()
        .spawn(UiCheckbox {
            label: "Music".into(),
            checked: true,
            disabled: false,
        })
        .id();
    let toggle = app
        .world_mut()
        .spawn(UiToggle {
            checked: false,
            disabled: true,
        })
        .id();
    let slider = app
        .world_mut()
        .spawn(UiSlider {
            value: 0.25,
            min: 0.0,
            max: 2.0,
            disabled: false,
        })
        .id();
    let input = app
        .world_mut()
        .spawn(UiTextInput {
            value: "Player".into(),
            placeholder: "Name".into(),
            max_characters: Some(20),
            disabled: false,
        })
        .id();

    app.update();

    assert!(app.world().entity(checkbox).contains::<Checkbox>());
    assert!(app.world().entity(checkbox).contains::<FeathersCheckbox>());
    assert!(app.world().entity(checkbox).contains::<Checked>());
    assert!(app.world().entity(toggle).contains::<Checkbox>());
    assert!(
        app.world()
            .entity(toggle)
            .contains::<FeathersToggleSwitch>()
    );
    assert!(app.world().entity(toggle).contains::<InteractionDisabled>());
    assert!(app.world().entity(slider).contains::<Slider>());
    assert!(app.world().entity(slider).contains::<FeathersSlider>());
    assert_eq!(
        app.world().get::<SliderValue>(slider),
        Some(&SliderValue(0.25))
    );
    assert_eq!(
        app.world().get::<SliderRange>(slider),
        Some(&SliderRange::new(0.0, 2.0))
    );
    let slider_labels = descendants(app.world(), slider)
        .into_iter()
        .filter_map(|entity| app.world().get::<Text>(entity))
        .collect::<Vec<_>>();
    assert_eq!(
        slider_labels,
        vec![&Text::new("0.25")],
        "the authored value should reuse the Feathers-owned value label"
    );
    assert!(app.world().entity(input).contains::<FeathersTextInput>());
    assert_eq!(
        app.world()
            .get::<EditableText>(input)
            .map(|text| text.value().to_string()),
        Some("Player".to_string())
    );
}

fn spawn_themed_button(app: &mut App, theme: Handle<JackdawUiTheme>) -> Entity {
    let canvas = app
        .world_mut()
        .spawn((UiCanvas::default(), UiThemeScope(theme)))
        .id();
    app.world_mut()
        .spawn((
            UiButton {
                label: "Action".into(),
                variant: ButtonVariant::Primary,
                disabled: false,
            },
            ChildOf(canvas),
        ))
        .id()
}

fn generated_label(world: &World, button: Entity) -> Entity {
    let labels = world
        .get::<Children>(button)
        .into_iter()
        .flatten()
        .copied()
        .filter(|child| world.get::<UiGeneratedPart>(*child) == Some(&UiGeneratedPart::ButtonLabel))
        .collect::<Vec<_>>();
    assert_eq!(labels.len(), 1, "button should have exactly one label");
    labels[0]
}

fn descendants(world: &World, owner: Entity) -> Vec<Entity> {
    let mut descendants = world
        .get::<Children>(owner)
        .map(|children| children.to_vec())
        .unwrap_or_default();
    let mut cursor = 0;
    while cursor < descendants.len() {
        if let Some(children) = world.get::<Children>(descendants[cursor]) {
            descendants.extend_from_slice(children);
        }
        cursor += 1;
    }
    descendants
}
