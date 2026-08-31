//! Layout presets: one press puts a node in a named place in its parent.
//!
//! The nine anchors are the corners, edges and middle of the parent box; a
//! node takes one of them by being placed absolutely with the offsets that
//! name it, and by letting an automatic margin take the centring on any axis
//! it is centred on. Full Rect stretches the node over the whole parent
//! instead, and Center leaves the node in the parent's flow with automatic
//! margins on every side.
//!
//! Each preset states every field it touches, so applying one twice, or
//! applying two in a row, leaves the node saying exactly what the last
//! preset says and nothing left over from the one before.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{ButtonOperatorCall, ButtonProps, IconButtonProps, button, icon_button},
    icons::Icon,
    tokens,
    tooltip::Tooltip,
};

use crate::{EditorEntity, commands::push_layout_edits, selection::Selection};

/// The operator a preset button dispatches.
pub const LAYOUT_PRESET_OP: &str = "ui.layout_preset";

/// One named place a node can be put in its parent.
pub struct LayoutPreset {
    /// What a clause and a button name it by.
    pub id: &'static str,
    /// What the tooltip calls it.
    pub label: &'static str,
    /// The glyph its button carries.
    pub icon: Icon,
    position_type: PositionType,
    left: Val,
    right: Val,
    top: Val,
    bottom: Val,
    margin: UiRect,
    /// The size the preset states, for the one preset that states a size.
    /// `None` leaves the node's own width and height alone.
    size: Option<(Val, Val)>,
}

impl LayoutPreset {
    /// `node` with this preset's fields written over it.
    pub fn applied(&self, node: &Node) -> Node {
        let mut node = node.clone();
        node.position_type = self.position_type;
        node.left = self.left;
        node.right = self.right;
        node.top = self.top;
        node.bottom = self.bottom;
        node.margin = self.margin;

        if let Some((width, height)) = self.size {
            node.width = width;
            node.height = height;
        }
        node
    }
}

/// Zero on every side: the margin an anchor that centres on neither axis has.
const NO_MARGIN: UiRect = UiRect::all(Val::Px(0.0));

/// An automatic margin on the horizontal pair, which is what centres a node
/// between a `left` and a `right` that are both stated.
const CENTRE_X: UiRect = UiRect {
    left: Val::Auto,
    right: Val::Auto,
    top: Val::Px(0.0),
    bottom: Val::Px(0.0),
};

/// [`CENTRE_X`] for the vertical pair.
const CENTRE_Y: UiRect = UiRect {
    left: Val::Px(0.0),
    right: Val::Px(0.0),
    top: Val::Auto,
    bottom: Val::Auto,
};

/// An anchor: absolutely placed, keeping the node's own size.
const fn anchor(
    id: &'static str,
    label: &'static str,
    icon: Icon,
    left: Val,
    right: Val,
    top: Val,
    bottom: Val,
    margin: UiRect,
) -> LayoutPreset {
    LayoutPreset {
        id,
        label,
        icon,
        position_type: PositionType::Absolute,
        left,
        right,
        top,
        bottom,
        margin,
        size: None,
    }
}

const ZERO: Val = Val::Px(0.0);
const FREE: Val = Val::Auto;

/// The 3x3 anchor grid, in reading order.
pub const ANCHOR_PRESETS: [LayoutPreset; 9] = [
    anchor(
        "top_left",
        "Top Left",
        Icon::ArrowUpLeft,
        ZERO,
        FREE,
        ZERO,
        FREE,
        NO_MARGIN,
    ),
    anchor(
        "top_center",
        "Top Center",
        Icon::ArrowUp,
        ZERO,
        ZERO,
        ZERO,
        FREE,
        CENTRE_X,
    ),
    anchor(
        "top_right",
        "Top Right",
        Icon::ArrowUpRight,
        FREE,
        ZERO,
        ZERO,
        FREE,
        NO_MARGIN,
    ),
    anchor(
        "center_left",
        "Center Left",
        Icon::ArrowLeft,
        ZERO,
        FREE,
        ZERO,
        ZERO,
        CENTRE_Y,
    ),
    anchor(
        "middle_center",
        "Middle Center",
        Icon::Dot,
        ZERO,
        ZERO,
        ZERO,
        ZERO,
        UiRect::all(Val::Auto),
    ),
    anchor(
        "center_right",
        "Center Right",
        Icon::ArrowRight,
        FREE,
        ZERO,
        ZERO,
        ZERO,
        CENTRE_Y,
    ),
    anchor(
        "bottom_left",
        "Bottom Left",
        Icon::ArrowDownLeft,
        ZERO,
        FREE,
        FREE,
        ZERO,
        NO_MARGIN,
    ),
    anchor(
        "bottom_center",
        "Bottom Center",
        Icon::ArrowDown,
        ZERO,
        ZERO,
        FREE,
        ZERO,
        CENTRE_X,
    ),
    anchor(
        "bottom_right",
        "Bottom Right",
        Icon::ArrowDownRight,
        FREE,
        ZERO,
        FREE,
        ZERO,
        NO_MARGIN,
    ),
];

