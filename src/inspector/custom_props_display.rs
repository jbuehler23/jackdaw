use crate::commands::{CommandHistory, EditorCommand};
use crate::custom_properties::{CustomProperties, PropertyValue, SetCustomProperties};

use bevy::ecs::lifecycle::Insert;
use bevy::ecs::system::SystemState;
use bevy::feathers::containers::{flex_spacer, pane_body};
use bevy::feathers::controls::{
    FeathersCheckbox, FeathersMenu, FeathersMenuButton, FeathersMenuItem, FeathersMenuPopup,
    FeathersTextInput, FeathersTextInputContainer,
};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input_focus::{FocusLost, FocusedInput};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::{EditableText, TextEdit};
use bevy::ui::Checked;
use bevy::ui_widgets::{Activate, ValueChange, observe};
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::tokens;
use jackdaw_feathers::tooltip::Tooltip;

use crate::default_style;

use super::{
    CustomPropertyAddRow, CustomPropertyBinding, CustomPropertyNameInput,
    CustomPropertyTypeSelector, rebuild_inspector,
};

pub(super) fn spawn_custom_properties_display(
    commands: &mut Commands,
    parent: Entity,
    source_entity: Entity,
    cp: &CustomProperties,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) {
    // Seat the property rows and the add-property row in a feathers content pane.
    let body = commands
        .spawn_scene(pane_body())
        .insert(ChildOf(parent))
        .id();

    for (prop_name, prop_value) in &cp.properties {
        let row = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(tokens::SPACING_XS),
                    width: Val::Percent(100.0),
                    ..Default::default()
                },
                ChildOf(body),
            ))
            .id();

        // Property name label
        commands.spawn((
            Text::new(format!("{}:", prop_name)),
            TextFont {
                font: editor_font.clone().into(),
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            Node {
                min_width: px(20.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            ChildOf(row),
        ));

        let name = prop_name.clone();
        match prop_value {
            PropertyValue::Bool(val) => {
                // The `CustomPropertyBinding` rides on the checkbox entity, so
                // `on_custom_property_checkbox_commit` reads it off the
                // `ValueChange` source. The checkbox does not self-manage
                // `Checked`; seed the initial state here.
                let mut cb = commands.spawn_scene(bsn! { @FeathersCheckbox });
                cb.insert((
                    CustomPropertyBinding {
                        source_entity,
                        property_name: name,
                    },
                    ChildOf(row),
                ));
                if *val {
                    cb.insert(Checked);
                }
            }
            PropertyValue::Int(val) => {
                spawn_custom_text(
                    commands,
                    row,
                    &val.to_string(),
                    CustomPropertyBinding {
                        source_entity,
                        property_name: name,
                    },
                );
            }
            PropertyValue::Float(val) => {
                spawn_custom_text(
                    commands,
                    row,
                    &val.to_string(),
                    CustomPropertyBinding {
                        source_entity,
                        property_name: name,
                    },
                );
            }
            PropertyValue::String(val) => {
                spawn_custom_text(
                    commands,
                    row,
                    val,
                    CustomPropertyBinding {
                        source_entity,
                        property_name: name,
                    },
                );
            }
            PropertyValue::Vec2(val) => {
                let v = *val;
                let n_x = name.clone();
                let n_y = name.clone();
                spawn_custom_axis(
                    commands,
                    row,
                    "X",
                    v.x as f64,
                    default_style::INSPECTOR_AXIS_X,
                    source_entity,
                    n_x,
                    |new_f, old| {
                        if let PropertyValue::Vec2(v) = old {
                            v.x = new_f as f32;
                        }
                    },
                );
                spawn_custom_axis(
                    commands,
                    row,
                    "Y",
                    v.y as f64,
                    default_style::INSPECTOR_AXIS_Y,
                    source_entity,
                    n_y,
                    |new_f, old| {
                        if let PropertyValue::Vec2(v) = old {
                            v.y = new_f as f32;
                        }
                    },
                );
            }
            PropertyValue::Vec3(val) => {
                let v = *val;
                let n_x = name.clone();
                let n_y = name.clone();
                let n_z = name.clone();
                spawn_custom_axis(
                    commands,
                    row,
                    "X",
                    v.x as f64,
                    default_style::INSPECTOR_AXIS_X,
                    source_entity,
                    n_x,
                    |new_f, old| {
                        if let PropertyValue::Vec3(v) = old {
                            v.x = new_f as f32;
                        }
                    },
                );
                spawn_custom_axis(
                    commands,
                    row,
                    "Y",
                    v.y as f64,
                    default_style::INSPECTOR_AXIS_Y,
                    source_entity,
                    n_y,
                    |new_f, old| {
                        if let PropertyValue::Vec3(v) = old {
                            v.y = new_f as f32;
                        }
                    },
                );
                spawn_custom_axis(
                    commands,
                    row,
                    "Z",
                    v.z as f64,
                    default_style::INSPECTOR_AXIS_Z,
                    source_entity,
                    n_z,
                    |new_f, old| {
                        if let PropertyValue::Vec3(v) = old {
                            v.z = new_f as f32;
                        }
                    },
                );
            }
            PropertyValue::Color(val) => {
                let srgba = val.to_srgba();
                let rgba = [srgba.red, srgba.green, srgba.blue, srgba.alpha];
                let commit_name = name.clone();
                // Commit once on the final settle so a drag is one undo entry.
                super::reflect_fields::spawn_color_picker(
                    commands,
                    row,
                    rgba,
                    prop_name,
                    0.0,
                    move |world, rgba, is_final| {
                        if !is_final {
                            return;
                        }
                        let color = Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3]);
                        apply_custom_property_with_undo(
                            world,
                            source_entity,
                            &commit_name,
                            PropertyValue::Color(color),
                        );
                    },
                );
            }
            PropertyValue::Entity(val) => {
                // Entity values are read-only in the Custom Properties UI
                // for now; surface the bits so users can at least see
                // which entity is referenced. A pickable
                // entity-reference field is future work.
                commands.spawn((
                    Text::new(format!("Entity({})", val.to_bits())),
                    TextFont {
                        font_size: tokens::TEXT_SIZE,
                        ..default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                    ChildOf(row),
                ));
            }
        }

        // Remove property button (X icon)
        let n = prop_name.clone();
        commands.spawn((
            Text::new(String::from(Icon::X.unicode())),
            TextFont {
                font: icon_font.clone().into(),
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Hovered::default(),
            Tooltip::title("Remove Property")
                .with_description("Delete this custom property from the entity."),
            ChildOf(row),
            observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                let n = n.clone();
                commands.queue(move |world: &mut World| {
                    remove_custom_property(world, source_entity, &n);
                });
            }),
        ));
    }

    // "Add Property" row
    spawn_add_property_row(commands, body, source_entity, editor_font, icon_font);
}

