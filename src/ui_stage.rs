//! Selection, the selection outline, and direct manipulation for the 2D
//! viewport stage.
//!
//! A click hit-tests the authored UI rects the panel is showing and selects
//! the topmost one; the outline and its eight resize handles track that
//! node's live rect, and dragging either moves or resizes the authored
//! `Node` behind it, writing back through the scheme its author wrote (see
//! [`NodeAnchors`]). The overlay is editor chrome parented into the panel's
//! stage node, never into the authored tree. All of it is
//! [`Viewport2dMode::Edit`] behaviour. One gesture is at most one history
//! entry, handed over on release.
//!
//! Everything here is stated in the render-target pixels of
//! [`crate::viewport_2d::Ui2dView`], which are authored pixels. The only
//! conversions are the ones `viewport_2d` owns, composed in
//! [`cursor_stage_offset`] on the way in and inverted by
//! [`stage_pixels_per_target_pixel`] on the way out. There is no camera
//! term: Bevy renders UI through a view of its own, so a routed scene is
//! pinned to its render target whatever the 2D camera is doing.

use bevy::{
    ecs::system::SystemParam,
    picking::{
        events::{Drag, DragDrop, DragEnd, DragStart, Move, Pointer, Press},
        prelude::Pickable,
    },
    prelude::*,
    ui::{ComputedNode, ComputedStackIndex, ComputedUiTargetCamera, UiGlobalTransform},
};
use jackdaw_feathers::tokens;
use jackdaw_scene_types::{CanvasGuides, Locked};
use jackdaw_snap::{SnapLine, SnapRect, snap_edges_2d_with_winners};

use crate::{
    EditorEntity,
    canvas_snap::CanvasSnap,
    commands::push_layout_edits,
    prefab::AuthoredUiSceneRoot,
    selection::Selection,
    viewport_2d::{
        CanvasRuler, Scene2dViewport, Viewport2dMode, Viewport2dPanelHost, cursor_stage_offset,
        target_pixels_per_stage_pixel,
    },
};

/// Side of a square resize handle, in the stage's logical pixels.
pub const HANDLE_SIZE: f32 = 8.0;

/// Thickness of the selection outline, in the stage's logical pixels.
const OUTLINE_WIDTH: f32 = 1.0;

/// Thinnest a resize may leave a node, in authored pixels: the handle's own
/// side, below which the node could not be picked up by a handle again.
const MIN_NODE_SIZE: f32 = HANDLE_SIZE;

/// How small a node has to be on an axis before that axis's handles are
/// drawn outside it rather than straddling its edges, in authored pixels.
/// Three handles across is where the eight of them would cover the node
/// entirely and leave no press that is a move.
const HANDLES_OUTSIDE_BELOW: f32 = 3.0 * HANDLE_SIZE;

/// Draw order of the overlay inside the stage. Above the stage's own
/// frame, and above anything else placed in the stage alongside it.
const OVERLAY_Z: i32 = 50;

/// Draw order of the pre-select outline: over the canvas, under the
/// selection outline and its handles.
const HOVER_OUTLINE_Z: i32 = OVERLAY_Z - 1;

/// How wide a guide is to the pointer, in the stage's logical pixels. A
/// one-pixel line is drawn down the middle of the slab, which lies over the
/// canvas; what a press over a node does is [`guide_takes_the_press`].
const GUIDE_HIT_WIDTH: f32 = 5.0;

/// How close, in **pointer** pixels, a dragged edge has to come to a
/// neighbouring one before it lands on it. Pointer pixels rather than
/// authored ones, so the radius stays constant on screen at any zoom; the
/// gesture converts it with the scale the panel is drawing at (see
/// [`live_scale`]).
const EDGE_SNAP_PIXELS: f32 = 6.0;

/// The eight handle positions, clockwise from the top-left corner.
const HANDLE_POSITIONS: [(i8, i8); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
];

/// Which edges a handle drags. `0` means the edge is not moved.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct UiResizeHandle {
    pub x: i8,
    pub y: i8,
}

/// Which way a line runs across the canvas.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CanvasAxis {
    /// A line down the canvas, fixing an x coordinate.
    Vertical,
    /// A line across the canvas, fixing a y coordinate.
    Horizontal,
}

impl CanvasAxis {
    /// The axis `id` names, or `None` when it names neither.
    pub fn parse(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "vertical" => Some(Self::Vertical),
            "horizontal" => Some(Self::Horizontal),
            _ => None,
        }
    }
}

/// A line drawn across one panel's stage where a drag came to rest. One per
/// axis at most, and only while a gesture is landing on something.
#[derive(Component, Clone, Copy)]
pub struct SnapHighlight {
    /// The panel content entity carrying this stage's
    /// [`Viewport2dPanelHost`].
    pub host: Entity,
    /// Which way the line runs.
    pub axis: CanvasAxis,
}

/// One of the scene's guides, drawn over one panel's stage. The guides
/// belong to the scene, so every panel showing it draws the same set.
#[derive(Component, Clone, Copy)]
pub struct GuideLine {
    /// The panel content entity carrying this stage's
    /// [`Viewport2dPanelHost`].
    pub host: Entity,
    /// Which way the line runs.
    pub axis: CanvasAxis,
    /// Which guide of that axis it draws.
    pub index: usize,
}

/// The outline drawn around one selected authored UI node in one panel.
#[derive(Component, Clone, Copy)]
pub struct UiSelectionOverlay {
    /// The panel content entity carrying this stage's
    /// [`Viewport2dPanelHost`].
    pub host: Entity,
    /// The authored node this outline is drawn around.
    pub node: Entity,
    /// Whether this is the primary selection's outline, which is the one
    /// carrying the resize handles and the one the gestures are delivered
    /// to.
    pub primary: bool,
    /// Per axis, whether the node is small enough that this axis's handles
    /// are drawn outside it. Kept so the handles are re-placed when a
    /// resize crosses the size at which they move out, and not on every
    /// frame.
    pub handles_outside: BVec2,
}

/// The outline drawn around the authored UI node under the cursor, showing
/// what a press would pick. A lighter line than [`UiSelectionOverlay`],
/// with no handles, and never drawn over a selected node.
#[derive(Component, Clone, Copy)]
pub struct UiHoverOutline {
    /// The panel content entity carrying this stage's
    /// [`Viewport2dPanelHost`].
    pub host: Entity,
}

/// The node the cursor is over, and the panel it is over it on. One entry
/// covers every panel, since one pointer authors at a time.
#[derive(Resource, Default)]
pub struct UiHoverPreselect {
    /// The panel content entity carrying the stage the cursor is over.
    pub host: Option<Entity>,
    /// The authored node under the cursor.
    pub entity: Option<Entity>,
}

/// One authored node a stage click could land on.
#[derive(Clone, Copy, Debug)]
pub struct StageHit {
    pub entity: Entity,
    /// The node's rect in render-target pixels.
    pub rect: Rect,
    /// Bevy's own paint order for the node, from [`ComputedStackIndex`].
    pub stack: u32,
}

/// One node a gesture is editing.
struct GestureNode {
    entity: Entity,
    /// The `Node` as the gesture found it, for the undo entry and Escape.
    before: Node,
    /// Authored-pixel rect at gesture start: left, top, width, height.
    start: Vec4,
    /// How the node was positioned when the gesture began; the drag writes
    /// back through this. See [`NodeAnchors`].
    anchors: NodeAnchors,
    /// What this node's units are measured against, in authored pixels.
    /// Read once at the press.
    basis: UnitBasis,
}

/// The gesture in progress, if any. One gesture is one history entry.
///
/// A move carries the whole selection, each node keeping its own start rect
/// and scheme. A resize carries the primary alone, since the handles are
/// drawn around the primary's rect.
#[derive(Resource, Default)]
pub struct UiManipulation {
    /// Every node the gesture is editing, primary first. Empty when no
    /// gesture is running.
    nodes: Vec<GestureNode>,
    /// Edges being dragged; `(0, 0)` is a move.
    edges: (i8, i8),
    /// The panel the gesture is running on, so every drag event can ask
    /// it what the view is doing now. See [`live_scale`].
    host: Option<Entity>,
    /// Authored pixels per pointer-logical pixel, as of the press. What
    /// the gesture falls back on if the panel goes away under it.
    scale: f32,
    /// The panel's pixel lattice, in authored pixels: a copy of
    /// [`crate::viewport_2d::Ui2dView::grid`] taken at the press.
    grid: f32,
    /// Edges the gesture can land on, in the same authored pixels the
    /// primary's start rect is stated in. Gathered once, around the primary
    /// alone: the primary snaps and the rest of the selection moves by the
    /// delta it snapped to, so the selection cannot be pulled apart.
    candidates: SnapCandidates,
    /// Which kinds of line this gesture may land on, copied off
    /// [`CanvasSnap`] at the press.
    kinds: CanvasSnap,
    /// What the last drag event came to rest against, for the highlight
    /// drawn over the stage.
    last_snap: SnapOutcome,
}

impl UiManipulation {
    /// What the gesture's last drag event landed on, or an outcome with
    /// no landings when nothing is being dragged.
    pub fn last_snap(&self) -> SnapOutcome {
        self.last_snap
    }

    /// Whether a canvas gesture is in flight.
    pub fn is_running(&self) -> bool {
        !self.nodes.is_empty()
    }
}

/// Where one line a dragged edge can land on sits, and what it is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Candidate {
    /// The line's coordinate, in the authored offsets a node's own
    /// `left`/`top` are stated in.
    pub at: f32,
    /// What kind of line it is, so a landing can be drawn and filtered.
    pub kind: CandidateKind,
    /// Where the line sits in the parent box as a percentage, when it has
    /// one. A percent-authored offset takes this figure verbatim rather
    /// than one derived from pixels.
    pub percent: Option<f32>,
}

/// What a line a drag can land on came from. One per
/// [`crate::canvas_snap::CanvasSnapKind`] that offers lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CandidateKind {
    /// A line of the parent box a percentage names.
    PercentLine,
    /// A padding-box edge or the centre of the parent.
    Parent,
    /// A sibling's near or far edge.
    SiblingSide,
    /// A sibling's centre.
    SiblingCentre,
    /// One of the scene's guides.
    Guide,
    /// A node elsewhere in the scene, outside the dragged node's family.
    OtherNode,
}

/// The lines a gesture can land on, per axis, in precedence order.
#[derive(Default)]
struct SnapCandidates {
    x: Vec<Candidate>,
    y: Vec<Candidate>,
    /// The parent's padding-box corner in global authored pixels, which a
    /// candidate's coordinate is measured from.
    origin: Vec2,
}

/// The line one axis of a drag came to rest against.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SnapWinner {
    /// The line's coordinate, measured from the parent's padding-box
    /// corner rather than from the canvas.
    pub at: f32,
    pub kind: CandidateKind,
    /// The line's percentage of the parent box, when it has one.
    pub percent: Option<f32>,
    /// Which line of the dragged rect landed on it.
    pub line: SnapLine,
}

/// What one drag event's snapping came to.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct SnapOutcome {
    /// How far the drag has to move on top of the cursor's own
    /// distance, in authored pixels.
    pub nudge: Vec2,
    /// The line the x axis landed on, if any.
    pub x: Option<SnapWinner>,
    /// The line the y axis landed on, if any.
    pub y: Option<SnapWinner>,
}

impl SnapOutcome {
    /// The percentages this outcome lets a percent-authored offset be
    /// written as, rather than as a figure derived from pixels.
    fn exact_percent(&self) -> ExactPercent {
        let landing = |winner: &Option<SnapWinner>| {
            winner.and_then(|winner| {
                winner.percent.map(|percent| PercentLanding {
                    line: winner.line,
                    percent,
                })
            })
        };
        ExactPercent {
            x: landing(&self.x),
            y: landing(&self.y),
        }
    }
}

/// A landing on a line the parent box states as a percentage.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PercentLanding {
    /// Which line of the dragged rect landed on it.
    pub line: SnapLine,
    /// The line's percentage of the parent box.
    pub percent: f32,
}

/// What a gesture landed on, per axis, for the offsets it is about to
/// write. Default is "nothing exact".
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct ExactPercent {
    pub x: Option<PercentLanding>,
    pub y: Option<PercentLanding>,
}

/// How finely the pixels a gesture writes are stated.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum PixelRounding {
    /// Whole authored pixels, the canvas's own lattice.
    #[default]
    Whole,
    /// Two decimal places, keeping the fraction a zoomed-in drag produced.
    Fractional,
}

/// The lines of the parent box a percentage names, as percentages: the
/// quarters and the thirds.
const PERCENT_LINES: [f32; 7] = [0.0, 25.0, 100.0 / 3.0, 50.0, 200.0 / 3.0, 75.0, 100.0];

/// How finely a gesture states the pixels it writes. Keyed on the pixel
/// kind alone, so Ctrl and the master magnet cannot change the units a
/// drag commits.
fn pixel_rounding(kinds: &CanvasSnap) -> PixelRounding {
    if kinds.pixel {
        PixelRounding::Whole
    } else {
        PixelRounding::Fractional
    }
}

/// The unit half of an authored [`Val`], for the values a gesture writes
/// back. `Val::Auto` has no unit and is absent instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnchorUnit {
    Px,
    Percent,
    Vw,
    Vh,
    VMin,
    VMax,
}

impl AnchorUnit {
    /// The unit `value` is stated in, or `None` for `Val::Auto`, which
    /// is not a length the gesture can put a number back into.
    pub fn of(value: Val) -> Option<Self> {
        match value {
            Val::Auto => None,
            Val::Px(_) => Some(Self::Px),
            Val::Percent(_) => Some(Self::Percent),
            Val::Vw(_) => Some(Self::Vw),
            Val::Vh(_) => Some(Self::Vh),
            Val::VMin(_) => Some(Self::VMin),
            Val::VMax(_) => Some(Self::VMax),
        }
    }

    fn build(self, magnitude: f32) -> Val {
        match self {
            Self::Px => Val::Px(magnitude),
            Self::Percent => Val::Percent(magnitude),
            Self::Vw => Val::Vw(magnitude),
            Self::Vh => Val::Vh(magnitude),
            Self::VMin => Val::VMin(magnitude),
            Self::VMax => Val::VMax(magnitude),
        }
    }

    /// How many authored pixels one of this unit is worth, or `None` when
    /// nothing it is measured against has a usable size yet. A refusal
    /// rather than a fallback to pixels, so a parent measuring zero for one
    /// frame cannot rewrite `50%` as `Val::Px`.
    fn authored_px(self, parent: f32, viewport: Vec2) -> Option<f32> {
        let per = match self {
            Self::Px => 1.0,
            Self::Percent => parent / 100.0,
            Self::Vw => viewport.x / 100.0,
            Self::Vh => viewport.y / 100.0,
            Self::VMin => viewport.min_element() / 100.0,
            Self::VMax => viewport.max_element() / 100.0,
        };
        (per > 0.0 && per.is_finite()).then_some(per)
    }

    /// Decimals a magnitude in this unit is rounded to on the way back out:
    /// whole pixels while the canvas is on its pixel lattice, two places
    /// otherwise. A landing on one of the [`PERCENT_LINES`] is written whole
    /// instead and does not come through here; see [`exact_percent_for`].
    fn round(self, magnitude: f32, rounding: PixelRounding) -> f32 {
        match (self, rounding) {
            (Self::Px, PixelRounding::Whole) => magnitude.round(),
            _ => (magnitude * 100.0).round() / 100.0,
        }
    }
}

