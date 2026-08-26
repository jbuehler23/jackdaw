//! Drag-scrub number input widget.
//!
//! [`ScrubNumberInput`] adds an invisible scrubber overlay on top of a
//! numeric text entry. Dragging the overlay emits [`ValueChange<T>`] with
//! `is_final == false`, governed by [`SoftLimit`], [`HardLimit`],
//! [`NumberInputPrecision`], and [`NumberInputStep`]. Releasing, Enter, or
//! blur emits a final [`ValueChange<T>`] with `is_final == true`.
//!
//! The public types are prefixed `Scrub*`, and the value/format enums are
//! named to avoid colliding with the names `bevy::feathers` exports when
//! both are imported together.

use std::{f32::consts::PI, ops::Range};

use bevy::app::{Plugin, PreUpdate, PropagateOver};
use bevy::color::Color;
use bevy::ecs::{
    change_detection::DetectChanges,
    component::Component,
    entity::Entity,
    event::EntityEvent,
    hierarchy::{ChildOf, Children},
    lifecycle::{Add, Insert, Remove},
    observer::On,
    query::{Has, With},
    reflect::ReflectComponent,
    schedule::IntoScheduleConfigs,
    system::{Commands, Query, Res, ResMut},
    world::World,
};
use bevy::input::{
    ButtonInput,
    keyboard::{Key, KeyCode, KeyboardInput},
};
use bevy::input_focus::{
    AcquireFocus, FocusCause, FocusGained, FocusLost, FocusedInput, InputFocus,
};
use bevy::log::{warn, warn_once};
use bevy::math::ops;
use bevy::picking::{
    PickingSystems,
    events::{Cancel, Drag, DragEnd, DragStart, Pointer, Press, Release},
    hover::Hovered,
    pointer::PointerButton,
};
use bevy::reflect::Reflect;
use bevy::reflect::std_traits::ReflectDefault;
use bevy::scene::prelude::*;
use bevy::text::{
    EditableText, EditableTextFilter, FontSourceTemplate, Justify, LineHeight, TextEdit, TextFont,
    TextLayout,
};
use bevy::ui::{
    AlignItems, AlignSelf, BackgroundGradient, ColorStop, ComputedNode, ComputedUiRenderTargetInfo,
    Display, Gradient, InteractionDisabled, InterpolationColorSpace, JustifyContent,
    LinearGradient, Node, PositionType, UiGlobalTransform, UiRect, UiScale, percent, px,
    widget::{Text, TextScroll},
};
use bevy::ui_widgets::ValueChange;

use bevy::feathers::{
    constants::{fonts, size},
    controls::{FeathersTextInput, FeathersTextInputContainer},
    cursor::EntityCursor,
    rounded_corners::RoundedCorners,
    theme::{ThemeBackgroundColor, ThemeBorderColor, ThemeTextColor, ThemeToken, UiTheme},
    tokens,
};

/// Threshold used to distinguish between a "click" and a "drag" gesture.
const DRAG_THRESHOLD_DISTANCE: f32 = 0.5;

const BASE_DRAG_SPEED: f64 = 0.01f64;

/// Widget that permits text entry of floating-point numbers. This widget implements two-way
/// synchronization:
/// * it emits values (via a [`ValueChange<T>`]) event as the user types or drags.
///   The type of ``T`` will be ``f32``, ``f64``, ``i32``, or ``i64`` depending on the
///   [`ScrubNumberInputValue`] component variant.
/// * it listens for the insertion of the [`ScrubNumberInputValue`] component, and replaces
///   the contents of the text buffer based on the value in that event.
///
/// This is spawnable by inheriting it as a "scene component" with optional [`ScrubNumberInputProps`].
///
/// To avoid excessive updating, you should only update the number value when there is an actual
/// change, that is, when the new value is different from the current value.
///
/// In most cases, the actual source of truth for the numeric value will be external, that is,
/// some property in an app-specific data structure. It's the responsibility of the app to
/// synchronize this value with the [`ScrubNumberInput`] widget in both directions:
/// * When a [`ValueChange`] event is received, update the app-specific property.
/// * When the app-specific property changes - either in response to a [`ValueChange`] event, or
///   because of some other action, insert a [`ScrubNumberInputValue`] component to update the
///   displayed value.
///
/// The `is_final` boolean in [`ValueChange`] is set to false while dragging, however you should
/// still update the widget in response to these events, as otherwise the user won't be able to
/// see the updated value.
///
/// Additional components can be inserted into this widget to customize the behavior: see
/// [`SoftLimit`], [`HardLimit`], [`NumberInputPrecision`], and [`NumberInputStep`].
///
/// Not built on `bevy::feathers`' own `FeathersNumberInput`: that widget is text-entry only
/// (no drag-scrub, no range/limit components), and `FeathersSlider` is a click-anywhere track
/// slider with no combined numeric text box, so neither covers this widget's contract.
///
/// Domain behavior riding on top (limits, precision, tooltips, field bindings) is attached as
/// separate components plus observers at the widget's entity boundary rather than folded into
/// its scene.
#[derive(SceneComponent, Default, Clone, Reflect)]
#[scene(ScrubNumberInputProps)]
#[reflect(Component, Default, Clone)]
#[require(ScrubNumberInputValue)]
pub struct ScrubNumberInput;

/// Props used to construct a [`ScrubNumberInput`] scene.
pub struct ScrubNumberInputProps {
    /// The "sigil" is a colored strip along the left edge of the input, which is used to
    /// distinguish between different axes. The default is transparent (no sigil).
    pub sigil_color: ThemeToken,
    /// A caption to be placed on the left side of the input, next to the colored stripe.
    /// Usually one of "X", "Y" or "Z".
    pub label_text: Option<&'static str>,
}

impl Default for ScrubNumberInputProps {
    fn default() -> Self {
        Self {
            sigil_color: tokens::TEXT_INPUT_BG,
            label_text: None,
        }
    }
}

