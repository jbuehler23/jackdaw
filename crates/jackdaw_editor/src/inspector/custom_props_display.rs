use crate::commands::{CommandHistory, EditorCommand};
use crate::custom_properties::{CustomProperties, PropertyValue, SetCustomProperties};

use bevy::ecs::system::SystemState;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::observe;
use jackdaw_feathers::combobox::{ComboBoxSelectedIndex, combobox_with_selected};
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_feathers::{
    checkbox::{CheckboxCommitEvent, CheckboxProps, checkbox},
    color_picker::{ColorPickerCommitEvent, ColorPickerProps, color_picker},
    icons::Icon,
    text_edit::{self, TextEditCommitEvent, TextEditProps, TextEditValue},
    tokens,
};

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
    // Render each property row based on its variant type
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
                ChildOf(parent),
            ))
            .id();

        // Property name label
        commands.spawn((
            Text::new(format!("{}:", prop_name)),
            TextFont {
                font: editor_font.clone(),
                font_size: tokens::FONT_SM,
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
                let checked = *val;
                commands.spawn((
                    checkbox(
                        CheckboxProps::new("").checked(checked),
                        editor_font,
                        icon_font,
                    ),
                    CustomPropertyBinding {
                        source_entity,
                        property_name: name,
                    },
                    ChildOf(row),
                ));
            }
            PropertyValue::Int(val) => {
                commands.spawn((
                    text_edit::text_edit(
                        TextEditProps::default()
                            .numeric_f32()
                            .grow()
                            .with_default_value((*val).to_string()),
                    ),
                    CustomPropertyBinding {
                        source_entity,
                        property_name: name,
                    },
                    ChildOf(row),
                ));
            }
            PropertyValue::Float(val) => {
                commands.spawn((
                    text_edit::text_edit(
                        TextEditProps::default()
                            .numeric_f32()
                            .grow()
                            .with_default_value(val.to_string()),
                    ),
                    CustomPropertyBinding {
                        source_entity,
                        property_name: name,
                    },
                    ChildOf(row),
                ));
            }
            PropertyValue::String(val) => {
                commands.spawn((
                    text_edit::text_edit(
                        TextEditProps::default()
                            .grow()
                            .with_default_value(val.to_string())
                            .allow_empty(),
                    ),
                    CustomPropertyBinding {
                        source_entity,
                        property_name: name,
                    },
                    ChildOf(row),
                ));
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
                let n = name.clone();
                commands
                    .spawn((
                        color_picker(ColorPickerProps::new().with_color(rgba)),
                        ChildOf(row),
                    ))
                    .observe(
                        move |event: On<ColorPickerCommitEvent>, mut commands: Commands| {
                            let color = event.color;
                            let n = n.clone();
                            commands.queue(move |world: &mut World| {
                                let new_color =
                                    Color::srgba(color[0], color[1], color[2], color[3]);
                                apply_custom_property_with_undo(
                                    world,
                                    source_entity,
                                    &n,
                                    PropertyValue::Color(new_color),
                                );
                            });
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
                font: icon_font.clone(),
                font_size: tokens::FONT_SM,
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
    spawn_add_property_row(commands, parent, source_entity, editor_font, icon_font);
}

/// Marker that links a custom property axis input to its property name and mutation function.
#[derive(Component)]
pub(super) struct CustomAxisBinding {
    source_entity: Entity,
    property_name: String,
    mutate: fn(f64, &mut PropertyValue),
}

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
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(label_color),
        Node {
            flex_shrink: 0.0,
            ..Default::default()
        },
        ChildOf(parent),
    ));

    commands.spawn((
        text_edit::text_edit(
            TextEditProps::default()
                .numeric_f32()
                .grow()
                .with_default_value(value.to_string()),
        ),
        CustomAxisBinding {
            source_entity,
            property_name,
            mutate,
        },
        ChildOf(parent),
    ));
}

fn spawn_add_property_row(
    commands: &mut Commands,
    parent: Entity,
    source_entity: Entity,
    _editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) {
    let row = commands
        .spawn((
            CustomPropertyAddRow,
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
    commands.spawn((
        CustomPropertyNameInput,
        text_edit::text_edit(
            TextEditProps::default()
                .grow()
                .with_placeholder("name...")
                .allow_empty(),
        ),
        ChildOf(row),
    ));

    // Type selector ComboBox
    let type_names: Vec<String> = PropertyValue::all_type_names()
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    commands.spawn((
        CustomPropertyTypeSelector,
        combobox_with_selected(type_names, 2), // default to "Float"
        ChildOf(row),
    ));

    // Confirm button
    let font = icon_font.clone();
    commands.spawn((
        Text::new(String::from(Icon::Plus.unicode())),
        TextFont {
            font,
            font_size: tokens::FONT_SM,
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

/// Read the name input and type selector, then add a new property.
fn add_custom_property_from_ui(
    In(source_entity): In<Entity>,
    world: &mut World,
    text_edit: &mut SystemState<Single<&TextEditValue, With<CustomPropertyNameInput>>>,
    combo_box_index: &mut SystemState<
        Single<&ComboBoxSelectedIndex, With<CustomPropertyTypeSelector>>,
    >,
) {
    // Read the name input value
    let name = {
        let input = *text_edit.get(world);
        let name = input.0.trim().to_string();
        if name.is_empty() {
            return;
        }
        name
    };

    // Read the type selector
    let type_name = {
        let index = *combo_box_index.get(world);
        let all_types = PropertyValue::all_type_names();
        let idx = index.0.min(all_types.len().saturating_sub(1));
        all_types[idx].to_string()
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

/// Handle `TextEditCommitEvent` for custom property numeric/string fields + axis bindings.
pub(crate) fn on_custom_property_text_commit(
    event: On<TextEditCommitEvent>,
    bindings: Query<&CustomPropertyBinding>,
    axis_bindings: Query<&CustomAxisBinding>,
    child_of_query: Query<&ChildOf>,
    mut commands: Commands,
) {
    // Walk up from the committed entity to find a CustomPropertyBinding or CustomAxisBinding
    let mut current = event.entity;
    for _ in 0..4 {
        let Ok(child_of) = child_of_query.get(current) else {
            break;
        };
        let parent = child_of.parent();

        // Check for direct property binding (Int/Float/String)
        if let Ok(binding) = bindings.get(parent) {
            let source = binding.source_entity;
            let name = binding.property_name.clone();
            let text = event.text.clone();
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
            let new_f: f64 = event.text.parse().unwrap_or(0.0);
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
    event: On<CheckboxCommitEvent>,
    bindings: Query<&CustomPropertyBinding>,
    mut commands: Commands,
) {
    let Ok(binding) = bindings.get(event.entity) else {
        return;
    };
    let source = binding.source_entity;
    let name = binding.property_name.clone();
    let checked = event.checked;
    commands.queue(move |world: &mut World| {
        apply_custom_property_with_undo(world, source, &name, PropertyValue::Bool(checked));
    });
}