/// How one axis of a node is positioned, as its author wrote it: which
/// of the two offsets they set, whether they gave it a size, and the
/// unit each of the three is stated in.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct AxisAnchor {
    /// `left`, or `top`.
    pub near: Option<AnchorUnit>,
    /// `right`, or `bottom`.
    pub far: Option<AnchorUnit>,
    /// `width`, or `height`.
    pub size: Option<AnchorUnit>,
}

impl AxisAnchor {
    fn of(near: Val, far: Val, size: Val) -> Self {
        Self {
            near: AnchorUnit::of(near),
            far: AnchorUnit::of(far),
            size: AnchorUnit::of(size),
        }
    }

    /// Whether the axis is a stretch: both offsets pinned and no size of
    /// its own. All three set is over-constrained, which Bevy resolves by
    /// the near offset and the size, so the size is what a resize writes.
    fn stretched(self) -> bool {
        self.near.is_some() && self.far.is_some() && self.size.is_none()
    }
}

/// How a node is positioned on both axes.
///
/// A gesture computes a rect in authored pixels and projects it back
/// through the scheme it found, so a node pinned to its parent's
/// bottom-right corner stays pinned there and a percentage stays a
/// percentage. The projection:
///
/// - an offset the author set is written, in the unit they wrote it in;
/// - an offset they left `Auto` stays `Auto`;
/// - `right`/`bottom` take the far edge measured from the parent's far
///   edge, so a move slides them by the negated delta;
/// - a size is written only when the gesture resized, and never on a
///   stretched axis, where the two offsets already say what the size is;
/// - a node with neither offset set is a flex child the drag promotes,
///   and is placed from the near edge in pixels.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct NodeAnchors {
    pub x: AxisAnchor,
    pub y: AxisAnchor,
}

impl NodeAnchors {
    /// The scheme `node` is written in.
    pub fn of(node: &Node) -> Self {
        Self {
            x: AxisAnchor::of(node.left, node.right, node.width),
            y: AxisAnchor::of(node.top, node.bottom, node.height),
        }
    }
}

/// What the units on a node's `Val`s are measured against, in authored
/// pixels.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct UnitBasis {
    /// The parent's offset box: what a percentage on this node is a
    /// percentage of, and what a `right`/`bottom` offset is measured
    /// back from.
    pub parent: Vec2,
    /// The canvas the scene lays out in: what `vw`, `vh`, `vmin` and
    /// `vmax` are stated against. A routed UI scene is pinned to its
    /// render target, so this is the panel's target size.
    pub viewport: Vec2,
}

/// Write one axis of a manipulated rect back through `anchor`.
///
/// `min` and `extent` are the axis of the new rect, in authored pixels from
/// the parent's offset box, and `parent` is that box's extent on the same
/// axis. `resized` says the gesture moved an edge rather than the whole
/// node. `exact` is the axis's landing when it came to rest on a line the
/// parent box states as a percentage, which a percent-authored offset takes
/// verbatim. A value whose unit has nothing usable to be measured against
/// is left alone; see [`AnchorUnit::authored_px`].
#[expect(
    clippy::too_many_arguments,
    reason = "one axis of a rect, its scheme, and what it is measured against"
)]
fn write_axis(
    near: &mut Val,
    far: &mut Val,
    size: &mut Val,
    anchor: AxisAnchor,
    min: f32,
    extent: f32,
    resized: bool,
    placed: bool,
    parent: f32,
    viewport: Vec2,
    rounding: PixelRounding,
    exact: Option<PercentLanding>,
) {
    let write = |slot: &mut Val, unit: AnchorUnit, authored: f32| {
        if let Some(per) = unit.authored_px(parent, viewport) {
            *slot = unit.build(unit.round(authored / per, rounding));
        }
    };

    let near_unit = anchor
        .near
        .or_else(|| anchor.far.is_none().then_some(AnchorUnit::Px));
    if let Some(unit) = near_unit
        && placed
    {
        match exact_percent_for(unit, exact, SnapLine::Min) {
            Some(percent) => *near = Val::Percent(percent),
            None => write(near, unit, min),
        }
    }
    // The far offset is measured against the parent, so it needs one.
    if let Some(unit) = anchor.far
        && placed
        && parent > 0.0
        && parent.is_finite()
    {
        match exact_percent_for(unit, exact, SnapLine::Max) {
            Some(percent) => *far = Val::Percent(100.0 - percent),
            None => write(far, unit, parent - (min + extent)),
        }
    }
    if resized && !anchor.stretched() {
        write(size, anchor.size.unwrap_or(AnchorUnit::Px), extent);
    }
}

/// The percentage an offset written in percent takes verbatim, when the
/// landing was on `wanted`: the near edge for `left`/`top` and the far
/// edge for `right`/`bottom`. A centre landing names no edge and yields
/// nothing.
fn exact_percent_for(
    unit: AnchorUnit,
    exact: Option<PercentLanding>,
    wanted: SnapLine,
) -> Option<f32> {
    let landing = exact?;
    (unit == AnchorUnit::Percent && landing.line == wanted).then_some(landing.percent)
}

/// Write a manipulated rect back into `node` through the scheme its author
/// wrote it in. See [`NodeAnchors`].
///
/// `rect` is `(left, top, width, height)` in authored pixels from the
/// parent's offset box; `edges` is the gesture's, `(0, 0)` for a move.
/// `rounding` says how finely pixels are stated, and `exact` carries any
/// percentage the gesture landed on.
///
/// A move on a flowed child promotes it to `PositionType::Absolute` and
/// writes offsets. A resize says nothing about placement, so on a flowed
/// child it writes `width`/`height` alone and leaves the position to the
/// parent. A node already absolute keeps both halves on either gesture.
pub fn apply_authored_rect(
    node: &mut Node,
    anchors: NodeAnchors,
    rect: Vec4,
    edges: (i8, i8),
    basis: UnitBasis,
    rounding: PixelRounding,
    exact: ExactPercent,
) {
    let moving = edges == (0, 0);
    let placed = moving || node.position_type == PositionType::Absolute;
    if moving {
        node.position_type = PositionType::Absolute;
    }
    write_axis(
        &mut node.left,
        &mut node.right,
        &mut node.width,
        anchors.x,
        rect.x,
        rect.z,
        edges.0 != 0,
        placed,
        basis.parent.x,
        basis.viewport,
        rounding,
        exact.x,
    );
    write_axis(
        &mut node.top,
        &mut node.bottom,
        &mut node.height,
        anchors.y,
        rect.y,
        rect.w,
        edges.1 != 0,
        placed,
        basis.parent.y,
        basis.viewport,
        rounding,
        exact.y,
    );
}

pub struct UiStagePlugin;

impl Plugin for UiStagePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiManipulation>()
            .init_resource::<GuideManipulation>()
            .init_resource::<UiHoverPreselect>()
            .init_resource::<MarqueeSelect>()
            .init_resource::<StageHitCache>()
            .add_systems(First, drop_stage_hit_cache)
            .add_observer(on_stage_press)
            .add_observer(on_stage_double_press)
            .add_observer(on_stage_asset_drop)
            .add_observer(on_stage_hover)
            .add_observer(on_stage_leave)
            .add_observer(on_marquee_start)
            .add_observer(on_marquee_drag)
            .add_observer(on_marquee_end)
            .add_observer(on_gesture_start)
            .add_observer(on_gesture_drag)
            .add_observer(on_gesture_end)
            .add_observer(on_guide_press)
            .add_observer(on_guide_drag_start)
            .add_observer(on_guide_drag)
            .add_observer(on_guide_drag_end)
            .add_systems(
                Update,
                (
                    cancel_manipulation,
                    cancel_marquee,
                    cancel_guide_drag,
                    sync_selection_overlays,
                    sync_hover_outlines,
                    sync_marquee_overlay,
                    sync_guide_lines,
                    sync_snap_highlights,
                    sync_drag_readouts,
                )
                    .chain(),
            );
    }
}

/// Authored point (render-target pixels, origin at the canvas's top-left
/// corner) under a cursor sitting `stage_offset` render-target pixels from
/// the centre of the stage, as [`cursor_stage_offset`] reports it.
pub fn stage_to_authored(stage_offset: Vec2, target_size: UVec2) -> Vec2 {
    stage_offset + target_size.as_vec2() / 2.0
}

/// Inverse of [`stage_to_authored`].
pub fn authored_to_stage(authored: Vec2, target_size: UVec2) -> Vec2 {
    authored - target_size.as_vec2() / 2.0
}

/// Stage-node logical pixels per render-target pixel: what an authored
/// measurement is multiplied by to become an overlay `Node` value.
///
/// `target_scale` is [`target_pixels_per_stage_pixel`] for the panel and
/// `inverse_scale_factor` is the stage's own
/// `ComputedNode::inverse_scale_factor()`. A degenerate stage yields the
/// identity rather than an infinity.
pub fn stage_pixels_per_target_pixel(target_scale: f32, inverse_scale_factor: f32) -> f32 {
    if target_scale <= 0.0 {
        return 1.0;
    }
    inverse_scale_factor / target_scale
}

/// Authored (render-target) pixels per pointer pixel: what a
/// [`Pointer<Drag>`] distance is multiplied by to become an authored delta.
///
/// `stage_scale` is [`stage_pixels_per_target_pixel`] inverted with the
/// [`UiScale`] taken back out, since pointer locations are reported before
/// the UI scale is applied. A degenerate factor yields the identity.
pub fn target_pixels_per_pointer_pixel(stage_scale: f32, ui_scale: f32) -> f32 {
    let factor = stage_scale * ui_scale;
    if factor <= 0.0 {
        return 1.0;
    }
    1.0 / factor
}

/// The authored node a click at `point` lands on, or `None` when it misses
/// every one: the one Bevy paints last, the highest [`ComputedStackIndex`].
///
/// The tree-order tiebreak covers the frame before a stack pass has run,
/// where every index reads `0`. `hits` is built depth-first, so taking the
/// last entry matches what Bevy would paint last.
pub fn topmost_hit(point: Vec2, hits: &[StageHit]) -> Option<Entity> {
    hits.iter()
        .enumerate()
        .filter(|(_, hit)| hit.rect.contains(point))
        .max_by_key(|(ordinal, hit)| (hit.stack, *ordinal))
        .map(|(_, hit)| hit.entity)
}

/// Authored UI nodes: everything the panel could select, which is
/// everything under a routed root that is not editor chrome.
type AuthoredNodes<'w, 's> = Query<
    'w,
    's,
    (
        &'static ComputedNode,
        &'static UiGlobalTransform,
        Option<&'static ComputedStackIndex>,
        Option<&'static Locked>,
    ),
    Without<EditorEntity>,
>;

/// The selected node as the overlay reads it: its rect and the camera it
/// draws into, and never a piece of editor chrome.
type SelectedNode<'w, 's> = Query<
    'w,
    's,
    (
        &'static ComputedNode,
        &'static UiGlobalTransform,
        &'static ComputedUiTargetCamera,
    ),
    Without<EditorEntity>,
>;

/// What a cursor over a panel's stage resolves to.
pub(crate) enum StagePick {
    /// The authored node under the cursor.
    Hit(Entity),
    /// The stage is showing a scene and the cursor is over none of it.
    Miss,
    /// Nothing is routed to this panel, or the cursor is not over its stage
    /// at all, so it says nothing about this panel's selection.
    Empty,
}

/// The authored node `cursor` (ui-logical pixels) is over on `host`'s
/// stage. The one place the stage's pixels become an authored node.
/// Every authored node routed to one camera, in paint order, gathered once
/// a frame and thrown away at the start of the next. Layout is written once
/// a frame and nothing an observer does moves a rect, so several observers
/// asking within one frame get the same answer. One camera's worth; a
/// question about another panel refills it.
#[derive(Resource, Default)]
pub struct StageHitCache {
    camera: Option<Entity>,
    hits: Vec<StageHit>,
}

/// Throw last frame's hit list away, before anything reads one.
fn drop_stage_hit_cache(mut cache: ResMut<StageHitCache>) {
    cache.camera = None;
    cache.hits.clear();
}

/// The hit list for `camera`, from the cache when this frame has already
/// gathered it.
fn stage_hits<'cache>(
    camera: Entity,
    roots: &Query<(Entity, &UiTargetCamera), AuthoredUiSceneRoot>,
    nodes: &AuthoredNodes,
    children: &Query<&Children>,
    cache: &'cache mut StageHitCache,
) -> &'cache [StageHit] {
    if cache.camera != Some(camera) {
        cache.camera = Some(camera);
        cache.hits.clear();
        for (root, routed) in roots {
            if routed.entity() == camera {
                collect_stage_hits(root, nodes, children, &mut cache.hits);
            }
        }
    }
    &cache.hits
}

pub(crate) fn hit_at(
    cursor: Vec2,
    host: &Viewport2dPanelHost,
    stage: (&ComputedNode, &UiGlobalTransform),
    roots: &Query<(Entity, &UiTargetCamera), AuthoredUiSceneRoot>,
    nodes: &AuthoredNodes,
    children: &Query<&Children>,
    cache: &mut StageHitCache,
) -> StagePick {
    let (computed, transform) = stage;
    let target_scale = target_pixels_per_stage_pixel(computed.size(), host.target_size);
    let Some(offset) = cursor_stage_offset(
        cursor,
        transform.translation,
        computed.size(),
        computed.inverse_scale_factor(),
        target_scale,
    ) else {
        return StagePick::Empty;
    };
    let point = stage_to_authored(offset, host.target_size);

    let hits = stage_hits(host.camera, roots, nodes, children, cache);
    if hits.is_empty() {
        return StagePick::Empty;
    }

    match topmost_hit(point, hits) {
        Some(entity) => StagePick::Hit(entity),
        None => StagePick::Miss,
    }
}

/// The rubber band a drag from bare canvas is pulling out. A drag that
/// started on a node is that node's move instead; the two never both run.
#[derive(Resource, Default)]
pub struct MarqueeSelect {
    band: Option<Marquee>,
    seed: Option<MarqueeSeed>,
}

/// What the press left behind for a band that may follow it: the selection
/// as it was *before* the press, since a press on the backdrop selects the
/// scene's root and a band is not a selection of the root.
struct MarqueeSeed {
    /// The panel content entity the press landed on.
    host: Entity,
    base: Vec<Entity>,
    extend: bool,
    toggle: bool,
}

impl MarqueeSelect {
    /// The band's two corners in authored pixels, or `None` when no band
    /// is being pulled out.
    pub fn corners(&self) -> Option<(Vec2, Vec2)> {
        self.band.as_ref().map(|band| (band.start, band.current))
    }
}

/// One band being pulled out over one panel's canvas.
struct Marquee {
    host: Entity,
    stage: Entity,
    /// The entity the drag events are being delivered to: the stage, or
    /// the selection outline lying over it when the backdrop is what is
    /// selected.
    target: Entity,
    /// Where the drag began, in authored pixels.
    start: Vec2,
    /// Where the cursor is now, in the same pixels.
    current: Vec2,
    /// Shift and Ctrl as of the press: whether what the band gathers is
    /// added to, or toggled against, the seed selection.
    extend: bool,
    toggle: bool,
    /// That selection, so a band swept back and forth answers the state the
    /// gesture started from.
    base: Vec<Entity>,
}