/// Marker that links a custom property axis input to its property name and mutation function.
#[derive(Component)]
pub(super) struct CustomAxisBinding {
    source_entity: Entity,
    property_name: String,
    mutate: fn(f64, &mut PropertyValue),
}

/// The type selected in the "Add Property" row's type menu, held on the row.
/// `add_custom_property_from_ui` reads the type name off this component.
#[derive(Component)]
pub(super) struct CustomPropertyTypeChoice(String);

/// Initial text staged on a custom-property text input container. The
/// `Insert`-triggered `seed_custom_text` observer reads it once the child text
/// entry spawns, writes it into the editable buffer, then removes it.
#[derive(Component)]
struct PendingCustomText(String);

fn spawn_custom_axis(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    value: f64,
    label_color: Color,
    source_entity: Entity,
    property_name: String,
    mutate: fn(f64, &mut PropertyValue),
) {
    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(label_color),
        Node {
            flex_shrink: 0.0,
            ..Default::default()
        },
        ChildOf(parent),
    ));

    // The `CustomAxisBinding` rides on the text input container, so
    // `on_custom_property_text_commit` finds it by walking up from the inner
    // text entry.
    spawn_custom_text_with(
        commands,
        parent,
        &value.to_string(),
        CustomAxisBinding {
            source_entity,
            property_name,
            mutate,
        },
    );
}

/// Spawn a `FeathersTextInput` bound to a custom property. The binding and the
/// staged text ride on the container; the inner text entry emits
/// `ValueChange<String>` on Enter or blur, which `on_custom_property_text_commit`
/// writes back through `apply_custom_property_with_undo`.
fn spawn_custom_text(
    commands: &mut Commands,
    parent: Entity,
    current_value: &str,
    binding: CustomPropertyBinding,
) {
    spawn_custom_text_with(commands, parent, current_value, binding);
}