impl ScrubNumberInput {
    fn scene(props: ScrubNumberInputProps) -> impl Scene {
        bsn! {
            @FeathersTextInputContainer
            Node {
                column_gap: px(0),
                border: UiRect {
                    left: px(if props.label_text.is_some() { 3.0 } else { 0.0 }),
                },
                padding: UiRect {
                    left: px(0.0),
                    right: px(0.0),
                },
            }
            ThemeBorderColor({props.sigil_color})
            ScrubNumberInput
            on(number_input_on_insert_value)
            on(number_input_on_insert_disabled)
            on(number_input_on_remove_disabled)
            Children [
                {
                    // Label section
                    props.label_text.map(|text| {
                        bsn_list!(
                            Node {
                                display: Display::Flex,
                                align_items: AlignItems::Center,
                                align_self: AlignSelf::Stretch,
                                justify_content: JustifyContent::Center,
                                padding: UiRect::axes(px(6), px(0)),
                            }
                            ThemeBackgroundColor(tokens::TEXT_INPUT_LABEL_BG)
                            Children [
                                Text(text)
                                TextFont {
                                    font: FontSourceTemplate::Handle(fonts::REGULAR),
                                    font_size: size::COMPACT_FONT,
                                }
                                PropagateOver<TextFont>
                                ThemeTextColor(tokens::TEXT_INPUT_TEXT)
                            ]
                        )
                    })
                },

                (
                    // The editable text entity
                    @FeathersTextInput {
                        @max_characters: 20usize,
                    }
                    Node {
                        flex_grow: 1.0,
                        align_items: AlignItems::Center,
                        align_self: AlignSelf::Stretch,
                        border_radius: {
                            if props.label_text.is_some() {
                                RoundedCorners::Right.to_border_radius(4.0)
                            } else {
                                RoundedCorners::All.to_border_radius(4.0)
                            }
                        },
                    }
                    Hovered
                    EditableTextFilter::new(|c| {
                        c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')
                    })
                    template_value(LineHeight::Px(24.0)) // TODO: Make const for this
                    TextLayout {
                        justify: Justify::Center,
                    }
                    ThemeTextColor(tokens::TEXT_INPUT_TEXT)
                    // Use a gradient to draw the moving bar, this lets us round corners
                    BackgroundGradient(vec![Gradient::Linear(LinearGradient {
                        angle: PI * 0.5,
                        stops: vec![
                            ColorStop::new(Color::WHITE, percent(0)),
                            ColorStop::new(Color::WHITE, percent(50)),
                            ColorStop::new(Color::NONE, percent(50)),
                            ColorStop::new(Color::NONE, percent(100)),
                        ],
                        color_space: InterpolationColorSpace::Srgba,
                    })])
                    EntityCursor::System(bevy::window::SystemCursorIcon::ColResize)
                    on(number_input_init)
                    on(number_input_on_enter_key)
                    on(number_input_on_focus_gained)
                    on(number_input_on_focus_lost)
                    on(number_input_hovered)
                    Children [
                        (
                            // Invisible child on top of input field which intercepts drag
                            // events (conditionally) and handles scrubbing gestures.
                            ScrubberDragState
                            Node {
                                position_type: PositionType::Absolute,
                                left: px(0),
                                top: px(0),
                                bottom: px(0),
                                right: px(0),
                            }
                            on(scrubber_on_acquire_focus)
                            on(scrubber_on_press)
                            on(scrubber_on_release)
                            on(scrubber_on_drag_start)
                            on(scrubber_on_drag)
                            on(scrubber_on_drag_end)
                            on(scrubber_on_drag_cancel)
                        ),
                    ]
                ),
            ]
        }
    }
}

/// Used to indicate what format of numbers we are editing. This affects the type
/// of [`ValueChange`] event that is emitted.
#[derive(Default, Clone, Copy, Reflect)]
pub enum ScrubNumberFormat {
    /// A 32-bit float
    #[default]
    F32,
    /// A 64-bit float
    F64,
    /// A 32-bit integer
    I32,
    /// A 64-bit integer
    I64,
}

/// Represents numbers in different formats.
#[derive(Component, Debug, PartialEq, Clone, Copy, Reflect)]
#[component(immutable)]
pub enum ScrubNumberInputValue {
    /// An `f32` value
    F32(f32),
    /// An `f64` value
    F64(f64),
    /// An `i32` value
    I32(i32),
    /// An `i64` value
    I64(i64),
}

impl core::fmt::Display for ScrubNumberInputValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ScrubNumberInputValue::F32(v) => write!(f, "{}", v),
            ScrubNumberInputValue::F64(v) => write!(f, "{}", v),
            ScrubNumberInputValue::I32(v) => write!(f, "{}", v),
            ScrubNumberInputValue::I64(v) => write!(f, "{}", v),
        }
    }
}

impl ScrubNumberInputValue {
    fn format(&self) -> ScrubNumberFormat {
        match self {
            Self::F32(_) => ScrubNumberFormat::F32,
            Self::F64(_) => ScrubNumberFormat::F64,
            Self::I32(_) => ScrubNumberFormat::I32,
            Self::I64(_) => ScrubNumberFormat::I64,
        }
    }

    fn parse_from(value: String, fmt: ScrubNumberFormat) -> Result<Self, String> {
        match fmt {
            ScrubNumberFormat::F32 => value
                .parse::<f32>()
                .map(ScrubNumberInputValue::F32)
                .map_err(|_| format!("Could not parse '{}' as f32", value)),
            ScrubNumberFormat::F64 => value
                .parse::<f64>()
                .map(ScrubNumberInputValue::F64)
                .map_err(|_| format!("Could not parse '{}' as f64", value)),
            ScrubNumberFormat::I32 => value
                .parse::<i32>()
                .map(ScrubNumberInputValue::I32)
                .map_err(|_| format!("Could not parse '{}' as i32", value)),
            ScrubNumberFormat::I64 => value
                .parse::<i64>()
                .map(ScrubNumberInputValue::I64)
                .map_err(|_| format!("Could not parse '{}' as i64", value)),
        }
    }

    /// Offset this value by `delta` (in value units), preserving the variant.
    fn offset_by(self, delta: f64) -> Self {
        match self {
            ScrubNumberInputValue::F32(v) => ScrubNumberInputValue::F32(v + delta as f32),
            ScrubNumberInputValue::F64(v) => ScrubNumberInputValue::F64(v + delta),
            ScrubNumberInputValue::I32(v) => {
                ScrubNumberInputValue::I32(v.saturating_add(delta.round() as i32))
            }
            ScrubNumberInputValue::I64(v) => {
                ScrubNumberInputValue::I64(v.saturating_add(delta.round() as i64))
            }
        }
    }

    fn as_f64(&self) -> f64 {
        match *self {
            ScrubNumberInputValue::F32(v) => v as f64,
            ScrubNumberInputValue::F64(v) => v,
            ScrubNumberInputValue::I32(v) => v as f64,
            ScrubNumberInputValue::I64(v) => v as f64,
        }
    }
}

impl Default for ScrubNumberInputValue {
    fn default() -> Self {
        Self::F32(0.0)
    }
}

/// Represents numeric limits in different number formats.
#[derive(Debug, PartialEq, Clone, Reflect)]
pub enum NumberInputRange {
    /// An 'f32' range.
    F32(Range<f32>),
    /// An 'f64' range.
    F64(Range<f64>),
    /// An 'i32' range.
    I32(Range<i32>),
    /// An 'i64' range.
    I64(Range<i64>),
}

