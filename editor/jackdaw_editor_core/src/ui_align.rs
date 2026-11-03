//! Aligning a selection to its bounding box, and spreading it evenly between
//! the two ends of that box. Offsets are written back through the node's
//! authored unit scheme; a node its parent lays out is refused.

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use crate::{
    EditorEntity,
    commands::push_layout_edits,
    selection::Selection,
    ui_stage::{
        ExactPercent, NodeAnchors, PixelRounding, apply_authored_rect, authored_offset_of,
        global_node_rect, unit_basis_of, without_selected_ancestors,
    },
};

/// One of the six lines of the selection's box a node can be moved onto.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlignEdge {
    Left,
    CenterX,
    Right,
    Top,
    CenterY,
    Bottom,
}

impl AlignEdge {
    /// Where `rect` goes so it sits on this line of `bounds`, in global
    /// authored pixels. Only the axis this edge names moves.
    fn place(self, rect: Rect, bounds: Rect) -> Vec2 {
        let size = rect.size();
        match self {
            Self::Left => Vec2::new(bounds.min.x, rect.min.y),
            Self::CenterX => Vec2::new(bounds.center().x - size.x / 2.0, rect.min.y),
            Self::Right => Vec2::new(bounds.max.x - size.x, rect.min.y),
            Self::Top => Vec2::new(rect.min.x, bounds.min.y),
            Self::CenterY => Vec2::new(rect.min.x, bounds.center().y - size.y / 2.0),
            Self::Bottom => Vec2::new(rect.min.x, bounds.max.y - size.y),
        }
    }
}

/// Which way a distribution spreads its nodes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DistributeAxis {
    Horizontal,
    Vertical,
}

impl DistributeAxis {
    fn of(self, point: Vec2) -> f32 {
        match self {
            Self::Horizontal => point.x,
            Self::Vertical => point.y,
        }
    }

    /// `point` with this axis replaced by `value`.
    fn with(self, point: Vec2, value: f32) -> Vec2 {
        match self {
            Self::Horizontal => Vec2::new(value, point.y),
            Self::Vertical => Vec2::new(point.x, value),
        }
    }
}

/// One selected node an alignment can move.
struct Member {
    entity: Entity,
    /// The node's laid-out box in global authored pixels.
    rect: Rect,
}

/// Why a selection cannot be aligned.
enum Refusal {
    /// Fewer than two nodes with a box to move.
    TooFew,
    /// A member its parent lays out, named so the notice can say which.
    Flowed(String),
}

fn name_of(world: &World, entity: Entity) -> String {
    world
        .get::<Name>(entity)
        .map_or_else(|| "a node".to_string(), |name| name.as_str().to_owned())
}

/// The selected nodes an alignment acts on, or why it cannot act. A node
/// inside another selected node is left out; layout already carries it.
fn members(world: &World) -> Result<Vec<Member>, Refusal> {
    let selected: Vec<Entity> = world
        .get_resource::<Selection>()
        .map(|selection| selection.entities.clone())
        .unwrap_or_default();
    let outermost = without_selected_ancestors(world, &selected);
    let mut members = Vec::new();
    for entity in outermost {
        if world.get::<EditorEntity>(entity).is_some() {
            continue;
        }
        // Skipped rather than refused, so a locked backdrop does not block
        // aligning the nodes on it.
        if world.get::<jackdaw_scene_types::Locked>(entity).is_some() {
            continue;
        }
        let Some(node) = world.get::<Node>(entity) else {
            continue;
        };
        if node.position_type != PositionType::Absolute {
            return Err(Refusal::Flowed(name_of(world, entity)));
        }
        let Some(rect) = global_node_rect(world, entity) else {
            continue;
        };
        members.push(Member { entity, rect });
    }
    if members.len() < 2 {
        return Err(Refusal::TooFew);
    }
    Ok(members)
}

/// The box the whole selection sits inside.
fn bounding_rect(members: &[Member]) -> Rect {
    members
        .iter()
        .map(|member| member.rect)
        .reduce(|left, right| left.union(right))
        .unwrap_or(Rect::from_corners(Vec2::ZERO, Vec2::ZERO))
}

/// Move `member` so its box's corner lands on `min`, and hand back the
/// `Node` before and after for the history entry.
fn move_member(world: &mut World, member: &Member, min: Vec2) -> Option<(Entity, Node, Node)> {
    let before = world.get::<Node>(member.entity)?.clone();
    let delta = min - member.rect.min;
    let offset = authored_offset_of(world, member.entity)? + delta;
    let basis = unit_basis_of(world, member.entity);
    let mut after = before.clone();
    apply_authored_rect(
        &mut after,
        NodeAnchors::of(&before),
        Vec4::new(
            offset.x,
            offset.y,
            member.rect.width(),
            member.rect.height(),
        ),
        (0, 0),
        basis,
        PixelRounding::Whole,
        ExactPercent::default(),
    );
    let mut live = world.get_mut::<Node>(member.entity)?;
    *live = after.clone();
    Some((member.entity, before, after))
}

/// Say why the selection was left alone.
fn refuse(world: &mut World, refusal: Refusal, what: &str) {
    let message = match refusal {
        Refusal::TooFew => format!("{what} needs at least two placed nodes selected"),
        Refusal::Flowed(name) => format!(
            "{name} is placed by its parent's layout. Set Position to Absolute to {what} it."
        ),
    };
    crate::status_bar::notify_error(world, message);
}