fn spawn_custom_text_with(
    commands: &mut Commands,
    parent: Entity,
    current_value: &str,
    binding: impl Bundle,
) {
    commands.spawn_scene(custom_text_scene()).insert((
        binding,
        PendingCustomText(current_value.to_string()),
        ChildOf(parent),
    ));
}

/// Container framing a `FeathersTextInput`. The inner text entry drives the
/// commit observers; the container holds the binding and staged value, and
/// seeds the text buffer once the staged value lands.
fn custom_text_scene() -> impl Scene {
    bsn! {
        @FeathersTextInputContainer
        on(seed_custom_text)
        Children [
            @FeathersTextInput
            on(custom_text_on_enter_key)
            on(custom_text_on_focus_lost)
        ]
    }
}

/// Write the staged text into the editable buffer once it is inserted on the
/// container, then clear it so a later inspector refresh does not re-seed it.
fn seed_custom_text(
    inserted: On<Insert, PendingCustomText>,
    q_children: Query<&Children>,
    q_pending: Query<&PendingCustomText>,
    mut q_text: Query<&mut EditableText>,
    mut commands: Commands,
) {
    let container = inserted.event_target();
    let Ok(pending) = q_pending.get(container) else {
        return;
    };
    let text_id = q_children
        .iter_descendants(container)
        .find(|e| q_text.contains(*e));
    if let Some(text_id) = text_id
        && let Ok(mut editable) = q_text.get_mut(text_id)
    {
        editable.queue_edit(TextEdit::SelectAll);
        editable.queue_edit(TextEdit::Insert(pending.0.clone().into()));
    }
    commands.entity(container).remove::<PendingCustomText>();
}

/// Emit a final `ValueChange<String>` when Enter is pressed in a custom text input.
fn custom_text_on_enter_key(
    key_input: On<FocusedInput<KeyboardInput>>,
    q_text: Query<&EditableText>,
    mut commands: Commands,
) {
    if key_input.input.key_code != KeyCode::Enter {
        return;
    }
    let text_id = key_input.event_target();
    if let Ok(editable) = q_text.get(text_id) {
        commands.trigger(ValueChange {
            source: text_id,
            value: editable.value().to_string(),
            is_final: true,
        });
    }
}

/// Emit a final `ValueChange<String>` when a custom text input loses focus.
fn custom_text_on_focus_lost(
    focus_lost: On<FocusLost>,
    q_text: Query<&EditableText>,
    mut commands: Commands,
) {
    let text_id = focus_lost.event_target();
    if let Ok(editable) = q_text.get(text_id) {
        commands.trigger(ValueChange {
            source: text_id,
            value: editable.value().to_string(),
            is_final: true,
        });
    }
}

fn spawn_add_property_row(
    commands: &mut Commands,
    parent: Entity,
    source_entity: Entity,
    _editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) {
    let type_names: Vec<String> = PropertyValue::all_type_names()
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    // Default to "Float".
    let default_type = type_names.get(2).cloned().unwrap_or_default();

    let row = commands
        .spawn((
            CustomPropertyAddRow,
            CustomPropertyTypeChoice(default_type.clone()),
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                padding: UiRect::top(Val::Px(tokens::SPACING_SM)),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    // Name input
    commands.spawn_scene(custom_text_scene()).insert((
        CustomPropertyNameInput,
        PendingCustomText(String::new()),
        ChildOf(row),
    ));

    // Type selector menu. The button caption is the selected type; each item
    // writes its type name onto the row's `CustomPropertyTypeChoice`.
    spawn_type_menu(commands, row, &type_names, &default_type);

    // Right-align the confirm button.
    commands.spawn_scene(flex_spacer()).insert(ChildOf(row));

    // Confirm button
    let font = icon_font.clone();
    commands.spawn((
        Text::new(String::from(Icon::Plus.unicode())),
        TextFont {
            font: font.into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_ACCENT),
        Hovered::default(),
        Tooltip::title("Add Custom Property")
            .with_description("Create a new custom property with the entered name and type."),
        ChildOf(row),
        observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.run_system_cached_with(add_custom_property_from_ui, source_entity);
        }),
    ));
}