/// The band drawn over one panel's stage.
#[derive(Component, Clone, Copy)]
pub struct MarqueeOverlay {
    /// The panel content entity carrying this stage's
    /// [`Viewport2dPanelHost`].
    pub host: Entity,
}

/// Draw order of the band: over the canvas and over the selection outlines.
const MARQUEE_Z: i32 = OVERLAY_Z + 2;

/// Where `cursor` (ui-logical pixels) is on the canvas `stage` is drawing,
/// in authored pixels. Unbounded, since a band is pulled past the canvas
/// edge as readily as across it.
fn authored_at(
    cursor: Vec2,
    host: &Viewport2dPanelHost,
    stage: (&ComputedNode, &UiGlobalTransform),
) -> Vec2 {
    let (computed, transform) = stage;
    let target_scale = target_pixels_per_stage_pixel(computed.size(), host.target_size);
    let offset = crate::viewport_2d::stage_offset_unbounded(
        cursor,
        transform.translation,
        computed.inverse_scale_factor(),
        target_scale,
    );
    stage_to_authored(offset, host.target_size)
}

/// Start a band when a drag begins on the canvas rather than on a node.
///
/// The canvas is the backdrop: bare stage, or the scene's own root, which
/// fills it. The drag can be delivered to the stage or to the selection
/// outline lying over it, so the band remembers which and answers only that
/// one.
fn on_marquee_start(
    mut event: On<Pointer<DragStart>>,
    ui_scale: Res<UiScale>,
    overlays: Query<&UiSelectionOverlay>,
    handles: Query<(), With<UiResizeHandle>>,
    hosts: Query<(Entity, &Viewport2dPanelHost)>,
    stages: Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dViewport>>,
    roots: Query<(Entity, &UiTargetCamera), AuthoredUiSceneRoot>,
    nodes: AuthoredNodes,
    children: Query<&Children>,
    selection: Res<Selection>,
    mut marquee: ResMut<MarqueeSelect>,
    mut hit_cache: ResMut<StageHitCache>,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let target = event.event_target();
    // A handle is a resize whatever is under it.
    if handles.contains(target) {
        return;
    }
    // A drag on an outline is that node's move, unless the node is the
    // scene's own root, whose outline covers the whole canvas.
    let panel = match overlays.get(target) {
        Ok(overlay) => selection
            .primary()
            .filter(|&primary| roots.contains(primary))
            .map(|_| overlay.host),
        Err(_) => hosts
            .iter()
            .find(|(_, host)| host.stage == target)
            .map(|(panel, _)| panel),
    };
    let Some((panel, host)) = panel.and_then(|panel| hosts.get(panel).ok()) else {
        return;
    };
    if host.mode != Viewport2dMode::Edit {
        return;
    }
    let Ok(stage) = stages.get(host.stage) else {
        return;
    };
    let cursor = event.pointer_location.position / ui_scale.0;
    if let StagePick::Hit(entity) = hit_at(
        cursor,
        host,
        stage,
        &roots,
        &nodes,
        &children,
        &mut hit_cache,
    ) && !roots.contains(entity)
    {
        marquee.seed = None;
        return;
    }
    event.propagate(false);
    let seed = marquee
        .seed
        .take()
        .filter(|seed| seed.host == panel)
        .unwrap_or(MarqueeSeed {
            host: panel,
            base: Vec::new(),
            extend: false,
            toggle: false,
        });
    let at = authored_at(cursor, host, stage);
    marquee.band = Some(Marquee {
        host: panel,
        stage: host.stage,
        target,
        start: at,
        current: at,
        extend: seed.extend,
        toggle: seed.toggle,
        base: seed.base,
    });
}

fn on_marquee_drag(
    mut event: On<Pointer<Drag>>,
    ui_scale: Res<UiScale>,
    hosts: Query<&Viewport2dPanelHost>,
    stages: Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dViewport>>,
    mut marquee: ResMut<MarqueeSelect>,
) {
    let Some(band) = marquee.band.as_mut() else {
        return;
    };
    if event.event_target() != band.target {
        return;
    }
    event.propagate(false);
    let Ok(host) = hosts.get(band.host) else {
        return;
    };
    let Ok(stage) = stages.get(band.stage) else {
        return;
    };
    band.current = authored_at(event.pointer_location.position / ui_scale.0, host, stage);
}

/// Take everything the band was pulled across. Intersection rather than
/// containment, so a container wider than the panel does not have to be
/// swept end to end. The scene's own root is never picked up.
fn on_marquee_end(
    mut event: On<Pointer<DragEnd>>,
    hosts: Query<&Viewport2dPanelHost>,
    roots: Query<(Entity, &UiTargetCamera), AuthoredUiSceneRoot>,
    nodes: AuthoredNodes,
    children: Query<&Children>,
    mut selection: ResMut<Selection>,
    mut marquee: ResMut<MarqueeSelect>,
    mut commands: Commands,
    mut hit_cache: ResMut<StageHitCache>,
) {
    let Some(band) = marquee.band.take() else {
        return;
    };
    if event.event_target() != band.target {
        marquee.band = Some(band);
        return;
    }
    event.propagate(false);
    let Ok(host) = hosts.get(band.host) else {
        return;
    };
    let rect = Rect::from_corners(band.start, band.current);

    let scene_roots: Vec<Entity> = roots
        .iter()
        .filter(|(_, routed)| routed.entity() == host.camera)
        .map(|(root, _)| root)
        .collect();
    let swept: Vec<Entity> = stage_hits(host.camera, &roots, &nodes, &children, &mut hit_cache)
        .iter()
        .filter(|hit| !scene_roots.contains(&hit.entity))
        .filter(|hit| overlaps(rect, hit.rect))
        .map(|hit| hit.entity)
        .collect();

    let wanted = if band.extend {
        let mut wanted = band.base.clone();
        wanted.extend(swept.iter().filter(|entity| !band.base.contains(entity)));
        wanted
    } else if band.toggle {
        let mut wanted: Vec<Entity> = band
            .base
            .iter()
            .copied()
            .filter(|entity| !swept.contains(entity))
            .collect();
        wanted.extend(swept.iter().filter(|entity| !band.base.contains(entity)));
        wanted
    } else {
        swept
    };
    selection.select_multiple(&mut commands, &wanted);
}

/// Whether two rects share any area. A band pulled out with no width or
/// height still touches what it lies across.
fn overlaps(left: Rect, right: Rect) -> bool {
    left.min.x <= right.max.x
        && left.max.x >= right.min.x
        && left.min.y <= right.max.y
        && left.max.y >= right.min.y
}

/// Escape drops the band and leaves the selection as the press left it. The
/// seed goes with it, so the next band does not build on an abandoned one.
fn cancel_marquee(
    keys: Res<ButtonInput<KeyCode>>,
    focus: crate::keybind_focus::KeybindFocus,
    mut marquee: ResMut<MarqueeSelect>,
) {
    if focus.keyboard_is_spoken_for() {
        return;
    }
    if marquee.band.is_some() && keys.just_pressed(KeyCode::Escape) {
        marquee.band = None;
        marquee.seed = None;
    }
}

/// Keep one band drawn over the panel it is being pulled out on.
fn sync_marquee_overlay(
    mut commands: Commands,
    marquee: Res<MarqueeSelect>,
    hosts: Query<&Viewport2dPanelHost>,
    stages: Query<&ComputedNode, With<Scene2dViewport>>,
    bands: Query<(Entity, &MarqueeOverlay)>,
    mut nodes: Query<&mut Node>,
) {
    let wanted = marquee.band.as_ref().and_then(|band| {
        let host = hosts.get(band.host).ok()?;
        let stage = stages.get(band.stage).ok()?;
        let target_scale = target_pixels_per_stage_pixel(stage.size(), host.target_size);
        let scale = stage_pixels_per_target_pixel(target_scale, stage.inverse_scale_factor());
        Some((
            band.host,
            band.stage,
            Rect::from_corners(band.start * scale, band.current * scale),
        ))
    });

    for (entity, overlay) in &bands {
        match wanted {
            Some((host, _, rect)) if host == overlay.host => {
                if let Ok(mut node) = nodes.get_mut(entity) {
                    place_outline(&mut node, rect);
                }
            }
            _ => {
                if let Ok(mut entity) = commands.get_entity(entity) {
                    entity.despawn();
                }
            }
        }
    }

    if let Some((host, stage, rect)) = wanted
        && !bands.iter().any(|(_, overlay)| overlay.host == host)
    {
        let mut node = Node {
            position_type: PositionType::Absolute,
            border: UiRect::all(px(OUTLINE_WIDTH)),
            ..default()
        };
        place_outline(&mut node, rect);
        commands.spawn((
            MarqueeOverlay { host },
            EditorEntity,
            node,
            BackgroundColor(crate::default_style::SELECTION_MARQUEE_BG),
            BorderColor::all(crate::default_style::SELECTION_MARQUEE_BORDER),
            ZIndex(MARQUEE_Z),
            Pickable::IGNORE,
            ChildOf(stage),
        ));
    }
}

/// The label a running gesture draws beside the node it is moving.
#[derive(Component, Clone, Copy)]
pub struct DragReadout {
    /// The panel content entity carrying this stage's
    /// [`Viewport2dPanelHost`].
    pub host: Entity,
}

/// The line of the readout stating what the gesture is writing: where the
/// node is, or how big it is.
#[derive(Component)]
pub struct DragReadoutMeasure;

/// The line of the readout stating how far the dragged node is from its
/// nearest neighbour on each axis.
#[derive(Component)]
pub struct DragReadoutSpacing;

/// How far from the node's bottom-right corner the readout sits, in the
/// stage's logical pixels: clear of the corner handle.
const READOUT_OFFSET: f32 = HANDLE_SIZE;

/// Draw order of the readout: above everything else the gesture draws.
const READOUT_Z: i32 = OVERLAY_Z + 3;

/// What a gesture's readout says this frame.
struct Readout {
    host: Entity,
    stage: Entity,
    /// Where the label goes, in the stage's logical pixels.
    at: Vec2,
    /// The position or the size, in authored units.
    measure: String,
    /// The gaps to the nearest neighbouring edge per axis, or empty when
    /// there is no neighbour within reach on either.
    spacing: String,
}

/// A figure the readout states, in whole authored pixels.
fn readout_figure(value: f32) -> String {
    format!("{}", value.round())
}

/// The distance from the dragged rect's edges on one axis to the nearest
/// sibling edge, or `None` when no sibling offers one. Read off the
/// gesture's own candidates, so the figure matches the geometry the magnet
/// lands on.
///
/// Signed: a sibling edge outside the dragged rect is a gap and reads
/// positive, one that has crossed inside reads negative. An overlap wins a
/// tie against a gap of the same size.
fn nearest_sibling_gap(candidates: &[Candidate], near: f32, far: f32) -> Option<f32> {
    candidates
        .iter()
        .filter(|candidate| candidate.kind == CandidateKind::SiblingSide)
        .map(|candidate| edge_gap(candidate.at, near, far))
        .min_by(|left, right| {
            left.abs()
                .total_cmp(&right.abs())
                .then_with(|| left.total_cmp(right))
        })
}

/// How far the sibling edge at `at` is from the dragged span `near..far`:
/// positive outside it, negative once it has crossed inside.
fn edge_gap(at: f32, near: f32, far: f32) -> f32 {
    if at <= near {
        near - at
    } else if at >= far {
        at - far
    } else {
        -(at - near).min(far - at)
    }
}

/// What the running gesture wants drawn, or `None` when nothing is being
/// dragged on a canvas being authored.
fn gesture_readout(world: &World) -> Option<Readout> {
    let manipulation = world.get_resource::<UiManipulation>()?;
    let primary = manipulation.nodes.first()?;
    let host_entity = manipulation.host?;
    let host = world.get::<Viewport2dPanelHost>(host_entity)?;
    if host.mode != Viewport2dMode::Edit {
        return None;
    }
    let stage = world.get::<ComputedNode>(host.stage)?;
    let target_scale = target_pixels_per_stage_pixel(stage.size(), host.target_size);
    let scale = stage_pixels_per_target_pixel(target_scale, stage.inverse_scale_factor());

    let global = global_node_rect(world, primary.entity)?;
    let offsets = authored_rect(world, primary.entity)?;
    let measure = if manipulation.edges == (0, 0) {
        format!(
            "{}, {}",
            readout_figure(offsets.min.x),
            readout_figure(offsets.min.y)
        )
    } else {
        format!(
            "{} x {}",
            readout_figure(offsets.width()),
            readout_figure(offsets.height())
        )
    };

    let gaps = [
        nearest_sibling_gap(&manipulation.candidates.x, offsets.min.x, offsets.max.x)
            .map(|gap| format!("x {}", readout_figure(gap))),
        nearest_sibling_gap(&manipulation.candidates.y, offsets.min.y, offsets.max.y)
            .map(|gap| format!("y {}", readout_figure(gap))),
    ];
    let spacing = gaps.into_iter().flatten().collect::<Vec<_>>().join("  ");

    Some(Readout {
        host: host_entity,
        stage: host.stage,
        at: global.max * scale + Vec2::splat(READOUT_OFFSET),
        measure,
        spacing,
    })
}

