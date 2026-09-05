//! Runtime-safe ECS-native UI authoring primitives for Jackdaw.
//!
//! Authored widgets are small reflected components. Systems in this crate
//! materialize the Bevy/Feathers implementation details around those
//! components, keeping generated entities out of the authored scene model.

use std::collections::HashSet;

use bevy::{
    app::{App, Plugin, Update},
    asset::{Asset, AssetApp, Assets, Handle},
    ecs::{
        component::Component,
        hierarchy::ChildOf,
        observer::On,
        query::{Changed, Has, Or, With},
        reflect::ReflectComponent,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Query, Res},
    },
    feathers::controls::{
        ButtonVariant, FeathersButton, FeathersButtonProps, FeathersCheckbox,
        FeathersCheckboxProps, FeathersSlider, FeathersSliderProps, FeathersTextInput,
        FeathersTextInputProps, FeathersToggleSwitch,
    },
    input::InputPlugin,
    picking::hover::Hovered,
    prelude::{
        BackgroundColor, Color, Entity, Node, Pickable, PositionType, Reflect, ReflectDefault,
        TextColor, TextFont, UVec2, Val, Visibility,
    },
    text::{EditableText, TextLayout, TextPlugin},
    ui::{Checked, InteractionDisabled, Pressed, widget::Text},
    ui_widgets::{
        Button, ButtonPlugin, CheckboxPlugin, EditableTextInputPlugin, Slider, SliderPlugin,
        SliderRange, SliderValue, ValueChange,
    },
};
use bevy_scene::{EntityWorldMutSceneExt, SceneComponent, ScenePlugin};

/// Default [`Node`] for a [`UiCanvas`].
///
/// A canvas is the screen-space coordinate system its children lay out in, so
/// it fills its target and positions absolutely: several canvases can target
/// one camera without stacking each other in a flex row.
fn canvas_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        top: Val::Px(0.0),
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        ..Default::default()
    }
}

/// A root that establishes a screen-space UI coordinate system.
///
/// A canvas intentionally remains an ECS root at runtime. Scene membership is
/// tracked independently from [`ChildOf`] so Bevy's UI layout recognizes it as
/// a root.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Clone, Default)]
#[require(Node = canvas_node(), Visibility)]
pub struct UiCanvas {
    /// Reference resolution used by editor projections and runtime scaling.
    pub reference_size: UVec2,
    /// How the reference resolution maps into the target viewport.
    pub scale_mode: UiScaleMode,
    /// Draw order relative to other canvases targeting the same camera.
    pub order: i32,
    /// Whether this canvas participates in rendering and interaction.
    pub enabled: bool,
}

impl Default for UiCanvas {
    fn default() -> Self {
        Self {
            reference_size: UVec2::new(1920, 1080),
            scale_mode: UiScaleMode::ScaleToFit,
            order: 0,
            enabled: true,
        }
    }
}

/// Scaling policy for a [`UiCanvas`].
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiScaleMode {
    /// Preserve authored pixel sizes.
    ConstantPixels,
    /// Scale uniformly so the reference size fits inside the target.
    #[default]
    ScaleToFit,
    /// Stretch both axes to fill the target.
    Stretch,
}

/// Typed authored data for a Feathers-compatible button.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Clone, Default)]
#[require(Node, Visibility)]
pub struct UiButton {
    /// Text displayed by the generated label.
    pub label: String,
    /// Feathers visual variant.
    pub variant: ButtonVariant,
    /// Whether the runtime behavior is disabled.
    pub disabled: bool,
}

/// Typed authored data for a Feathers checkbox.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Clone, Default)]
#[require(Node, Visibility)]
pub struct UiCheckbox {
    pub label: String,
    pub checked: bool,
    pub disabled: bool,
}

/// Typed authored data for a Feathers toggle switch.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Clone, Default)]
#[require(Node, Visibility)]
pub struct UiToggle {
    pub checked: bool,
    pub disabled: bool,
}

/// Typed authored data for a Feathers scalar slider.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Clone, Default)]
#[require(Node, Visibility)]
pub struct UiSlider {
    pub value: f32,
    pub min: f32,
    pub max: f32,
    pub disabled: bool,
}

impl Default for UiSlider {
    fn default() -> Self {
        Self {
            value: 0.5,
            min: 0.0,
            max: 1.0,
            disabled: false,
        }
    }
}

