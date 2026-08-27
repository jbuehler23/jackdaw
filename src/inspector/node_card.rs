//! The structured `Node` card.
//!
//! `Node`'s forty layout fields, grouped into sections: what kind of box it
//! is, where it sits, how big it is, how it aligns its children, with the
//! flex and grid details in collapsible sub-sections.
//!
//! No `Node` field falls through to the generic reflect renderer. Every one
//! has its own control, described by the tables below: an enum field is one
//! row in `NodeEnumField`, an optional number one row in `NodeOptionalField`,
//! and each carries the closure that reads its live value, so the write path
//! and the refresh path work off the same entry.

use bevy::{
    ecs::system::SystemState, prelude::*, ui::InteractionDisabled, ui_widgets::ValueChange,
};
use jackdaw_feathers::{
    combobox::{
        ComboBoxChangeEvent, ComboBoxOptionData, ComboBoxSelectedIndex, combobox_with_selected,
    },
    field_row::{FieldRowProps, spawn_field_row},
    icons::{EditorFont, Icon, IconFont},
    number_input::{
        HardLimit, NumberInputPrecision, ScrubNumberInput, ScrubNumberInputValue, SoftLimit,
    },
    panel_card::{PanelCardCollapseState, PanelCardProps, spawn_panel_card},
    tokens,
};

use super::val_field::{spawn_ui_rect_field, spawn_val_field};

/// The reflect path of the component this card stands in for. Also the
/// card's `ComponentDisplayTypePath`, so a targeted body refresh finds it.
/// Asked of the type rather than spelled out, so a bevy release that moves
/// `Node` moves the card with it.
pub(crate) fn node_type_path() -> &'static str {
    <Node as bevy::reflect::TypePath>::type_path()
}

/// Decimal places an optional-number row shows.
const RATIO_PRECISION: i32 = 2;

/// Marker on the card body, so a test (and a targeted refresh) can tell the
/// structured card from the generic one.
#[derive(Component)]
pub struct NodeCardBody;

// ---------------------------------------------------------------------------
// Field tables
// ---------------------------------------------------------------------------

/// One enum-valued `Node` field: where it lives, what it can be, and how to
/// read the live value. `current` returns an index into `variants`, which is
/// both what a control shows and what a refresh compares against.
pub(crate) struct NodeEnumField {
    label: &'static str,
    path: &'static str,
    variants: &'static [&'static str],
    current: fn(&Node) -> usize,
}

/// One `Option`-valued `Node` field. `integer` picks the JSON the commit
/// writes, because the grid lines are `NonZero` integers and the aspect
/// ratio is a float.
///
/// `range` means something different on each side of `integer`. For a grid
/// line it is the `NonZero` width's own limit: the widget is hard-limited to
/// it and the commit clamps into it, since a number outside it cannot become
/// the value. For the aspect ratio it is a drag range only; a typed value may
/// exceed it, and the commit's one rule is that a ratio be positive.
pub(crate) struct NodeOptionalField {
    label: &'static str,
    path: &'static str,
    integer: bool,
    range: core::ops::RangeInclusive<f64>,
    current: fn(&Node) -> Option<f64>,
}

impl NodeOptionalField {
    /// The value this field would hold for `entered`, or `None` when the
    /// answer is "absent": zero on a `NonZero` line, a ratio that is not a
    /// positive number. The commit goes through here, so the document records
    /// what the component will take.
    fn settle(&self, entered: f64) -> Option<f64> {
        if self.integer {
            let clamped = entered
                .round()
                .clamp(*self.range.start(), *self.range.end());
            (clamped != 0.0).then_some(clamped)
        } else {
            (entered > 0.0).then_some(entered)
        }
    }
}

const DISPLAY: NodeEnumField = NodeEnumField {
    label: "display",
    path: "display",
    variants: &["Flex", "Grid", "Block", "None"],
    current: |node| match node.display {
        Display::Flex => 0,
        Display::Grid => 1,
        Display::Block => 2,
        Display::None => 3,
    },
};

const POSITION_TYPE: NodeEnumField = NodeEnumField {
    label: "position",
    path: "position_type",
    variants: &["Relative", "Absolute"],
    current: |node| match node.position_type {
        PositionType::Relative => 0,
        PositionType::Absolute => 1,
    },
};

const BOX_SIZING: NodeEnumField = NodeEnumField {
    label: "box sizing",
    path: "box_sizing",
    variants: &["BorderBox", "ContentBox"],
    current: |node| match node.box_sizing {
        BoxSizing::BorderBox => 0,
        BoxSizing::ContentBox => 1,
    },
};

const DIRECTION: NodeEnumField = NodeEnumField {
    label: "direction",
    path: "direction",
    variants: &["Ltr", "Rtl"],
    current: |node| match node.direction {
        InlineDirection::Ltr => 0,
        InlineDirection::Rtl => 1,
    },
};

const OVERFLOW_AXIS_VARIANTS: &[&str] = &["Visible", "Clip", "Hidden", "Scroll"];

fn overflow_axis_index(axis: OverflowAxis) -> usize {
    match axis {
        OverflowAxis::Visible => 0,
        OverflowAxis::Clip => 1,
        OverflowAxis::Hidden => 2,
        OverflowAxis::Scroll => 3,
    }
}

