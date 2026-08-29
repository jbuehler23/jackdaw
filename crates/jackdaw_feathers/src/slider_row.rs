//! The row shape for a continuous value: a [`crate::field_row`] row with a
//! native `bevy::feathers` `FeathersSlider` in its control slot, its digits
//! drawn inside the bar.
//!
//! Used by every surface that edits a continuous value: terrain brush and
//! generation fields, the scatter section, the material kit's scalars.
//!
//! The row is generic over a tag component, so each surface brings its own
//! binding enum and reads commits back in its own
//! `On<bevy::ui_widgets::ValueChange<f32>>` observer rather than one handler
//! covering every field in the editor.
//!
//! `FeathersSlider` does not self-manage `SliderValue`: the caller re-inserts
//! it on the returned slider whenever the backing state changes for a reason
//! other than the widget's own drag (undo/redo, an edit on another surface),
//! and on every frame the caller's `ValueChange` handler writes that state
//! during a drag, so the fill and digits follow the gesture rather than
//! snapping into place on release.

use std::ops::Range;

use bevy::feathers::controls::FeathersSlider;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::SliderPrecision;

use crate::field_row::{FieldRow, FieldRowProps, spawn_field_row};
use crate::tokens;
use crate::tooltip::Tooltip;

/// Decimal places a [`FieldKind::Continuous`] row shows. Without it a dragged
/// `f32` shows float noise in its digits (`0.45000012` instead of `0.45`).
pub const CONTINUOUS_PRECISION: i32 = 2;

/// Whether a row's field is a whole-number count (Seed, Octaves, Iterations,
/// a layer count) or a continuous quantity. Changes display and drag rounding
/// only; both scrub over the same range, through the `SliderPrecision` the
/// slider already reads.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum FieldKind {
    /// Whole numbers only: 0 decimal places, and a drag rounds to the
    /// nearest integer.
    Count,
    /// Fractional values: [`CONTINUOUS_PRECISION`] decimal places.
    #[default]
    Continuous,
}

impl FieldKind {
    pub fn precision(self) -> i32 {
        match self {
            FieldKind::Count => 0,
            FieldKind::Continuous => CONTINUOUS_PRECISION,
        }
    }
}

/// What a spawned slider row hands back.
pub struct SliderRow {
    /// The row itself: where a marker or observer goes.
    pub row: Entity,
    /// The slider: where the tag rides, where `ValueChange<f32>` fires, and
    /// what a caller re-inserts `SliderValue` on.
    pub slider: Entity,
}

/// `range` is a hard limit on what the widget can produce: a slider has no
/// typed entry, so drag and range are the same bound.
#[derive(Clone)]
pub struct SliderRowProps {
    pub label: String,
    /// Shown on hover rather than as a second line, so the row stays one
    /// line high.
    pub tooltip: Option<String>,
    /// Nesting depth, passed through to [`FieldRowProps::indented`].
    pub indent: u8,
    pub value: f32,
    pub range: Range<f32>,
    pub kind: FieldKind,
}

impl SliderRowProps {
    pub fn new(label: impl Into<String>, value: f32, range: Range<f32>) -> Self {
        Self {
            label: label.into(),
            tooltip: None,
            indent: 0,
            value,
            range,
            kind: FieldKind::Continuous,
        }
    }

    pub fn with_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn indented(mut self, levels: u8) -> Self {
        self.indent = levels;
        self
    }

    pub fn with_kind(mut self, kind: FieldKind) -> Self {
        self.kind = kind;
        self
    }
}