/// Typed authored data for a Feathers editable text field.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Clone, Default)]
#[require(Node, Visibility)]
pub struct UiTextInput {
    pub value: String,
    pub placeholder: String,
    pub max_characters: Option<usize>,
    pub disabled: bool,
}

impl Default for UiButton {
    fn default() -> Self {
        Self {
            label: "Button".into(),
            variant: ButtonVariant::Normal,
            disabled: false,
        }
    }
}

/// Names the semantic slot an authored child occupies in its UI parent.
///
/// The ECS hierarchy remains the source of containment and ordering. This
/// optional component lets containers distinguish content from adornments
/// without introducing a second hierarchy.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq, Eq)]
#[reflect(Component, Clone, Default)]
pub struct UiSlot(pub String);

/// Opts one entity into widget materialization under
/// [`UiMaterializePolicy::Marked`].
///
/// The editor authors UI as inert reflected data and marks only its per-view
/// projections, so no Feathers or `bevy_ui_widgets` implementation component
/// is ever written onto an authored entity.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Component, Clone, Default)]
pub struct UiMaterialize;

/// Which entities [`JackdawUiPlugin`] materializes widgets on.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiMaterializePolicy {
    /// Every authored widget becomes a live widget. Used by the game runtime.
    #[default]
    All,
    /// Only entities carrying [`UiMaterialize`] become live widgets. Used by
    /// the editor, where the authored tree is a document and the live widgets
    /// live in view-local projections.
    Marked,
}

/// Identifies an implementation-owned child of an authored UI entity.
///
/// Generated parts never carry authored scene identity and are rebuilt from
/// their owner's reflected component.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Eq)]
#[reflect(Component, Clone)]
pub enum UiGeneratedPart {
    /// The text child generated for a [`UiButton`].
    ButtonLabel,
    CheckboxLabel,
    /// Descendant created by a Feathers scene component expansion.
    FeathersInternal,
}

/// State colors for one button variant.
#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct UiButtonPalette {
    /// Resting background.
    pub normal: Color,
    /// Pointer-hover background.
    pub hovered: Color,
    /// Pointer/key-press background.
    pub pressed: Color,
    /// Disabled background.
    pub disabled: Color,
    /// Normal text color.
    pub text: Color,
    /// Disabled text color.
    pub disabled_text: Color,
}

/// Shared surface colors for non-button Feathers controls.
#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct UiControlPalette {
    pub surface: Color,
    pub accent: Color,
    pub disabled: Color,
    pub text: Color,
    pub disabled_text: Color,
}

impl Default for UiControlPalette {
    fn default() -> Self {
        Self {
            surface: Color::srgb(0.13, 0.15, 0.19),
            accent: Color::srgb(0.20, 0.48, 0.86),
            disabled: Color::srgb(0.15, 0.16, 0.18),
            text: Color::WHITE,
            disabled_text: Color::srgb(0.55, 0.57, 0.61),
        }
    }
}

impl Default for UiButtonPalette {
    fn default() -> Self {
        Self {
            normal: Color::srgb(0.20, 0.22, 0.26),
            hovered: Color::srgb(0.26, 0.29, 0.34),
            pressed: Color::srgb(0.15, 0.17, 0.20),
            disabled: Color::srgb(0.16, 0.17, 0.19),
            text: Color::WHITE,
            disabled_text: Color::srgb(0.55, 0.57, 0.61),
        }
    }
}

/// Project-owned theme asset resolved from the nearest [`UiThemeScope`].
///
/// This is deliberately separate from Feathers' world-global editor
/// [`bevy::feathers::theme::UiTheme`], allowing several game canvases and the
/// editor chrome to use different themes in the same ECS World.
#[derive(Asset, Reflect, Clone, Debug, PartialEq)]
pub struct JackdawUiTheme {
    /// Standard button colors.
    pub normal: UiButtonPalette,
    /// Primary/call-to-action button colors.
    pub primary: UiButtonPalette,
    /// Backgroundless button colors.
    pub plain: UiButtonPalette,
    /// Shared surfaces used by checkboxes, toggles, sliders, and inputs.
    pub controls: UiControlPalette,
}