impl NumberInputRange {
    /// Clamp a numeric value of varying type to be within this range.
    pub fn clamp(&self, n: ScrubNumberInputValue) -> ScrubNumberInputValue {
        match (self, n) {
            (Self::F32(r), ScrubNumberInputValue::F32(v)) => {
                ScrubNumberInputValue::F32(v.clamp(r.start, r.end))
            }
            (Self::F64(r), ScrubNumberInputValue::F64(v)) => {
                ScrubNumberInputValue::F64(v.clamp(r.start, r.end))
            }
            (Self::I32(r), ScrubNumberInputValue::I32(v)) => {
                ScrubNumberInputValue::I32(v.clamp(r.start, r.end))
            }
            (Self::I64(r), ScrubNumberInputValue::I64(v)) => {
                ScrubNumberInputValue::I64(v.clamp(r.start, r.end))
            }
            (range, value) => {
                warn_once!("Number input range type mismatch: {range:?} {value:?}");
                n
            }
        }
    }

    /// Compute the position of the thumb on the slide bar, as a value between 0 and 1, taking
    /// into account the proportion of the value between the minimum and maximum limits.
    pub fn thumb_position(&self, value: ScrubNumberInputValue) -> f32 {
        match (self, value) {
            (Self::F32(range), ScrubNumberInputValue::F32(n)) => {
                if range.end > range.start {
                    (n - range.start) / (range.end - range.start)
                } else {
                    0.5
                }
            }

            (Self::F64(range), ScrubNumberInputValue::F64(n)) => {
                if range.end > range.start {
                    ((n - range.start) / (range.end - range.start)) as f32
                } else {
                    0.5
                }
            }

            (Self::I32(range), ScrubNumberInputValue::I32(n)) => {
                if range.end > range.start {
                    (n - range.start) as f32 / (range.end - range.start) as f32
                } else {
                    0.5
                }
            }

            (Self::I64(range), ScrubNumberInputValue::I64(n)) => {
                if range.end > range.start {
                    (n - range.start) as f32 / (range.end - range.start) as f32
                } else {
                    0.5
                }
            }

            (range, value) => {
                warn_once!("Number input range type mismatch: {range:?} {value:?}");
                0.5
            }
        }
    }
}

impl Default for NumberInputRange {
    fn default() -> Self {
        Self::F32(0.0..0.0)
    }
}

/// A soft limit represents the range of values that can be reached via dragging. Values outside
/// this range can still be entered by typing.
#[derive(Component, Default, Clone, Reflect)]
pub struct SoftLimit(pub NumberInputRange);

impl SoftLimit {
    /// Create a [`SoftLimit`] for `f32` values.
    pub fn f32(range: Range<f32>) -> Self {
        Self(NumberInputRange::F32(range))
    }

    /// Create a [`SoftLimit`] for `f64` values.
    pub fn f64(range: Range<f64>) -> Self {
        Self(NumberInputRange::F64(range))
    }

    /// Create a [`SoftLimit`] for `i32` values.
    pub fn i32(range: Range<i32>) -> Self {
        Self(NumberInputRange::I32(range))
    }

    /// Create a [`SoftLimit`] for `i64` values.
    pub fn i64(range: Range<i64>) -> Self {
        Self(NumberInputRange::I64(range))
    }
}

/// A hard limit represents an absolute constraint on the value. Values outside this range will
/// be clamped within the range.
// Note: Similar in concept to `SliderRange`, but the latter only handles f32s.
#[derive(Component, Default, Clone, Reflect)]
pub struct HardLimit(pub NumberInputRange);

impl HardLimit {
    /// Create a [`HardLimit`] for `f32` values.
    pub fn f32(range: Range<f32>) -> Self {
        Self(NumberInputRange::F32(range))
    }

    /// Create a [`HardLimit`] for `f64` values.
    pub fn f64(range: Range<f64>) -> Self {
        Self(NumberInputRange::F64(range))
    }

    /// Create a [`HardLimit`] for `i32` values.
    pub fn i32(range: Range<i32>) -> Self {
        Self(NumberInputRange::I32(range))
    }

    /// Create a [`HardLimit`] for `i64` values.
    pub fn i64(range: Range<i64>) -> Self {
        Self(NumberInputRange::I64(range))
    }
}

/// A component which controls the rounding of the number value during dragging. This is also used
/// as a heuristic to determine drag speed when there is no soft limit or step size specified.
///
/// Stepping is not affected, although presumably the step size will be an integer multiple of the
/// rounding factor. This also doesn't prevent the edited value from being set to non-rounded values
/// by other means, such as manually entering digits via a numeric input field.
///
/// The value in this component represents the number of decimal places of desired precision, so a
/// value of 2 would round to the nearest 1/100th. A value of -3 would round to the nearest
/// thousand.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component, Default)]
pub struct NumberInputPrecision(pub i32);

impl NumberInputPrecision {
    fn round_f32(&self, value: f32) -> f32 {
        let factor = ops::powf(10.0_f32, self.0 as f32);
        (value * factor).round() / factor
    }

    fn round_f64(&self, value: f64) -> f64 {
        let factor = f64::powf(10.0_f64, self.0 as f64);
        (value * factor).round() / factor
    }

    fn round(&self, value: ScrubNumberInputValue) -> ScrubNumberInputValue {
        match value {
            ScrubNumberInputValue::F32(v) => ScrubNumberInputValue::F32(self.round_f32(v)),
            ScrubNumberInputValue::F64(v) => ScrubNumberInputValue::F64(self.round_f64(v)),
            // Decimal-place rounding only affects integers at negative precision
            // (round to 10/100/...); left as identity for now.
            other => other,
        }
    }
}

/// The digits a field shows for `value`.
///
/// A fractional value is shown to its field's own precision rather than to
/// `f32`'s full decimal expansion: a value arriving from a scroll-wheel resize
/// or a scaled drag carries float noise several places down (`1.889105`).
/// Whole-number kinds pass a precision of 0 and show no decimal point.
///
/// Shared by the initial spawn and every later re-insert, so a field's digits
/// do not depend on which of the two last wrote them.
pub(crate) fn display_digits(
    value: &ScrubNumberInputValue,
    precision: Option<&NumberInputPrecision>,
) -> String {
    let Some(NumberInputPrecision(places)) = precision else {
        return value.to_string();
    };
    let places = (*places).max(0) as usize;
    match value {
        ScrubNumberInputValue::F32(v) => format!("{v:.places$}"),
        ScrubNumberInputValue::F64(v) => format!("{v:.places$}"),
        // Integer kinds have no decimals to trim either way.
        other => other.to_string(),
    }
}

impl Default for NumberInputPrecision {
    fn default() -> Self {
        Self(2)
    }
}

