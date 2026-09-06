use bevy::feathers::controls::{ColorPlaneValue, ColorSwatchValue, SliderBaseColor};
use bevy::prelude::*;
use bevy::ui_widgets::SliderValue;

use super::{ColorPickerState, ColorSubWidget, TriggerLabel};

/// Mirror the picker's colour onto every feathers widget that shows it.
///
/// The plane takes red on its x axis and blue on its y with green as its
/// fixed channel; each slider takes its own channel plus the whole
/// colour, which is what its gradient is drawn from.
pub(super) fn update_color_picker_visuals(
    mut commands: Commands,
    pickers: Query<(Entity, &ColorPickerState), Changed<ColorPickerState>>,
    widgets: Query<(
        Entity,
        &ColorSubWidget,
        Has<ColorPlaneValue>,
        Has<ColorSwatchValue>,
        Has<SliderValue>,
    )>,
) {
    for (picker, state) in &pickers {
        let rgba = state.to_rgba();
        let color = Color::Srgba(state.to_srgba());

        for (entity, sub, has_plane, has_swatch, has_slider) in &widgets {
            if sub.picker != picker {
                continue;
            }
            let mut widget = commands.entity(entity);
            match sub.part.channel() {
                None => {
                    if has_plane {
                        widget.insert(ColorPlaneValue(Vec3::new(rgba[0], rgba[2], rgba[1])));
                    }
                    if has_swatch {
                        widget.insert(ColorSwatchValue(color));
                    }
                }
                Some(channel) => {
                    if has_slider {
                        widget.insert((SliderValue(rgba[channel]), SliderBaseColor(color)));
                    }
                }
            }
        }
    }
}

pub(super) fn update_trigger_display(
    pickers: Query<(Entity, &ColorPickerState), Changed<ColorPickerState>>,
    mut trigger_labels: Query<(&TriggerLabel, &mut Text)>,
) {
    for (picker_entity, state) in &pickers {
        let hex = state.to_hex();
        for (label, mut text) in &mut trigger_labels {
            if label.0 != picker_entity {
                continue;
            }
            **text = hex.clone();
        }
    }
}