/// Keep one readout drawn beside the node a gesture is dragging. Exclusive,
/// so it reads the gesture's own geometry through the helpers the gesture
/// writes through.
fn sync_drag_readouts(
    world: &mut World,
    // Kept across frames: a fresh `QueryState` walks every archetype in the
    // world, and this system runs whether or not a gesture is going on.
    mut drawn: Local<Option<QueryState<(Entity, &'static DragReadout)>>>,
) {
    let wanted = gesture_readout(world);
    let drawn = drawn.get_or_insert_with(|| world.query::<(Entity, &DragReadout)>());
    let existing: Vec<(Entity, Entity)> = drawn
        .iter(world)
        .map(|(entity, readout)| (entity, readout.host))
        .collect();

    let Some(readout) = wanted else {
        for (entity, _) in existing {
            world.entity_mut(entity).despawn();
        }
        return;
    };

    for (entity, host) in &existing {
        if *host != readout.host {
            world.entity_mut(*entity).despawn();
        }
    }
    let label = existing
        .iter()
        .find(|(_, host)| *host == readout.host)
        .map(|(entity, _)| *entity);

    let label = match label {
        Some(label) => {
            if let Some(mut node) = world.get_mut::<Node>(label) {
                node.left = px(readout.at.x);
                node.top = px(readout.at.y);
            }
            label
        }
        None => spawn_readout(world, readout.host, readout.stage, readout.at),
    };

    let lines: Vec<Entity> = world
        .get::<Children>(label)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    for line in lines {
        let measure = world.get::<DragReadoutMeasure>(line).is_some();
        let wanted = if measure {
            readout.measure.clone()
        } else {
            readout.spacing.clone()
        };
        if let Some(mut text) = world.get_mut::<Text>(line)
            && text.0 != wanted
        {
            text.0 = wanted.clone();
        }
        if let Some(mut node) = world.get_mut::<Node>(line) {
            let display = if wanted.is_empty() {
                Display::None
            } else {
                Display::Flex
            };
            if node.display != display {
                node.display = display;
            }
        }
    }
}

/// Spawn the two-line readout: the measure, and the gaps under it.
fn spawn_readout(world: &mut World, host: Entity, stage: Entity, at: Vec2) -> Entity {
    let line = |font_colour: Color| {
        (
            EditorEntity,
            Text::new(String::new()),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(font_colour),
            Node::default(),
            Pickable::IGNORE,
        )
    };
    world
        .spawn((
            DragReadout { host },
            EditorEntity,
            Node {
                position_type: PositionType::Absolute,
                left: px(at.x),
                top: px(at.y),
                flex_direction: FlexDirection::Column,
                padding: UiRect::axes(px(4.0), px(2.0)),
                ..default()
            },
            BackgroundColor(tokens::SHADOW_COLOR),
            ZIndex(READOUT_Z),
            Pickable::IGNORE,
            ChildOf(stage),
        ))
        .with_children(|parent| {
            parent.spawn((DragReadoutMeasure, line(tokens::TEXT_PRIMARY)));
            parent.spawn((DragReadoutSpacing, line(tokens::SNAP_HIGHLIGHT)));
        })
        .id()
}

/// Select the authored node under a press on the stage, in
/// [`Viewport2dMode::Edit`]. In `Interact` the press is still claimed off
/// the dock but selects nothing.
///
/// Propagation is stopped synchronously, before the observer defers
/// anything, so a press cannot climb out of the stage and start a panel
/// drag. Only the primary button is claimed.
///
/// The outline covers the whole selected node, so the press is re-resolved
/// through [`hit_at`] wherever it lands: the selected node under the cursor
/// is the move gesture, anything else is selected instead. The handles keep
/// their resize gesture unconditionally.
///
/// Shift adds the node under the cursor to the selection and Ctrl toggles
/// it, both read at the press. Ctrl is also the snap magnet's inverter,
/// which [`on_gesture_drag`] reads separately on every drag event.
fn on_stage_press(
    mut event: On<Pointer<Press>>,
    ui_scale: Res<UiScale>,
    keys: Res<ButtonInput<KeyCode>>,
    handles: Query<(), With<UiResizeHandle>>,
    overlays: Query<&UiSelectionOverlay>,
    hosts: Query<(Entity, &Viewport2dPanelHost)>,
    stages: Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dViewport>>,
    roots: Query<(Entity, &UiTargetCamera), AuthoredUiSceneRoot>,
    nodes: AuthoredNodes,
    children: Query<&Children>,
    mut selection: ResMut<Selection>,
    mut marquee: ResMut<MarqueeSelect>,
    mut commands: Commands,
    mut hit_cache: ResMut<StageHitCache>,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let target = event.event_target();

    if handles.contains(target) {
        event.propagate(false);
        return;
    }

    let on_overlay = overlays.get(target).ok().map(|overlay| overlay.host);
    let Some((panel, host)) = on_overlay
        .and_then(|panel| hosts.get(panel).ok())
        .or_else(|| hosts.iter().find(|(_, host)| host.stage == target))
    else {
        return;
    };
    event.propagate(false);
    if host.mode != Viewport2dMode::Edit {
        return;
    }

    let Ok(stage) = stages.get(host.stage) else {
        return;
    };
    let cursor = event.pointer_location.position / ui_scale.0;
    let pick = hit_at(
        cursor,
        host,
        stage,
        &roots,
        &nodes,
        &children,
        &mut hit_cache,
    );
    let extend = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let toggle = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);

    // What a band that follows this press would build from, taken before
    // the press has had its say on the selection.
    marquee.seed = match pick {
        StagePick::Hit(entity) if !roots.contains(entity) => None,
        _ => Some(MarqueeSeed {
            host: panel,
            base: selection.entities.clone(),
            extend,
            toggle,
        }),
    };

    if on_overlay.is_some() {
        // A miss must not clear a selection about to be dragged: the
        // outline can extend past whatever laid the scene out.
        let StagePick::Hit(entity) = pick else {
            return;
        };
        if extend {
            selection.extend(&mut commands, entity);
        } else if toggle {
            selection.toggle(&mut commands, entity);
        } else if Some(entity) != selection.primary() {
            selection.select_single(&mut commands, entity);
        }
        return;
    }

    match pick {
        StagePick::Hit(entity) if extend => selection.extend(&mut commands, entity),
        StagePick::Hit(entity) if toggle => selection.toggle(&mut commands, entity),
        StagePick::Hit(entity) => selection.select_single(&mut commands, entity),
        StagePick::Miss if !extend && !toggle => selection.clear(&mut commands),
        StagePick::Miss | StagePick::Empty => {}
    }
}

/// Land an asset dragged out of the asset browser on the canvas, claimed
/// off the dock the way a press is and only in [`Viewport2dMode::Edit`].
/// What the drop means is decided by what is under the cursor; see
/// [`crate::ui_asset_drop::classify_drop`].
fn on_stage_asset_drop(
    mut event: On<Pointer<DragDrop>>,
    ui_scale: Res<UiScale>,
    overlays: Query<&UiSelectionOverlay>,
    hosts: Query<(Entity, &Viewport2dPanelHost)>,
    stages: Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dViewport>>,
    roots: Query<(Entity, &UiTargetCamera), AuthoredUiSceneRoot>,
    nodes: AuthoredNodes,
    children: Query<&Children>,
    mut drag: ResMut<crate::asset_browser::ActiveAssetDrag>,
    mut commands: Commands,
    mut hit_cache: ResMut<StageHitCache>,
) {
    let target = event.event_target();
    let panel = match overlays.get(target) {
        Ok(overlay) => Some(overlay.host),
        Err(_) => hosts
            .iter()
            .find(|(_, host)| host.stage == target)
            .map(|(panel, _)| panel),
    };
    let Some((_, host)) = panel.and_then(|panel| hosts.get(panel).ok()) else {
        return;
    };
    if host.mode != Viewport2dMode::Edit {
        return;
    }
    let Some(path) = drag.image.take() else {
        return;
    };
    event.propagate(false);
    let Ok(stage) = stages.get(host.stage) else {
        return;
    };
    let cursor = event.pointer_location.position / ui_scale.0;
    let under = match hit_at(
        cursor,
        host,
        stage,
        &roots,
        &nodes,
        &children,
        &mut hit_cache,
    ) {
        StagePick::Hit(entity) => Some(entity),
        StagePick::Miss | StagePick::Empty => None,
    };
    let at = authored_at(cursor, host, stage);
    let path = path.to_string_lossy().replace('\\', "/");
    commands.queue(move |world: &mut World| {
        let under = under.filter(|&entity| crate::ui_asset_drop::is_authored(world, entity));
        let landing = crate::ui_asset_drop::classify_drop(world, under, at);
        if let Some(spawned) = crate::ui_asset_drop::drop_image(world, &path, landing) {
            crate::selection::select_only(world, spawned);
        }
    });
}

/// A second press on a node carrying text opens an entry over it.
///
/// The pair is counted against the *authored node* under the cursor, not
/// against the entity the picking backend handed over: the first press
/// spawns an outline, so the second lands on a different entity and arrives
/// carrying a count of one. A press on a resize handle counts too, since on
/// a small node the handles leave nowhere else to press.
fn on_stage_double_press(
    event: On<Pointer<Press>>,
    ui_scale: Res<UiScale>,
    time: Res<Time>,
    overlays: Query<&UiSelectionOverlay>,
    handles: Query<&ChildOf, With<UiResizeHandle>>,
    hosts: Query<(Entity, &Viewport2dPanelHost)>,
    stages: Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dViewport>>,
    roots: Query<(Entity, &UiTargetCamera), AuthoredUiSceneRoot>,
    nodes: AuthoredNodes,
    children: Query<&Children>,
    mut last_press: Local<Option<(Entity, f64)>>,
    mut commands: Commands,
    mut hit_cache: ResMut<StageHitCache>,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let target = event.event_target();
    let chrome = handles.get(target).map_or(target, ChildOf::parent);
    let panel = match overlays.get(chrome) {
        Ok(overlay) => Some(overlay.host),
        Err(_) => hosts
            .iter()
            .find(|(_, host)| host.stage == target)
            .map(|(panel, _)| panel),
    };
    let Some((panel, host)) = panel.and_then(|panel| hosts.get(panel).ok()) else {
        return;
    };
    if host.mode != Viewport2dMode::Edit {
        return;
    }
    let Ok(stage) = stages.get(host.stage) else {
        return;
    };
    let cursor = event.pointer_location.position / ui_scale.0;
    let StagePick::Hit(entity) = hit_at(
        cursor,
        host,
        stage,
        &roots,
        &nodes,
        &children,
        &mut hit_cache,
    ) else {
        *last_press = None;
        return;
    };
    let now = time.elapsed_secs_f64();
    let doubled = matches!(*last_press, Some((node, at))
        if node == entity && now - at < crate::hierarchy::DOUBLE_CLICK_SECS);
    *last_press = (!doubled).then_some((entity, now));
    if !doubled {
        return;
    }
    commands.queue(move |world: &mut World| {
        if crate::ui_text_edit::is_editable_text(world, entity) {
            crate::ui_text_edit::open_text_editor(world, entity, panel);
        }
    });
}

/// Track the authored node under the cursor, for the pre-select outline:
/// the same resolution the press does, on every pointer move. Nothing is
/// tracked in `Interact`, and a running gesture clears it.
fn on_stage_hover(
    event: On<Pointer<Move>>,
    ui_scale: Res<UiScale>,
    manipulation: Res<UiManipulation>,
    overlays: Query<&UiSelectionOverlay>,
    handles: Query<&ChildOf, With<UiResizeHandle>>,
    hosts: Query<(Entity, &Viewport2dPanelHost)>,
    stages: Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dViewport>>,
    roots: Query<(Entity, &UiTargetCamera), AuthoredUiSceneRoot>,
    nodes: AuthoredNodes,
    children: Query<&Children>,
    mut hover: ResMut<UiHoverPreselect>,
    mut hit_cache: ResMut<StageHitCache>,
) {
    let target = event.event_target();
    let on_chrome = handles.get(target).map(ChildOf::parent).unwrap_or(target);
    let panel = overlays.get(on_chrome).ok().map(|overlay| overlay.host);
    let found = match panel {
        Some(panel) => hosts.get(panel).ok(),
        None => hosts.iter().find(|(_, host)| host.stage == target),
    };
    let Some((panel, host)) = found else {
        return;
    };

    let picked = (host.mode == Viewport2dMode::Edit && manipulation.nodes.is_empty())
        .then(|| stages.get(host.stage).ok())
        .flatten()
        .and_then(|stage| {
            let cursor = event.pointer_location.position / ui_scale.0;
            match hit_at(
                cursor,
                host,
                stage,
                &roots,
                &nodes,
                &children,
                &mut hit_cache,
            ) {
                StagePick::Hit(entity) => Some(entity),
                StagePick::Miss | StagePick::Empty => None,
            }
        });

    if hover.entity != picked || hover.host != Some(panel) {
        hover.host = picked.is_some().then_some(panel);
        hover.entity = picked;
    }
}

/// Forget the pre-select as the pointer leaves the stage.
/// [`on_stage_hover`] only runs while the pointer is over a stage, so
/// without this the last node it passed over stays outlined.
fn on_stage_leave(
    event: On<Pointer<Out>>,
    hosts: Query<&Viewport2dPanelHost>,
    mut hover: ResMut<UiHoverPreselect>,
) {
    let target = event.event_target();
    if !hosts.iter().any(|host| host.stage == target) {
        return;
    }
    hover.host = None;
    hover.entity = None;
}

/// Keep at most one pre-select outline, over the node [`UiHoverPreselect`]
/// names. Built on the same placement the selection outline uses. Every
/// selected node is skipped, since each already carries an outline.
fn sync_hover_outlines(
    mut commands: Commands,
    hover: Res<UiHoverPreselect>,
    selection: Res<Selection>,
    hosts: Query<(Entity, &Viewport2dPanelHost)>,
    stages: Query<&ComputedNode, With<Scene2dViewport>>,
    authored: SelectedNode,
    outlines: Query<(Entity, &UiHoverOutline)>,
    mut nodes: Query<&mut Node>,
) {
    let wanted = hover
        .entity
        .filter(|entity| !selection.is_selected(*entity));

    for (host_entity, host) in &hosts {
        let outline = outlines
            .iter()
            .find(|(_, outline)| outline.host == host_entity)
            .map(|(entity, _)| entity);

        let placement = match (host.mode, wanted, hover.host) {
            (Viewport2dMode::Edit, Some(entity), Some(panel)) if panel == host_entity => {
                overlay_placement(entity, host, &stages, &authored)
            }
            _ => Placement::Drop,
        };

        match placement {
            Placement::At(rect) => match outline {
                Some(outline) => {
                    if let Ok(mut node) = nodes.get_mut(outline) {
                        place_outline(&mut node, rect);
                    }
                }
                None => spawn_hover_outline(&mut commands, host_entity, host.stage, rect),
            },
            Placement::Hold => {}
            Placement::Drop => {
                if let Some(outline) = outline
                    && let Ok(mut entity) = commands.get_entity(outline)
                {
                    entity.despawn();
                }
            }
        }
    }
}

/// Spawn one panel's pre-select outline: one thin border and nothing else.
/// [`Pickable::IGNORE`], so the press that follows the hover reaches the
/// stage underneath.
fn spawn_hover_outline(commands: &mut Commands, host: Entity, stage: Entity, rect: Rect) {
    let mut node = Node {
        position_type: PositionType::Absolute,
        border: UiRect::all(px(OUTLINE_WIDTH)),
        ..default()
    };
    place_outline(&mut node, rect);

    commands.spawn((
        UiHoverOutline { host },
        EditorEntity,
        node,
        BorderColor::all(tokens::TEXT_ACCENT),
        ZIndex(HOVER_OUTLINE_Z),
        Pickable::IGNORE,
        ChildOf(stage),
    ));
}

/// Collect `entity` and its descendants in tree order, the order
/// [`topmost_hit`] resolves ties in. A [`Locked`] node contributes no hit
/// of its own, so a press over it reaches whatever else is there; its
/// children still do.
fn collect_stage_hits(
    entity: Entity,
    nodes: &AuthoredNodes,
    children: &Query<&Children>,
    hits: &mut Vec<StageHit>,
) {
    if let Ok((computed, transform, stack, locked)) = nodes.get(entity)
        && locked.is_none()
    {
        let size = computed.size();
        if size.x > 0.0 && size.y > 0.0 {
            hits.push(StageHit {
                entity,
                rect: Rect::from_center_size(transform.translation, size),
                stack: stack.map_or(0, |stack| **stack),
            });
        }
    }
    for child in children.get(entity).into_iter().flatten() {
        collect_stage_hits(*child, nodes, children, hits);
    }
}