impl Default for JackdawUiTheme {
    fn default() -> Self {
        Self {
            normal: UiButtonPalette::default(),
            primary: UiButtonPalette {
                normal: Color::srgb(0.18, 0.42, 0.78),
                hovered: Color::srgb(0.22, 0.50, 0.90),
                pressed: Color::srgb(0.13, 0.34, 0.68),
                ..Default::default()
            },
            plain: UiButtonPalette {
                normal: Color::NONE,
                hovered: Color::srgba(1.0, 1.0, 1.0, 0.08),
                pressed: Color::srgba(1.0, 1.0, 1.0, 0.14),
                disabled: Color::NONE,
                ..Default::default()
            },
            controls: UiControlPalette::default(),
        }
    }
}

/// Selects a project UI theme for an entity and its authored descendants.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Clone, Default)]
pub struct UiThemeScope(pub Handle<JackdawUiTheme>);

/// Per-instance visual values layered over the nearest scoped theme.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Clone, Default)]
pub struct UiStyleOverride {
    /// Optional background color for every interaction state.
    pub background: Option<Color>,
    /// Optional label/text color for every interaction state.
    pub text: Option<Color>,
}

#[derive(Resource, Default)]
struct FallbackUiTheme(JackdawUiTheme);

/// Registers runtime UI authoring types without installing behavior systems.
pub struct JackdawUiTypesPlugin;

impl Plugin for JackdawUiTypesPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<UiButton>()
            .register_type::<UiCheckbox>()
            .register_type::<UiToggle>()
            .register_type::<UiSlider>()
            .register_type::<UiTextInput>()
            .register_type::<UiGeneratedPart>()
            .register_type::<UiMaterialize>()
            .register_type::<UiCanvas>()
            .register_type::<UiScaleMode>()
            .register_type::<UiButtonPalette>()
            .register_type::<UiControlPalette>()
            .register_type::<JackdawUiTheme>()
            .register_type::<UiThemeScope>()
            .register_type::<UiStyleOverride>()
            .register_type::<UiSlot>();
    }
}

/// Materializes authored UI components into interactive Bevy widgets.
#[derive(Default)]
pub struct JackdawUiPlugin {
    /// Which entities this app materializes widgets on.
    pub policy: UiMaterializePolicy,
}

impl JackdawUiPlugin {
    /// Materialize only entities carrying [`UiMaterialize`]. The editor uses
    /// this so authored UI stays inert document data.
    pub fn marked_only() -> Self {
        Self {
            policy: UiMaterializePolicy::Marked,
        }
    }
}

fn materializes(policy: UiMaterializePolicy, marked: bool) -> bool {
    policy == UiMaterializePolicy::All || marked
}

impl Plugin for JackdawUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<ScenePlugin>() {
            app.add_plugins(ScenePlugin);
        }
        if !app.is_plugin_added::<TextPlugin>() {
            app.add_plugins(TextPlugin);
        }
        app.add_plugins(JackdawUiTypesPlugin);
        if !app.is_plugin_added::<ButtonPlugin>() {
            app.add_plugins(ButtonPlugin);
        }
        if !app.is_plugin_added::<CheckboxPlugin>() {
            app.add_plugins(CheckboxPlugin);
        }
        if !app.is_plugin_added::<SliderPlugin>() {
            app.add_plugins(SliderPlugin);
        }
        if app.is_plugin_added::<InputPlugin>() && !app.is_plugin_added::<EditableTextInputPlugin>()
        {
            app.add_plugins(EditableTextInputPlugin);
        }
        app.init_asset::<JackdawUiTheme>()
            .init_resource::<FallbackUiTheme>()
            .insert_resource(self.policy)
            .add_observer(update_authored_toggle_value)
            .add_observer(update_authored_slider_value)
            .add_systems(
                Update,
                (
                    refresh_changed_buttons,
                    refresh_changed_checkboxes,
                    refresh_changed_toggles,
                    refresh_changed_sliders,
                    refresh_changed_text_inputs,
                    apply_button_styles,
                    apply_control_styles,
                )
                    .chain(),
            );
    }
}

