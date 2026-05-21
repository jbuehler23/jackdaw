use bevy_ecs::prelude::*;
use bevy_input_focus::InputFocus;
use bevy_text::prelude::*;
use bevy_ui::prelude::*;
use bevy_ui_text_input::TextInputQueue;
use bevy_ui_text_input::actions::{TextInputAction, TextInputEdit};
use bevy_utils::prelude::*;

use super::color_math::{parse_hex, rgb_to_hsv};
use super::{
    ColorInputMode, ColorInputRow, ColorPickerChangeEvent, ColorPickerCommitEvent, ColorPickerState,
};

use crate::combobox::{ComboBoxChangeEvent, combobox_icon_with_selected};
use crate::text_edit::{EditorTextEdit, TextEditPrefix, TextEditProps, text_edit};
use crate::tokens::{TEXT_MUTED_COLOR, TEXT_SIZE};
use crate::utils::{find_ancestor, is_descendant_of};

#[derive(Component, Clone, Copy)]
pub(super) enum InputFieldKind {
    Hex,
    Red,
    Green,
    Blue,
    Hue,
    Saturation,
    Brightness,
    Alpha,
    RawRed,
    RawGreen,
    RawBlue,
}

impl InputFieldKind {
    pub(super) fn parse_and_apply(&self, text: &str, state: &mut ColorPickerState) -> bool {
        match self {
            Self::Hex => {
                let Some(rgba) = parse_hex(text) else {
                    return false;
                };
                let (h, s, v) = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
                state.hue = h;
                state.saturation = s;
                state.brightness = v;
                state.alpha = rgba[3];
                true
            }
            Self::Red | Self::Green | Self::Blue => {
                let Ok(v) = text.parse::<i32>() else {
                    return false;
                };
                let channel = (v.clamp(0, 255) as f32) / 255.0;
                let mut rgba = state.to_rgba();
                match self {
                    Self::Red => rgba[0] = channel,
                    Self::Green => rgba[1] = channel,
                    Self::Blue => rgba[2] = channel,
                    _ => unreachable!(),
                }
                let (h, s, br) = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
                state.hue = h;
                state.saturation = s;
                state.brightness = br;
                true
            }
            Self::Hue => {
                let Ok(v) = text.parse::<i32>() else {
                    return false;
                };
                state.hue = v.clamp(0, 360) as f32;
                true
            }
            Self::Saturation | Self::Brightness | Self::Alpha => {
                let Ok(v) = text.parse::<i32>() else {
                    return false;
                };
                let value = (v.clamp(0, 100) as f32) / 100.0;
                match self {
                    Self::Saturation => state.saturation = value,
                    Self::Brightness => state.brightness = value,
                    Self::Alpha => state.alpha = value,
                    _ => unreachable!(),
                }
                true
            }
            Self::RawRed | Self::RawGreen | Self::RawBlue => {
                let Ok(v) = text.parse::<f32>() else {
                    return false;
                };
                let channel = v.clamp(0.0, 100.0);
                let mut rgba = state.to_rgba();
                match self {
                    Self::RawRed => rgba[0] = channel,
                    Self::RawGreen => rgba[1] = channel,
                    Self::RawBlue => rgba[2] = channel,
                    _ => unreachable!(),
                }
                let (h, s, br) = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
                state.hue = h;
                state.saturation = s;
                state.brightness = br;
                true
            }
        }
    }

    pub(super) fn format_value(&self, state: &ColorPickerState) -> String {
        match self {
            Self::Hex => state.to_hex(),
            Self::Red | Self::Green | Self::Blue => {
                let rgba = state.to_rgba();
                let index = match self {
                    Self::Red => 0,
                    Self::Green => 1,
                    Self::Blue => 2,
                    _ => unreachable!(),
                };
                ((rgba[index].clamp(0.0, 1.0) * 255.0).round() as i32).to_string()
            }
            Self::Hue => (state.hue.round() as i32).to_string(),
            Self::Saturation => ((state.saturation * 100.0).round() as i32).to_string(),
            Self::Brightness => ((state.brightness * 100.0).round() as i32).to_string(),
            Self::Alpha => ((state.alpha * 100.0).round() as i32).to_string(),
            Self::RawRed | Self::RawGreen | Self::RawBlue => {
                let rgba = state.to_rgba();
                let index = match self {
                    Self::RawRed => 0,
                    Self::RawGreen => 1,
                    Self::RawBlue => 2,
                    _ => unreachable!(),
                };
                format!("{:.1}", rgba[index])
            }
        }
    }
}