/// The two presets that are not anchors: stretch over the parent, and sit in
/// the middle of the parent's own flow.
pub const WIDE_PRESETS: [LayoutPreset; 2] = [
    LayoutPreset {
        id: "full_rect",
        label: "Full Rect",
        icon: Icon::Maximize,
        position_type: PositionType::Absolute,
        left: ZERO,
        right: ZERO,
        top: ZERO,
        bottom: ZERO,
        margin: NO_MARGIN,
        // Stated, because a node with a width of its own would keep it and
        // stretch nowhere.
        size: Some((FREE, FREE)),
    },
    LayoutPreset {
        id: "center",
        label: "Center",
        icon: Icon::AlignCenterHorizontal,
        position_type: PositionType::Relative,
        left: FREE,
        right: FREE,
        top: FREE,
        bottom: FREE,
        margin: UiRect::all(Val::Auto),
        size: None,
    },
];

/// Every preset, anchors first.
pub fn presets() -> impl Iterator<Item = &'static LayoutPreset> {
    ANCHOR_PRESETS.iter().chain(WIDE_PRESETS.iter())
}

/// The preset `id` names.
pub fn preset(id: &str) -> Option<&'static LayoutPreset> {
    presets().find(|preset| preset.id == id)
}

/// Write `preset` over every selected node's `Node`, as one history entry.
fn apply_preset(world: &mut World, preset: &'static LayoutPreset) {
    let selected: Vec<Entity> = world.resource::<Selection>().entities.clone();
    let mut edits: Vec<(Entity, Node, Node)> = Vec::new();
    for entity in selected {
        if world.get::<EditorEntity>(entity).is_some() {
            continue;
        }
        let Some(before) = world.get::<Node>(entity).cloned() else {
            continue;
        };
        let after = preset.applied(&before);
        if let Some(mut node) = world.get_mut::<Node>(entity) {
            *node = after.clone();
        }
        edits.push((entity, before, after));
    }
    // One entry however many nodes the selection held, so one undo puts the
    // whole press back.
    push_layout_edits(world, edits);
}

/// A preset needs a selected node to put somewhere.
fn has_selected_node(
    keybind_focus: crate::keybind_focus::KeybindFocus,
    selection: Res<Selection>,
    nodes: Query<(), (With<Node>, Without<EditorEntity>)>,
) -> bool {
    if keybind_focus.is_typing() {
        return false;
    }
    selection
        .entities
        .iter()
        .any(|&entity| nodes.contains(entity))
}

#[operator(
    id = "ui.layout_preset",
    label = "Layout Preset",
    description = "Put the selected nodes in a named place in their parent.",
    is_available = has_selected_node,
    params(name(
        String,
        doc = "Preset id, e.g. \"top_left\", \"full_rect\", \"center\"."
    ))
)]
pub(crate) fn ui_layout_preset(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(name) = params.as_str("name") else {
        warn!("ui.layout_preset: missing `name` parameter");
        return OperatorResult::Cancelled;
    };
    let Some(preset) = preset(name) else {
        let ids: Vec<&str> = presets().map(|preset| preset.id).collect();
        warn!(
            "ui.layout_preset: no preset `{name}`; the presets are {}",
            ids.join(", ")
        );
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| apply_preset(world, preset));
    OperatorResult::Finished
}

/// A preset button: a feathers control carrying the call that applies it.
fn preset_call(preset: &'static LayoutPreset) -> (ButtonOperatorCall, Tooltip) {
    (
        ButtonOperatorCall::new(LAYOUT_PRESET_OP).with_param("name", preset.id),
        Tooltip::title(preset.label),
    )
}

/// Marker on the row of preset buttons, so a test can find it.
#[derive(Component)]
pub struct LayoutPresetRow;

/// Fill `parent` with the preset row: the 3x3 anchor grid, then the two
/// presets that are not anchors, each button dispatching the operator.
pub fn spawn_preset_row(commands: &mut Commands, parent: Entity, icon_font: &Handle<Font>) {
    let row = commands
        .spawn((
            LayoutPresetRow,
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                padding: UiRect::all(px(tokens::SPACING_SM)),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();

    let grid = commands
        .spawn((
            Node {
                display: Display::Grid,
                grid_template_columns: RepeatedGridTrack::auto(3),
                row_gap: px(tokens::SPACING_XS),
                column_gap: px(tokens::SPACING_XS),
                justify_content: JustifyContent::Center,
                ..default()
            },
            ChildOf(row),
        ))
        .id();
    for preset in &ANCHOR_PRESETS {
        commands.spawn((
            icon_button(IconButtonProps::new(preset.icon), icon_font),
            preset_call(preset),
            ChildOf(grid),
        ));
    }

    for preset in &WIDE_PRESETS {
        commands.spawn((
            button(ButtonProps::new(preset.label).align_left()),
            preset_call(preset),
            ChildOf(row),
        ));
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<UiLayoutPresetOp>();
}