const OVERFLOW_X: NodeEnumField = NodeEnumField {
    label: "overflow x",
    path: "overflow.x",
    variants: OVERFLOW_AXIS_VARIANTS,
    current: |node| overflow_axis_index(node.overflow.x),
};

const OVERFLOW_Y: NodeEnumField = NodeEnumField {
    label: "overflow y",
    path: "overflow.y",
    variants: OVERFLOW_AXIS_VARIANTS,
    current: |node| overflow_axis_index(node.overflow.y),
};

const CLIP_VISUAL_BOX: NodeEnumField = NodeEnumField {
    label: "clip box",
    path: "overflow_clip_margin.visual_box",
    variants: &["ContentBox", "PaddingBox", "BorderBox"],
    current: |node| match node.overflow_clip_margin.visual_box {
        VisualBox::ContentBox => 0,
        VisualBox::PaddingBox => 1,
        VisualBox::BorderBox => 2,
    },
};

const ALIGN_ITEMS: NodeEnumField = NodeEnumField {
    label: "items",
    path: "align_items",
    variants: &[
        "Default",
        "Start",
        "End",
        "FlexStart",
        "FlexEnd",
        "Center",
        "Baseline",
        "Stretch",
    ],
    current: |node| match node.align_items {
        AlignItems::Default => 0,
        AlignItems::Start => 1,
        AlignItems::End => 2,
        AlignItems::FlexStart => 3,
        AlignItems::FlexEnd => 4,
        AlignItems::Center => 5,
        AlignItems::Baseline => 6,
        AlignItems::Stretch => 7,
    },
};

const ALIGN_SELF: NodeEnumField = NodeEnumField {
    label: "self",
    path: "align_self",
    variants: &[
        "Auto",
        "Start",
        "End",
        "FlexStart",
        "FlexEnd",
        "Center",
        "Baseline",
        "Stretch",
    ],
    current: |node| match node.align_self {
        AlignSelf::Auto => 0,
        AlignSelf::Start => 1,
        AlignSelf::End => 2,
        AlignSelf::FlexStart => 3,
        AlignSelf::FlexEnd => 4,
        AlignSelf::Center => 5,
        AlignSelf::Baseline => 6,
        AlignSelf::Stretch => 7,
    },
};

const ALIGN_CONTENT: NodeEnumField = NodeEnumField {
    label: "content",
    path: "align_content",
    variants: &[
        "Default",
        "Start",
        "End",
        "FlexStart",
        "FlexEnd",
        "Center",
        "Stretch",
        "SpaceBetween",
        "SpaceEvenly",
        "SpaceAround",
    ],
    current: |node| match node.align_content {
        AlignContent::Default => 0,
        AlignContent::Start => 1,
        AlignContent::End => 2,
        AlignContent::FlexStart => 3,
        AlignContent::FlexEnd => 4,
        AlignContent::Center => 5,
        AlignContent::Stretch => 6,
        AlignContent::SpaceBetween => 7,
        AlignContent::SpaceEvenly => 8,
        AlignContent::SpaceAround => 9,
    },
};

const JUSTIFY_ITEMS: NodeEnumField = NodeEnumField {
    label: "items",
    path: "justify_items",
    variants: &["Default", "Start", "End", "Center", "Baseline", "Stretch"],
    current: |node| match node.justify_items {
        JustifyItems::Default => 0,
        JustifyItems::Start => 1,
        JustifyItems::End => 2,
        JustifyItems::Center => 3,
        JustifyItems::Baseline => 4,
        JustifyItems::Stretch => 5,
    },
};

const JUSTIFY_SELF: NodeEnumField = NodeEnumField {
    label: "self",
    path: "justify_self",
    variants: &["Auto", "Start", "End", "Center", "Baseline", "Stretch"],
    current: |node| match node.justify_self {
        JustifySelf::Auto => 0,
        JustifySelf::Start => 1,
        JustifySelf::End => 2,
        JustifySelf::Center => 3,
        JustifySelf::Baseline => 4,
        JustifySelf::Stretch => 5,
    },
};

const JUSTIFY_CONTENT: NodeEnumField = NodeEnumField {
    label: "content",
    path: "justify_content",
    variants: &[
        "Default",
        "Start",
        "End",
        "FlexStart",
        "FlexEnd",
        "Center",
        "Stretch",
        "SpaceBetween",
        "SpaceEvenly",
        "SpaceAround",
    ],
    current: |node| match node.justify_content {
        JustifyContent::Default => 0,
        JustifyContent::Start => 1,
        JustifyContent::End => 2,
        JustifyContent::FlexStart => 3,
        JustifyContent::FlexEnd => 4,
        JustifyContent::Center => 5,
        JustifyContent::Stretch => 6,
        JustifyContent::SpaceBetween => 7,
        JustifyContent::SpaceEvenly => 8,
        JustifyContent::SpaceAround => 9,
    },
};

const FLEX_DIRECTION: NodeEnumField = NodeEnumField {
    label: "direction",
    path: "flex_direction",
    variants: &["Row", "Column", "RowReverse", "ColumnReverse"],
    current: |node| match node.flex_direction {
        FlexDirection::Row => 0,
        FlexDirection::Column => 1,
        FlexDirection::RowReverse => 2,
        FlexDirection::ColumnReverse => 3,
    },
};