fn refresh_changed_checkboxes(
    widgets: Query<
        (
            Entity,
            &UiCheckbox,
            &Node,
            Has<FeathersCheckbox>,
            Has<UiMaterialize>,
        ),
        Changed<UiCheckbox>,
    >,
    policy: Res<UiMaterializePolicy>,
    generated: Query<(Entity, &ChildOf, &UiGeneratedPart)>,
    mut commands: Commands,
) {
    for (entity, authored, node, materialized, marked) in &widgets {
        if !materializes(*policy, marked) {
            continue;
        }
        if !materialized {
            let node = node.clone();
            commands.queue(move |world: &mut bevy::prelude::World| {
                let before = direct_children(world, entity);
                let scene =
                    <FeathersCheckbox as SceneComponent>::scene(FeathersCheckboxProps::default());
                {
                    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                        return;
                    };
                    let _ = entity_mut.apply_scene(scene);
                    entity_mut.insert(node);
                }
                mark_new_feathers_children(world, entity, &before);
            });
        }
        let mut root = commands.entity(entity);
        set_checked_disabled(&mut root, authored.checked, authored.disabled);
        upsert_generated_text(
            entity,
            UiGeneratedPart::CheckboxLabel,
            &authored.label,
            &generated,
            &mut commands,
        );
    }
}

fn refresh_changed_toggles(
    widgets: Query<
        (
            Entity,
            &UiToggle,
            &Node,
            Has<FeathersToggleSwitch>,
            Has<UiMaterialize>,
        ),
        Changed<UiToggle>,
    >,
    policy: Res<UiMaterializePolicy>,
    mut commands: Commands,
) {
    for (entity, authored, node, materialized, marked) in &widgets {
        if !materializes(*policy, marked) {
            continue;
        }
        if !materialized {
            let node = node.clone();
            commands.queue(move |world: &mut bevy::prelude::World| {
                let before = direct_children(world, entity);
                let scene = <FeathersToggleSwitch as SceneComponent>::scene(());
                {
                    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                        return;
                    };
                    let _ = entity_mut.apply_scene(scene);
                    entity_mut.insert(node);
                }
                mark_new_feathers_children(world, entity, &before);
            });
        }
        let mut root = commands.entity(entity);
        set_checked_disabled(&mut root, authored.checked, authored.disabled);
    }
}

fn refresh_changed_sliders(
    widgets: Query<
        (
            Entity,
            &UiSlider,
            &Node,
            Has<FeathersSlider>,
            Has<UiMaterialize>,
        ),
        Changed<UiSlider>,
    >,
    policy: Res<UiMaterializePolicy>,
    mut commands: Commands,
) {
    for (entity, authored, node, materialized, marked) in &widgets {
        if !materializes(*policy, marked) {
            continue;
        }
        let value = authored.value.clamp(authored.min, authored.max);
        if !materialized {
            let node = node.clone();
            let props = FeathersSliderProps {
                value,
                min: authored.min,
                max: authored.max,
            };
            commands.queue(move |world: &mut bevy::prelude::World| {
                let before = direct_children(world, entity);
                let scene = <FeathersSlider as SceneComponent>::scene(props);
                {
                    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                        return;
                    };
                    let _ = entity_mut.apply_scene(scene);
                    entity_mut.insert(node);
                }
                mark_new_feathers_children(world, entity, &before);
            });
        }
        let mut root = commands.entity(entity);
        root.try_insert((
            Slider::default(),
            SliderValue(value),
            SliderRange::new(authored.min, authored.max),
        ));
        set_disabled(&mut root, authored.disabled);
        commands.queue(move |world: &mut bevy::prelude::World| {
            set_feathers_generated_text(world, entity, &format!("{value:.2}"));
        });
    }
}

fn refresh_changed_text_inputs(
    widgets: Query<
        (
            Entity,
            &UiTextInput,
            &Node,
            Has<FeathersTextInput>,
            Has<UiMaterialize>,
        ),
        Changed<UiTextInput>,
    >,
    policy: Res<UiMaterializePolicy>,
    mut commands: Commands,
) {
    for (entity, authored, node, materialized, marked) in &widgets {
        if !materializes(*policy, marked) {
            continue;
        }
        if !materialized {
            let node = node.clone();
            let props = FeathersTextInputProps {
                max_characters: authored.max_characters,
                ..Default::default()
            };
            commands.queue(move |world: &mut bevy::prelude::World| {
                let before = direct_children(world, entity);
                let scene = <FeathersTextInput as SceneComponent>::scene(props);
                {
                    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                        return;
                    };
                    let _ = entity_mut.apply_scene(scene);
                    entity_mut.insert(node);
                }
                mark_new_feathers_children(world, entity, &before);
            });
        }
        let mut editable = EditableText::new(&authored.value);
        editable.max_characters = authored.max_characters;
        let mut root = commands.entity(entity);
        root.try_insert((
            editable,
            TextLayout::no_wrap(),
            TextFont::default(),
            TextColor(Color::srgb(0.90, 0.91, 0.94)),
            BackgroundColor(Color::srgb(0.12, 0.13, 0.16)),
        ));
        set_disabled(&mut root, authored.disabled);
    }
}