/// A component which controls the step size when incrementing or decrementing the value.
/// This also is used as a heuristic to determine drag speed when there is no soft limit present.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component, Default)]
pub struct NumberInputStep(pub f64);

impl Default for NumberInputStep {
    fn default() -> Self {
        Self(1.0f64)
    }
}

/// Component used to manage the state of a number during dragging ("scrubbing").
#[derive(Component, Default, Clone, Reflect)]
#[reflect(Component)]
struct ScrubberDragState {
    /// Whether the input is currently being dragged.
    dragging: bool,

    /// Conversion factor from pixels dragged to value
    drag_speed: f64,

    /// Similar to drag distance in the drag event, but includes scaling caused by modifier keys.
    value_offset: f64,

    /// The maximum absolute distance during the drag - used to detect click vs drag gesture.
    max_distance: f32,

    /// The value of the input when dragging started.
    base_value: ScrubNumberInputValue,
}

/// Observer which sets the text content of the field when the number value component changes.
fn number_input_on_insert_value(
    update: On<Insert, ScrubNumberInputValue>,
    q_children: Query<&Children>,
    q_number_input: Query<
        (
            &ScrubNumberInputValue,
            Option<&SoftLimit>,
            Option<&HardLimit>,
            Option<&NumberInputPrecision>,
        ),
        With<ScrubNumberInput>,
    >,
    mut q_text_input: Query<(&mut EditableText, &mut BackgroundGradient)>,
    input_focus: Res<InputFocus>,
) {
    let text_input_id = q_children
        .iter_descendants(update.event_target())
        .find(|e| q_text_input.contains(*e));

    if let Ok((&input_value, soft_limit, hard_limit, precision)) =
        q_number_input.get(update.event_target())
        && let Some(text_id) = text_input_id
        // Never overwrite the field while it is focused, since a focused
        // field owns its own text and the user may be typing or selecting.
        // An external value re-insert, such as an inspector refresh where
        // f32->f64 formatting makes the value's `to_string` differ from the
        // shown text, must not run SelectAll+Insert and clobber the
        // selection. Scrub drags operate only on an unfocused field; see
        // `scrubber_on_drag_start`.
        && input_focus.get() != Some(text_id)
    {
        let clamped_value = match hard_limit {
            Some(limit) => limit.0.clamp(input_value),
            None => input_value,
        };
        let (mut editable_text, mut gradient) = q_text_input.get_mut(text_id).unwrap();
        let new_digits = display_digits(&clamped_value, precision);
        if editable_text.value().to_string() != new_digits {
            editable_text.queue_edit(TextEdit::SelectAll);
            editable_text.queue_edit(TextEdit::Insert(new_digits.into()));
        }

        update_slider_pos(&clamped_value, soft_limit, &mut gradient);
    }
}

/// Observer changes the colors based on disabled status.
fn number_input_on_insert_disabled(
    insert: On<Insert, InteractionDisabled>,
    q_children: Query<&Children>,
    q_number_input: Query<Has<InteractionDisabled>, With<ScrubNumberInput>>,
    mut q_text_input: Query<(&Hovered, &mut BackgroundGradient)>,
    theme: Res<UiTheme>,
    input_focus: Res<InputFocus>,
    mut commands: Commands,
) {
    let text_input_id = q_children
        .iter_descendants(insert.event_target())
        .find(|e| q_text_input.contains(*e));

    if let Some(text_id) = text_input_id
        && let Ok((&Hovered(hovered), mut gradient)) = q_text_input.get_mut(text_id)
        && let Ok(is_disabled) = q_number_input.get(insert.event_target())
    {
        set_slidebar_styles(
            text_id,
            &theme,
            is_disabled,
            false,
            hovered,
            input_focus.get() == Some(text_id),
            &mut gradient,
            &mut commands,
        );
    }
}

/// Observer changes the colors based on disabled status.
fn number_input_on_remove_disabled(
    remove: On<Remove, InteractionDisabled>,
    q_children: Query<&Children>,
    q_number_input: Query<Has<InteractionDisabled>, With<ScrubNumberInput>>,
    mut q_text_input: Query<(&Hovered, &mut BackgroundGradient)>,
    theme: Res<UiTheme>,
    input_focus: Res<InputFocus>,
    mut commands: Commands,
) {
    let text_input_id = q_children
        .iter_descendants(remove.event_target())
        .find(|e| q_text_input.contains(*e));

    if let Some(text_id) = text_input_id
        && let Ok((&Hovered(hovered), mut gradient)) = q_text_input.get_mut(text_id)
        && let Ok(is_disabled) = q_number_input.get(remove.event_target())
    {
        set_slidebar_styles(
            text_id,
            &theme,
            is_disabled,
            false,
            hovered,
            input_focus.get() == Some(text_id),
            &mut gradient,
            &mut commands,
        );
    }
}

/// Observer which initializes the text edit once it has completed spawning.
fn number_input_init(
    insert: On<Add, EditableText>,
    q_parent: Query<&ChildOf>,
    q_number_input: Query<
        (
            &ScrubNumberInputValue,
            Option<&SoftLimit>,
            Option<&NumberInputPrecision>,
            Has<InteractionDisabled>,
        ),
        With<ScrubNumberInput>,
    >,
    mut q_text_input: Query<(&mut EditableText, &Hovered, &mut BackgroundGradient)>,
    theme: Res<UiTheme>,
    input_focus: Res<InputFocus>,
    mut commands: Commands,
) {
    let text_id = insert.event_target();
    if let Ok((mut editable_text, &Hovered(hovered), mut gradient)) = q_text_input.get_mut(text_id)
        && let Ok(&ChildOf(root_id)) = q_parent.get(text_id)
        && let Ok((input_value, limit, precision, is_disabled)) = q_number_input.get(root_id)
    {
        let new_digits = display_digits(input_value, precision);
        let old_digits = editable_text.value().to_string();
        if old_digits != new_digits {
            editable_text.queue_edit(TextEdit::SelectAll);
            editable_text.queue_edit(TextEdit::Insert(new_digits.into()));
        }

        update_slider_pos(input_value, limit, &mut gradient);
        set_slidebar_styles(
            text_id,
            &theme,
            is_disabled,
            false,
            hovered,
            input_focus.get() == Some(text_id),
            &mut gradient,
            &mut commands,
        );
    }
}