const FLEX_WRAP: NodeEnumField = NodeEnumField {
    label: "wrap",
    path: "flex_wrap",
    variants: &["NoWrap", "Wrap", "WrapReverse"],
    current: |node| match node.flex_wrap {
        FlexWrap::NoWrap => 0,
        FlexWrap::Wrap => 1,
        FlexWrap::WrapReverse => 2,
    },
};

const GRID_AUTO_FLOW: NodeEnumField = NodeEnumField {
    label: "auto flow",
    path: "grid_auto_flow",
    variants: &["Row", "Column", "RowDense", "ColumnDense"],
    current: |node| match node.grid_auto_flow {
        GridAutoFlow::Row => 0,
        GridAutoFlow::Column => 1,
        GridAutoFlow::RowDense => 2,
        GridAutoFlow::ColumnDense => 3,
    },
};

/// How far a grid line may be dragged. Narrower than the `NonZeroI16` it is
/// stored in: taffy converts grid lines into its own origin-zero space and
/// panics near the extremes. Zero is the hole in the middle, which the row
/// spells as "auto".
const GRID_LINE_RANGE: core::ops::RangeInclusive<f64> = -1000.0..=1000.0;

/// The same ceiling for a span, which starts at one track.
const GRID_SPAN_RANGE: core::ops::RangeInclusive<f64> = 1.0..=1000.0;

const ASPECT_RATIO: NodeOptionalField = NodeOptionalField {
    label: "aspect ratio",
    path: "aspect_ratio",
    integer: false,
    // A drag range, not a limit on what can be typed: a ratio only has to be
    // positive.
    range: 0.01..=1000.0,
    current: |node| node.aspect_ratio.map(f64::from),
};

const GRID_ROW_START: NodeOptionalField = NodeOptionalField {
    label: "row start",
    path: "grid_row.start",
    integer: true,
    range: GRID_LINE_RANGE,
    current: |node| node.grid_row.get_start().map(f64::from),
};

const GRID_ROW_SPAN: NodeOptionalField = NodeOptionalField {
    label: "row span",
    path: "grid_row.span",
    integer: true,
    range: GRID_SPAN_RANGE,
    current: |node| node.grid_row.get_span().map(f64::from),
};

const GRID_ROW_END: NodeOptionalField = NodeOptionalField {
    label: "row end",
    path: "grid_row.end",
    integer: true,
    range: GRID_LINE_RANGE,
    current: |node| node.grid_row.get_end().map(f64::from),
};

const GRID_COLUMN_START: NodeOptionalField = NodeOptionalField {
    label: "column start",
    path: "grid_column.start",
    integer: true,
    range: GRID_LINE_RANGE,
    current: |node| node.grid_column.get_start().map(f64::from),
};

const GRID_COLUMN_SPAN: NodeOptionalField = NodeOptionalField {
    label: "column span",
    path: "grid_column.span",
    integer: true,
    range: GRID_SPAN_RANGE,
    current: |node| node.grid_column.get_span().map(f64::from),
};

const GRID_COLUMN_END: NodeOptionalField = NodeOptionalField {
    label: "column end",
    path: "grid_column.end",
    integer: true,
    range: GRID_LINE_RANGE,
    current: |node| node.grid_column.get_end().map(f64::from),
};

// ---------------------------------------------------------------------------
// Card body
// ---------------------------------------------------------------------------

/// Fill the `Node` card body for `source`. World-exclusive and deferred by the
/// caller, like the material cards: the layout is read off the live component
/// rather than reflected field by field.
pub(crate) fn fill_node_card_body(world: &mut World, source: Entity, body: Entity) {
    if world.get_entity(body).is_err() {
        return;
    }
    let Some(node) = world.get::<Node>(source).cloned() else {
        return;
    };
    let icon_font = world
        .get_resource::<IconFont>()
        .map(|font| font.0.clone())
        .unwrap_or_default();
    let collapse = PanelCardCollapseState(
        world
            .get_resource::<PanelCardCollapseState>()
            .map(|state| state.0.clone())
            .unwrap_or_default(),
    );

    let editor_font = world
        .get_resource::<EditorFont>()
        .map(|font| font.0.clone())
        .unwrap_or_default();
    let type_registry = world
        .get_resource::<AppTypeRegistry>()
        .cloned()
        .unwrap_or_default();

    // The grid track lists are handed to the generic reflect renderer, and
    // that renderer reads entity names, so the card's one world-exclusive
    // call needs a real `Query` beside its `Commands`.
    let mut state: SystemState<(Commands, Query<&Name>)> = SystemState::new(world);
    if let Ok((mut commands, entity_names)) = state.get_mut(world) {
        commands.entity(body).insert(NodeCardBody);
        build_node_card(
            &mut commands,
            body,
            source,
            &node,
            &icon_font,
            &collapse,
            &entity_names,
            &type_registry,
            &editor_font,
        );
    }
    state.apply(world);
    world.flush();
}

