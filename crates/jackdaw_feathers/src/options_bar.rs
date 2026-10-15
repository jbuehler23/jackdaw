//! Fields for the options bar above the viewport, where a tool's settings
//! sit side by side on one wrapping row: a labelled number chip, a checkbox
//! and a line of hint text.
//!
//! A field is tagged with a component the caller chooses, which is how the
//! caller's `ValueChange` observer tells its fields from every other one.

use std::ops::Range;

use bevy::feathers::controls::FeathersCheckbox;
use bevy::feathers::theme::ThemedText;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::Checked;

use crate::number_input::{
    NumberInputPrecision, ScrubNumberInput, ScrubNumberInputValue, SoftLimit,
};
use crate::slider_row::FieldKind;
use crate::tokens;
use crate::tooltip::Tooltip;

/// Width of the value and scrub region in a [`spawn_scrub_chip`] chip.
/// Narrower than a panel row's, since several chips share one bar line.
const SCRUB_CHIP_INPUT_WIDTH: f32 = 64.0;

/// A compact inline label and value chip sized to its content: a
/// `ScrubNumberInput` tagged with `field`, sitting beside sibling chips on one
/// options-bar row. Same binding, tooltip and [`FieldKind`] contract as
/// [`crate::slider_row::spawn_slider_row`], different container: no `width: 100%`, so the bar's
/// `FlexDirection::Row` lays chips out side by side rather than one full-width
/// bar per field.
///
/// `kind` reaches the widget as its `NumberInputPrecision`, the same number
/// `FieldKind` gives a slider row's `SliderPrecision`, so a value shows the
/// same digits in either shape.
pub fn spawn_scrub_chip<C: Component>(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    tooltip: &str,
    value: f32,
    soft_range: Range<f32>,
    kind: FieldKind,
    field: C,
) {
    let chip = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_XS),
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(parent),
            Tooltip::title(label).with_description(tooltip),
            Hovered::default(),
        ))
        .id();

    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(chip),
    ));

    commands
        .spawn_scene(bsn! {
            @ScrubNumberInput
            Node {
                width: px(SCRUB_CHIP_INPUT_WIDTH),
                height: px(22.0),
                flex_shrink: 0.0,
            }
        })
        .insert((
            ScrubNumberInputValue::F32(value),
            SoftLimit::f32(soft_range),
            NumberInputPrecision(kind.precision()),
            field,
            ChildOf(chip),
        ));
}

/// A labeled native `bevy_feathers` checkbox, tagged with `field` so a caller's
/// `ValueChange<bool>` observer can tell it apart from every other checkbox in
/// the editor.
///
/// `FeathersCheckbox` does not self-manage `Checked`: this seeds the initial
/// state from `checked`, and the caller's commit handler has to insert or
/// remove it on every commit.
pub fn spawn_checkbox<C: Component>(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    checked: bool,
    field: C,
) {
    let label = label.to_string();
    let mut cb = commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text(label) ThemedText } }
    });
    cb.insert((field, ChildOf(parent)));
    if checked {
        cb.insert(Checked);
    }
}

/// A line of small secondary text, such as what a tool last did.
pub fn spawn_hint(commands: &mut Commands, parent: Entity, text: &str) {
    commands.spawn((
        Text::new(text),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(parent),
    ));
}