/// Observer which looks for changes in the hover state.
fn number_input_hovered(
    insert: On<Insert, Hovered>,
    q_parent: Query<&ChildOf>,
    q_number_input: Query<Has<InteractionDisabled>, With<ScrubNumberInput>>,
    mut q_text_input: Query<(&Hovered, &mut BackgroundGradient)>,
    theme: Res<UiTheme>,
    input_focus: Res<InputFocus>,
    mut commands: Commands,
) {
    let text_id = insert.event_target();
    if let Ok((&Hovered(hovered), mut gradient)) = q_text_input.get_mut(text_id)
        && let Ok(&ChildOf(root_id)) = q_parent.get(text_id)
        && let Ok(is_disabled) = q_number_input.get(root_id)
    {
        set_slidebar_styles(
            text_id,
            &theme,
            is_disabled,
            false,
            hovered,
            input_focus.get() == Some(text_id),
            &mut gradient,
            &mut commands,
        );

        if input_focus.get() == Some(text_id) {
            commands
                .entity(text_id)
                .insert(EntityCursor::System(bevy::window::SystemCursorIcon::Text));
        }
    }
}

fn number_input_on_enter_key(
    key_input: On<FocusedInput<KeyboardInput>>,
    q_parent: Query<&ChildOf>,
    q_number_input: Query<(&ScrubNumberInputValue, Option<&HardLimit>), With<ScrubNumberInput>>,
    q_text_input: Query<&EditableText>,
    mut commands: Commands,
) {
    if key_input.input.key_code != KeyCode::Enter {
        return;
    }

    if let Ok(&ChildOf(root)) = q_parent.get(key_input.event_target())
        && let Ok((input_value, hard_limit)) = q_number_input.get(root)
        && let Ok(editable_text) = q_text_input.get(key_input.event_target())
    {
        let text_value = editable_text.value().to_string();
        emit_value_change(
            text_value,
            input_value.format(),
            root,
            hard_limit,
            &mut commands,
            true,
        );
    }
}

fn number_input_on_focus_gained(focus_gained: On<FocusGained>, mut commands: Commands) {
    // Change cursor to I-Beam.
    let editable_text_id = focus_gained.event_target();
    commands
        .entity(editable_text_id)
        .insert(EntityCursor::System(bevy::window::SystemCursorIcon::Text));
}

fn number_input_on_focus_lost(
    focus_lost: On<FocusLost>,
    q_parent: Query<&ChildOf>,
    q_number_input: Query<(&ScrubNumberInputValue, Option<&HardLimit>), With<ScrubNumberInput>>,
    mut q_text_input: Query<&mut EditableText>,
    mut commands: Commands,
) {
    let editable_text_id = focus_lost.event_target();

    if let Ok(&ChildOf(root)) = q_parent.get(editable_text_id)
        && let Ok((input_value, hard_limit)) = q_number_input.get(root)
        && let Ok(editable_text) = q_text_input.get_mut(editable_text_id)
    {
        let text_value = editable_text.value().to_string();
        emit_value_change(
            text_value,
            input_value.format(),
            root,
            hard_limit,
            &mut commands,
            true,
        );

        // Restore cursor back to normal.
        commands
            .entity(editable_text_id)
            .insert(EntityCursor::System(
                bevy::window::SystemCursorIcon::ColResize,
            ));
    }
}

/// Suppress the standard "click to focus" behavior, we want to handle this ourselves (focus
/// happens on release rather than press so we can detect drags).
fn scrubber_on_acquire_focus(mut acquire_focus: On<AcquireFocus>) {
    acquire_focus.propagate(false);
}

fn scrubber_on_press(
    mut press: On<Pointer<Press>>,
    mut q_scrubber: Query<&mut ScrubberDragState>,
    q_parent: Query<&ChildOf>,
    mut focus: ResMut<InputFocus>,
) {
    if let Ok(&ChildOf(text_id)) = q_parent.get(press.event_target())
        && let Ok(mut drag_state) = q_scrubber.get_mut(press.entity)
    {
        drag_state.max_distance = 0.0;
        // If the text input has focus, then allow the press event to go through
        if focus.get() != Some(text_id) {
            // If some other widget has focus, clear it.
            focus.clear();
            press.propagate(false);
        }
    }
}

fn scrubber_on_release(
    mut release: On<Pointer<Release>>,
    mut q_text: Query<(
        &mut EditableText,
        &ComputedNode,
        &ComputedUiRenderTargetInfo,
        &UiGlobalTransform,
        &TextScroll,
    )>,
    q_scrubber: Query<(&ComputedNode, &UiGlobalTransform, &mut ScrubberDragState)>,
    q_root: Query<Has<InteractionDisabled>>,
    q_parent: Query<&ChildOf>,
    mut input_focus: ResMut<InputFocus>,
    ui_scale: Res<UiScale>,
) {
    if let Ok(&ChildOf(text_id)) = q_parent.get(release.event_target())
        && let Ok(&ChildOf(root_id)) = q_parent.get(text_id)
        && let Ok((mut editable_text, node, target, transform, text_scroll)) =
            q_text.get_mut(text_id)
        && let Ok((_, _, drag_state)) = q_scrubber.get(release.entity)
        && let Ok(disabled) = q_root.get(root_id)
    {
        // If editable text has focus, then pass the event through.
        if input_focus.get() == Some(text_id) {
            return;
        }

        release.propagate(false);

        // Copy of logic from EditableText / text_input, but done on pointer up instead of down.
        if !disabled && drag_state.max_distance <= DRAG_THRESHOLD_DISTANCE {
            if release.button != PointerButton::Primary {
                return;
            }

            if editable_text.is_composing() {
                // The IME is active; all input needs to be routed there, including pointer presses.
                return;
            }

            let Some(local_pos) = transform.try_inverse().map(|inverse| {
                inverse.transform_point2(
                    release.pointer_location.position * target.scale_factor() / ui_scale.0,
                ) - node.content_box().min
                    + text_scroll.0
            }) else {
                return;
            };

            editable_text.queue_edit(TextEdit::MoveToPoint(local_pos));
            input_focus.set(text_id, FocusCause::Pressed);
        }
    }
}

/// Conversion factor from pixels dragged to value change, in priority order:
/// a [`SoftLimit`] maps the full slider width to the full range, so dragging
/// edge to edge covers the range exactly; otherwise [`NumberInputStep`] or
/// integer formatting give a step-sized nudge per pixel; otherwise
/// [`NumberInputPrecision`] or the value's own magnitude are used.
fn compute_drag_speed(
    soft_limit: Option<&SoftLimit>,
    step: Option<&NumberInputStep>,
    precision: Option<&NumberInputPrecision>,
    input_value: &ScrubNumberInputValue,
    slider_size: f64,
) -> f64 {
    if let Some(SoftLimit(nrange)) = soft_limit {
        match nrange {
            NumberInputRange::F32(range) => (range.end - range.start) as f64 / slider_size,
            NumberInputRange::F64(range) => (range.end - range.start) / slider_size,
            NumberInputRange::I32(range) => (range.end - range.start) as f64 / slider_size,
            NumberInputRange::I64(range) => (range.end - range.start) as f64 / slider_size,
        }
    } else if let Some(NumberInputStep(step)) = step {
        *step * BASE_DRAG_SPEED
    } else if matches!(
        input_value.format(),
        ScrubNumberFormat::I32 | ScrubNumberFormat::I64
    ) {
        // Treat integers as having a step size of 1
        BASE_DRAG_SPEED
    } else if let Some(prec) = precision {
        // Derive from precision
        10.0_f64.powf(-(prec.0 as f64))
    } else {
        // Nothing to derive from, so scale by the nearest power of 10 to the
        // current magnitude.
        let m = input_value.as_f64().abs();
        let decade = if m >= 1.0 { m.log10().floor() } else { 0.0 };
        BASE_DRAG_SPEED * 10f64.powf(decade)
    }
}