/// One collapsible sub-section of the card.
fn section(
    commands: &mut Commands,
    parent: Entity,
    title: &'static str,
    icon: Icon,
    icon_font: &Handle<Font>,
    collapse: &PanelCardCollapseState,
    collapsed: bool,
) -> Entity {
    spawn_panel_card(
        commands,
        parent,
        PanelCardProps::new(title)
            .with_icon(icon)
            .default_collapsed(collapsed)
            .remembered_as(format!("node_card::{title}")),
        icon_font,
        collapse,
    )
    .body
}

#[expect(
    clippy::too_many_arguments,
    reason = "the card's own inputs plus what the generic renderer needs for the track lists"
)]
fn build_node_card(
    commands: &mut Commands,
    body: Entity,
    source: Entity,
    node: &Node,
    icon_font: &Handle<Font>,
    collapse: &PanelCardCollapseState,
    entity_names: &Query<&Name>,
    type_registry: &AppTypeRegistry,
    editor_font: &Handle<Font>,
) {
    // Display, position, overflow, then the lengths. The two choices that
    // change what every field below them means are segmented controls, so the
    // current answer reads without opening a menu.
    spawn_segments(commands, body, source, node, &DISPLAY);
    spawn_segments(commands, body, source, node, &POSITION_TYPE);

    let overflow = section(
        commands,
        body,
        "Overflow",
        Icon::Eye,
        icon_font,
        collapse,
        false,
    );
    for field in [&OVERFLOW_X, &OVERFLOW_Y, &CLIP_VISUAL_BOX] {
        spawn_enum_combo(commands, overflow, source, node, field);
    }
    super::reflect_fields::spawn_numeric_field_row(
        commands,
        overflow,
        "scrollbar width",
        f64::from(node.scrollbar_width),
        "scrollbar_width".to_string(),
        source,
        node_type_path(),
        false,
    );
    super::reflect_fields::spawn_numeric_field_row(
        commands,
        overflow,
        "clip margin",
        f64::from(node.overflow_clip_margin.margin),
        "overflow_clip_margin.margin".to_string(),
        source,
        node_type_path(),
        false,
    );

    let position = section(
        commands,
        body,
        "Position",
        Icon::Move,
        icon_font,
        collapse,
        false,
    );
    for (name, value) in [
        ("left", node.left),
        ("top", node.top),
        ("right", node.right),
        ("bottom", node.bottom),
    ] {
        spawn_val_field(
            commands,
            position,
            name,
            value,
            0,
            name.to_string(),
            source,
            node_type_path(),
        );
    }

    let size = section(
        commands,
        body,
        "Size",
        Icon::Scaling,
        icon_font,
        collapse,
        false,
    );
    for (name, path, value) in [
        ("width", "width", node.width),
        ("height", "height", node.height),
        ("min width", "min_width", node.min_width),
        ("min height", "min_height", node.min_height),
        ("max width", "max_width", node.max_width),
        ("max height", "max_height", node.max_height),
    ] {
        spawn_val_field(
            commands,
            size,
            name,
            value,
            0,
            path.to_string(),
            source,
            node_type_path(),
        );
    }
    spawn_optional_number(commands, size, source, node, &ASPECT_RATIO);
    spawn_enum_combo(commands, size, source, node, &BOX_SIZING);

    let align = section(
        commands,
        body,
        "Align",
        Icon::AlignVerticalSpaceAround,
        icon_font,
        collapse,
        false,
    );
    for field in [&ALIGN_ITEMS, &ALIGN_SELF, &ALIGN_CONTENT] {
        spawn_enum_combo(commands, align, source, node, field);
    }

    let justify = section(
        commands,
        body,
        "Justify",
        Icon::AlignHorizontalSpaceAround,
        icon_font,
        collapse,
        false,
    );
    for field in [&JUSTIFY_ITEMS, &JUSTIFY_SELF, &JUSTIFY_CONTENT] {
        spawn_enum_combo(commands, justify, source, node, field);
    }

    let spacing = section(
        commands,
        body,
        "Spacing",
        Icon::Frame,
        icon_font,
        collapse,
        false,
    );
    for (name, value) in [
        ("margin", node.margin),
        ("padding", node.padding),
        ("border", node.border),
    ] {
        spawn_ui_rect_field(
            commands,
            spacing,
            name,
            value,
            0,
            name.to_string(),
            source,
            node_type_path(),
        );
    }
    for (name, path, value) in [
        ("row gap", "row_gap", node.row_gap),
        ("column gap", "column_gap", node.column_gap),
        (
            "radius top left",
            "border_radius.top_left",
            node.border_radius.top_left,
        ),
        (
            "radius top right",
            "border_radius.top_right",
            node.border_radius.top_right,
        ),
        (
            "radius bottom right",
            "border_radius.bottom_right",
            node.border_radius.bottom_right,
        ),
        (
            "radius bottom left",
            "border_radius.bottom_left",
            node.border_radius.bottom_left,
        ),
    ] {
        spawn_val_field(
            commands,
            spacing,
            name,
            value,
            0,
            path.to_string(),
            source,
            node_type_path(),
        );
    }

    let flex = section(
        commands,
        body,
        "Flex",
        Icon::StretchHorizontal,
        icon_font,
        collapse,
        true,
    );
    for field in [&FLEX_DIRECTION, &FLEX_WRAP] {
        spawn_enum_combo(commands, flex, source, node, field);
    }
    for (name, path, value) in [
        ("grow", "flex_grow", node.flex_grow),
        ("shrink", "flex_shrink", node.flex_shrink),
    ] {
        super::reflect_fields::spawn_numeric_field_row(
            commands,
            flex,
            name,
            f64::from(value),
            path.to_string(),
            source,
            node_type_path(),
            false,
        );
    }
    spawn_val_field(
        commands,
        flex,
        "basis",
        node.flex_basis,
        0,
        "flex_basis".to_string(),
        source,
        node_type_path(),
    );
    spawn_enum_combo(commands, flex, source, node, &DIRECTION);

    let grid = section(
        commands,
        body,
        "Grid",
        Icon::Grid3x3,
        icon_font,
        collapse,
        true,
    );
    spawn_enum_combo(commands, grid, source, node, &GRID_AUTO_FLOW);
    for field in [
        &GRID_ROW_START,
        &GRID_ROW_SPAN,
        &GRID_ROW_END,
        &GRID_COLUMN_START,
        &GRID_COLUMN_SPAN,
        &GRID_COLUMN_END,
    ] {
        spawn_optional_number(commands, grid, source, node, field);
    }
    // Track lists go to the generic reflect renderer, which gives a row per
    // track with each sizing function editable in place.
    for (label, path) in [
        ("template rows", "grid_template_rows"),
        ("template columns", "grid_template_columns"),
        ("auto rows", "grid_auto_rows"),
        ("auto columns", "grid_auto_columns"),
    ] {
        let Some(value) = <Node as bevy::prelude::Struct>::field(node, path) else {
            continue;
        };
        super::reflect_fields::spawn_field_row(
            commands,
            grid,
            label,
            value,
            0,
            path.to_string(),
            source,
            node_type_path(),
            entity_names,
            type_registry,
            editor_font,
            icon_font,
        );
    }
}