/// Keep one overlay per selected authored node per panel, covering that
/// node's live rect.
///
/// Every selected node is outlined. The primary's outline carries the
/// resize handles and takes the gestures; the rest are `Pickable::IGNORE`,
/// so a press on one falls through and reselects. The rect is read off each
/// entity every frame, so an outline follows layout on its own.
///
/// A panel in [`Viewport2dMode::Interact`] has no overlay: they are
/// despawned rather than hidden, so the gesture observers have nothing to
/// fire on.
fn sync_selection_overlays(
    mut commands: Commands,
    selection: Res<Selection>,
    hosts: Query<(Entity, &Viewport2dPanelHost)>,
    stages: Query<&ComputedNode, With<Scene2dViewport>>,
    authored: SelectedNode,
    mut overlays: Query<(Entity, &mut UiSelectionOverlay)>,
    handles: Query<(Entity, &UiResizeHandle, &ChildOf)>,
    mut nodes: Query<&mut Node>,
) {
    let primary = selection.primary();
    // One pass over the outlines rather than one scan per selected node
    // per panel.
    let index: std::collections::HashMap<(Entity, Entity, bool), Entity> = overlays
        .iter()
        .map(|(entity, overlay)| ((overlay.host, overlay.node, overlay.primary), entity))
        .collect();

    for (host_entity, host) in &hosts {
        let wanted: &[Entity] = if host.mode == Viewport2dMode::Edit {
            &selection.entities
        } else {
            &[]
        };
        // The handles hang off the primary's overlay, so a change of
        // primary is a respawn rather than a re-place.
        for (entity, overlay) in &overlays {
            if overlay.host != host_entity {
                continue;
            }
            let keep = wanted.contains(&overlay.node)
                && overlay.primary == (primary == Some(overlay.node));
            if !keep && let Ok(mut entity) = commands.get_entity(entity) {
                entity.despawn();
            }
        }

        for node in wanted {
            let is_primary = primary == Some(*node);
            let overlay = index.get(&(host_entity, *node, is_primary)).copied();

            match overlay_placement(*node, host, &stages, &authored) {
                Placement::At(rect) => match overlay {
                    Some(overlay) => {
                        if let Ok(mut node) = nodes.get_mut(overlay) {
                            place_outline(&mut node, rect);
                        }
                        reseat_handles(overlay, rect, &mut overlays, &handles, &mut nodes);
                    }
                    None => spawn_overlay(
                        &mut commands,
                        host_entity,
                        host.stage,
                        rect,
                        *node,
                        is_primary,
                    ),
                },
                Placement::Hold => {}
                Placement::Drop => {
                    if let Some(overlay) = overlay
                        && let Ok(mut entity) = commands.get_entity(overlay)
                    {
                        entity.despawn();
                    }
                }
            }
        }
    }
}

/// What this frame has to say about a panel's outline.
enum Placement {
    /// The selected node is this panel's, and here is its rect in the
    /// stage's logical pixels.
    At(Rect),
    /// The selected node is this panel's but has no layout to draw against
    /// yet, so the overlay holds the rect it has rather than vanishing and
    /// taking a running gesture with it.
    Hold,
    /// Not this panel's node at all: nothing selected, a 3D entity, or a
    /// scene another panel is showing. There should be no overlay.
    Drop,
}

fn overlay_placement(
    selected: Entity,
    host: &Viewport2dPanelHost,
    stages: &Query<&ComputedNode, With<Scene2dViewport>>,
    authored: &SelectedNode,
) -> Placement {
    let Ok((computed, transform, camera)) = authored.get(selected) else {
        return Placement::Drop;
    };
    if camera.get() != Some(host.camera) {
        return Placement::Drop;
    }
    let Ok(stage) = stages.get(host.stage) else {
        return Placement::Drop;
    };
    let size = computed.size();
    if size.x <= 0.0 || size.y <= 0.0 {
        return Placement::Hold;
    }
    let target_scale = target_pixels_per_stage_pixel(stage.size(), host.target_size);
    let scale = stage_pixels_per_target_pixel(target_scale, stage.inverse_scale_factor());
    Placement::At(Rect::from_center_size(
        transform.translation * scale,
        size * scale,
    ))
}

/// Spawn a panel's outline and its eight handles.
///
/// The chrome is drawn over authored content of any colour, so the handles
/// are a light neutral fill with an accent border and the outline carries a
/// dark edge outside its accent line. That edge is one [`Pickable::IGNORE`]
/// box pinned a pixel outside on all four sides, so a press on the outline
/// body still reaches the overlay underneath.
fn spawn_overlay(
    commands: &mut Commands,
    host: Entity,
    stage: Entity,
    rect: Rect,
    authored: Entity,
    primary: bool,
) {
    let mut node = Node {
        position_type: PositionType::Absolute,
        border: UiRect::all(px(OUTLINE_WIDTH)),
        ..default()
    };
    place_outline(&mut node, rect);

    let overlay = commands
        .spawn((
            UiSelectionOverlay {
                host,
                node: authored,
                primary,
                handles_outside: handles_outside(rect),
            },
            EditorEntity,
            node,
            BorderColor::all(tokens::ACCENT_BLUE),
            ZIndex(OVERLAY_Z),
            // A press on a non-primary outline reaches the node under it,
            // which is how a second selected node is made the primary.
            if primary {
                Pickable::default()
            } else {
                Pickable::IGNORE
            },
            ChildOf(stage),
        ))
        .id();

    commands.spawn((
        EditorEntity,
        Node {
            position_type: PositionType::Absolute,
            left: px(-OUTLINE_WIDTH),
            right: px(-OUTLINE_WIDTH),
            top: px(-OUTLINE_WIDTH),
            bottom: px(-OUTLINE_WIDTH),
            border: UiRect::all(px(OUTLINE_WIDTH)),
            ..default()
        },
        BorderColor::all(tokens::SHADOW_COLOR),
        Pickable::IGNORE,
        ChildOf(overlay),
    ));

    if !primary {
        return;
    }

    let outside = handles_outside(rect);
    for (x, y) in HANDLE_POSITIONS {
        commands.spawn((
            UiResizeHandle { x, y },
            EditorEntity,
            handle_node(x, y, outside),
            BackgroundColor(tokens::TEXT_PRIMARY),
            BorderColor::all(tokens::ACCENT_BLUE),
            Pickable::default(),
            ChildOf(overlay),
        ));
    }
}

/// Move an outline's handles inside or outside the node, when a resize has
/// taken it across [`HANDLES_OUTSIDE_BELOW`] on either axis. Only on the
/// crossing: the handles otherwise follow the outline on their own.
fn reseat_handles(
    overlay: Entity,
    rect: Rect,
    overlays: &mut Query<(Entity, &mut UiSelectionOverlay)>,
    handles: &Query<(Entity, &UiResizeHandle, &ChildOf)>,
    nodes: &mut Query<&mut Node>,
) {
    let outside = handles_outside(rect);
    let Ok((_, mut state)) = overlays.get_mut(overlay) else {
        return;
    };
    if state.handles_outside == outside {
        return;
    }
    state.handles_outside = outside;
    for (entity, handle, parent) in handles {
        if parent.parent() != overlay {
            continue;
        }
        if let Ok(mut node) = nodes.get_mut(entity) {
            place_handle(&mut node, handle.x, handle.y, outside);
        }
    }
}

fn place_outline(node: &mut Node, rect: Rect) {
    node.left = px(rect.min.x);
    node.top = px(rect.min.y);
    node.width = px(rect.width().max(1.0));
    node.height = px(rect.height().max(1.0));
}

/// Everything the gesture observers need to resolve a pointer event: the
/// overlay parts it could be on, and the panels they belong to.
#[derive(SystemParam)]
struct GestureTargets<'w, 's> {
    handles: Query<'w, 's, &'static UiResizeHandle>,
    overlays: Query<'w, 's, &'static UiSelectionOverlay>,
    parents: Query<'w, 's, &'static ChildOf>,
    hosts: Query<'w, 's, &'static Viewport2dPanelHost>,
}

/// Which edges a gesture on `target` drags, and the overlay it belongs to,
/// if `target` is part of one on a panel that is being authored. Resolved
/// synchronously so the observer can stop propagation before the event
/// reaches the dock. Only the primary button and only
/// [`Viewport2dMode::Edit`].
fn gesture_edges(
    target: Entity,
    button: PointerButton,
    parts: &GestureTargets,
) -> Option<(Entity, (i8, i8))> {
    if button != PointerButton::Primary {
        return None;
    }
    let (overlay, edges) = match parts.handles.get(target) {
        Ok(handle) => (
            parts.parents.get(target).ok().map(ChildOf::parent)?,
            (handle.x, handle.y),
        ),
        Err(_) => (target, (0, 0)),
    };
    let host = parts.overlays.get(overlay).ok()?.host;
    if parts.hosts.get(host).ok()?.mode != Viewport2dMode::Edit {
        return None;
    }
    Some((overlay, edges))
}

fn on_gesture_start(
    mut event: On<Pointer<DragStart>>,
    parts: GestureTargets,
    mut commands: Commands,
) {
    let target = event.event_target();
    let Some((overlay, edges)) = gesture_edges(target, event.button, &parts) else {
        return;
    };
    event.propagate(false);
    commands.queue(move |world: &mut World| {
        let started = begin_manipulation(world, overlay, edges);
        *world.resource_mut::<UiManipulation>() = started.unwrap_or_default();
    });
}

/// Everything a gesture needs to know at the moment the pointer went down,
/// or `None` when there is nothing measurable to drag. A move takes the
/// whole selection, a resize the primary alone; either way the primary
/// leads the list.
fn begin_manipulation(world: &World, overlay: Entity, edges: (i8, i8)) -> Option<UiManipulation> {
    let host_entity = world.get::<UiSelectionOverlay>(overlay)?.host;
    let host = world.get::<Viewport2dPanelHost>(host_entity)?;
    let selection = world.get_resource::<Selection>()?;
    let primary = selection.primary()?;
    // The scene's own root is the canvas, not something on it: a drag from
    // the backdrop pulls a band out instead.
    if edges == (0, 0) && is_scene_root(world, primary) {
        return None;
    }
    let primary_node = gesture_node(world, primary, host)?;
    let nodes = if edges == (0, 0) {
        let movable = without_selected_ancestors(world, &selection.entities);
        // Primary first, so the node under the cursor anchors the gesture.
        let mut nodes = Vec::new();
        if movable.contains(&primary) {
            nodes.push(primary_node);
        }
        nodes.extend(
            movable
                .into_iter()
                .filter(|entity| *entity != primary)
                .filter_map(|entity| gesture_node(world, entity, host)),
        );
        nodes
    } else {
        vec![primary_node]
    };
    if nodes.is_empty() {
        return None;
    }
    let kinds = world
        .get_resource::<CanvasSnap>()
        .copied()
        .unwrap_or_default();
    Some(UiManipulation {
        edges,
        host: Some(host_entity),
        scale: gesture_scale(world, host),
        grid: host.view.grid,
        candidates: gather_candidates(world, primary, &kinds),
        kinds,
        last_snap: SnapOutcome::default(),
        nodes,
    })
}

/// Whether `entity` is a scene's own root rather than a node inside one.
fn is_scene_root(world: &World, entity: Entity) -> bool {
    world
        .get::<jackdaw_scene_types::UiSceneRoot>(entity)
        .is_some()
        || world
            .get::<jackdaw_scene_types::Scene2dRoot>(entity)
            .is_some()
}

/// The selected entities that no other selected entity contains. Layout
/// already carries a child when its container moves, so applying a delta to
/// both would move the child twice.
pub(crate) fn without_selected_ancestors(world: &World, selected: &[Entity]) -> Vec<Entity> {
    let set: std::collections::HashSet<Entity> = selected.iter().copied().collect();
    selected
        .iter()
        .copied()
        .filter(|entity| {
            let mut cursor = *entity;
            while let Some(parent) = world.get::<ChildOf>(cursor).map(|c| c.0) {
                if set.contains(&parent) {
                    return false;
                }
                cursor = parent;
            }
            true
        })
        .collect()
}

/// One selected entity as a gesture sees it, or `None` when it is not an
/// authored node of this panel's canvas with a rect to drag. The camera
/// check keeps out a selection made in another viewport.
fn gesture_node(world: &World, entity: Entity, host: &Viewport2dPanelHost) -> Option<GestureNode> {
    if world.get::<EditorEntity>(entity).is_some() {
        return None;
    }
    if world.get::<ComputedUiTargetCamera>(entity)?.get() != Some(host.camera) {
        return None;
    }
    let before = world.get::<Node>(entity)?.clone();
    let rect = authored_rect(world, entity)?;
    let offset = authored_offset(world, entity, rect);
    let viewport = host.target_size.as_vec2();
    // A routed scene's root has no parent node, so its offset box measures
    // zero. Bevy lays it out against the render target, which is what its
    // percentages and far offsets are stated against. Per axis, so a parent
    // degenerate on one side is covered too.
    let measured = parent_offset_box(world, entity).size();
    let parent = Vec2::new(
        if measured.x > 0.0 {
            measured.x
        } else {
            viewport.x
        },
        if measured.y > 0.0 {
            measured.y
        } else {
            viewport.y
        },
    );
    Some(GestureNode {
        entity,
        anchors: NodeAnchors::of(&before),
        basis: UnitBasis { parent, viewport },
        before,
        start: Vec4::new(offset.x, offset.y, rect.width(), rect.height()),
    })
}

/// The gesture's scale as of right now, or `None` when the panel it started
/// on has gone. Read again on every drag event, since the canvas can zoom
/// mid-gesture and a scale captured at the press would then convert pointer
/// pixels at a rate the panel has stopped drawing at.
fn live_scale(world: &World, host: Option<Entity>) -> Option<f32> {
    let host = world.get::<Viewport2dPanelHost>(host?)?;
    Some(gesture_scale(world, host))
}

/// Authored pixels per pointer pixel for the panel `host` describes.
fn gesture_scale(world: &World, host: &Viewport2dPanelHost) -> f32 {
    let Some(stage) = world.get::<ComputedNode>(host.stage) else {
        return 1.0;
    };
    let target_scale = target_pixels_per_stage_pixel(stage.size(), host.target_size);
    let ui_scale = world.get_resource::<UiScale>().map_or(1.0, |scale| scale.0);
    target_pixels_per_pointer_pixel(
        stage_pixels_per_target_pixel(target_scale, stage.inverse_scale_factor()),
        ui_scale,
    )
}

/// The node's laid-out rect in the space its own `left`/`top` are stated
/// in: authored pixels from its parent's top-left corner.
fn authored_rect(world: &World, entity: Entity) -> Option<Rect> {
    let rect = global_node_rect(world, entity)?;
    let origin = parent_offset_box(world, entity).min;
    Some(Rect::from_corners(rect.min - origin, rect.max - origin))
}

/// The node's laid-out rect in the global authored pixels layout reports,
/// or `None` for a node layout has not measured. The space two nodes under
/// different parents can be compared in.
pub(crate) fn global_node_rect(world: &World, entity: Entity) -> Option<Rect> {
    let size = world.get::<ComputedNode>(entity)?.size();
    if size.x <= 0.0 || size.y <= 0.0 {
        return None;
    }
    let centre = world.get::<UiGlobalTransform>(entity)?.translation;
    Some(Rect::from_corners(centre - size / 2.0, centre + size / 2.0))
}