/// Spawn the add-property type selector: a `FeathersMenu` whose button caption is
/// the selected type and whose popup lists one item per property type. Picking an
/// item writes its type name onto the add-row's `CustomPropertyTypeChoice`, which
/// `add_custom_property_from_ui` reads back.
fn spawn_type_menu(commands: &mut Commands, add_row: Entity, type_names: &[String], current: &str) {
    let menu = commands
        .spawn_scene(bsn! { @FeathersMenu })
        .insert((CustomPropertyTypeSelector, ChildOf(add_row)))
        .id();

    let button = commands
        .spawn_scene(bsn! {
            @FeathersMenuButton {
                @caption: bsn! { Text({current.to_string()}) ThemedText },
            }
        })
        .insert(ChildOf(menu))
        .id();

    let popup = commands
        .spawn_scene(bsn! { @FeathersMenuPopup })
        .insert(ChildOf(menu))
        .id();

    for name in type_names {
        let type_name = name.clone();
        commands
            .spawn_scene(bsn! {
                @FeathersMenuItem {
                    @caption: bsn! { Text({name.clone()}) ThemedText },
                }
            })
            .insert(ChildOf(popup))
            .observe(move |_activate: On<Activate>, mut commands: Commands| {
                let type_name = type_name.clone();
                // Record the chosen type on the add-row and repaint the button
                // caption so the selection is visible.
                commands.queue(move |world: &mut World| {
                    if let Some(mut choice) = world.get_mut::<CustomPropertyTypeChoice>(add_row) {
                        choice.0 = type_name.clone();
                    }
                    set_menu_button_caption(world, button, &type_name);
                });
            });
    }
}

/// Rewrite the text of a `FeathersMenuButton`'s caption to `text`.
fn set_menu_button_caption(world: &mut World, button: Entity, text: &str) {
    let mut descendants: Vec<Entity> = Vec::new();
    if let Ok(children) = world.query::<&Children>().get(world, button) {
        descendants.extend(children.iter());
    }
    while let Some(entity) = descendants.pop() {
        if world.get::<Text>(entity).is_some() {
            world.entity_mut(entity).insert(Text::new(text.to_string()));
            return;
        }
        if let Ok(children) = world.query::<&Children>().get(world, entity) {
            descendants.extend(children.iter());
        }
    }
}

/// Read the name input and selected type, then add a new property.
fn add_custom_property_from_ui(
    In(source_entity): In<Entity>,
    world: &mut World,
    name_input: &mut SystemState<Single<Entity, With<CustomPropertyNameInput>>>,
    type_choice: &mut SystemState<Single<&CustomPropertyTypeChoice, With<CustomPropertyAddRow>>>,
) {
    // Read the name from the input's inner editable text entry.
    let name = {
        let Ok(container) = name_input.get(world).map(Single::into_inner) else {
            return;
        };
        let text = read_editable_text(world, container).unwrap_or_default();
        let name = text.trim().to_string();
        if name.is_empty() {
            return;
        }
        name
    };

    // Read the selected type from the add-row state.
    let type_name = {
        let Ok(choice) = type_choice.get(world).map(Single::into_inner) else {
            return;
        };
        choice.0.clone()
    };

    let Some(default_value) = PropertyValue::default_for_type(&type_name) else {
        return;
    };

    let Some(cp) = world.get::<CustomProperties>(source_entity) else {
        return;
    };
    let old = cp.clone();
    let mut new = old.clone();
    new.properties.insert(name, default_value);

    let mut cmd = SetCustomProperties {
        entity: source_entity,
        old_properties: old,
        new_properties: new,
    };
    cmd.execute(world);

    let mut history = world.resource_mut::<CommandHistory>();
    history.push_executed(Box::new(cmd));

    // Rebuild inspector
    rebuild_inspector(world, source_entity);
}

/// Read the text of the `EditableText` entry nested under `container`.
fn read_editable_text(world: &mut World, container: Entity) -> Option<String> {
    let mut descendants: Vec<Entity> = Vec::new();
    if let Ok(children) = world.query::<&Children>().get(world, container) {
        descendants.extend(children.iter());
    }
    while let Some(entity) = descendants.pop() {
        if let Some(editable) = world.get::<EditableText>(entity) {
            return Some(editable.value().to_string());
        }
        if let Ok(children) = world.query::<&Children>().get(world, entity) {
            descendants.extend(children.iter());
        }
    }
    None
}