// ---------------------------------------------------------------------------
// Enum controls
// ---------------------------------------------------------------------------

/// The read-only entities every write path on the card has to check for.
type RemoteProxies<'w, 's> =
    Query<'w, 's, (), With<crate::remote::entity_browser::RemoteEntityProxy>>;

/// True when an edit must be dropped before it moves anything: the row has
/// lost its binding, or it points at a remote proxy, which is a view of
/// another process's world rather than a document this editor owns.
fn card_edit_skipped(target: Option<Entity>, remote_proxies: &RemoteProxies) -> bool {
    target.is_none_or(|entity| remote_proxies.contains(entity))
}

/// One clickable segment of a segmented enum control.
#[derive(Component)]
pub struct NodeSegment {
    source: Entity,
    field: &'static NodeEnumField,
    index: usize,
}

/// A `Node` enum field rendered as a dropdown.
#[derive(Component)]
pub struct NodeEnumCombo {
    source: Entity,
    field: &'static NodeEnumField,
}

/// A `Node` enum field rendered as a segmented control.
///
/// Not [`jackdaw_feathers::tab_strip`]: that widget spaces its tabs apart
/// rather than joining them in one bordered box, does not wrap, and
/// dispatches a named operator with a string parameter. A segment here
/// commits through the undo-backed write path below, carrying a
/// `&'static NodeEnumField` no operator parameter can name.
fn spawn_segments(
    commands: &mut Commands,
    parent: Entity,
    source: Entity,
    node: &Node,
    field: &'static NodeEnumField,
) {
    let row = spawn_field_row(commands, parent, FieldRowProps::new(field.label));
    let bar = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                border: UiRect::all(px(1.0)),
                border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
                overflow: Overflow::clip(),
                // Four variants side by side are wider than the control slot
                // at the default panel width, so the bar wraps what does not
                // fit onto a second line rather than clipping it.
                flex_wrap: FlexWrap::Wrap,
                flex_shrink: 1.0,
                min_width: px(0.0),
                ..Default::default()
            },
            BackgroundColor(tokens::ELEVATED_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(row.control),
        ))
        .id();

    let current = (field.current)(node);
    for (index, variant) in field.variants.iter().enumerate() {
        let background = if index == current {
            tokens::TOOLBAR_ACTIVE_BG
        } else {
            Color::NONE
        };
        commands
            .spawn((
                NodeSegment {
                    source,
                    field,
                    index,
                },
                Interaction::default(),
                jackdaw_feathers::tooltip::Tooltip::title(format!("{}: {variant}", field.label)),
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
                    ..Default::default()
                },
                BackgroundColor(background),
                ChildOf(bar),
                children![(
                    Text::new(*variant),
                    TextFont {
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                )],
            ))
            .observe(on_segment_click);
    }
}

fn spawn_enum_combo(
    commands: &mut Commands,
    parent: Entity,
    source: Entity,
    node: &Node,
    field: &'static NodeEnumField,
) {
    let row = spawn_field_row(commands, parent, FieldRowProps::new(field.label));
    let current = (field.current)(node);
    let options: Vec<ComboBoxOptionData> = field
        .variants
        .iter()
        .map(|variant| ComboBoxOptionData::new(*variant))
        .collect();
    commands
        .spawn((
            combobox_with_selected(options, current),
            ChildOf(row.control),
        ))
        .insert((
            NodeEnumCombo { source, field },
            ComboBoxSelectedIndex(current),
        ));
}