fn scrubber_on_drag_start(
    mut drag_start: On<Pointer<DragStart>>,
    q_root: Query<(
        &ScrubNumberInputValue,
        Option<&SoftLimit>,
        Option<&NumberInputPrecision>,
        Option<&NumberInputStep>,
        Has<InteractionDisabled>,
    )>,
    mut q_text_input: Query<&mut BackgroundGradient>,
    mut q_scrubber: Query<(&ComputedNode, &mut ScrubberDragState)>,
    q_parent: Query<&ChildOf>,
    input_focus: Res<InputFocus>,
    theme: Res<UiTheme>,
    mut commands: Commands,
) {
    if let Ok(&ChildOf(text_id)) = q_parent.get(drag_start.event_target())
        && let Ok(&ChildOf(root_id)) = q_parent.get(text_id)
        && let Ok((input_value, soft_limit, precision, step, disabled)) = q_root.get(root_id)
        && let Ok(mut gradient) = q_text_input.get_mut(text_id)
        && !disabled
        && input_focus.get() != Some(text_id)
        && let Ok((node, mut drag)) = q_scrubber.get_mut(drag_start.event_target())
    {
        let slider_size = (node.size().x * node.inverse_scale_factor).max(1.0) as f64;
        drag_start.propagate(false);
        drag.dragging = true;
        drag.base_value = *input_value;
        drag.max_distance = 0.0;
        drag.value_offset = 0.0f64;
        drag.drag_speed = compute_drag_speed(soft_limit, step, precision, input_value, slider_size);

        set_slidebar_styles(
            text_id,
            &theme,
            disabled,
            true,
            false,
            input_focus.get() == Some(text_id),
            &mut gradient,
            &mut commands,
        );
    }
}

fn scrubber_on_drag(
    mut drag: On<Pointer<Drag>>,
    q_root: Query<(
        Option<&SoftLimit>,
        Option<&HardLimit>,
        Option<&NumberInputPrecision>,
        Has<InteractionDisabled>,
    )>,
    mut q_scrubber: Query<(&UiGlobalTransform, &mut ScrubberDragState)>,
    q_parent: Query<&ChildOf>,
    focus: Res<InputFocus>,
    mut commands: Commands,
    ui_scale: Res<UiScale>,
    keys: Res<ButtonInput<Key>>,
) {
    if let Ok(&ChildOf(text_id)) = q_parent.get(drag.event_target())
        && focus.get() != Some(text_id)
        && let Ok(&ChildOf(root_id)) = q_parent.get(text_id)
        && let Ok((soft_limit, hard_limit, precision, disabled)) = q_root.get(root_id)
        && let Ok((transform, mut drag_state)) = q_scrubber.get_mut(drag.entity)
    {
        drag_state.max_distance = drag_state.max_distance.max(drag.distance.length());
        drag.propagate(false);
        if drag_state.dragging && !disabled && drag_state.max_distance > DRAG_THRESHOLD_DISTANCE {
            let drag_delta = transform.transform_vector2(drag.delta / ui_scale.0).x;
            let mut delta = drag_delta as f64 * drag_state.drag_speed;
            if keys.pressed(Key::Shift) {
                delta *= 0.1;
            }
            drag_state.value_offset += delta;
            emit_drag_value_change(
                &mut commands,
                root_id,
                soft_limit,
                hard_limit,
                precision,
                &mut drag_state,
                false,
            );
        }
    }
}

fn scrubber_on_drag_end(
    mut drag_end: On<Pointer<DragEnd>>,
    q_root: Query<(
        Option<&SoftLimit>,
        Option<&HardLimit>,
        Option<&NumberInputPrecision>,
        Has<InteractionDisabled>,
    )>,
    mut q_text_input: Query<(&Hovered, &mut BackgroundGradient)>,
    mut q_scrubber: Query<&mut ScrubberDragState>,
    q_parent: Query<&ChildOf>,
    input_focus: Res<InputFocus>,
    mut commands: Commands,
    theme: Res<UiTheme>,
) {
    if let Ok(&ChildOf(text_id)) = q_parent.get(drag_end.event_target())
        && input_focus.get() != Some(text_id)
        && let Ok(&ChildOf(root_id)) = q_parent.get(text_id)
        && let Ok((soft_limit, hard_limit, precision, disabled)) = q_root.get(root_id)
        && let Ok(mut drag_state) = q_scrubber.get_mut(drag_end.entity)
        && let Ok((&Hovered(hovered), mut gradient)) = q_text_input.get_mut(text_id)
    {
        drag_end.propagate(false);
        if drag_state.dragging {
            if !disabled {
                emit_drag_value_change(
                    &mut commands,
                    root_id,
                    soft_limit,
                    hard_limit,
                    precision,
                    &mut drag_state,
                    true,
                );
            }
            set_slidebar_styles(
                text_id,
                &theme,
                disabled,
                false,
                hovered,
                false,
                &mut gradient,
                &mut commands,
            );
            drag_state.dragging = false;
        }
    }
}

fn scrubber_on_drag_cancel(
    mut drag_cancel: On<Pointer<Cancel>>,
    q_parent: Query<&ChildOf>,
    mut q_text_input: Query<(&Hovered, &mut BackgroundGradient)>,
    mut q_scrubber: Query<&mut ScrubberDragState>,
    theme: Res<UiTheme>,
    input_focus: Res<InputFocus>,
    mut commands: Commands,
) {
    if let Ok(&ChildOf(text_id)) = q_parent.get(drag_cancel.event_target())
        && let Ok(mut drag_state) = q_scrubber.get_mut(drag_cancel.entity)
        && let Ok((&Hovered(hovered), mut gradient)) = q_text_input.get_mut(text_id)
    {
        set_slidebar_styles(
            text_id,
            &theme,
            false,
            false,
            hovered,
            input_focus.get() == Some(text_id),
            &mut gradient,
            &mut commands,
        );
        drag_cancel.propagate(false);
        drag_state.dragging = false;
    }
}