#[derive(Component)]
pub(super) struct ColorInputField {
    pub(super) picker: Entity,
    pub(super) kind: InputFieldKind,
}

pub(super) struct InputFieldConfig {
    pub(super) kind: InputFieldKind,
    pub(super) label: &'static str,
    pub(super) min: f64,
    pub(super) max: f64,
}

pub(super) fn spawn_input_fields(
    parent: &mut ChildSpawnerCommands,
    picker_entity: Entity,
    mode: ColorInputMode,
    state: &ColorPickerState,
) {
    let fields: &[InputFieldConfig] = match mode {
        ColorInputMode::Hex => &[InputFieldConfig {
            kind: InputFieldKind::Hex,
            label: "Hex",
            min: 0.0,
            max: 0.0,
        }],
        ColorInputMode::Rgb => &[
            InputFieldConfig {
                kind: InputFieldKind::Red,
                label: "R",
                min: 0.0,
                max: 255.0,
            },
            InputFieldConfig {
                kind: InputFieldKind::Green,
                label: "G",
                min: 0.0,
                max: 255.0,
            },
            InputFieldConfig {
                kind: InputFieldKind::Blue,
                label: "B",
                min: 0.0,
                max: 255.0,
            },
        ],
        ColorInputMode::Hsb => &[
            InputFieldConfig {
                kind: InputFieldKind::Hue,
                label: "H",
                min: 0.0,
                max: 360.0,
            },
            InputFieldConfig {
                kind: InputFieldKind::Saturation,
                label: "S",
                min: 0.0,
                max: 100.0,
            },
            InputFieldConfig {
                kind: InputFieldKind::Brightness,
                label: "B",
                min: 0.0,
                max: 100.0,
            },
        ],
        ColorInputMode::Raw => &[
            InputFieldConfig {
                kind: InputFieldKind::RawRed,
                label: "R",
                min: 0.0,
                max: 100.0,
            },
            InputFieldConfig {
                kind: InputFieldKind::RawGreen,
                label: "G",
                min: 0.0,
                max: 100.0,
            },
            InputFieldConfig {
                kind: InputFieldKind::RawBlue,
                label: "B",
                min: 0.0,
                max: 100.0,
            },
        ],
    };

    for config in fields {
        spawn_single_input_field(parent, picker_entity, config, state, false);
    }

    // Alpha field (always shown)
    spawn_single_input_field(
        parent,
        picker_entity,
        &InputFieldConfig {
            kind: InputFieldKind::Alpha,
            label: "A",
            min: 0.0,
            max: 100.0,
        },
        state,
        true,
    );

    // Input mode selector
    parent
        .spawn((
            ColorInputField {
                picker: picker_entity,
                kind: InputFieldKind::Hex,
            },
            Node {
                flex_shrink: 0.0,
                ..default()
            },
        ))
        .with_child(combobox_icon_with_selected(
            vec!["Hex", "RGB", "HSB", "RAW"],
            state.input_mode.index(),
        ));
}

fn spawn_single_input_field(
    parent: &mut ChildSpawnerCommands,
    picker_entity: Entity,
    config: &InputFieldConfig,
    state: &ColorPickerState,
    fixed_width: bool,
) {
    let value = config.kind.format_value(state);
    let is_hex = matches!(config.kind, InputFieldKind::Hex);

    let mut props = TextEditProps::default().with_default_value(value);

    if is_hex {
        props = props.with_prefix(TextEditPrefix::Label {
            label: "#".to_string(),
            size: TEXT_SIZE,
            color: None,
        });
    }

    let is_raw = matches!(
        config.kind,
        InputFieldKind::RawRed | InputFieldKind::RawGreen | InputFieldKind::RawBlue
    );

    let is_alpha = matches!(config.kind, InputFieldKind::Alpha);

    if !is_hex {
        props = if is_raw {
            props.numeric_f32()
        } else {
            props.numeric_i32()
        }
        .with_min(config.min)
        .with_max(config.max)
        .drag_bottom();
        props.prefix = None;
    }

    if is_alpha {
        props = props.with_suffix("%");
    }

    let mut column_node = Node {
        flex_direction: FlexDirection::Column,
        row_gap: px(6.0),
        flex_grow: if fixed_width { 0.0 } else { 1.0 },
        flex_shrink: 1.0,
        flex_basis: px(0),
        ..default()
    };

    if fixed_width {
        column_node.width = px(48.0);
        column_node.flex_basis = Val::Auto;
    }

    parent
        .spawn((
            ColorInputField {
                picker: picker_entity,
                kind: config.kind,
            },
            column_node,
        ))
        .with_children(|col| {
            col.spawn(text_edit(props));
            col.spawn((
                Text::new(config.label),
                TextFont {
                    font_size: TEXT_SIZE,
                    ..default()
                },
                TextColor(TEXT_MUTED_COLOR.into()),
                Node {
                    align_self: AlignSelf::Center,
                    ..default()
                },
            ));
        });
}