/// Write path shared by both enum controls: the same undo-backed variant
/// commit the generic enum menu uses, so a segment and a dropdown mint the
/// same history entry and honour PIE Live the same way.
fn commit_variant(
    commands: &mut Commands,
    source: Entity,
    field: &'static NodeEnumField,
    index: usize,
) {
    let Some(variant) = field.variants.get(index).copied() else {
        // The control and the variant table disagreed, so the pick goes
        // nowhere.
        warn!(
            "a variant pick on '{}' was dropped: no variant {index} of {}",
            field.path,
            field.variants.len()
        );
        return;
    };
    commands.queue(move |world: &mut World| {
        super::reflect_fields::apply_enum_variant_with_undo(
            world,
            source,
            node_type_path(),
            field.path,
            variant,
        );
    });
}

fn on_segment_click(
    click: On<Pointer<Click>>,
    segments: Query<(&NodeSegment, Has<InteractionDisabled>)>,
    remote_proxies: RemoteProxies,
    mut commands: Commands,
) {
    let Ok((segment, disabled)) = segments.get(click.event_target()) else {
        return;
    };
    if disabled {
        return;
    }
    if card_edit_skipped(Some(segment.source), &remote_proxies) {
        return;
    }
    commit_variant(&mut commands, segment.source, segment.field, segment.index);
}

pub(crate) fn on_node_enum_change(
    event: On<ComboBoxChangeEvent>,
    combos: Query<&NodeEnumCombo>,
    remote_proxies: RemoteProxies,
    mut commands: Commands,
) {
    let Ok(combo) = combos.get(event.entity) else {
        return;
    };
    if card_edit_skipped(Some(combo.source), &remote_proxies) {
        return;
    }
    commit_variant(&mut commands, combo.source, combo.field, event.selected);
}

/// Keep the segmented controls showing what the component holds. Undo, a
/// second inspector, or the running game all move the value behind the
/// card's back.
pub fn paint_node_segments(
    nodes: Query<&Node>,
    mut segments: Query<(&NodeSegment, &mut BackgroundColor)>,
) {
    for (segment, mut background) in &mut segments {
        let Ok(node) = nodes.get(segment.source) else {
            continue;
        };
        let color = if (segment.field.current)(node) == segment.index {
            tokens::TOOLBAR_ACTIVE_BG
        } else {
            Color::NONE
        };
        if background.0 != color {
            background.0 = color;
        }
    }
}

/// Same job for the dropdowns. The widget repaints its own label from
/// `ComboBoxSelectedIndex`, so putting the live index there is the whole
/// refresh.
pub fn refresh_node_enum_combos(
    nodes: Query<&Node>,
    combos: Query<(Entity, &NodeEnumCombo, &ComboBoxSelectedIndex)>,
    mut commands: Commands,
) {
    for (entity, combo, shown) in &combos {
        let Ok(node) = nodes.get(combo.source) else {
            continue;
        };
        let live = (combo.field.current)(node);
        if live != shown.0 {
            commands.entity(entity).insert(ComboBoxSelectedIndex(live));
        }
    }
}

// ---------------------------------------------------------------------------
// Optional-number control
// ---------------------------------------------------------------------------

/// The state of one optional-number row, on the row root. Held here rather
/// than read back off the widgets because "absent" and "zero" are different
/// answers and only one of them is a number.
#[derive(Component)]
pub struct NodeOptionalNumber {
    source: Entity,
    field: &'static NodeOptionalField,
    value: Option<f64>,
}

/// Links the number half of an optional row back to its root.
#[derive(Component)]
pub struct NodeOptionalInput(Entity);

/// Links the auto|set dropdown back to its root.
#[derive(Component)]
pub struct NodeOptionalMode(Entity);

/// The dropdown's two options, in index order.
const OPTIONAL_MODES: [&str; 2] = ["auto", "set"];

/// Room the auto|set dropdown asks for: what the unit dropdown on the
/// length rows above it takes, so the two line up down the panel.
const OPTIONAL_MODE_WIDTH: f32 = 70.0;
/// How far it gives way in a narrow panel: "auto" plus the chevron still
/// reads here, one glyph does not.
const OPTIONAL_MODE_MIN_WIDTH: f32 = 34.0;