/// The box a child's `left`/`top` are measured from, the parent's padding
/// box, in the global authored pixels layout reports.
///
/// Inside the border, not at the parent's outer corner: an absolutely
/// placed child's offsets start where the border ends, and the shifts do
/// not cancel because the offset comes from the node's own `Val::Px` while
/// the candidates come from layout. A node with no parent is measured from
/// the canvas itself.
pub(crate) fn parent_offset_box(world: &World, entity: Entity) -> Rect {
    let Some(parent) = world.get::<ChildOf>(entity).map(ChildOf::parent) else {
        return Rect::from_corners(Vec2::ZERO, Vec2::ZERO);
    };
    let (Some(computed), Some(transform)) = (
        world.get::<ComputedNode>(parent),
        world.get::<UiGlobalTransform>(parent),
    ) else {
        return Rect::from_corners(Vec2::ZERO, Vec2::ZERO);
    };
    let border = computed.border;
    let half = computed.size() / 2.0;
    Rect::from_corners(
        transform.translation - half + border.min_inset,
        transform.translation + half - border.max_inset,
    )
}

/// Authored left/top the gesture starts from. A node with no explicit
/// offset starts from where layout put it, so promoting a flex child does
/// not make it jump on the first drag event.
fn authored_offset(world: &World, entity: Entity, rect: Rect) -> Vec2 {
    if let Some(node) = world.get::<Node>(entity)
        && node.position_type == PositionType::Absolute
        && let (Val::Px(left), Val::Px(top)) = (node.left, node.top)
    {
        return Vec2::new(left, top);
    }
    rect.min
}

/// The `left`/`top` a move on `entity` starts from, in the authored offsets
/// its own `Node` states them in. The pointer's own starting figure, for a
/// caller that moves a node without a pointer.
pub(crate) fn authored_offset_of(world: &World, entity: Entity) -> Option<Vec2> {
    let rect = authored_rect(world, entity)?;
    Some(authored_offset(world, entity, rect))
}

/// What `entity`'s units are measured against, read off layout. A caller
/// with no panel takes the viewport from the scene's own root, which is
/// laid out directly against the render target. A parent that measures zero
/// on an axis falls back to that same canvas.
pub(crate) fn unit_basis_of(world: &World, entity: Entity) -> UnitBasis {
    let viewport = world
        .get::<ComputedNode>(scene_root(world, entity))
        .map(ComputedNode::size)
        .unwrap_or(Vec2::ZERO);
    let measured = parent_offset_box(world, entity).size();
    let parent = Vec2::new(
        if measured.x > 0.0 {
            measured.x
        } else {
            viewport.x
        },
        if measured.y > 0.0 {
            measured.y
        } else {
            viewport.y
        },
    );
    UnitBasis { parent, viewport }
}

/// Where `entity`'s box sits on `host`'s stage, in that stage's logical
/// pixels: the rect a piece of chrome drawn over the node is placed at, by
/// the same mapping the selection outline uses.
pub(crate) fn node_overlay_rect(world: &World, entity: Entity, host: Entity) -> Option<Rect> {
    let host = world.get::<Viewport2dPanelHost>(host)?;
    let stage = world.get::<ComputedNode>(host.stage)?;
    let rect = global_node_rect(world, entity)?;
    let target_scale = target_pixels_per_stage_pixel(stage.size(), host.target_size);
    let scale = stage_pixels_per_target_pixel(target_scale, stage.inverse_scale_factor());
    Some(Rect::from_corners(rect.min * scale, rect.max * scale))
}

/// The lines a gesture on `entity` can land on, in the same offset space
/// [`authored_rect`] reports.
///
/// The order is the precedence a landing is decided by, nearest distance
/// first and this order to break a tie: the parent's own edges and centre,
/// sibling sides, sibling centres, the scene's guides, nodes elsewhere in
/// the tree, and last the lines a percentage of the parent box names. An
/// edge something in the scene actually has beats a line that is only a
/// figure; a percent line still wins when it is strictly nearer.
///
/// `kinds` decides which are offered at all; an off kind contributes
/// nothing rather than being filtered later. Editor chrome is skipped.
fn gather_candidates(world: &World, entity: Entity, kinds: &CanvasSnap) -> SnapCandidates {
    let mut candidates = SnapCandidates::default();
    let Some(parent) = world.get::<ChildOf>(entity).map(ChildOf::parent) else {
        return candidates;
    };
    let offset_box = parent_offset_box(world, entity);
    let size = offset_box.size();
    let origin = offset_box.min;
    candidates.origin = origin;

    if kinds.parent {
        // The parent's own lines carry their percentage too, so a
        // percent-authored node landing on one writes the exact figure.
        for (at, percent) in [(0.0, 0.0), (size.x / 2.0, 50.0), (size.x, 100.0)] {
            candidates.x.push(Candidate {
                at,
                kind: CandidateKind::Parent,
                percent: Some(percent),
            });
        }
        for (at, percent) in [(0.0, 0.0), (size.y / 2.0, 50.0), (size.y, 100.0)] {
            candidates.y.push(Candidate {
                at,
                kind: CandidateKind::Parent,
                percent: Some(percent),
            });
        }
    }

    let siblings: Vec<Rect> = world
        .get::<Children>(parent)
        .into_iter()
        .flatten()
        .copied()
        .filter(|sibling| *sibling != entity)
        .filter_map(|sibling| node_rect(world, sibling, origin))
        .collect();
    if kinds.sibling_sides {
        for rect in &siblings {
            push_rect_sides(&mut candidates, *rect, CandidateKind::SiblingSide);
        }
    }
    if kinds.sibling_centers {
        for rect in &siblings {
            let centre = rect.center();
            candidates.x.push(Candidate {
                at: centre.x,
                kind: CandidateKind::SiblingCentre,
                percent: None,
            });
            candidates.y.push(Candidate {
                at: centre.y,
                kind: CandidateKind::SiblingCentre,
                percent: None,
            });
        }
    }

    if kinds.guides
        && let Some(guides) = world.get::<CanvasGuides>(scene_root(world, entity))
    {
        for at in &guides.vertical {
            candidates.x.push(Candidate {
                at: at - origin.x,
                kind: CandidateKind::Guide,
                percent: None,
            });
        }
        for at in &guides.horizontal {
            candidates.y.push(Candidate {
                at: at - origin.y,
                kind: CandidateKind::Guide,
                percent: None,
            });
        }
    }

    if kinds.other_nodes {
        for rect in other_node_rects(world, entity, parent, origin) {
            push_rect_sides(&mut candidates, rect, CandidateKind::OtherNode);
        }
    }

    if kinds.percent_lines {
        for percent in PERCENT_LINES {
            let fraction = percent / 100.0;
            candidates.x.push(Candidate {
                at: size.x * fraction,
                kind: CandidateKind::PercentLine,
                percent: Some(percent),
            });
            candidates.y.push(Candidate {
                at: size.y * fraction,
                kind: CandidateKind::PercentLine,
                percent: Some(percent),
            });
        }
    }

    candidates
}

/// One node's laid-out rect, measured from `origin`, or `None` when it
/// is editor chrome or has nothing laid out to measure.
fn node_rect(world: &World, entity: Entity, origin: Vec2) -> Option<Rect> {
    if world.get::<EditorEntity>(entity).is_some() {
        return None;
    }
    let computed = world.get::<ComputedNode>(entity)?;
    let size = computed.size();
    if size.x <= 0.0 || size.y <= 0.0 {
        return None;
    }
    let min = world.get::<UiGlobalTransform>(entity)?.translation - size / 2.0 - origin;
    Some(Rect::from_corners(min, min + size))
}

fn push_rect_sides(candidates: &mut SnapCandidates, rect: Rect, kind: CandidateKind) {
    for at in [rect.min.x, rect.max.x] {
        candidates.x.push(Candidate {
            at,
            kind,
            percent: None,
        });
    }
    for at in [rect.min.y, rect.max.y] {
        candidates.y.push(Candidate {
            at,
            kind,
            percent: None,
        });
    }
}

/// The topmost ancestor of `entity`: the root of the scene it is part of,
/// and where the scene's guides are kept.
fn scene_root(world: &World, entity: Entity) -> Entity {
    let mut root = entity;
    while let Some(next) = world.get::<ChildOf>(root).map(ChildOf::parent) {
        root = next;
    }
    root
}

/// Every authored node under the same routed root that is not part of the
/// dragged node's family, measured from `origin`. The family is the parent,
/// its own children (the siblings, which have their own kinds), and the
/// whole selection with everything under it.
///
fn other_node_rects(world: &World, entity: Entity, parent: Entity, origin: Vec2) -> Vec<Rect> {
    let root = scene_root(world, entity);
    let mut family: std::collections::HashSet<Entity> = std::collections::HashSet::new();
    family.insert(parent);
    family.extend(world.get::<Children>(parent).into_iter().flatten().copied());
    let carried: std::collections::HashSet<Entity> = world
        .get_resource::<Selection>()
        .map(|selection| selection.entities.iter().copied().collect())
        .unwrap_or_default();

    let mut rects = Vec::new();
    collect_other_nodes(world, root, &family, &carried, origin, &mut rects);
    rects
}

fn collect_other_nodes(
    world: &World,
    entity: Entity,
    family: &std::collections::HashSet<Entity>,
    carried: &std::collections::HashSet<Entity>,
    origin: Vec2,
    rects: &mut Vec<Rect>,
) {
    // A carried node takes its whole subtree out: layout moves the
    // descendants with it.
    if carried.contains(&entity) {
        return;
    }
    if !family.contains(&entity)
        && let Some(rect) = node_rect(world, entity, origin)
    {
        rects.push(rect);
    }
    for child in world.get::<Children>(entity).into_iter().flatten().copied() {
        collect_other_nodes(world, child, family, carried, origin, rects);
    }
}

/// Move or resize what the gesture picked up, every drag event. The primary
/// is what snaps, and the rest of the selection moves by the delta it
/// snapped to. See [`UiManipulation`].
fn on_gesture_drag(mut event: On<Pointer<Drag>>, parts: GestureTargets, mut commands: Commands) {
    let target = event.event_target();
    if gesture_edges(target, event.button, &parts).is_none() {
        return;
    }
    event.propagate(false);
    let distance = event.distance;
    commands.queue(move |world: &mut World| {
        world.resource_scope(|world, mut manipulation: Mut<UiManipulation>| {
            let Some(primary) = manipulation.nodes.first() else {
                return;
            };
            let edges = manipulation.edges;
            let scale = live_scale(world, manipulation.host).unwrap_or(manipulation.scale);
            let dragged = drag_edges(primary.start, edges, distance * scale);
            let ctrl = world
                .get_resource::<ButtonInput<KeyCode>>()
                .is_some_and(|keys| {
                    keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight])
                });
            let outcome = snap_gesture(
                dragged,
                edges,
                &manipulation.candidates,
                manipulation.kinds.magnet(ctrl),
                manipulation.grid,
                scale,
            );
            let delta = distance * scale + outcome.nudge;
            let rounding = pixel_rounding(&manipulation.kinds);
            let exact = outcome.exact_percent();
            for (ordinal, node) in manipulation.nodes.iter().enumerate() {
                let edges = if ordinal == 0 { edges } else { (0, 0) };
                let exact = if ordinal == 0 {
                    exact
                } else {
                    ExactPercent::default()
                };
                let rect = floor_size(drag_edges(node.start, edges, delta), edges);
                if let Some(mut value) = world.get_mut::<Node>(node.entity) {
                    apply_authored_rect(
                        &mut value,
                        node.anchors,
                        rect,
                        edges,
                        node.basis,
                        rounding,
                        exact,
                    );
                }
            }
            manipulation.last_snap = outcome;
        });
    });
}

/// Move the edges `edges` names by `delta`, in authored pixels, over a rect
/// of `(left, top, width, height)`. A move slides both offsets; a resize
/// holds the opposite edges still, which is why a near edge takes the delta
/// back out of the size.
fn drag_edges(rect: Vec4, edges: (i8, i8), delta: Vec2) -> Vec4 {
    let (mut left, mut top, mut width, mut height) = (rect.x, rect.y, rect.z, rect.w);
    match edges {
        (0, 0) => {
            left += delta.x;
            top += delta.y;
        }
        (x, y) => {
            if x < 0 {
                left += delta.x;
                width -= delta.x;
            } else if x > 0 {
                width += delta.x;
            }
            if y < 0 {
                top += delta.y;
                height -= delta.y;
            } else if y > 0 {
                height += delta.y;
            }
        }
    }
    Vec4::new(left, top, width, height)
}

/// The rect a gesture may actually write: no thinner than
/// [`MIN_NODE_SIZE`] on either axis, with the origin held back by however
/// much the size had to be floored. Without holding the origin, dragging a
/// left or top handle past the opposite edge would walk the node off across
/// the canvas a single pixel wide.
fn floor_size(rect: Vec4, edges: (i8, i8)) -> Vec4 {
    let (mut left, mut top, mut width, mut height) = (rect.x, rect.y, rect.z, rect.w);
    if width < MIN_NODE_SIZE {
        if edges.0 < 0 {
            left += width - MIN_NODE_SIZE;
        }
        width = MIN_NODE_SIZE;
    }
    if height < MIN_NODE_SIZE {
        if edges.1 < 0 {
            top += height - MIN_NODE_SIZE;
        }
        height = MIN_NODE_SIZE;
    }
    Vec4::new(left, top, width, height)
}

/// How far the gesture's dragged edges have to move to land on a neighbour,
/// or on the canvas's pixel grid when no neighbour is near.
///
/// The moving geometry is the whole rect for a move and the dragged edge or
/// corner for a resize. Both kinds of snapping are decided by `magnet`
/// once, at the top, so Ctrl and the master's off state mean one thing.
///
/// `grid` is the lattice in authored pixels; `scale` is authored pixels per
/// pointer pixel, which turns [`EDGE_SNAP_PIXELS`] into a radius.
fn snap_gesture(
    rect: Vec4,
    edges: (i8, i8),
    candidates: &SnapCandidates,
    magnet: bool,
    grid: f32,
    scale: f32,
) -> SnapOutcome {
    if !magnet {
        return SnapOutcome::default();
    }
    let min = Vec2::new(rect.x, rect.y);
    let moving = match edges {
        (0, 0) => SnapRect::from_min_size(min, Vec2::new(rect.z, rect.w)),
        (x, y) => {
            let corner = Vec2::new(
                if x > 0 { min.x + rect.z } else { min.x },
                if y > 0 { min.y + rect.w } else { min.y },
            );
            SnapRect {
                min: corner,
                max: corner,
            }
        }
    };
    const NONE: [Candidate; 0] = [];
    let x = if edges == (0, 0) || edges.0 != 0 {
        candidates.x.as_slice()
    } else {
        &NONE
    };
    let y = if edges == (0, 0) || edges.1 != 0 {
        candidates.y.as_slice()
    } else {
        &NONE
    };
    let at_x: Vec<f32> = x.iter().map(|candidate| candidate.at).collect();
    let at_y: Vec<f32> = y.iter().map(|candidate| candidate.at).collect();
    let (won_x, won_y) = snap_edges_2d_with_winners(moving, &at_x, &at_y, EDGE_SNAP_PIXELS * scale);
    // The grid only has a say on an axis no neighbour claimed; rounding an
    // edge landing afterwards would take it back off.
    let lattice = snap_to_pixel_grid(moving.min, grid) - moving.min;
    SnapOutcome {
        nudge: Vec2::new(
            won_x.map_or(lattice.x, |snap| snap.delta),
            won_y.map_or(lattice.y, |snap| snap.delta),
        ),
        x: won_x.map(|snap| winner(snap, x, edges.0)),
        y: won_y.map(|snap| winner(snap, y, edges.1)),
    }
}

