mod color_math;
mod input_fields;
mod setup;
mod visuals;

use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;

use color_math::{hsv_to_rgb, rgb_to_hsv};

use crate::popover::PopoverTracker;

const COLOR_PLANE_HEIGHT: f32 = 192.0;
const PREVIEW_SWATCH_SIZE: f32 = 36.0;
const SWATCH_SIZE: f32 = 16.0;
const POPOVER_WIDTH: f32 = 256.0;

pub fn plugin(app: &mut App) {
    app.add_observer(setup::handle_trigger_click)
        .add_observer(input_fields::handle_input_mode_change)
        .add_observer(on_color_plane_change)
        .add_observer(on_color_slider_change)
        .add_systems(
            Update,
            (
                setup::setup_color_picker,
                setup::setup_trigger_swatch,
                setup::setup_color_picker_content,
                setup::despawn_orphaned_color_picker_popovers,
                setup::despawn_orphaned_color_picker_roots,
                visuals::update_color_picker_visuals,
                input_fields::handle_input_field_blur,
                visuals::update_trigger_display,
                input_fields::sync_text_inputs_to_state,
            ),
        );
}

/// Which part of the picker a sub-widget edits.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum ColorPart {
    /// The plane and the swatches, which carry a whole colour rather than
    /// one channel.
    Whole,
    Red,
    Green,
    Blue,
    Alpha,
}

impl ColorPart {
    /// The index this part writes in an rgba quad, or `None` for the
    /// parts that carry a whole colour.
    pub(super) fn channel(self) -> Option<usize> {
        match self {
            ColorPart::Whole => None,
            ColorPart::Red => Some(0),
            ColorPart::Green => Some(1),
            ColorPart::Blue => Some(2),
            ColorPart::Alpha => Some(3),
        }
    }
}

/// Ties a feathers colour widget back to the picker whose state it edits.
#[derive(Component)]
pub(super) struct ColorSubWidget {
    pub(super) picker: Entity,
    pub(super) part: ColorPart,
}

/// The plane writes red on its x axis and blue on its y.
fn on_color_plane_change(
    event: On<ValueChange<Vec2>>,
    sub_widgets: Query<&ColorSubWidget>,
    mut states: Query<&mut ColorPickerState>,
    mut commands: Commands,
) {
    let Ok(sub) = sub_widgets.get(event.source) else {
        return;
    };
    let Ok(mut state) = states.get_mut(sub.picker) else {
        return;
    };
    let mut rgba = state.to_rgba();
    rgba[0] = event.value.x;
    rgba[2] = event.value.y;
    state.set_from_rgba(rgba);
    emit_color(&mut commands, sub.picker, rgba, event.is_final);
}

/// One slider per channel, read off the part the slider was spawned with.
fn on_color_slider_change(
    event: On<ValueChange<f32>>,
    sub_widgets: Query<&ColorSubWidget>,
    mut states: Query<&mut ColorPickerState>,
    mut commands: Commands,
) {
    let Ok(sub) = sub_widgets.get(event.source) else {
        return;
    };
    let Some(channel) = sub.part.channel() else {
        return;
    };
    let Ok(mut state) = states.get_mut(sub.picker) else {
        return;
    };
    let mut rgba = state.to_rgba();
    rgba[channel] = event.value;
    state.set_from_rgba(rgba);
    emit_color(&mut commands, sub.picker, rgba, event.is_final);
}

fn emit_color(commands: &mut Commands, picker: Entity, color: [f32; 4], is_final: bool) {
    commands.trigger(ColorPickerChangeEvent {
        entity: picker,
        color,
    });
    if is_final {
        commands.trigger(ColorPickerCommitEvent {
            entity: picker,
            color,
        });
    }
}

#[derive(Component)]
pub struct EditorColorPicker;