fn spawn_optional_number(
    commands: &mut Commands,
    parent: Entity,
    source: Entity,
    node: &Node,
    field: &'static NodeOptionalField,
) {
    let value = (field.current)(node);
    let row = spawn_field_row(commands, parent, FieldRowProps::new(field.label));
    commands.entity(row.row).insert((
        NodeOptionalNumber {
            source,
            field,
            value,
        },
        // The row is one property with two controls, so it takes one property
        // marker, the way a `Vec3` row's three axes do.
        super::InspectorFieldRow {
            source_entity: source,
            type_path: node_type_path().to_string(),
            field_path: field.path.to_string(),
        },
    ));

    let options: Vec<ComboBoxOptionData> = OPTIONAL_MODES
        .iter()
        .map(|mode| ComboBoxOptionData::new(*mode))
        .collect();
    let mode_index = usize::from(value.is_some());
    commands
        .spawn((
            combobox_with_selected(options, mode_index),
            ChildOf(row.control),
        ))
        .insert((
            NodeOptionalMode(row.row),
            ComboBoxSelectedIndex(mode_index),
            // Wide enough for "auto" and its chevron, and shrinkable: a fixed
            // width would push the number beside it off the row.
            Node {
                width: px(OPTIONAL_MODE_WIDTH),
                min_width: px(OPTIONAL_MODE_MIN_WIDTH),
                flex_shrink: 1.0,
                ..Default::default()
            },
        ));

    let number = commands
        .spawn_scene(bsn! { @ScrubNumberInput })
        .insert((
            ScrubNumberInputValue::F64(value.unwrap_or_default()),
            NumberInputPrecision(if field.integer { 0 } else { RATIO_PRECISION }),
            NodeOptionalInput(row.row),
            ChildOf(row.control),
        ))
        .id();
    // A grid line's range is a hard wall: a number past it is not a value the
    // component would take. A ratio's range only bounds dragging, so it stays
    // soft and a typed number can leave it.
    let low = *field.range.start();
    let high = *field.range.end();
    if field.integer {
        commands.entity(number).insert(HardLimit::f64(low..high));
    } else {
        commands.entity(number).insert(SoftLimit::f64(low..high));
    }
    if value.is_none() {
        commands.entity(number).insert(InteractionDisabled);
    }
}

/// Author the row's current answer. An `Option` is written as the bare value
/// or `null`: reflect deserializes options through serde's own option path,
/// not as a named variant. [`NodeOptionalField::settle`] decides what counts
/// as absent, so a zeroed grid line and a degenerate ratio both land as
/// "auto".
///
/// A drag tick previews on live ECS and a settled value mints one undo-backed
/// document write, so a whole gesture is one entry in history.
fn commit_optional_number(world: &mut World, root: Entity, is_final: bool) {
    let Some(state) = world.get::<NodeOptionalNumber>(root) else {
        return;
    };
    let (source, field, value) = (state.source, state.field, state.value);
    let json = match value.and_then(|value| field.settle(value)) {
        Some(value) if field.integer => serde_json::json!(value.round() as i64),
        Some(value) => serde_json::json!(value),
        None => serde_json::Value::Null,
    };
    if super::reflect_fields::try_route_pie_live_field_edit(
        world,
        source,
        node_type_path(),
        field.path,
        json.clone(),
    ) {
        return;
    }
    if is_final {
        crate::commands::field_edit_commit(
            world,
            node_type_path(),
            field.path,
            &json,
            "Set layout on multiple entities",
        );
    } else {
        crate::commands::field_edit_preview(world, node_type_path(), field.path, &json);
    }
}

/// Push the row's state onto its widgets after the state moves.
fn sync_optional_widgets(world: &mut World, root: Entity) {
    let Some(state) = world.get::<NodeOptionalNumber>(root) else {
        return;
    };
    let value = state.value;
    let inputs: Vec<Entity> = world
        .query::<(Entity, &NodeOptionalInput)>()
        .iter(world)
        .filter(|(_, input)| input.0 == root)
        .map(|(entity, _)| entity)
        .collect();
    for input in inputs {
        let Ok(mut entity) = world.get_entity_mut(input) else {
            continue;
        };
        entity.insert(ScrubNumberInputValue::F64(value.unwrap_or_default()));
        if value.is_some() {
            entity.remove::<InteractionDisabled>();
        } else {
            entity.insert(InteractionDisabled);
        }
    }

    let modes: Vec<Entity> = world
        .query::<(Entity, &NodeOptionalMode)>()
        .iter(world)
        .filter(|(_, mode)| mode.0 == root)
        .map(|(entity, _)| entity)
        .collect();
    for mode in modes {
        if let Ok(mut entity) = world.get_entity_mut(mode) {
            entity.insert(ComboBoxSelectedIndex(usize::from(value.is_some())));
        }
    }
}

/// Write path for the number half. The input emits every drag tick with
/// `is_final == false` and once on release, so only the release mints undo.
pub(crate) fn on_optional_number_change(
    event: On<ValueChange<f64>>,
    inputs: Query<&NodeOptionalInput>,
    rows: Query<&NodeOptionalNumber>,
    remote_proxies: RemoteProxies,
    mut commands: Commands,
) {
    let source = event.source;
    let Ok(input) = inputs.get(source) else {
        return;
    };
    let root = input.0;
    if card_edit_skipped(rows.get(root).ok().map(|row| row.source), &remote_proxies) {
        return;
    }
    let value = event.value;
    let is_final = event.is_final;
    commands
        .entity(source)
        .insert(ScrubNumberInputValue::F64(value));
    commands.queue(move |world: &mut World| {
        let Some(mut state) = world.get_mut::<NodeOptionalNumber>(root) else {
            return;
        };
        if state.value.is_none() {
            return;
        }
        state.value = Some(value);
        commit_optional_number(world, root, is_final);
    });
}