/// Remove a property and push undo.
fn remove_custom_property(world: &mut World, source_entity: Entity, property_name: &str) {
    let Some(cp) = world.get::<CustomProperties>(source_entity) else {
        return;
    };
    let old = cp.clone();
    let mut new = old.clone();
    new.properties.remove(property_name);

    let mut cmd = SetCustomProperties {
        entity: source_entity,
        old_properties: old,
        new_properties: new,
    };
    cmd.execute(world);

    let mut history = world.resource_mut::<CommandHistory>();
    history.push_executed(Box::new(cmd));

    rebuild_inspector(world, source_entity);
}

/// Apply a custom property value change with undo.
fn apply_custom_property_with_undo(
    world: &mut World,
    source_entity: Entity,
    property_name: &str,
    new_value: PropertyValue,
) {
    let Some(cp) = world.get::<CustomProperties>(source_entity) else {
        return;
    };
    let old = cp.clone();
    let mut new = old.clone();
    new.properties.insert(property_name.to_string(), new_value);

    let mut cmd = SetCustomProperties {
        entity: source_entity,
        old_properties: old,
        new_properties: new,
    };
    cmd.execute(world);

    let mut history = world.resource_mut::<CommandHistory>();
    history.push_executed(Box::new(cmd));
}

/// Handle `ValueChange<String>` for custom property numeric/string fields + axis
/// bindings. The event fires on the inner text entry, so the binding is found by
/// walking up to its container.
pub(crate) fn on_custom_property_text_commit(
    event: On<ValueChange<String>>,
    bindings: Query<&CustomPropertyBinding>,
    axis_bindings: Query<&CustomAxisBinding>,
    child_of_query: Query<&ChildOf>,
    mut commands: Commands,
) {
    if !event.is_final {
        return;
    }

    // Walk up from the committed entity to find a CustomPropertyBinding or CustomAxisBinding
    let mut current = event.source;
    for _ in 0..4 {
        let Ok(child_of) = child_of_query.get(current) else {
            break;
        };
        let parent = child_of.parent();

        // Check for direct property binding (Int/Float/String)
        if let Ok(binding) = bindings.get(parent) {
            let source = binding.source_entity;
            let name = binding.property_name.clone();
            let text = event.value.clone();
            commands.queue(move |world: &mut World| {
                // Determine current type and apply accordingly
                let Some(cp) = world.get::<CustomProperties>(source) else {
                    return;
                };
                let Some(current_val) = cp.properties.get(&name) else {
                    return;
                };
                let new_val = match current_val {
                    PropertyValue::Int(_) => PropertyValue::Int(text.parse().unwrap_or(0)),
                    PropertyValue::Float(_) => PropertyValue::Float(text.parse().unwrap_or(0.0)),
                    PropertyValue::String(_) => PropertyValue::String(text.into()),
                    other => other.clone(),
                };
                apply_custom_property_with_undo(world, source, &name, new_val);
            });
            return;
        }

        // Check for axis binding (Vec2/Vec3 component)
        if let Ok(axis) = axis_bindings.get(parent) {
            let source = axis.source_entity;
            let name = axis.property_name.clone();
            let mutate = axis.mutate;
            let new_f: f64 = event.value.parse().unwrap_or(0.0);
            commands.queue(move |world: &mut World| {
                let Some(cp) = world.get::<CustomProperties>(source) else {
                    return;
                };
                let Some(current) = cp.properties.get(&name) else {
                    return;
                };
                let mut new_val = current.clone();
                mutate(new_f, &mut new_val);
                apply_custom_property_with_undo(world, source, &name, new_val);
            });
            return;
        }

        current = parent;
    }
}

/// Handle checkbox commit for custom property booleans.
pub(crate) fn on_custom_property_checkbox_commit(
    event: On<ValueChange<bool>>,
    bindings: Query<&CustomPropertyBinding>,
    mut commands: Commands,
) {
    // This global observer sees `ValueChange<bool>` for every checkbox, so the
    // `CustomPropertyBinding` lookup self-filters to custom property fields.
    let target = event.source;
    let Ok(binding) = bindings.get(target) else {
        return;
    };
    let source = binding.source_entity;
    let name = binding.property_name.clone();
    let checked = event.value;
    // The checkbox does not self-update `Checked`; reflect the new value so the
    // box renders the change.
    if checked {
        commands.entity(target).insert(Checked);
    } else {
        commands.entity(target).remove::<Checked>();
    }
    commands.queue(move |world: &mut World| {
        apply_custom_property_with_undo(world, source, &name, PropertyValue::Bool(checked));
    });
}