fn set_checked_disabled(
    entity: &mut bevy::ecs::system::EntityCommands<'_>,
    checked: bool,
    disabled: bool,
) {
    if checked {
        entity.try_insert(Checked);
    } else {
        entity.try_remove::<Checked>();
    }
    set_disabled(entity, disabled);
}

fn set_disabled(entity: &mut bevy::ecs::system::EntityCommands<'_>, disabled: bool) {
    if disabled {
        entity.try_insert(InteractionDisabled);
    } else {
        entity.try_remove::<InteractionDisabled>();
    }
}

fn upsert_generated_text(
    owner: Entity,
    part: UiGeneratedPart,
    text: &str,
    generated: &Query<(Entity, &ChildOf, &UiGeneratedPart)>,
    commands: &mut Commands,
) {
    if let Some((entity, _, _)) = generated
        .iter()
        .find(|(_, parent, candidate)| parent.parent() == owner && **candidate == part)
    {
        commands
            .entity(entity)
            .try_insert(Text::new(text.to_string()));
    } else {
        let text = text.to_string();
        commands.queue(move |world: &mut bevy::prelude::World| {
            if world.get_entity(owner).is_err() {
                return;
            }
            world.spawn((
                part,
                Text::new(text),
                TextFont::default(),
                TextColor(Color::WHITE),
                Pickable::IGNORE,
                ChildOf(owner),
            ));
        });
    }
}

fn update_authored_toggle_value(
    event: On<ValueChange<bool>>,
    mut checkboxes: Query<&mut UiCheckbox>,
    mut toggles: Query<&mut UiToggle>,
    mut commands: Commands,
) {
    if let Ok(mut checkbox) = checkboxes.get_mut(event.source) {
        checkbox.checked = event.value;
    }
    if let Ok(mut toggle) = toggles.get_mut(event.source) {
        toggle.checked = event.value;
    }
    if event.value {
        commands.entity(event.source).try_insert(Checked);
    } else {
        commands.entity(event.source).try_remove::<Checked>();
    }
}

fn update_authored_slider_value(
    event: On<ValueChange<f32>>,
    mut sliders: Query<&mut UiSlider>,
    mut commands: Commands,
) {
    if let Ok(mut slider) = sliders.get_mut(event.source) {
        slider.value = event.value;
        commands
            .entity(event.source)
            .try_insert(SliderValue(event.value));
    }
}

fn refresh_changed_buttons(
    buttons: Query<
        (
            Entity,
            &UiButton,
            &Node,
            Has<FeathersButton>,
            Has<UiMaterialize>,
        ),
        Changed<UiButton>,
    >,
    policy: Res<UiMaterializePolicy>,
    generated: Query<(Entity, &ChildOf, &UiGeneratedPart)>,
    mut commands: Commands,
) {
    for (entity, button, node, materialized, marked) in &buttons {
        if !materializes(*policy, marked) {
            continue;
        }
        materialize_button(
            entity,
            button,
            node,
            materialized,
            &generated,
            &mut commands,
        );
    }
}