#[derive(Component, Clone)]
pub struct ColorPickerState {
    pub hue: f32,
    pub saturation: f32,
    pub brightness: f32,
    pub alpha: f32,
    pub input_mode: ColorInputMode,
}

impl Default for ColorPickerState {
    fn default() -> Self {
        Self {
            hue: 0.0,
            saturation: 0.0,
            brightness: 1.0,
            alpha: 1.0,
            input_mode: ColorInputMode::Rgb,
        }
    }
}

impl ColorPickerState {
    pub fn from_rgba(rgba: [f32; 4]) -> Self {
        let (h, s, v) = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
        Self {
            hue: h,
            saturation: s,
            brightness: v,
            alpha: rgba[3],
            input_mode: ColorInputMode::Rgb,
        }
    }

    pub fn to_rgba(&self) -> [f32; 4] {
        let (r, g, b) = hsv_to_rgb(self.hue, self.saturation, self.brightness);
        [r, g, b, self.alpha]
    }

    pub fn set_from_rgba(&mut self, rgba: [f32; 4]) {
        let (h, s, v) = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
        self.hue = h;
        self.saturation = s;
        self.brightness = v;
        self.alpha = rgba[3];
    }

    pub fn to_srgba(&self) -> Srgba {
        let rgba = self.to_rgba();
        Srgba::new(
            rgba[0].clamp(0.0, 1.0),
            rgba[1].clamp(0.0, 1.0),
            rgba[2].clamp(0.0, 1.0),
            rgba[3].clamp(0.0, 1.0),
        )
    }

    pub fn to_hex(&self) -> String {
        let rgba = self.to_rgba();
        let r = (rgba[0].clamp(0.0, 1.0) * 255.0).round() as u8;
        let g = (rgba[1].clamp(0.0, 1.0) * 255.0).round() as u8;
        let b = (rgba[2].clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("{:02X}{:02X}{:02X}", r, g, b)
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
pub enum ColorInputMode {
    Hex,
    #[default]
    Rgb,
    Hsb,
    Raw,
}

impl ColorInputMode {
    fn index(&self) -> usize {
        match self {
            Self::Hex => 0,
            Self::Rgb => 1,
            Self::Hsb => 2,
            Self::Raw => 3,
        }
    }

    fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Hex,
            2 => Self::Hsb,
            3 => Self::Raw,
            _ => Self::Rgb,
        }
    }
}

#[derive(EntityEvent)]
pub struct ColorPickerChangeEvent {
    pub entity: Entity,
    pub color: [f32; 4],
}

#[derive(EntityEvent)]
pub struct ColorPickerCommitEvent {
    pub entity: Entity,
    pub color: [f32; 4],
}

#[derive(Default)]
pub struct ColorPickerProps {
    pub color: [f32; 4],
    pub inline: bool,
}

impl ColorPickerProps {
    pub fn new() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0],
            inline: false,
        }
    }

    pub fn with_color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    pub fn inline(mut self) -> Self {
        self.inline = true;
        self
    }
}

pub fn color_picker(props: ColorPickerProps) -> impl Bundle {
    let ColorPickerProps { color, inline } = props;

    (
        EditorColorPicker,
        ColorPickerState::from_rgba(color),
        ColorPickerConfig { inline },
        PopoverTracker::default(),
        Node {
            flex_direction: FlexDirection::Column,
            ..default()
        },
    )
}

// --- Internal marker components ---

#[derive(Component)]
struct ColorPickerConfig {
    inline: bool,
}

#[derive(Component)]
struct ColorPickerTrigger(Entity);

#[derive(Component)]
struct ColorPickerPopover(Entity);

#[derive(Component)]
struct ColorPickerContent(Entity);

#[derive(Component)]
struct ColorInputRow(Entity);

#[derive(Component)]
struct TriggerSwatchConfig {
    picker: Entity,
    color: Srgba,
}

#[derive(Component)]
struct TriggerSwatch;

#[derive(Component)]
struct TriggerLabel(Entity);