/// Name the line an axis landed on. A resize collapses the moving rect to
/// the dragged corner, so `edge` is what decides which of the node's lines
/// it was: positive for the far side, negative for the near one.
fn winner(snap: jackdaw_snap::AxisSnap, candidates: &[Candidate], edge: i8) -> SnapWinner {
    let candidate = candidates[snap.candidate];
    let line = match edge {
        0 => snap.line,
        edge if edge > 0 => SnapLine::Max,
        _ => SnapLine::Min,
    };
    SnapWinner {
        at: candidate.at,
        kind: candidate.kind,
        percent: candidate.percent,
        line,
    }
}

/// `point` rounded onto a lattice of `grid` authored pixels. A non-positive
/// or non-finite grid is no grid at all rather than a division by zero.
pub fn snap_to_pixel_grid(point: Vec2, grid: f32) -> Vec2 {
    if grid <= 0.0 || !grid.is_finite() {
        return point;
    }
    (point / grid).round() * grid
}

fn on_gesture_end(mut event: On<Pointer<DragEnd>>, parts: GestureTargets, mut commands: Commands) {
    if gesture_edges(event.event_target(), event.button, &parts).is_none() {
        return;
    }
    event.propagate(false);
    commands.queue(|world: &mut World| finish_manipulation(world, true));
}

/// End the gesture, either committing what the drag already wrote or
/// putting back what it started from.
fn finish_manipulation(world: &mut World, commit: bool) {
    let nodes = {
        let mut manipulation = world.resource_mut::<UiManipulation>();
        manipulation.last_snap = SnapOutcome::default();
        std::mem::take(&mut manipulation.nodes)
    };
    if nodes.is_empty() {
        return;
    }
    if !commit {
        for node in nodes {
            if let Some(mut value) = world.get_mut::<Node>(node.entity) {
                *value = node.before;
            }
        }
        return;
    }
    let edits = nodes
        .into_iter()
        .filter_map(|node| {
            let after = world.get::<Node>(node.entity).cloned()?;
            Some((node.entity, node.before, after))
        })
        .collect();
    push_layout_edits(world, edits);
}

/// Move every selected UI node on an authored canvas one step in
/// `direction`, and say whether there was anything there to move.
///
/// The keyboard half of direct manipulation, written through the same
/// [`drag_edges`] and [`apply_authored_rect`] the pointer uses. The step is
/// one authored pixel, or the panel's own canvas grid with Shift held; not
/// the 3D grid, which is a lattice of world units. One history entry per
/// press, matching [`crate::entity_ops::nudge_selected`], which the same
/// keys reach for a 3D selection.
pub(crate) fn nudge_ui_selection(world: &mut World, direction: Vec2) -> bool {
    let Some(primary) = world
        .get_resource::<Selection>()
        .and_then(Selection::primary)
    else {
        return false;
    };
    let Some(camera) = world
        .get::<ComputedUiTargetCamera>(primary)
        .and_then(ComputedUiTargetCamera::get)
    else {
        return false;
    };
    // Only while the panel is being authored: in `Interact` the keys
    // belong to the scene.
    let host_entity = {
        let mut panels = world.query::<(Entity, &Viewport2dPanelHost)>();
        panels
            .iter(world)
            .find(|(_, host)| host.camera == camera && host.mode == Viewport2dMode::Edit)
            .map(|(entity, _)| entity)
    };
    let Some(host_entity) = host_entity else {
        return false;
    };
    let coarse = world
        .get_resource::<ButtonInput<KeyCode>>()
        .is_some_and(|keys| keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]));

    let selected = world
        .get_resource::<Selection>()
        .map(|selection| selection.entities.clone())
        .unwrap_or_default();
    let (nodes, step) = {
        let Some(host) = world.get::<Viewport2dPanelHost>(host_entity) else {
            return false;
        };
        let step = if coarse { host.view.grid } else { 1.0 };
        // A locked node is out of the canvas's reach, and the keys are the
        // canvas.
        let nodes: Vec<GestureNode> = without_selected_ancestors(world, &selected)
            .into_iter()
            .filter(|&entity| world.get::<Locked>(entity).is_none())
            .filter_map(|entity| gesture_node(world, entity, host))
            .collect();
        (nodes, step)
    };
    if nodes.is_empty() {
        return false;
    }
    // A node its parent lays out has no offsets to step, and promoting it
    // is a change to the layout rather than the move the key asked for. The
    // canvas still answers, so the 3D nudge does not take the key instead.
    if let Some(flowed) = nodes
        .iter()
        .find(|node| node.before.position_type != PositionType::Absolute)
    {
        let name = world
            .get::<Name>(flowed.entity)
            .map(|name| name.as_str().to_owned())
            .unwrap_or_else(|| "node".to_string());
        crate::status_bar::notify_error(
            world,
            format!(
                "{name} is placed by its parent's layout. Set Position to Absolute to move it."
            ),
        );
        return true;
    }

    let delta = direction * step;
    let mut edits = Vec::new();
    for node in nodes {
        let rect = drag_edges(node.start, (0, 0), delta);
        let Some(mut value) = world.get_mut::<Node>(node.entity) else {
            continue;
        };
        apply_authored_rect(
            &mut value,
            node.anchors,
            rect,
            (0, 0),
            node.basis,
            PixelRounding::Whole,
            ExactPercent::default(),
        );
        let after = value.clone();
        edits.push((node.entity, node.before, after));
    }
    push_layout_edits(world, edits);
    true
}

/// Escape restores the exact node the gesture started from.
fn cancel_manipulation(
    keys: Res<ButtonInput<KeyCode>>,
    focus: crate::keybind_focus::KeybindFocus,
    manipulation: Res<UiManipulation>,
    mut commands: Commands,
) {
    if focus.keyboard_is_spoken_for() {
        return;
    }
    if !manipulation.nodes.is_empty() && keys.just_pressed(KeyCode::Escape) {
        commands.queue(|world: &mut World| finish_manipulation(world, false));
    }
}

/// Keep a line drawn across the stage wherever the running gesture is
/// landing on something, and nowhere else. Driven off the gesture, so the
/// line comes up with the first drag event that lands and goes on release.
fn sync_snap_highlights(
    mut commands: Commands,
    manipulation: Res<UiManipulation>,
    hosts: Query<&Viewport2dPanelHost>,
    stages: Query<&ComputedNode, With<Scene2dViewport>>,
    highlights: Query<(Entity, &SnapHighlight)>,
    mut nodes: Query<&mut Node>,
) {
    let wanted = highlight_lines(&manipulation, &hosts, &stages);

    for (entity, highlight) in &highlights {
        match wanted
            .iter()
            .find(|line| line.host == highlight.host && line.axis == highlight.axis)
        {
            Some(line) => {
                if let Ok(mut node) = nodes.get_mut(entity) {
                    place_highlight(&mut node, line.axis, line.at);
                }
            }
            None => {
                if let Ok(mut entity) = commands.get_entity(entity) {
                    entity.despawn();
                }
            }
        }
    }

    for line in wanted {
        if highlights
            .iter()
            .any(|(_, highlight)| highlight.host == line.host && highlight.axis == line.axis)
        {
            continue;
        }
        let mut node = Node {
            position_type: PositionType::Absolute,
            ..default()
        };
        place_highlight(&mut node, line.axis, line.at);
        commands.spawn((
            SnapHighlight {
                host: line.host,
                axis: line.axis,
            },
            EditorEntity,
            node,
            BackgroundColor(tokens::SNAP_HIGHLIGHT),
            ZIndex(OVERLAY_Z + 1),
            Pickable::IGNORE,
            ChildOf(line.stage),
        ));
    }
}

/// One line the running gesture wants drawn. Every landing is painted
/// [`tokens::SNAP_HIGHLIGHT`], whatever kind of line it was.
struct HighlightLine {
    host: Entity,
    stage: Entity,
    axis: CanvasAxis,
    /// Where the line sits in the stage's logical pixels.
    at: f32,
}

/// The lines the gesture in progress is landing on, in the stage's own
/// logical pixels. A candidate is stated from the dragged node's parent, so
/// the canvas position is the landing plus the parent's own corner.
fn highlight_lines(
    manipulation: &UiManipulation,
    hosts: &Query<&Viewport2dPanelHost>,
    stages: &Query<&ComputedNode, With<Scene2dViewport>>,
) -> Vec<HighlightLine> {
    if manipulation.nodes.is_empty() {
        return Vec::new();
    }
    let Some(host_entity) = manipulation.host else {
        return Vec::new();
    };
    let Ok(host) = hosts.get(host_entity) else {
        return Vec::new();
    };
    if host.mode != Viewport2dMode::Edit {
        return Vec::new();
    }
    let Ok(stage) = stages.get(host.stage) else {
        return Vec::new();
    };
    let target_scale = target_pixels_per_stage_pixel(stage.size(), host.target_size);
    let scale = stage_pixels_per_target_pixel(target_scale, stage.inverse_scale_factor());
    let origin = manipulation.candidates.origin;
    let outcome = manipulation.last_snap;

    [
        (outcome.x, CanvasAxis::Vertical, origin.x),
        (outcome.y, CanvasAxis::Horizontal, origin.y),
    ]
    .into_iter()
    .filter_map(|(won, axis, corner)| {
        let won = won?;
        Some(HighlightLine {
            host: host_entity,
            stage: host.stage,
            axis,
            at: (won.at + corner) * scale,
        })
    })
    .collect()
}

/// Lay a highlight across the whole stage, a pixel thick.
fn place_highlight(node: &mut Node, axis: CanvasAxis, at: f32) {
    match axis {
        CanvasAxis::Vertical => {
            node.left = px(at);
            node.top = px(0);
            node.width = px(1);
            node.height = percent(100);
        }
        CanvasAxis::Horizontal => {
            node.left = px(0);
            node.top = px(at);
            node.width = percent(100);
            node.height = px(1);
        }
    }
}

/// Draw the open scene's guides over every panel showing it. Guides are
/// scene data, so they stand whatever the canvas is doing; what takes them
/// down is the panel leaving [`Viewport2dMode::Edit`] or the canvas
/// settings hiding them.
fn sync_guide_lines(
    mut commands: Commands,
    snap: Res<CanvasSnap>,
    roots: Query<(Entity, &CanvasGuides), AuthoredUiSceneRoot>,
    hosts: Query<(Entity, &Viewport2dPanelHost)>,
    stages: Query<&ComputedNode, With<Scene2dViewport>>,
    lines: Query<(Entity, &GuideLine)>,
    mut nodes: Query<&mut Node>,
) {
    let wanted = guide_lines(&snap, &roots, &hosts, &stages);

    for (entity, line) in &lines {
        match wanted.iter().find(|want| {
            want.host == line.host && want.axis == line.axis && want.index == line.index
        }) {
            Some(want) => {
                if let Ok(mut node) = nodes.get_mut(entity) {
                    place_guide(&mut node, want.axis, want.at);
                }
            }
            None => {
                if let Ok(mut entity) = commands.get_entity(entity) {
                    entity.despawn();
                }
            }
        }
    }

    for want in wanted {
        if lines.iter().any(|(_, line)| {
            line.host == want.host && line.axis == want.axis && line.index == want.index
        }) {
            continue;
        }
        let mut node = Node {
            position_type: PositionType::Absolute,
            ..default()
        };
        place_guide(&mut node, want.axis, want.at);
        let mut line = Node {
            position_type: PositionType::Absolute,
            ..default()
        };
        place_highlight(&mut line, want.axis, (GUIDE_HIT_WIDTH - 1.0) / 2.0);
        commands.spawn((
            GuideLine {
                host: want.host,
                axis: want.axis,
                index: want.index,
            },
            EditorEntity,
            node,
            // Under the selection chrome.
            ZIndex(OVERLAY_Z - 1),
            // Pickable, because a guide is dragged by its line.
            Pickable::default(),
            ChildOf(want.stage),
            children![(
                EditorEntity,
                line,
                BackgroundColor(tokens::GUIDE_LINE),
                Pickable::IGNORE,
            )],
        ));
    }
}

/// One guide drawn over one panel.
struct WantedGuide {
    host: Entity,
    stage: Entity,
    axis: CanvasAxis,
    index: usize,
    /// Where the line sits in the stage's logical pixels.
    at: f32,
}

/// Every guide every panel wants drawn. A guide's position is canvas-global
/// authored pixels and the stage node *is* the canvas, so there is no
/// parent corner to add, unlike a snap landing.
fn guide_lines(
    snap: &CanvasSnap,
    roots: &Query<(Entity, &CanvasGuides), AuthoredUiSceneRoot>,
    hosts: &Query<(Entity, &Viewport2dPanelHost)>,
    stages: &Query<&ComputedNode, With<Scene2dViewport>>,
) -> Vec<WantedGuide> {
    if !snap.show_guides {
        return Vec::new();
    }
    // A malformed document holding several roots picks the lowest entity,
    // the same one the guide operators write.
    let Some((_, guides)) = roots.iter().min_by_key(|(entity, _)| *entity) else {
        return Vec::new();
    };

    let mut wanted = Vec::new();
    for (host_entity, host) in hosts {
        if host.mode != Viewport2dMode::Edit {
            continue;
        }
        let Ok(stage) = stages.get(host.stage) else {
            continue;
        };
        let target_scale = target_pixels_per_stage_pixel(stage.size(), host.target_size);
        let scale = stage_pixels_per_target_pixel(target_scale, stage.inverse_scale_factor());
        for (axis, positions) in [
            (CanvasAxis::Vertical, &guides.vertical),
            (CanvasAxis::Horizontal, &guides.horizontal),
        ] {
            for (index, at) in positions.iter().enumerate() {
                wanted.push(WantedGuide {
                    host: host_entity,
                    stage: host.stage,
                    axis,
                    index,
                    at: at * scale,
                });
            }
        }
    }
    wanted
}

/// Lay a guide's hit slab across the whole stage, centred on the line.
fn place_guide(node: &mut Node, axis: CanvasAxis, at: f32) {
    let half = GUIDE_HIT_WIDTH / 2.0;
    match axis {
        CanvasAxis::Vertical => {
            node.left = px(at - half);
            node.top = px(0);
            node.width = px(GUIDE_HIT_WIDTH);
            node.height = percent(100);
        }
        CanvasAxis::Horizontal => {
            node.left = px(0);
            node.top = px(at - half);
            node.width = percent(100);
            node.height = px(GUIDE_HIT_WIDTH);
        }
    }
}

/// The guide being dragged, if any. One drag is one history entry.
#[derive(Resource, Default)]
pub struct GuideManipulation {
    active: Option<GuideDrag>,
}

impl GuideManipulation {
    /// Where the guide being dragged is now, in canvas-global authored
    /// pixels, or `None` when no guide is being dragged.
    pub fn position(&self) -> Option<f32> {
        self.active.as_ref().map(|drag| drag.position)
    }
}