pub(super) fn handle_input_field_blur(
    input_focus: Res<InputFocus>,
    mut last_focus: Local<Option<Entity>>,
    mut commands: Commands,
    mut pickers: Query<&mut ColorPickerState>,
    input_fields: Query<&ColorInputField>,
    text_inputs: Query<&bevy_ui_text_input::TextInputBuffer, With<EditorTextEdit>>,
    parents: Query<&ChildOf>,
) {
    let current_focus = input_focus.0;
    let previous_focus = *last_focus;
    *last_focus = current_focus;

    let Some(blurred_entity) = previous_focus else {
        return;
    };
    if current_focus == Some(blurred_entity) {
        return;
    }

    let Ok(buffer) = text_inputs.get(blurred_entity) else {
        return;
    };

    let Some((_, field)) = find_ancestor(blurred_entity, &input_fields, &parents) else {
        return;
    };

    let Ok(mut state) = pickers.get_mut(field.picker) else {
        return;
    };

    let text = buffer.get_text();
    if text.is_empty() {
        return;
    }

    if field.kind.parse_and_apply(&text, &mut state) {
        commands.trigger(ColorPickerChangeEvent {
            entity: field.picker,
            color: state.to_rgba(),
        });
        commands.trigger(ColorPickerCommitEvent {
            entity: field.picker,
            color: state.to_rgba(),
        });
    }
}

pub(super) fn sync_text_inputs_to_state(
    input_focus: Res<InputFocus>,
    pickers: Query<(Entity, &ColorPickerState), Changed<ColorPickerState>>,
    input_fields: Query<(Entity, &ColorInputField)>,
    mut text_inputs: Query<(Entity, &mut TextInputQueue), With<EditorTextEdit>>,
    parents: Query<&ChildOf>,
) {
    for (picker_entity, state) in &pickers {
        for (field_entity, field) in &input_fields {
            if field.picker != picker_entity {
                continue;
            }

            let text = field.kind.format_value(state);

            for (text_input_entity, mut queue) in &mut text_inputs {
                if input_focus.0 == Some(text_input_entity) {
                    continue;
                }

                if is_descendant_of(text_input_entity, field_entity, &parents) {
                    queue.add(TextInputAction::Edit(TextInputEdit::SelectAll));
                    queue.add(TextInputAction::Edit(TextInputEdit::Paste(text.clone())));
                }
            }
        }
    }
}

pub(super) fn handle_input_mode_change(
    trigger: On<ComboBoxChangeEvent>,
    mut commands: Commands,
    input_fields: Query<&ColorInputField>,
    mut pickers: Query<&mut ColorPickerState>,
    input_rows: Query<(Entity, &ColorInputRow, &Children)>,
    parents: Query<&ChildOf>,
) {
    let Some((_, field)) = find_ancestor(trigger.entity, &input_fields, &parents) else {
        return;
    };

    if !matches!(field.kind, InputFieldKind::Hex) {
        return;
    }

    let new_mode = ColorInputMode::from_index(trigger.selected);
    let picker_entity = field.picker;

    let Ok(mut state) = pickers.get_mut(picker_entity) else {
        return;
    };

    if state.input_mode == new_mode {
        return;
    }

    state.input_mode = new_mode;

    for (row_entity, row, children) in &input_rows {
        if row.0 != picker_entity {
            continue;
        }

        for child in children.iter() {
            commands.entity(child).try_despawn();
        }

        commands.entity(row_entity).with_children(|parent| {
            spawn_input_fields(parent, picker_entity, new_mode, &state);
        });

        break;
    }
}