pub(crate) fn on_optional_number_mode_change(
    event: On<ComboBoxChangeEvent>,
    modes: Query<&NodeOptionalMode>,
    rows: Query<&NodeOptionalNumber>,
    remote_proxies: RemoteProxies,
    mut commands: Commands,
) {
    let Ok(mode) = modes.get(event.entity) else {
        return;
    };
    let root = mode.0;
    if card_edit_skipped(rows.get(root).ok().map(|row| row.source), &remote_proxies) {
        return;
    }
    let set = event.selected == 1;
    commands.queue(move |world: &mut World| {
        let Some(mut state) = world.get_mut::<NodeOptionalNumber>(root) else {
            return;
        };
        if state.value.is_some() == set {
            return;
        }
        // Leaving "auto" starts at 1 rather than 0: zero is a ratio nothing
        // can have and a grid line that does not exist.
        state.value = if set { Some(1.0) } else { None };
        sync_optional_widgets(world, root);
        commit_optional_number(world, root, true);
    });
}

/// Pull the optional rows back in line with the live component, the same job
/// `refresh_val_fields` does for the length rows.
pub fn refresh_node_optional_numbers(world: &mut World) {
    let stale: Vec<(Entity, Option<f64>)> = world
        .query::<(Entity, &NodeOptionalNumber)>()
        .iter(world)
        .filter_map(|(root, state)| {
            let node = world.get::<Node>(state.source)?;
            let live = (state.field.current)(node);
            (live != state.value).then_some((root, live))
        })
        .collect();
    for (root, live) in stale {
        if let Some(mut state) = world.get_mut::<NodeOptionalNumber>(root) {
            state.value = live;
        }
        sync_optional_widgets(world, root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field of `Node` the card renders a control for. A field added
    /// upstream is missing from this list until the card covers it.
    const COVERED_FIELDS: &[&str] = &[
        "display",
        "box_sizing",
        "position_type",
        "overflow",
        "scrollbar_width",
        "overflow_clip_margin",
        "left",
        "right",
        "top",
        "bottom",
        "width",
        "height",
        "min_width",
        "min_height",
        "max_width",
        "max_height",
        "aspect_ratio",
        "align_items",
        "justify_items",
        "align_self",
        "justify_self",
        "align_content",
        "justify_content",
        "direction",
        "margin",
        "padding",
        "border",
        "border_radius",
        "flex_direction",
        "flex_wrap",
        "flex_grow",
        "flex_shrink",
        "flex_basis",
        "row_gap",
        "column_gap",
        "grid_auto_flow",
        "grid_template_rows",
        "grid_template_columns",
        "grid_auto_rows",
        "grid_auto_columns",
        "grid_row",
        "grid_column",
    ];

    #[test]
    fn every_node_field_has_a_control_on_the_card() {
        let bevy::reflect::TypeInfo::Struct(info) = <Node as bevy::reflect::Typed>::type_info()
        else {
            panic!("Node is a struct");
        };
        let fields: Vec<&str> = info.iter().map(bevy::reflect::NamedField::name).collect();
        for name in &fields {
            assert!(
                COVERED_FIELDS.contains(name),
                "{name} has no control on the Node card",
            );
        }
        assert_eq!(
            fields.len(),
            COVERED_FIELDS.len(),
            "the coverage list has entries `Node` does not",
        );
    }

    /// What the row authors for a number the user entered. The clamp keeps
    /// the document and the component holding the same value.
    #[test]
    fn a_settled_value_is_one_the_field_can_hold() {
        // A grid line clamps into range rather than wrapping through it: the
        // reflect side refuses an out-of-range int.
        assert_eq!(GRID_ROW_SPAN.settle(90_000.0), Some(1000.0));
        assert_eq!(GRID_ROW_SPAN.settle(0.0), Some(1.0), "a span starts at one");
        assert_eq!(GRID_ROW_START.settle(-90_000.0), Some(-1000.0));
        assert_eq!(GRID_ROW_START.settle(2.4), Some(2.0), "lines are whole");
        assert_eq!(
            GRID_ROW_START.settle(0.0),
            None,
            "zero is the hole a NonZero line cannot hold; the row says auto",
        );

        // A ratio only has to be positive, so a typed value past the drag
        // range still commits.
        assert_eq!(ASPECT_RATIO.settle(1.5), Some(1.5));
        assert_eq!(ASPECT_RATIO.settle(4000.0), Some(4000.0));
        assert_eq!(ASPECT_RATIO.settle(0.0), None);
        assert_eq!(ASPECT_RATIO.settle(-2.0), None);
    }

    #[test]
    fn the_enum_tables_index_the_variants_they_name() {
        // A `current` closure that returns an index the variant list does
        // not have would leave a control blank and commit the wrong name.
        let node = Node::default();
        for field in [
            &DISPLAY,
            &POSITION_TYPE,
            &BOX_SIZING,
            &DIRECTION,
            &OVERFLOW_X,
            &OVERFLOW_Y,
            &CLIP_VISUAL_BOX,
            &ALIGN_ITEMS,
            &ALIGN_SELF,
            &ALIGN_CONTENT,
            &JUSTIFY_ITEMS,
            &JUSTIFY_SELF,
            &JUSTIFY_CONTENT,
            &FLEX_DIRECTION,
            &FLEX_WRAP,
            &GRID_AUTO_FLOW,
        ] {
            assert!(
                (field.current)(&node) < field.variants.len(),
                "{} indexes outside its variant list",
                field.path,
            );
        }
    }
}