/// One guide following the cursor.
struct GuideDrag {
    /// The panel the drag is running on, so every event maps the cursor
    /// through the canvas that panel is showing.
    host: Entity,
    /// The UI scene root the guides belong to.
    root: Entity,
    axis: CanvasAxis,
    /// The axis's other guides, which the drag leaves where they are.
    others: Vec<f32>,
    /// Where the dragged guide is now, in canvas-global authored pixels.
    position: f32,
    /// The guides as the drag found them, for the history entry and for
    /// Escape.
    before: Option<CanvasGuides>,
    /// Whether the cursor is over the guide's own ruler, which is where
    /// a release drops it.
    dropping: bool,
}

/// The guides the scene carries while `drag` is running, with the dragged
/// line in or out. A guide dragged onto another becomes the other one, the
/// way the operator that adds a guide refuses a duplicate.
fn drag_guides(drag: &GuideDrag, keep: bool) -> CanvasGuides {
    let mut guides = drag.before.clone().unwrap_or_default();
    let mut lines = drag.others.clone();
    if keep
        && !lines
            .iter()
            .any(|at| (at - drag.position).abs() <= crate::canvas_snap::GUIDE_MATCH)
    {
        lines.push(drag.position);
    }
    lines.sort_by(f32::total_cmp);
    match drag.axis {
        CanvasAxis::Vertical => guides.vertical = lines,
        CanvasAxis::Horizontal => guides.horizontal = lines,
    }
    guides
}

/// What the ruler and guide observers need to resolve a pointer event.
#[derive(SystemParam)]
struct GuideTargets<'w, 's> {
    rulers: Query<'w, 's, &'static CanvasRuler>,
    lines: Query<'w, 's, &'static GuideLine>,
    hosts: Query<'w, 's, &'static Viewport2dPanelHost>,
}

/// The panel a pointer event on a ruler or a guide belongs to, the axis of
/// the line it is about, and which guide of that axis it is, `None` for a
/// ruler, where a drag draws a new one. Resolved synchronously so the
/// observer can stop propagation before the event reaches the dock.
fn guide_gesture(
    target: Entity,
    button: PointerButton,
    parts: &GuideTargets,
) -> Option<(Entity, CanvasAxis, Option<usize>)> {
    if button != PointerButton::Primary {
        return None;
    }
    let (host, axis, index) = match parts.lines.get(target) {
        Ok(line) => (line.host, line.axis, Some(line.index)),
        Err(_) => {
            let ruler = parts.rulers.get(target).ok()?;
            (ruler.host, ruler.axis, None)
        }
    };
    if parts.hosts.get(host).ok()?.mode != Viewport2dMode::Edit {
        return None;
    }
    Some((host, axis, index))
}

/// What the guide gestures need to place the pointer against the line
/// that was drawn.
#[derive(SystemParam)]
struct GuidePointer<'w, 's> {
    ui_scale: Res<'w, UiScale>,
    slabs: Query<'w, 's, (&'static ComputedNode, &'static UiGlobalTransform), With<GuideLine>>,
}

/// How far from the drawn line a press still belongs to the guide, in
/// logical screen pixels, which is the same width at any zoom.
const GUIDE_GRAB_RADIUS: f32 = 1.0;

/// Whether a guide takes a pointer event at `cursor`, or lets it through to
/// the canvas underneath.
///
/// The slab is [`GUIDE_HIT_WIDTH`] across, but only the middle of it is the
/// guide's: a press within [`GUIDE_GRAB_RADIUS`] of the drawn line grabs
/// the line, and the pixels either side fall through, so a node a guide is
/// drawn along stays selectable. A ruler always takes its own.
fn guide_takes_the_press(
    pointer: &GuidePointer,
    guide: Option<Entity>,
    axis: CanvasAxis,
    cursor: Vec2,
) -> bool {
    let Some(guide) = guide else {
        return true;
    };
    let Ok((computed, transform)) = pointer.slabs.get(guide) else {
        return true;
    };
    let line = transform.translation * computed.inverse_scale_factor();
    (axis_of(cursor, axis) - axis_of(line, axis)).abs() <= GUIDE_GRAB_RADIUS
}

/// Claim a press on a ruler or a guide, so the dock never sees it.
fn on_guide_press(mut event: On<Pointer<Press>>, parts: GuideTargets, pointer: GuidePointer) {
    let target = event.event_target();
    let Some((_, axis, index)) = guide_gesture(target, event.button, &parts) else {
        return;
    };
    let cursor = event.pointer_location.position / pointer.ui_scale.0;
    if guide_takes_the_press(&pointer, index.map(|_| target), axis, cursor) {
        event.propagate(false);
    }
}

fn on_guide_drag_start(
    mut event: On<Pointer<DragStart>>,
    parts: GuideTargets,
    pointer: GuidePointer,
    mut commands: Commands,
) {
    let target = event.event_target();
    let Some((host, axis, index)) = guide_gesture(target, event.button, &parts) else {
        return;
    };
    let cursor = event.pointer_location.position / pointer.ui_scale.0;
    if !guide_takes_the_press(&pointer, index.map(|_| target), axis, cursor) {
        return;
    }
    event.propagate(false);
    commands.queue(move |world: &mut World| {
        let started = begin_guide_drag(world, host, axis, index, cursor);
        world.resource_mut::<GuideManipulation>().active = started;
    });
}

/// Pick a guide up, drawing a new one under the cursor when the drag came
/// off a ruler rather than off a line. The new guide goes straight onto the
/// scene; the history hears about it once, on release.
fn begin_guide_drag(
    world: &mut World,
    host: Entity,
    axis: CanvasAxis,
    index: Option<usize>,
    cursor: Vec2,
) -> Option<GuideDrag> {
    let root = world
        .query_filtered::<Entity, AuthoredUiSceneRoot>()
        .iter(world)
        .min()?;
    let before = world.get::<CanvasGuides>(root).cloned();
    let mut others = before
        .clone()
        .map(|guides| match axis {
            CanvasAxis::Vertical => guides.vertical,
            CanvasAxis::Horizontal => guides.horizontal,
        })
        .unwrap_or_default();
    let position = match index {
        Some(index) if index < others.len() => others.remove(index),
        Some(_) => return None,
        None => guide_landing(world, host, axis, cursor)?,
    };

    let drag = GuideDrag {
        host,
        root,
        axis,
        others,
        position,
        before,
        dropping: past_the_rulers_edge(world, host, axis, cursor),
    };
    crate::canvas_snap::preview_guides(
        world,
        root,
        crate::canvas_snap::held(drag_guides(&drag, true)),
    );
    Some(drag)
}

fn on_guide_drag(
    mut event: On<Pointer<Drag>>,
    parts: GuideTargets,
    ui_scale: Res<UiScale>,
    mut commands: Commands,
) {
    if guide_gesture(event.event_target(), event.button, &parts).is_none() {
        return;
    }
    event.propagate(false);
    let cursor = event.pointer_location.position / ui_scale.0;
    commands.queue(move |world: &mut World| {
        world.resource_scope(|world, mut manipulation: Mut<GuideManipulation>| {
            let Some(drag) = manipulation.active.as_mut() else {
                return;
            };
            let Some(position) = guide_landing(world, drag.host, drag.axis, cursor) else {
                return;
            };
            drag.position = position;
            drag.dropping = past_the_rulers_edge(world, drag.host, drag.axis, cursor);
            crate::canvas_snap::preview_guides(
                world,
                drag.root,
                crate::canvas_snap::held(drag_guides(drag, true)),
            );
        });
    });
}

fn on_guide_drag_end(mut event: On<Pointer<DragEnd>>, parts: GuideTargets, mut commands: Commands) {
    if guide_gesture(event.event_target(), event.button, &parts).is_none() {
        return;
    }
    event.propagate(false);
    commands.queue(|world: &mut World| finish_guide_drag(world, true));
}

/// End the guide drag, either keeping the line where the cursor left it or
/// putting the guides back the way the drag found them. A release over the
/// guide's own ruler takes it off the canvas. Exactly one history entry,
/// and none when the guides ended up where they started.
fn finish_guide_drag(world: &mut World, commit: bool) {
    let Some(drag) = world.resource_mut::<GuideManipulation>().active.take() else {
        return;
    };
    if !commit {
        crate::canvas_snap::preview_guides(world, drag.root, drag.before);
        return;
    }
    crate::canvas_snap::preview_guides(
        world,
        drag.root,
        crate::canvas_snap::held(drag_guides(&drag, !drag.dropping)),
    );
    crate::canvas_snap::commit_guides(world, drag.root, drag.before);
}

/// Put back whatever a canvas gesture is in the middle of, recording
/// nothing. Undo restores the very components a running gesture is editing,
/// so the gesture is cancelled first, exactly as Escape does; otherwise its
/// release would write its start state back over what the history put there.
pub fn cancel_canvas_gestures(world: &mut World) {
    finish_manipulation(world, false);
    finish_guide_drag(world, false);
}

/// Escape puts the guides back exactly as the drag found them.
fn cancel_guide_drag(
    keys: Res<ButtonInput<KeyCode>>,
    focus: crate::keybind_focus::KeybindFocus,
    manipulation: Res<GuideManipulation>,
    mut commands: Commands,
) {
    if focus.keyboard_is_spoken_for() {
        return;
    }
    if manipulation.active.is_some() && keys.just_pressed(KeyCode::Escape) {
        commands.queue(|world: &mut World| finish_guide_drag(world, false));
    }
}

/// Where a guide dragged to `cursor` comes to rest, in canvas-global
/// authored pixels. Whole pixels at the least, so the figure can be typed
/// back into the inspector; with the canvas's magnet on it lands on the
/// panel's own lattice instead, and Ctrl inverts that.
fn guide_landing(world: &World, host: Entity, axis: CanvasAxis, cursor: Vec2) -> Option<f32> {
    let raw = axis_of(cursor_on_canvas(world, host, cursor)?, axis);
    let ctrl = world
        .get_resource::<ButtonInput<KeyCode>>()
        .is_some_and(|keys| keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]));
    let kinds = world
        .get_resource::<CanvasSnap>()
        .copied()
        .unwrap_or_default();
    let grid = world
        .get::<Viewport2dPanelHost>(host)
        .map_or(0.0, |host| host.view.grid);
    if kinds.magnet(ctrl) && grid > 0.0 && grid.is_finite() {
        return Some((raw / grid).round() * grid);
    }
    Some(raw.round())
}

/// The coordinate a guide of this axis is stated in.
fn axis_of(point: Vec2, axis: CanvasAxis) -> f32 {
    match axis {
        CanvasAxis::Vertical => point.x,
        CanvasAxis::Horizontal => point.y,
    }
}

/// Where the cursor (ui-logical pixels) is over a panel's canvas, in
/// canvas-global authored pixels, on the canvas or off it.
fn cursor_on_canvas(world: &World, host: Entity, cursor: Vec2) -> Option<Vec2> {
    let host = world.get::<Viewport2dPanelHost>(host)?;
    let computed = world.get::<ComputedNode>(host.stage)?;
    let transform = world.get::<UiGlobalTransform>(host.stage)?;
    let target_scale = target_pixels_per_stage_pixel(computed.size(), host.target_size);
    let offset = crate::viewport_2d::stage_offset_unbounded(
        cursor,
        transform.translation,
        computed.inverse_scale_factor(),
        target_scale,
    );
    Some(stage_to_authored(offset, host.target_size))
}

/// Where `authored` (canvas-global authored pixels) is showing on a panel,
/// in ui-logical pixels: [`cursor_on_canvas`] run backwards, so a caller
/// can aim at the canvas's own coordinates rather than at wherever the
/// panel has been docked and panned. A degenerate stage answers `None`.
pub(crate) fn canvas_to_cursor(world: &World, host: Entity, authored: Vec2) -> Option<Vec2> {
    let host = world.get::<Viewport2dPanelHost>(host)?;
    let computed = world.get::<ComputedNode>(host.stage)?;
    let transform = world.get::<UiGlobalTransform>(host.stage)?;
    let target_scale = target_pixels_per_stage_pixel(computed.size(), host.target_size);
    if target_scale <= 0.0 || !target_scale.is_finite() {
        return None;
    }
    let offset = authored_to_stage(authored, host.target_size);
    Some((offset / target_scale + transform.translation) * computed.inverse_scale_factor())
}

/// Whether the cursor has gone past the stage area on the side the ruler
/// for `axis` sits on. Measured against the area rather than the ruler, so
/// going past the gutter altogether still counts.
fn past_the_rulers_edge(world: &World, host: Entity, axis: CanvasAxis, cursor: Vec2) -> bool {
    let Some(host) = world.get::<Viewport2dPanelHost>(host) else {
        return false;
    };
    let (Some(computed), Some(transform)) = (
        world.get::<ComputedNode>(host.area),
        world.get::<UiGlobalTransform>(host.area),
    ) else {
        return false;
    };
    let inverse_scale_factor = computed.inverse_scale_factor();
    let offset = cursor - transform.translation * inverse_scale_factor;
    let half = computed.size() * inverse_scale_factor / 2.0;
    match axis {
        CanvasAxis::Vertical => offset.y < -half.y,
        CanvasAxis::Horizontal => offset.x < -half.x,
    }
}

/// A handle straddling the edge or corner it drags: offset by half its own
/// size so it sits centred on the outline rather than inside it. On an axis
/// where `outside` is set the offset is the whole handle, so it sits clear
/// of the node. See [`HANDLES_OUTSIDE_BELOW`].
fn handle_node(x: i8, y: i8, outside: BVec2) -> Node {
    let mut node = Node {
        position_type: PositionType::Absolute,
        width: px(HANDLE_SIZE),
        height: px(HANDLE_SIZE),
        border: UiRect::all(px(OUTLINE_WIDTH)),
        ..default()
    };
    place_handle(&mut node, x, y, outside);
    node
}

/// Where the handle for `(x, y)` sits on an outline whose node is small
/// on the axes `outside` names.
fn place_handle(node: &mut Node, x: i8, y: i8, outside: BVec2) {
    let offset = |small: bool| {
        px(if small {
            -HANDLE_SIZE
        } else {
            -HANDLE_SIZE / 2.0
        })
    };
    node.margin = UiRect::default();
    match x {
        -1 => {
            node.left = offset(outside.x);
            node.right = Val::Auto;
        }
        1 => {
            node.right = offset(outside.x);
            node.left = Val::Auto;
        }
        _ => {
            node.left = percent(50);
            node.right = Val::Auto;
            node.margin.left = px(-HANDLE_SIZE / 2.0);
        }
    }
    match y {
        -1 => {
            node.top = offset(outside.y);
            node.bottom = Val::Auto;
        }
        1 => {
            node.bottom = offset(outside.y);
            node.top = Val::Auto;
        }
        _ => {
            node.top = percent(50);
            node.bottom = Val::Auto;
            node.margin.top = px(-HANDLE_SIZE / 2.0);
        }
    }
}

/// Whether each axis of a rect is small enough for its handles to be
/// drawn outside it.
fn handles_outside(rect: Rect) -> BVec2 {
    BVec2::new(
        rect.width() < HANDLES_OUTSIDE_BELOW,
        rect.height() < HANDLES_OUTSIDE_BELOW,
    )
}