fn update_slider_pos(
    input_value: &ScrubNumberInputValue,
    limit: Option<&SoftLimit>,
    gradient: &mut BackgroundGradient,
) {
    if let [Gradient::Linear(linear_gradient)] = &mut gradient.0[..] {
        let percent_value = if let Some(SoftLimit(range)) = limit {
            (range.thumb_position(*input_value) * 100.0).clamp(0.0, 100.0)
        } else {
            // If there's no soft limit, then don't show the slide bar.
            0.0
        };
        linear_gradient.stops[1].point = percent(percent_value);
        linear_gradient.stops[2].point = percent(percent_value);
    }
}

fn set_slidebar_styles(
    slidebar_id: Entity,
    theme: &UiTheme,
    disabled: bool,
    pressed: bool,
    hovered: bool,
    focused: bool,
    gradient: &mut BackgroundGradient,
    commands: &mut Commands,
) {
    let bar_color = theme.color(&if disabled {
        tokens::SLIDER_BAR_DISABLED
    } else if pressed {
        tokens::SLIDER_BAR_PRESSED
    } else if hovered {
        tokens::SLIDER_BAR_HOVER
    } else {
        tokens::SLIDER_BAR
    });

    let bg_color = theme.color(&if focused {
        tokens::TEXT_INPUT_BG
    } else if disabled {
        tokens::SLIDER_BG_DISABLED
    } else if pressed {
        tokens::SLIDER_BG_PRESSED
    } else if hovered {
        tokens::SLIDER_BG_HOVER
    } else {
        tokens::SLIDER_BG
    });

    let font_color_token = match disabled {
        true => tokens::TEXT_INPUT_TEXT_DISABLED,
        false => tokens::TEXT_INPUT_TEXT,
    };

    let cursor_shape = match disabled {
        true => bevy::window::SystemCursorIcon::NotAllowed,
        false => bevy::window::SystemCursorIcon::EwResize,
    };

    if let [Gradient::Linear(linear_gradient)] = &mut gradient.0[..] {
        linear_gradient.stops[0].color = bar_color;
        linear_gradient.stops[1].color = bar_color;
        linear_gradient.stops[2].color = bg_color;
        linear_gradient.stops[3].color = bg_color;
    }

    // Change cursor shape and text color
    commands
        .entity(slidebar_id)
        .insert(EntityCursor::System(cursor_shape))
        .insert(ThemeTextColor(font_color_token));
}

fn emit_drag_value_change(
    commands: &mut Commands,
    source: Entity,
    soft_limit: Option<&SoftLimit>,
    hard_limit: Option<&HardLimit>,
    precision: Option<&NumberInputPrecision>,
    drag_state: &mut ScrubberDragState,
    is_final: bool,
) {
    // Relative scrub: always measured from the value at drag start.
    let mut value = drag_state.base_value.offset_by(drag_state.value_offset);

    // Dragging is confined to the soft range; typing can still exceed it.
    if let Some(SoftLimit(range)) = soft_limit {
        value = range.clamp(value);
    }
    if let Some(precision) = precision {
        value = precision.round(value);
    }
    // Hard limit is absolute and always applied last.
    if let Some(HardLimit(range)) = hard_limit {
        value = range.clamp(value);
    }

    trigger_value_change(commands, value, source, is_final);
}

fn emit_value_change(
    text_value: String,
    format: ScrubNumberFormat,
    source: Entity,
    hard_limit: Option<&HardLimit>,
    commands: &mut Commands,
    is_final: bool,
) {
    let text_value = text_value.trim();
    if text_value.is_empty() {
        return;
    }

    let Ok(new_value) = ScrubNumberInputValue::parse_from(text_value.to_owned(), format) else {
        // TODO: should handle errors better than this
        warn!("number input parsing failed, invalid format");
        return;
    };

    let clamped_value = match hard_limit {
        Some(limit) => limit.0.clamp(new_value),
        None => new_value,
    };

    trigger_value_change(commands, clamped_value, source, is_final);
}

/// Decompose the input value enum and trigger a [`ValueChange`] with the appropriate generic
/// parameter type based on the enum variant.
fn trigger_value_change(
    commands: &mut Commands,
    value: ScrubNumberInputValue,
    source: Entity,
    is_final: bool,
) {
    match value {
        ScrubNumberInputValue::F32(value) => commands.trigger(ValueChange {
            source,
            value,
            is_final,
        }),
        ScrubNumberInputValue::F64(value) => commands.trigger(ValueChange {
            source,
            value,
            is_final,
        }),
        ScrubNumberInputValue::I32(value) => commands.trigger(ValueChange {
            source,
            value,
            is_final,
        }),
        ScrubNumberInputValue::I64(value) => commands.trigger(ValueChange {
            source,
            value,
            is_final,
        }),
    }
}

/// Re-apply the slidebar gradient colors for every number input when the theme changes.
fn update_slidebar_styles_theme(
    q_children: Query<&Children>,
    q_number_input: Query<(Entity, Has<InteractionDisabled>), With<ScrubNumberInput>>,
    mut q_text_input: Query<(&Hovered, &mut BackgroundGradient)>,
    theme: Res<UiTheme>,
    input_focus: Res<InputFocus>,
    mut commands: Commands,
) {
    if !theme.is_changed() {
        return;
    }
    for (root_entity, is_disabled) in q_number_input.iter() {
        let Some(text_entity) = q_children
            .iter_descendants(root_entity)
            .find(|e| q_text_input.contains(*e))
        else {
            continue;
        };
        if let Ok((&Hovered(hovered), mut gradient)) = q_text_input.get_mut(text_entity) {
            set_slidebar_styles(
                text_entity,
                &theme,
                is_disabled,
                false,
                hovered,
                input_focus.get() == Some(text_entity),
                &mut gradient,
                &mut commands,
            );
        }
    }
}

/// Returns `true` while the [`ScrubNumberInput`] rooted at `root` is being
/// actively edited, either drag-scrubbed with the internal scrubber's drag
/// flag set or focused for keyboard entry. Callers that push external value
/// updates back into the widget, such as an inspector refresh, use this to
/// avoid clobbering an in-progress edit. The drag flag lives on a private
/// child entity, so this accessor exists rather than exposing
/// `ScrubberDragState`.
pub fn is_scrubbing_or_focused(world: &World, root: Entity, focus: Option<Entity>) -> bool {
    fn walk(world: &World, entity: Entity, focus: Option<Entity>) -> bool {
        if world
            .get::<ScrubberDragState>(entity)
            .is_some_and(|state| state.dragging)
        {
            return true;
        }
        if focus == Some(entity) && world.get::<EditableText>(entity).is_some() {
            return true;
        }
        let Some(children) = world.get::<Children>(entity) else {
            return false;
        };
        children.iter().any(|&child| walk(world, child, focus))
    }
    walk(world, root, focus)
}