fn materialize_button(
    entity: Entity,
    authored: &UiButton,
    node: &Node,
    materialized: bool,
    generated: &Query<(Entity, &ChildOf, &UiGeneratedPart)>,
    commands: &mut Commands,
) {
    if !materialized {
        let node = node.clone();
        let props = FeathersButtonProps {
            variant: authored.variant.clone(),
            ..Default::default()
        };
        commands.queue(move |world: &mut bevy::prelude::World| {
            let before = direct_children(world, entity);
            let scene = <FeathersButton as SceneComponent>::scene(props);
            {
                let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                    return;
                };
                let _ = entity_mut.apply_scene(scene);
                entity_mut.insert(node);
            }
            mark_new_feathers_children(world, entity, &before);
        });
    }
    let mut root = commands.entity(entity);
    root.try_insert((
        Button,
        authored.variant.clone(),
        Hovered::default(),
        BackgroundColor(default_button_color(&authored.variant)),
    ));

    if authored.disabled {
        root.try_insert(InteractionDisabled);
    } else {
        root.try_remove::<InteractionDisabled>();
    }

    let label = generated
        .iter()
        .find(|(_, parent, part)| {
            parent.parent() == entity && **part == UiGeneratedPart::ButtonLabel
        })
        .map(|(label, _, _)| label);

    if let Some(label) = label {
        commands
            .entity(label)
            .try_insert((Text::new(authored.label.clone()), TextColor(Color::WHITE)));
    } else {
        let label = authored.label.clone();
        commands.queue(move |world: &mut bevy::prelude::World| {
            if world.get_entity(entity).is_err() {
                return;
            }
            world.spawn((
                UiGeneratedPart::ButtonLabel,
                Text::new(label),
                TextFont::default(),
                TextColor(Color::WHITE),
                bevy::picking::Pickable::IGNORE,
                ChildOf(entity),
            ));
        });
    }
}

fn direct_children(world: &bevy::prelude::World, entity: Entity) -> HashSet<Entity> {
    world
        .get::<bevy::prelude::Children>(entity)
        .map(|children| children.iter().copied().collect())
        .unwrap_or_default()
}