/// Spawn a labeled slider row under `parent`, tagged with `field`.
pub fn spawn_slider_row<C: Component>(
    commands: &mut Commands,
    parent: Entity,
    props: SliderRowProps,
    field: C,
) -> SliderRow {
    let FieldRow { row, control } = spawn_field_row(
        commands,
        parent,
        FieldRowProps::new(props.label.clone())
            .indented(props.indent)
            // A slider needs more room than a plain control; below that the
            // row wraps the track under the label rather than squeezing it.
            .with_control_min_width(tokens::SLIDER_MIN_WIDTH),
    );

    // Always tooltipped, description or not: the label column clips in a
    // narrow dock, so hover is the only way back to a cut name. See
    // `field_row`'s label node.
    let mut tip = Tooltip::title(props.label);
    if let Some(description) = props.tooltip {
        tip = tip.with_description(description);
    }
    commands.entity(row).insert((tip, Hovered::default()));

    let (min, max) = (props.range.start, props.range.end);
    let value = props.value.clamp(min, max);
    // The row's layout is patched onto the widget's own `Node` rather than
    // replacing it. `FeathersSlider` carries its track height, its padding and
    // the centring that positions the digits inside the track, and a
    // whole-`Node` insert drops all three, leaving a zero-height track with
    // its digits at the control's top-left.
    let slider = commands
        .spawn_scene(bsn! {
            @FeathersSlider { @value: {value}, @min: {min}, @max: {max} }
            Node {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_width: px(tokens::SLIDER_MIN_WIDTH),
            }
        })
        .insert((
            SliderPrecision(props.kind.precision()),
            field,
            ChildOf(control),
        ))
        .id();

    SliderRow { row, slider }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component)]
    struct Field;

    #[derive(Resource, Default)]
    struct Spawned(Option<SliderRow>);

    fn spawn(props: SliderRowProps) -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ));
        app.init_asset::<bevy::text::Font>();
        app.init_resource::<Spawned>();
        let id = app.world_mut().register_system(
            move |mut commands: Commands, mut out: ResMut<Spawned>| {
                let parent = commands.spawn(Node::default()).id();
                out.0 = Some(spawn_slider_row(
                    &mut commands,
                    parent,
                    props.clone(),
                    Field,
                ));
            },
        );
        app.world_mut().run_system(id).unwrap();
        app.world_mut().flush();
        app
    }

    fn row_props() -> SliderRowProps {
        SliderRowProps::new("Persistence", 0.5, 0.0..1.0)
    }

    /// A slider draws its digits centred inside its own track, so it keeps
    /// the track height, the padding and the centring `FeathersSlider` spawns
    /// with. Replacing the widget's `Node` with the row's layout drops all
    /// three, leaving digits over the label with no track under them.
    #[test]
    fn the_row_layout_is_added_to_the_widgets_own_node_not_swapped_for_it() {
        let app = spawn(row_props());
        let slider = app.world().resource::<Spawned>().0.as_ref().unwrap().slider;
        let node = app.world().get::<Node>(slider).unwrap();

        assert_ne!(node.height, Val::Auto, "the track keeps its own height");
        assert_ne!(
            node.padding,
            UiRect::all(Val::Px(0.0)),
            "the track keeps the padding its digits sit inside",
        );
        assert_eq!(node.justify_content, JustifyContent::Center);
        assert_eq!(node.align_items, AlignItems::Center);

        assert_eq!(node.flex_grow, 1.0, "and still fills the control slot");
        assert_eq!(node.min_width, Val::Px(tokens::SLIDER_MIN_WIDTH));
    }

    /// In a dock too narrow for both, the row wraps the track under the label
    /// rather than squeezing it, so the control slot asks for a slider's worth
    /// of room rather than a plain control's.
    #[test]
    fn the_control_slot_asks_for_a_sliders_width_before_the_row_wraps() {
        let app = spawn(row_props());
        let control = app.world().resource::<Spawned>().0.as_ref().unwrap().row;
        let control = app.world().get::<Children>(control).unwrap()[1];
        let node = app.world().get::<Node>(control).unwrap();

        assert_eq!(node.min_width, Val::Px(tokens::SLIDER_MIN_WIDTH));
        assert_eq!(
            node.flex_basis,
            Val::Px(tokens::SLIDER_MIN_WIDTH),
            "the basis is what decides the wrap",
        );
        const {
            assert!(tokens::SLIDER_MIN_WIDTH > tokens::FIELD_CONTROL_MIN_WIDTH);
        }
    }

    /// The label column clips rather than wraps, so every slider row is
    /// hoverable back to its full label, not only ones carrying a
    /// description.
    #[test]
    fn a_row_with_no_description_is_still_hoverable_back_to_its_label() {
        let app = spawn(row_props());
        let row = app.world().resource::<Spawned>().0.as_ref().unwrap().row;
        let tip = app.world().get::<Tooltip>(row).expect("a label tooltip");
        assert_eq!(tip.title, "Persistence");
        assert!(app.world().get::<Hovered>(row).is_some());
    }

    /// Raising the floor is one-way: a caller cannot use it to undercut
    /// the shared control column.
    #[test]
    fn the_control_floor_can_only_be_raised() {
        let props = FieldRowProps::new("Metallic").with_control_min_width(1.0);
        assert_eq!(
            props.control_min_width,
            Some(tokens::FIELD_CONTROL_MIN_WIDTH)
        );
    }

    /// `Count` fields display whole numbers: `SliderPrecision(0)`, which
    /// `bevy::feathers` formats as `{:.0}` with no decimal point, so Seed,
    /// Octaves and Iterations do not read as `42.00`.
    #[test]
    fn count_fields_show_zero_decimals() {
        assert_eq!(FieldKind::Count.precision(), 0);
    }

    /// Continuous fields use 2 decimal places.
    #[test]
    fn continuous_fields_keep_two_decimals() {
        assert_eq!(FieldKind::Continuous.precision(), CONTINUOUS_PRECISION);
        assert_eq!(FieldKind::Continuous.precision(), 2);
    }

    #[test]
    fn a_row_defaults_to_continuous_and_takes_a_tooltip_only_when_given_one() {
        let plain = SliderRowProps::new("Metallic", 0.5, 0.0..1.0);
        assert_eq!(plain.kind, FieldKind::Continuous);
        assert_eq!(plain.indent, 0);
        assert!(plain.tooltip.is_none());

        let described = SliderRowProps::new("Radius", 4.0, 0.1..50.0)
            .with_tooltip("Area of effect")
            .with_kind(FieldKind::Count)
            .indented(1);
        assert_eq!(described.tooltip.as_deref(), Some("Area of effect"));
        assert_eq!(described.kind, FieldKind::Count);
        assert_eq!(described.indent, 1);
    }
}