/// Returns `true` while `root`'s editable-text descendant holds keyboard
/// focus, meaning the user is mid-type. Unlike [`is_scrubbing_or_focused`],
/// this does not guard an in-progress drag: the caller's re-inserted value and
/// the drag's relative math (`ScrubberDragState::base_value` + `value_offset`,
/// on a private child entity untouched by inserting `ScrubNumberInputValue`)
/// never read from each other, so a resync mid-drag clobbers nothing. Use this
/// where the drag gesture itself is the source of the resync (`ValueChange` ->
/// app state -> resync within one frame), so live drag feedback is not
/// suppressed for the span of the drag.
pub fn is_focused_for_editing(world: &World, root: Entity, focus: Option<Entity>) -> bool {
    fn walk(world: &World, entity: Entity, focus: Option<Entity>) -> bool {
        if focus == Some(entity) && world.get::<EditableText>(entity).is_some() {
            return true;
        }
        let Some(children) = world.get::<Children>(entity) else {
            return false;
        };
        children.iter().any(|&child| walk(world, child, focus))
    }
    walk(world, root, focus)
}

#[cfg(test)]
mod display_tests {
    use super::*;

    /// A brush radius arriving from a scroll-wheel resize is a scaled `f32`,
    /// and printing it raw puts `1.889105` in a field asking for two
    /// places.
    #[test]
    fn a_fractional_value_shows_its_fields_precision_not_float_noise() {
        let noisy = ScrubNumberInputValue::F32(1.889105);
        assert_eq!(
            display_digits(&noisy, Some(&NumberInputPrecision(2))),
            "1.89",
        );
        assert_eq!(display_digits(&noisy, Some(&NumberInputPrecision(0))), "2");
    }

    /// A count field shows no decimal point, so Seed and Octaves do not read
    /// as `42.00`.
    #[test]
    fn a_count_precision_shows_whole_numbers() {
        assert_eq!(
            display_digits(
                &ScrubNumberInputValue::F32(42.0),
                Some(&NumberInputPrecision(0))
            ),
            "42",
        );
        assert_eq!(
            display_digits(
                &ScrubNumberInputValue::I32(42),
                Some(&NumberInputPrecision(2))
            ),
            "42",
            "an integer kind has no decimals to pad",
        );
    }

    /// Without a precision there is nothing to round to, and the value keeps
    /// its own spelling.
    #[test]
    fn a_field_with_no_precision_keeps_the_values_own_digits() {
        assert_eq!(
            display_digits(&ScrubNumberInputValue::F32(1.889105), None),
            "1.889105"
        );
    }

    /// A value landing on a whole number mid-drag keeps its padding, so the
    /// digits do not jump between `4` and `4.25`.
    #[test]
    fn a_whole_value_still_shows_its_fields_places() {
        assert_eq!(
            display_digits(
                &ScrubNumberInputValue::F32(4.0),
                Some(&NumberInputPrecision(2))
            ),
            "4.00",
        );
    }
}

#[cfg(test)]
mod drag_speed_tests {
    use super::*;

    /// A wider box, checking that sensitivity scales with width rather than
    /// being pinned to one field size.
    const ROW_WIDTH: f64 = 88.0;
    /// The options-bar chip's own scrub box (`SCRUB_CHIP_INPUT_WIDTH` in
    /// `src/terrain/ui_fields.rs`).
    const CHIP_WIDTH: f64 = 64.0;

    /// A bounded 0-1 field (Persistence, Inertia, Deposition, Evaporation)
    /// maps the field's own width to the full range, so dragging across a
    /// panel row moves 0 -> 1 exactly.
    #[test]
    fn zero_to_one_range_spans_its_own_field_width() {
        let limit = SoftLimit::f32(0.0..1.0);
        let speed = compute_drag_speed(
            Some(&limit),
            None,
            None,
            &ScrubNumberInputValue::F32(0.45),
            ROW_WIDTH,
        );
        assert_eq!(speed * ROW_WIDTH, 1.0, "edge-to-edge drag covers the range");
        // A single pixel is a small, but non-zero and non-coarse, nudge.
        assert!(
            (0.005..0.05).contains(&speed),
            "1px nudge should be a fine adjustment, got {speed}",
        );
    }

    /// Same range, narrower chip: sensitivity scales with the box being
    /// dragged across, not a fixed pixel count.
    #[test]
    fn zero_to_one_range_scales_with_chip_width() {
        let limit = SoftLimit::f32(0.0..1.0);
        let speed = compute_drag_speed(
            Some(&limit),
            None,
            None,
            &ScrubNumberInputValue::F32(0.45),
            CHIP_WIDTH,
        );
        assert_eq!(speed * CHIP_WIDTH, 1.0);
    }

    /// A wide range (Amplitude, 0..200) resolves edge-to-edge, so a
    /// full-width drag is never a no-op even though each pixel moves the
    /// value further.
    #[test]
    fn wide_range_still_spans_full_drag_without_going_dead() {
        let limit = SoftLimit::f32(0.0..200.0);
        let speed = compute_drag_speed(
            Some(&limit),
            None,
            None,
            &ScrubNumberInputValue::F32(50.0),
            ROW_WIDTH,
        );
        assert!(speed > 0.0, "must move the value on any drag distance");
        assert!(
            (speed * ROW_WIDTH - 200.0).abs() < 1e-9,
            "edge-to-edge drag should cover the range, got {}",
            speed * ROW_WIDTH
        );
    }

    /// A whole-frame relative-scrub simulation: starting at 0.45 and dragging
    /// the full row width right-to-left (negative delta) lands on the bottom
    /// of the soft range.
    #[test]
    fn full_width_drag_right_to_left_reaches_the_bottom_of_the_range() {
        let limit = SoftLimit::f32(0.0..1.0);
        let base = ScrubNumberInputValue::F32(0.45);
        let speed = compute_drag_speed(Some(&limit), None, None, &base, ROW_WIDTH);

        let mut drag_state = ScrubberDragState {
            dragging: true,
            drag_speed: speed,
            value_offset: 0.0,
            max_distance: 0.0,
            base_value: base,
        };
        // Drag the full field width leftward, in one step (matches the
        // math `scrubber_on_drag` does per pointer-move event).
        drag_state.value_offset += -ROW_WIDTH * speed;

        let mut value = drag_state.base_value.offset_by(drag_state.value_offset);
        value = limit.0.clamp(value);
        assert_eq!(value, ScrubNumberInputValue::F32(0.0));
    }
}

/// Plugin which keeps number-input slidebar colors in sync with the theme.
pub struct ScrubNumberInputPlugin;

impl Plugin for ScrubNumberInputPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.add_systems(
            PreUpdate,
            update_slidebar_styles_theme.in_set(PickingSystems::Last),
        );
    }
}