fn mark_new_feathers_children(
    world: &mut bevy::prelude::World,
    owner: Entity,
    before: &HashSet<Entity>,
) {
    let roots = world
        .get::<bevy::prelude::Children>(owner)
        .map(|children| {
            children
                .iter()
                .copied()
                .filter(|child| !before.contains(child))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut stack = roots;
    while let Some(entity) = stack.pop() {
        if let Some(children) = world.get::<bevy::prelude::Children>(entity) {
            stack.extend(children.iter().copied());
        }
        world
            .entity_mut(entity)
            .insert(UiGeneratedPart::FeathersInternal);
    }
}

fn set_feathers_generated_text(world: &mut bevy::prelude::World, owner: Entity, value: &str) {
    let mut descendants = world
        .get::<bevy::prelude::Children>(owner)
        .map(|children| children.to_vec())
        .unwrap_or_default();
    let mut cursor = 0;
    while cursor < descendants.len() {
        if let Some(children) = world.get::<bevy::prelude::Children>(descendants[cursor]) {
            descendants.extend_from_slice(children);
        }
        cursor += 1;
    }
    for entity in descendants {
        if world.get::<UiGeneratedPart>(entity) == Some(&UiGeneratedPart::FeathersInternal)
            && let Some(mut text) = world.get_mut::<Text>(entity)
        {
            text.0 = value.to_string();
        }
    }
}

fn apply_button_styles(
    buttons: Query<(
        Entity,
        &UiButton,
        &Hovered,
        Has<Pressed>,
        Has<InteractionDisabled>,
        Option<&UiStyleOverride>,
        Has<UiMaterialize>,
    )>,
    policy: Res<UiMaterializePolicy>,
    parents: Query<&ChildOf>,
    scopes: Query<&UiThemeScope>,
    themes: Res<Assets<JackdawUiTheme>>,
    fallback: Res<FallbackUiTheme>,
    generated: Query<(Entity, &ChildOf, &UiGeneratedPart)>,
    mut commands: Commands,
) {
    for (entity, button, hovered, pressed, disabled, style_override, marked) in &buttons {
        if !materializes(*policy, marked) {
            continue;
        }
        let theme = scoped_theme(entity, &parents, &scopes, &themes).unwrap_or(&fallback.0);
        let palette = match button.variant {
            ButtonVariant::Normal => &theme.normal,
            ButtonVariant::Primary => &theme.primary,
            ButtonVariant::Plain => &theme.plain,
        };
        let themed_background = if disabled {
            palette.disabled
        } else if pressed {
            palette.pressed
        } else if hovered.0 {
            palette.hovered
        } else {
            palette.normal
        };
        let background = style_override
            .and_then(|overrides| overrides.background)
            .unwrap_or(themed_background);
        commands
            .entity(entity)
            .try_insert(BackgroundColor(background));

        let themed_text_color = if disabled {
            palette.disabled_text
        } else {
            palette.text
        };
        let text_color = style_override
            .and_then(|overrides| overrides.text)
            .unwrap_or(themed_text_color);
        for (label, parent, part) in &generated {
            if parent.parent() == entity && *part == UiGeneratedPart::ButtonLabel {
                commands.entity(label).try_insert(TextColor(text_color));
            }
        }
    }
}

fn apply_control_styles(
    controls: Query<
        (
            Entity,
            Has<UiCheckbox>,
            Has<UiToggle>,
            Has<UiSlider>,
            Has<UiTextInput>,
            Has<Checked>,
            Has<InteractionDisabled>,
            Option<&UiStyleOverride>,
            Has<UiMaterialize>,
        ),
        Or<(
            With<UiCheckbox>,
            With<UiToggle>,
            With<UiSlider>,
            With<UiTextInput>,
        )>,
    >,
    policy: Res<UiMaterializePolicy>,
    parents: Query<&ChildOf>,
    scopes: Query<&UiThemeScope>,
    themes: Res<Assets<JackdawUiTheme>>,
    fallback: Res<FallbackUiTheme>,
    generated: Query<(Entity, &ChildOf, &UiGeneratedPart)>,
    text_nodes: Query<(), With<Text>>,
    mut commands: Commands,
) {
    for (
        entity,
        is_checkbox,
        is_toggle,
        is_slider,
        is_input,
        checked,
        disabled,
        style_override,
        marked,
    ) in &controls
    {
        if !materializes(*policy, marked) {
            continue;
        }
        let theme = scoped_theme(entity, &parents, &scopes, &themes).unwrap_or(&fallback.0);
        let palette = &theme.controls;
        let themed_background = if disabled {
            palette.disabled
        } else if (is_checkbox || is_toggle) && checked || is_slider {
            palette.accent
        } else {
            palette.surface
        };
        let background = style_override
            .and_then(|overrides| overrides.background)
            .unwrap_or(themed_background);
        commands
            .entity(entity)
            .try_insert(BackgroundColor(background));

        let themed_text = if disabled {
            palette.disabled_text
        } else {
            palette.text
        };
        let text = style_override
            .and_then(|overrides| overrides.text)
            .unwrap_or(themed_text);
        if is_input {
            commands.entity(entity).try_insert(TextColor(text));
        }
        for (label, parent, _) in &generated {
            if text_nodes.contains(label) && generated_owner(parent.parent(), &generated) == entity
            {
                commands.entity(label).try_insert(TextColor(text));
            }
        }
    }
}

fn generated_owner(
    mut candidate: Entity,
    generated: &Query<(Entity, &ChildOf, &UiGeneratedPart)>,
) -> Entity {
    while let Ok(parent) = generated.get(candidate).map(|(_, parent, _)| parent) {
        candidate = parent.parent();
    }
    candidate
}

fn scoped_theme<'a>(
    entity: Entity,
    parents: &Query<&ChildOf>,
    scopes: &Query<&UiThemeScope>,
    themes: &'a Assets<JackdawUiTheme>,
) -> Option<&'a JackdawUiTheme> {
    let mut current = Some(entity);
    while let Some(candidate) = current {
        if let Ok(scope) = scopes.get(candidate)
            && let Some(theme) = themes.get(&scope.0)
        {
            return Some(theme);
        }
        current = parents.get(candidate).ok().map(ChildOf::parent);
    }
    None
}

fn default_button_color(variant: &ButtonVariant) -> Color {
    match variant {
        ButtonVariant::Normal => Color::srgb(0.20, 0.22, 0.26),
        ButtonVariant::Primary => Color::srgb(0.18, 0.42, 0.78),
        ButtonVariant::Plain => Color::NONE,
    }
}

#[cfg(test)]
mod tests {
    use bevy::{
        ecs::{hierarchy::ChildOf, system::SystemState},
        prelude::*,
    };

    use super::*;

    #[test]
    fn queued_button_materialization_ignores_an_entity_despawned_before_apply() {
        let mut world = World::new();
        let button = world.spawn((UiButton::default(), Node::default())).id();
        let authored = world.get::<UiButton>(button).unwrap().clone();
        let node = world.get::<Node>(button).unwrap().clone();
        let mut state =
            SystemState::<(Query<(Entity, &ChildOf, &UiGeneratedPart)>, Commands)>::new(&mut world);

        {
            let (generated, mut commands) = state.get_mut(&mut world).unwrap();
            materialize_button(button, &authored, &node, false, &generated, &mut commands);
        }
        world.entity_mut(button).despawn();

        state.apply(&mut world);
    }
}