/// Move every selected node onto one line of the selection's box.
pub fn align_selection(world: &mut World, edge: AlignEdge) {
    let members = match members(world) {
        Ok(members) => members,
        Err(refusal) => return refuse(world, refusal, "aligning"),
    };
    let bounds = bounding_rect(&members);
    let edits: Vec<(Entity, Node, Node)> = members
        .iter()
        .filter_map(|member| {
            let min = edge.place(member.rect, bounds);
            move_member(world, member, min)
        })
        .collect();
    push_layout_edits(world, edits);
}

/// Even out the gaps between the selected nodes along one axis, holding the
/// two outermost still.
pub fn distribute_selection(world: &mut World, axis: DistributeAxis) {
    let mut members = match members(world) {
        Ok(members) => members,
        Err(refusal) => return refuse(world, refusal, "distributing"),
    };
    if members.len() < 3 {
        crate::status_bar::notify_error(
            world,
            "distributing needs at least three placed nodes selected",
        );
        return;
    }
    // By centre, not leading edge: a wide node enclosing a narrow one must
    // still sort last if it holds the far edge of the span.
    members.sort_by(|left, right| {
        let centre = |member: &Member| axis.of(member.rect.min) + axis.of(member.rect.max);
        centre(left).total_cmp(&centre(right))
    });

    let bounds = bounding_rect(&members);
    let span = axis.of(bounds.max) - axis.of(bounds.min);
    let filled: f32 = members
        .iter()
        .map(|member| axis.of(member.rect.size()))
        .sum();
    let gap = (span - filled) / (members.len() - 1) as f32;

    let mut cursor = axis.of(bounds.min);
    let mut edits = Vec::new();
    for member in &members {
        let min = axis.with(member.rect.min, cursor);
        if let Some(edit) = move_member(world, member, min) {
            edits.push(edit);
        }
        cursor += axis.of(member.rect.size()) + gap;
    }
    push_layout_edits(world, edits);
}

/// An alignment needs two authored nodes on an open canvas to line up.
fn can_align(
    keybind_focus: crate::keybind_focus::KeybindFocus,
    active: ActiveModalQuery,
    selection: Res<Selection>,
    ui_scenes: Query<(), crate::prefab::AuthoredUiSceneRoot>,
    nodes: Query<(), (With<Node>, Without<EditorEntity>)>,
) -> bool {
    if keybind_focus.keyboard_is_spoken_for() || active.is_modal_running() || ui_scenes.is_empty() {
        return false;
    }
    selection
        .entities
        .iter()
        .filter(|&&entity| nodes.contains(entity))
        .count()
        >= 2
}

#[operator(
    id = "ui.align_left",
    label = "Align Left",
    description = "Move the selection onto the left edge of its bounding box.",
    // `push_layout_edits` already records the undo entry.
    allows_undo = false,
    is_available = can_align
)]
pub(crate) fn ui_align_left(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| align_selection(world, AlignEdge::Left));
    OperatorResult::Finished
}

#[operator(
    id = "ui.align_center_x",
    label = "Align Center Horizontally",
    description = "Centre the selection horizontally in its bounding box.",
    // `push_layout_edits` already records the undo entry.
    allows_undo = false,
    is_available = can_align
)]
pub(crate) fn ui_align_center_x(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| align_selection(world, AlignEdge::CenterX));
    OperatorResult::Finished
}

#[operator(
    id = "ui.align_right",
    label = "Align Right",
    description = "Move the selection onto the right edge of its bounding box.",
    // `push_layout_edits` already records the undo entry.
    allows_undo = false,
    is_available = can_align
)]
pub(crate) fn ui_align_right(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| align_selection(world, AlignEdge::Right));
    OperatorResult::Finished
}

#[operator(
    id = "ui.align_top",
    label = "Align Top",
    description = "Move the selection onto the top edge of its bounding box.",
    // `push_layout_edits` already records the undo entry.
    allows_undo = false,
    is_available = can_align
)]
pub(crate) fn ui_align_top(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| align_selection(world, AlignEdge::Top));
    OperatorResult::Finished
}

#[operator(
    id = "ui.align_center_y",
    label = "Align Center Vertically",
    description = "Centre the selection vertically in its bounding box.",
    // `push_layout_edits` already records the undo entry.
    allows_undo = false,
    is_available = can_align
)]
pub(crate) fn ui_align_center_y(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| align_selection(world, AlignEdge::CenterY));
    OperatorResult::Finished
}

#[operator(
    id = "ui.align_bottom",
    label = "Align Bottom",
    description = "Move the selection onto the bottom edge of its bounding box.",
    // `push_layout_edits` already records the undo entry.
    allows_undo = false,
    is_available = can_align
)]
pub(crate) fn ui_align_bottom(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| align_selection(world, AlignEdge::Bottom));
    OperatorResult::Finished
}

#[operator(
    id = "ui.distribute_horizontal",
    label = "Distribute Horizontally",
    description = "Even out the horizontal gaps between the selected nodes.",
    allows_undo = false,
    is_available = can_align
)]
pub(crate) fn ui_distribute_horizontal(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| distribute_selection(world, DistributeAxis::Horizontal));
    OperatorResult::Finished
}

#[operator(
    id = "ui.distribute_vertical",
    label = "Distribute Vertically",
    description = "Even out the vertical gaps between the selected nodes.",
    allows_undo = false,
    is_available = can_align
)]
pub(crate) fn ui_distribute_vertical(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| distribute_selection(world, DistributeAxis::Vertical));
    OperatorResult::Finished
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<UiAlignLeftOp>()
        .register_operator::<UiAlignCenterXOp>()
        .register_operator::<UiAlignRightOp>()
        .register_operator::<UiAlignTopOp>()
        .register_operator::<UiAlignCenterYOp>()
        .register_operator::<UiAlignBottomOp>()
        .register_operator::<UiDistributeHorizontalOp>()
        .register_operator::<UiDistributeVerticalOp>();
}
