//! Selection, the selection outline, and direct manipulation for the 2D
//! viewport stage.
//!
//! A click on the stage hit-tests the authored UI rects the panel is
//! showing and selects the topmost one; the outline and its eight resize
//! handles then track that node's live rect, and dragging either moves
//! or resizes the authored `Node` behind it.
//!
//! The overlay is editor chrome: editor entities parented into the
//! panel's stage node, never into the authored tree. Nothing that
//! happens to the authored scene can interrupt a gesture halfway
//! through, and nothing the overlay does can reach a saved document.
//!
//! All of it is [`Viewport2dMode::Edit`] behaviour: the press observer,
//! the gesture observers, and the overlay sync each read the mode off
//! the panel the stage belongs to. In `Interact` the outline comes down
//! and a press on the stage belongs to the scene (see
//! `crate::viewport_2d::forward_pointer_into_stage` for what carries it
//! there).
//!
//! A gesture writes `Node` on every drag event, and hands the history
//! exactly one entry on release: none at all if the pointer is released
//! where it started, or Escape was pressed.
//!
//! What it writes is the scheme the node was authored in: a node pinned
//! to its parent's bottom-right corner comes back pinned there, a
//! stretched one comes back stretched, and a percentage stays a
//! percentage. See [`NodeAnchors`].
//!
//! # Units
//!
//! Everything here is stated in the render-target pixels of
//! [`crate::viewport_2d::Ui2dView`], which are authored pixels: the
//! panel's image is held at the scene's reference size, so Bevy lays the
//! authored tree out directly in them and `ComputedNode` and
//! `UiGlobalTransform` read as authored measurements with no conversion.
//! The only two conversions are the ones `viewport_2d` owns, ui-logical
//! to stage physical and stage physical to render-target, composed once
//! in [`cursor_stage_offset`] on the way in and inverted by
//! [`stage_pixels_per_target_pixel`] on the way back out.
//!
//! There is no camera term. Bevy renders UI through a view of its own
//! (`bevy_ui_render::extract_ui_camera_view` builds an orthographic
//! projection from the target's viewport rect and parks the view
//! transform at the origin), so a routed UI scene is pinned to its
//! render target whatever the 2D camera's pan and zoom are doing.
//! Putting the view into this mapping would walk the hit test off the
//! visible pixels by exactly the pan.

use bevy::{
    ecs::system::SystemParam,
    picking::{
        events::{Drag, DragEnd, DragStart, Move, Pointer, Press},
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

/// Thinnest a resize may leave a node, in authored pixels.
const MIN_NODE_SIZE: f32 = 1.0;

/// Draw order of the overlay inside the stage. Above the stage's own
/// frame, and above anything else placed in the stage alongside it.
const OVERLAY_Z: i32 = 50;

/// Draw order of the pre-select outline: over the canvas, under the
/// selection outline and its handles.
const HOVER_OUTLINE_Z: i32 = OVERLAY_Z - 1;

/// How wide a guide is to the pointer, in the stage's logical pixels. A
/// one-pixel line is drawn down the middle of it: the line has to be
/// thin to place a node against, and wide enough to pick up again.
///
/// Narrow, because the slab lies over the canvas: two pixels either side
/// of the line is enough to catch and little enough to keep the guide
/// off the nodes it was drawn to place. What a press over a node does
/// is [`guide_takes_the_press`].
const GUIDE_HIT_WIDTH: f32 = 5.0;

/// How close, in **pointer** pixels, a dragged edge has to come to a
/// neighbouring one before it lands on it.
///
/// Pointer pixels rather than authored ones, so the radius stays
/// constant on screen at any zoom. The gesture converts it with the
/// scale the panel is drawing at as each drag event arrives (see
/// [`live_scale`]), so a zoom mid-gesture moves the radius with it.
///
/// A module constant rather than a [`CanvasSnap`] field: the radius is
/// what "near" means on this canvas, not a preference the user states.
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

/// A line drawn across one panel's stage where a drag came to rest.
///
/// One per axis at most, and only while a gesture is landing on
/// something: the point of it is to say *why* the node stopped where it
/// did, so it appears with the landing and goes with it.
#[derive(Component, Clone, Copy)]
pub struct SnapHighlight {
    /// The panel content entity carrying this stage's
    /// [`Viewport2dPanelHost`].
    pub host: Entity,
    /// Which way the line runs.
    pub axis: CanvasAxis,
}

/// One of the scene's guides, drawn over one panel's stage.
///
/// The guides belong to the scene rather than to a panel, so every panel
/// showing that scene draws the same set. `index` is the guide's place
/// in its axis's list, which is kept sorted, so it names the same line
/// for as long as the list holds still.
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

/// The outline drawn around the selected authored UI node in one panel.
#[derive(Component, Clone, Copy)]
pub struct UiSelectionOverlay {
    /// The panel content entity carrying this stage's
    /// [`Viewport2dPanelHost`].
    pub host: Entity,
}

/// The outline drawn around the authored UI node under the cursor, before
/// anything has been clicked.
///
/// A canvas of nested boxes says nothing about where one ends and the next
/// begins until something is selected, so a press is a guess. This draws
/// what the press would pick.
///
/// Told apart from [`UiSelectionOverlay`] by everything: a lighter line,
/// no handles, no dark edge, and no pointer of its own. It never covers
/// the selected node, whose own outline is the answer there.
#[derive(Component, Clone, Copy)]
pub struct UiHoverOutline {
    /// The panel content entity carrying this stage's
    /// [`Viewport2dPanelHost`].
    pub host: Entity,
}

/// The node the cursor is over, and the panel it is over it on.
///
/// One pointer authors at a time, so one entry covers every panel: moving
/// onto another canvas replaces it, and moving off the canvases clears it.
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
    /// The `Node` as the gesture found it, for the undo entry and for
    /// Escape.
    before: Node,
    /// Authored-pixel rect at gesture start: left, top, width, height.
    start: Vec4,
    /// How the node was positioned when the gesture began. The drag
    /// writes back through this, editing the scheme the author wrote
    /// rather than replacing it. See [`NodeAnchors`].
    anchors: NodeAnchors,
    /// What this node's units are measured against, in authored pixels.
    /// Read once at the press: the parents hold still for the length of
    /// a gesture.
    basis: UnitBasis,
}

/// The gesture in progress, if any. One gesture is one history entry.
///
/// # What a gesture carries
///
/// A move carries the whole selection, and every node in it keeps its
/// own start rect and its own scheme: the selected nodes can sit under
/// different parents, in different units, pinned to different edges, and
/// each one has to be written back through what its author wrote.
///
/// A resize carries the primary alone. The handles are drawn around the
/// primary's rect and only its rect, so a resize is a gesture on that
/// one node.
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
    /// primary's start rect is stated in. Gathered once, around the
    /// primary alone: the parent and the siblings hold still for the
    /// length of a gesture.
    ///
    /// The primary is what snaps, and the snapped delta is what the rest
    /// of the selection moves by. Letting every selected node answer its
    /// own neighbours would pull a selection apart: two nodes eight
    /// pixels apart land on two different edges and stop being eight
    /// pixels apart.
    candidates: SnapCandidates,
    /// Which kinds of line this gesture may land on, copied off
    /// [`CanvasSnap`] at the press so a preference changed mid-drag
    /// cannot move a node that is already following the cursor.
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

    /// Whether a canvas gesture is in flight. A drag holds nodes from the
    /// press until the release, whatever the pointer is doing meanwhile.
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
    /// Where the line sits in the parent box as a percentage, when it
    /// is a line the parent box can state that way. A node whose author
    /// wrote the matching offset in percent takes this figure verbatim
    /// rather than one derived from pixels.
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
    /// The parent's padding-box corner in global authored pixels: what a
    /// candidate's coordinate is measured from, and so what turns one
    /// back into a position on the canvas.
    origin: Vec2,
}

/// The line one axis of a drag came to rest against.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SnapWinner {
    /// The line's coordinate, in the authored offsets the drag is
    /// stated in: measured from the parent's padding-box corner, not
    /// from the canvas.
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
/// write. Default is "nothing exact", which is every drag that came to
/// rest somewhere the parent box has no percentage for.
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
    /// Two decimal places, so a drag zoomed past one pixel per pointer
    /// pixel keeps the fraction it produced.
    Fractional,
}

/// The lines of the parent box a percentage names, as percentages.
///
/// The quarters and the thirds. A third is a figure a `Val::Percent`
/// layout reaches for as readily as a quarter -- three columns across a
/// row, a panel a third of the way down -- and it is the one an author
/// cannot get by eye, because the pixels it lands on round to something
/// that is not a third of anything.
const PERCENT_LINES: [f32; 7] = [0.0, 25.0, 100.0 / 3.0, 50.0, 200.0 / 3.0, 75.0, 100.0];

/// How finely a gesture states the pixels it writes.
///
/// Keyed on the pixel kind alone. The master magnet and Ctrl decide
/// whether a drag lands on a neighbour at all; how many decimals the
/// result is written with is a separate question, answered once, and
/// tying the two would make Ctrl silently change the units a drag
/// commits.
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

    /// How many authored pixels one of this unit is worth, or `None`
    /// when nothing it is measured against has a usable size yet.
    ///
    /// A degenerate basis is a refusal rather than a fallback to pixels,
    /// so that a parent measuring zero for one frame cannot rewrite
    /// `50%` as `Val::Px`.
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

    /// Decimals a magnitude in this unit is rounded to on the way back
    /// out. Whole pixels while the canvas is on its pixel lattice; two
    /// places otherwise and for every other unit, matching what the
    /// inspector's `Val` field shows and commits.
    ///
    /// A landing on one of the [`PERCENT_LINES`] is exempt: the figure
    /// is written whole, so a third of the parent box is
    /// `Val::Percent(100.0 / 3.0)` rather than a `33.33` that is a third
    /// of nothing. Those landings do not come through here at all; see
    /// [`exact_percent_for`].
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
    /// its own, so the size is whatever the two edges leave between them.
    ///
    /// An author who set all three said something over-constrained, and
    /// Bevy resolves that by the near offset and the size. Writing the
    /// size there is what makes a resize resize.
    fn stretched(self) -> bool {
        self.near.is_some() && self.far.is_some() && self.size.is_none()
    }
}

/// How a node is positioned on both axes.
///
/// # What manipulation preserves
///
/// A gesture computes a rect in authored pixels and projects it back
/// through the scheme it found, so a dialog pinned to the bottom-right
/// corner stays pinned there, a bar stretched across its parent stays
/// stretched, and a panel laid out in percentages keeps following the
/// canvas. Writing `left`/`top` and clearing `right`/`bottom` instead
/// would look identical in the frame the gesture ends and wrong at the
/// next resolution.
///
/// The projection:
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
/// `min` and `extent` are the axis of the new rect, in authored pixels
/// from the parent's offset box, and `parent` is that box's extent on
/// the same axis. `resized` says the gesture moved an edge rather than
/// the whole node.
///
/// A value whose unit has nothing usable to be measured against is left
/// alone rather than rewritten in pixels: see [`AnchorUnit::authored_px`].
///
/// `exact` is the axis's landing, if it came to rest on a line the
/// parent box states as a percentage. An offset its author wrote in
/// percent then takes that figure verbatim: a parent whose width is not
/// a multiple of four has no exact percentage for a quarter line once
/// the figure has been through pixels and back.
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
    // The far offset is the gap between the node's far edge and the
    // parent's, so it needs a parent that has been measured.
    if let Some(unit) = anchor.far
        && placed
        && parent > 0.0
        && parent.is_finite()
    {
        match exact_percent_for(unit, exact, SnapLine::Max) {
            // The far offset is measured back from the parent's far
            // edge, so a landing a quarter of the way in is three
            // quarters of the way back.
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

/// Write a manipulated rect back into `node` through the scheme its
/// author wrote it in. See [`NodeAnchors`].
///
/// `rect` is `(left, top, width, height)` in authored pixels from the
/// parent's offset box; `edges` is the gesture's, `(0, 0)` for a move.
/// `rounding` says how finely pixels are stated, and `exact` carries any
/// percentage the gesture landed on.
///
/// # What a resize does to a flowed child
///
/// A move is a statement about where the node goes, and a node whose
/// parent lays it out has nowhere to go until it leaves that layout, so a
/// move promotes it to `PositionType::Absolute` and writes offsets.
///
/// A resize says nothing about placement. Promoting there would take a
/// row's child out of the row the moment the user widened it, moving the
/// node and every sibling after it while the user was dragging one edge.
/// So a resize on a flowed child writes `width`/`height` alone and leaves
/// both the placement and `position_type` to the parent.
///
/// A node already absolute keeps both halves on either gesture: its
/// offsets are what place it, so a handle that drags its left edge has to
/// move `left`.
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
            .add_observer(on_stage_press)
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
                )
                    .chain(),
            );
    }
}

/// Authored point (render-target pixels, origin at the canvas's top-left
/// corner) under a cursor sitting `stage_offset` render-target pixels
/// from the centre of the stage, as [`cursor_stage_offset`] reports it.
///
/// The whole mapping is this recentring: the panel's image *is* the
/// authored canvas, at one image pixel per authored pixel.
pub fn stage_to_authored(stage_offset: Vec2, target_size: UVec2) -> Vec2 {
    stage_offset + target_size.as_vec2() / 2.0
}

/// Inverse of [`stage_to_authored`].
///
/// The overlay is placed from layout, so no editor code calls this; the
/// mapping tests state the contract as a round trip through both
/// directions, which catches an asymmetric recentring.
pub fn authored_to_stage(authored: Vec2, target_size: UVec2) -> Vec2 {
    authored - target_size.as_vec2() / 2.0
}

/// Stage-node logical pixels per render-target pixel: what an authored
/// measurement is multiplied by to become an overlay `Node` value.
///
/// `target_scale` is [`target_pixels_per_stage_pixel`] for the panel and
/// `inverse_scale_factor` is the stage's own
/// `ComputedNode::inverse_scale_factor()`, so this is the two factors of
/// the cursor path run backwards. A degenerate stage yields the identity
/// rather than an infinity: this runs every frame something is selected,
/// including the frame a panel is first laid out.
pub fn stage_pixels_per_target_pixel(target_scale: f32, inverse_scale_factor: f32) -> f32 {
    if target_scale <= 0.0 {
        return 1.0;
    }
    inverse_scale_factor / target_scale
}

/// Authored (render-target) pixels per pointer pixel: what a
/// [`Pointer<Drag>`] distance is multiplied by to become an authored
/// delta.
///
/// `stage_scale` is [`stage_pixels_per_target_pixel`], so this is that
/// factor inverted with the [`UiScale`] taken back out: pointer
/// locations are reported before the UI scale is applied, and `Node`
/// values are stated after it. A degenerate factor yields the identity
/// rather than an infinity.
pub fn target_pixels_per_pointer_pixel(stage_scale: f32, ui_scale: f32) -> f32 {
    let factor = stage_scale * ui_scale;
    if factor <= 0.0 {
        return 1.0;
    }
    1.0 / factor
}

/// The authored node a click at `point` lands on, or `None` when it
/// misses every one.
///
/// The pick is the one Bevy paints last, the highest
/// [`ComputedStackIndex`]. `ui_stack_system` assigns it from the tree
/// walk and `ZIndex` together and it is unique per node, so it decides
/// on its own wherever layout has run.
///
/// The tree-order tiebreak covers the frame before it has.
/// `ComputedStackIndex` is missing on a node that has never been through
/// a stack pass and reads `0` for all of them, and `hits` is built
/// depth-first (parents before children, siblings in `Children` order),
/// so taking the last entry matches what Bevy would paint last: the
/// later sibling over the earlier, the child over its parent.
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
    /// Nothing is routed to this panel, or the cursor is not over its
    /// stage at all. The cursor says nothing about this panel's
    /// selection: an empty stage must not erase a selection made in
    /// another viewport.
    Empty,
}

/// The authored node `cursor` (ui-logical pixels) is over on `host`'s
/// stage.
///
/// The one place the stage's pixels become an authored node: resolving
/// what a press on the overlay is over, and hit-testing a drag, both go
/// through here rather than through a second copy of the mapping.
pub(crate) fn hit_at(
    cursor: Vec2,
    host: &Viewport2dPanelHost,
    stage: (&ComputedNode, &UiGlobalTransform),
    roots: &Query<(Entity, &UiTargetCamera), AuthoredUiSceneRoot>,
    nodes: &AuthoredNodes,
    children: &Query<&Children>,
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

    let mut hits = Vec::new();
    for (root, routed) in roots {
        if routed.entity() == host.camera {
            collect_stage_hits(root, nodes, children, &mut hits);
        }
    }
    if hits.is_empty() {
        return StagePick::Empty;
    }

    match topmost_hit(point, &hits) {
        Some(entity) => StagePick::Hit(entity),
        None => StagePick::Miss,
    }
}

/// The rubber band a drag from bare canvas is pulling out.
///
/// A drag that started on a node is that node's move; this is the other
/// drag, the one that starts where there is nothing and gathers up
/// whatever it is pulled across. The two never both run: the band only
/// starts when the press under it hit nothing.
#[derive(Resource, Default)]
pub struct MarqueeSelect {
    band: Option<Marquee>,
    seed: Option<MarqueeSeed>,
}

/// What the press left behind for a band that may follow it.
///
/// The press runs before the drag starts and has already had its say on
/// the selection -- a press on the backdrop selects the scene's root, the
/// way it always has. A band built from the selection as the drag finds it
/// would therefore carry that root, so the press records what was selected
/// *before* it, and the band builds from that instead.
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
    /// Shift and Ctrl as of the press, the same reading
    /// [`on_stage_press`] takes: what the band gathers is added to, or
    /// toggled against, the selection that press left.
    extend: bool,
    toggle: bool,
    /// That selection, so a band swept back and forth answers the state
    /// the gesture started from rather than the one it last wrote.
    base: Vec<Entity>,
}

/// The band drawn over one panel's stage.
#[derive(Component, Clone, Copy)]
pub struct MarqueeOverlay {
    /// The panel content entity carrying this stage's
    /// [`Viewport2dPanelHost`].
    pub host: Entity,
}

/// Draw order of the band: over the canvas and over the selection
/// outlines, because it is what the pointer is doing right now.
const MARQUEE_Z: i32 = OVERLAY_Z + 2;

/// Where `cursor` (ui-logical pixels) is on the canvas `stage` is drawing,
/// in authored pixels, whether or not it is still over the stage.
///
/// Unbounded, because a band is pulled past the canvas edge as readily as
/// across it, and a drag that stopped reporting at the edge would leave the
/// band frozen there while the pointer went on.
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
/// fills it. A press on either selects the root and has done so since the
/// canvas was built, so the band is what a drag from there means; a press
/// on any other node is that node's move.
///
/// The drag can be delivered to the stage or to the selection outline
/// lying over it, depending on what the press selected, so the band
/// remembers which and answers only that one.
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
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let target = event.event_target();
    // A handle is a resize whatever is under it.
    if handles.contains(target) {
        return;
    }
    // An outline belongs to the node it is drawn around, and a drag on it
    // is that node's move -- unless the node is the scene's own root,
    // whose outline covers the whole canvas and would otherwise swallow
    // every band pulled out after the backdrop was clicked once.
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
    if let StagePick::Hit(entity) = hit_at(cursor, host, stage, &roots, &nodes, &children)
        && !roots.contains(entity)
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

/// Take everything the band was pulled across.
///
/// Intersection rather than containment: what the band touches is what it
/// picks up, so a container wider than the panel does not have to be swept
/// end to end.
///
/// The scene's own root is never picked up. It covers the whole canvas, so
/// every band would take it, and a band that always selects the root is a
/// band that selects nothing in particular.
fn on_marquee_end(
    mut event: On<Pointer<DragEnd>>,
    hosts: Query<&Viewport2dPanelHost>,
    roots: Query<(Entity, &UiTargetCamera), AuthoredUiSceneRoot>,
    nodes: AuthoredNodes,
    children: Query<&Children>,
    mut selection: ResMut<Selection>,
    mut marquee: ResMut<MarqueeSelect>,
    mut commands: Commands,
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

    let mut swept = Vec::new();
    for (root, routed) in &roots {
        if routed.entity() != host.camera {
            continue;
        }
        let mut hits = Vec::new();
        collect_stage_hits(root, &nodes, &children, &mut hits);
        swept.extend(
            hits.into_iter()
                .filter(|hit| hit.entity != root)
                .filter(|hit| overlaps(rect, hit.rect))
                .map(|hit| hit.entity),
        );
    }

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

/// Escape drops the band and leaves the selection as the press left it.
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
            // The band lies over the canvas it is being pulled across, and
            // the drag it belongs to is delivered to the stage underneath.
            Pickable::IGNORE,
            ChildOf(stage),
        ));
    }
}

/// Select the authored node under a press on the stage, in
/// [`Viewport2dMode::Edit`].
///
/// In `Interact` the press belongs to the scene: it is claimed off the
/// dock all the same, but it selects nothing.
///
/// Propagation is stopped synchronously, before the observer defers
/// anything: a press that climbed out of the stage would reach the dock
/// leaf and start a panel drag under the gesture the user meant for the
/// canvas. Only the primary button is claimed, so a middle-drag still
/// reaches the pan handler and a right-click still reaches whatever
/// context menu the panel carries.
///
/// # Presses on the outline
///
/// The outline covers the whole selected node, so every press on the
/// selected node's own area lands on the overlay rather than on the
/// stage. Claiming those as the move gesture without asking what is
/// under them makes a selected container swallow every click on its
/// children.
///
/// So the press is re-resolved through [`hit_at`] wherever it lands. The
/// selected node under the cursor is the move gesture; anything else, a
/// child or an overlapping sibling, is selected instead. The handles
/// keep their resize gesture unconditionally.
///
/// # Modifiers
///
/// Shift adds the node under the cursor to the selection, Ctrl toggles it
/// in or out. Both are read at the press, before any drag has started.
///
/// Ctrl is also the snap magnet's inverter, which [`on_gesture_drag`]
/// reads on every drag event. One key, two jobs, and a Ctrl-press that
/// then drags does both: the modifiers held at the press decide the
/// selection, and the modifiers held during the drag decide the
/// snapping. Neither reads the other's moment, so holding Ctrl only after
/// the button went down inverts the magnet without touching the
/// selection, and releasing it after the press leaves the toggle done and
/// the magnet alone.
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
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let target = event.event_target();

    // A press on a handle is the start of a resize, and must not fall
    // through to the dock.
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
    let pick = hit_at(cursor, host, stage, &roots, &nodes, &children);
    let extend = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let toggle = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);

    // What a band that follows this press would build from. Taken here,
    // before the press has had its say, because a press on the backdrop
    // selects the scene's root and a band is not a selection of the root.
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
        // Only a hit on something else re-selects. A miss must not clear
        // a selection about to be dragged: the outline can extend past
        // whatever laid the scene out.
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
        // A modified press is building a selection, so a miss beside the
        // nodes it is building from must not empty it.
        StagePick::Miss if !extend && !toggle => selection.clear(&mut commands),
        StagePick::Miss | StagePick::Empty => {}
    }
}

/// Track the authored node under the cursor, for the pre-select outline.
///
/// The same resolution the press does, on every pointer move: the panel
/// the cursor is over, then [`hit_at`] on its stage. In `Interact` the
/// pointer belongs to the scene, so nothing is tracked there.
///
/// A running gesture clears it. The node being dragged is already
/// outlined, and a second outline chasing whatever the cursor passes over
/// mid-drag says nothing about what the release will do.
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
) {
    let target = event.event_target();
    // A handle is part of the overlay it hangs off, so the cursor on one
    // is still the cursor on that panel's stage.
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
            match hit_at(cursor, host, stage, &roots, &nodes, &children) {
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
///
/// [`on_stage_hover`] only runs while the pointer is over a stage, so
/// without this the last node it passed over stays named, and its outline
/// stays drawn, for as long as the pointer is somewhere else: over a
/// panel, over another window, or off the screen entirely, which fires no
/// move at all.
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

/// Keep at most one pre-select outline, over the node
/// [`UiHoverPreselect`] names.
///
/// Built on the same placement the selection outline uses, so the two
/// agree on where a node is. Every selected node is skipped, not only the
/// primary one: each already carries a selection outline, and drawing a
/// second line over one of them says the hover would change the selection
/// when it would not.
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

/// Spawn one panel's pre-select outline: one thin border and nothing
/// else.
///
/// [`Pickable::IGNORE`] because it lies over the node it is drawn around:
/// the press that follows the hover has to reach the stage underneath.
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
/// [`topmost_hit`] resolves ties in.
///
/// A [`Locked`] node contributes no hit of its own, so a press over it
/// reaches whatever else is there: the node under it, or the canvas. Its
/// children still do -- the lock is on the node the author locked, not on
/// everything inside it -- and nothing is said about the press either way,
/// because a lock is asking for the clicks to go elsewhere and a notice
/// per click would be the noise it was set to stop.
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

/// Keep exactly one overlay per panel being authored, covering the
/// selected authored node's live rect.
///
/// The rect is read off the selected entity every frame rather than
/// cached, so the outline follows layout without anything having to tell
/// it that layout moved.
///
/// A panel in [`Viewport2dMode::Interact`] has no overlay at all.
/// Despawning it rather than hiding it leaves the gesture observers
/// nothing to fire on, whatever the selection does meanwhile.
fn sync_selection_overlays(
    mut commands: Commands,
    selection: Res<Selection>,
    hosts: Query<(Entity, &Viewport2dPanelHost)>,
    stages: Query<&ComputedNode, With<Scene2dViewport>>,
    authored: SelectedNode,
    overlays: Query<(Entity, &UiSelectionOverlay)>,
    mut nodes: Query<&mut Node>,
) {
    let selected = selection.primary();

    for (host_entity, host) in &hosts {
        let overlay = overlays
            .iter()
            .find(|(_, overlay)| overlay.host == host_entity)
            .map(|(entity, _)| entity);

        let placement = match (host.mode, selected) {
            (Viewport2dMode::Edit, Some(entity)) => {
                overlay_placement(entity, host, &stages, &authored)
            }
            _ => Placement::Drop,
        };

        match placement {
            Placement::At(rect) => match overlay {
                Some(overlay) => {
                    if let Ok(mut node) = nodes.get_mut(overlay) {
                        place_outline(&mut node, rect);
                    }
                }
                None => spawn_overlay(&mut commands, host_entity, host.stage, rect),
            },
            // Nothing to move it to this frame; leave what is on screen.
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

/// What this frame has to say about a panel's outline.
enum Placement {
    /// The selected node is this panel's, and here is its rect in the
    /// stage's logical pixels.
    At(Rect),
    /// The selected node is this panel's but has no layout to draw
    /// against yet. Hold the rect the overlay already has: an overlay
    /// that vanishes for a frame takes any gesture running on it with
    /// it.
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
/// # Reading against the content
///
/// The chrome is drawn over authored content of any colour, so a single
/// accent colour would disappear against content of the same luminance.
/// The handles are a light neutral fill with an accent border, reading
/// on dark content by their fill and on light content by their border,
/// and the outline carries a dark edge just outside its accent line for
/// the same reason in the other direction.
///
/// The dark edge is one node, not one per side: a box pinned a pixel
/// outside the outline on all four edges draws its border and nothing
/// else. It is [`Pickable::IGNORE`] because it covers the outline body,
/// and a press on the body is a gesture or a reselection (see
/// [`on_stage_press`]) that must still reach the overlay underneath.
fn spawn_overlay(commands: &mut Commands, host: Entity, stage: Entity, rect: Rect) {
    let mut node = Node {
        position_type: PositionType::Absolute,
        border: UiRect::all(px(OUTLINE_WIDTH)),
        ..default()
    };
    place_outline(&mut node, rect);

    let overlay = commands
        .spawn((
            UiSelectionOverlay { host },
            EditorEntity,
            node,
            BorderColor::all(tokens::ACCENT_BLUE),
            ZIndex(OVERLAY_Z),
            Pickable::default(),
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

    for (x, y) in HANDLE_POSITIONS {
        commands.spawn((
            UiResizeHandle { x, y },
            EditorEntity,
            handle_node(x, y),
            BackgroundColor(tokens::TEXT_PRIMARY),
            BorderColor::all(tokens::ACCENT_BLUE),
            Pickable::default(),
            ChildOf(overlay),
        ));
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

/// Which edges a gesture on `target` drags, and the overlay it belongs
/// to, if `target` is part of one on a panel that is being authored.
///
/// Resolved synchronously so the observer can stop propagation before
/// the event climbs out of the panel and into the dock. Only the primary
/// button is claimed, so a middle-drag that starts on the outline still
/// pans the view, and only [`Viewport2dMode::Edit`] is claimed at all.
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
        // A gesture that could not be measured leaves no half-built
        // state behind for the next drag event to act on.
        *world.resource_mut::<UiManipulation>() = started.unwrap_or_default();
    });
}

/// Everything a gesture needs to know at the moment the pointer went
/// down, or `None` when there is nothing measurable to drag.
///
/// A move takes the whole selection; a resize takes the primary alone
/// (see [`UiManipulation`]). Either way the primary leads the list, so
/// the snap and the scale are read off the node under the cursor.
fn begin_manipulation(world: &World, overlay: Entity, edges: (i8, i8)) -> Option<UiManipulation> {
    let host_entity = world.get::<UiSelectionOverlay>(overlay)?.host;
    let host = world.get::<Viewport2dPanelHost>(host_entity)?;
    let selection = world.get_resource::<Selection>()?;
    let primary = selection.primary()?;
    // The scene's own root is the canvas, not something on it: a drag
    // from the backdrop pulls a band out (see [`on_marquee_start`]), so
    // there is no move here to pick up.
    if edges == (0, 0) && is_scene_root(world, primary) {
        return None;
    }
    let primary_node = gesture_node(world, primary, host)?;
    let nodes = if edges == (0, 0) {
        let movable = without_selected_ancestors(world, &selection.entities);
        // Primary first, so the node under the cursor anchors the
        // gesture, unless a selected container above it is what moves.
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
        // A resize is the primary's alone: dragging one handle must not
        // stretch every other node in the selection.
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

/// The selected entities that no other selected entity contains.
///
/// A container and a node inside it can both be in the selection. Layout
/// already carries the child when its container moves, so applying the
/// gesture's delta to both would move the child twice. Only the
/// outermost selected node of each chain is written.
pub(crate) fn without_selected_ancestors(world: &World, selected: &[Entity]) -> Vec<Entity> {
    let set: std::collections::HashSet<Entity> = selected.iter().copied().collect();
    selected
        .iter()
        .copied()
        .filter(|entity| {
            let mut cursor = *entity;
            // The whole chain, not just the immediate parent: a
            // grandchild of a selected container is carried too.
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
/// authored node of this panel's canvas with a rect to drag.
///
/// The camera check keeps out a selection made in another viewport: the
/// editor's selection is one list for the whole editor, so a 3D entity
/// or a node another panel is showing can be sitting in it while this
/// canvas is dragged.
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
    // zero. Bevy lays such a root out directly against the render target, and
    // that is what its percentages and its right/bottom offsets are stated
    // against; left at zero those units resolve to nothing and the root is
    // undraggable. Per axis, so a parent degenerate on one side is covered
    // too.
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

/// The gesture's scale as of right now, or `None` when the panel it
/// started on has gone.
///
/// Read again on every drag event rather than taken once at the press.
/// The wheel still belongs to the panel while the button is down, so the
/// canvas can zoom mid-gesture; a scale captured at the press would then
/// convert pointer pixels at a rate the panel has stopped drawing at,
/// and the node would trail or outrun the cursor for the rest of the
/// drag.
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
/// or `None` for a node layout has not measured.
///
/// This is the space two nodes under different parents can be compared in,
/// which is what a bounding box over a selection needs.
pub(crate) fn global_node_rect(world: &World, entity: Entity) -> Option<Rect> {
    let size = world.get::<ComputedNode>(entity)?.size();
    if size.x <= 0.0 || size.y <= 0.0 {
        return None;
    }
    let centre = world.get::<UiGlobalTransform>(entity)?.translation;
    Some(Rect::from_corners(centre - size / 2.0, centre + size / 2.0))
}

/// The box a child's `left`/`top` are measured from, the parent's
/// padding box, in the global authored pixels layout reports.
///
/// Inside the border, not at the parent's outer corner: an absolutely
/// placed child's offsets start where the border ends, so reading them
/// against the border box would shift the offset a gesture starts from
/// and every edge it can snap to. The two shifts do not cancel, because
/// the offset comes from the node's own `Val::Px` and the candidates
/// from layout, so a bordered parent would land a snap one border-width
/// past the edge it aimed at and make a promoted flex child jump.
///
/// A node with no parent is measured from the canvas itself.
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

/// Authored left/top the gesture starts from.
///
/// A node with no explicit offset starts from where layout put it, so
/// promoting a flex child to absolute placement does not make it jump on
/// the first drag event.
fn authored_offset(world: &World, entity: Entity, rect: Rect) -> Vec2 {
    if let Some(node) = world.get::<Node>(entity)
        && node.position_type == PositionType::Absolute
        && let (Val::Px(left), Val::Px(top)) = (node.left, node.top)
    {
        return Vec2::new(left, top);
    }
    rect.min
}

/// The `left`/`top` a move on `entity` starts from, in the authored
/// offsets its own `Node` states them in.
///
/// The pointer's own starting figure, for a caller that moves a node
/// without a pointer: an operator computes where the node should end up in
/// the global pixels layout reports, and this is what that answer is added
/// to on the way back into the node's own space.
pub(crate) fn authored_offset_of(world: &World, entity: Entity) -> Option<Vec2> {
    let rect = authored_rect(world, entity)?;
    Some(authored_offset(world, entity, rect))
}

/// What `entity`'s units are measured against, read off layout.
///
/// [`gesture_node`] takes the viewport from the panel the drag is running
/// on; a caller with no panel takes it from the scene's own root, which is
/// laid out directly against the render target and so measures the canvas.
/// A parent that measures zero on an axis falls back to that same canvas,
/// exactly as a gesture's does.
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

/// The lines a gesture on `entity` can land on, in the same offset space
/// [`authored_rect`] reports, so they can be compared with the value the
/// gesture is about to write.
///
/// The order is the precedence a landing is decided by, nearest distance
/// first and this order to break a tie: the parent's own edges and
/// centre, sibling sides, sibling centres, the scene's guides, nodes
/// elsewhere in the tree, and last the lines a percentage of the parent
/// box names. An edge something in the scene actually has beats a line
/// that is only a figure: the thirds put a percent line within reach of
/// a sibling often enough that a tie is a real case, and the author can
/// see the edge. A percent line still wins when it is strictly nearer.
///
/// `kinds` decides which of those are offered at all; an off kind
/// contributes nothing rather than being filtered later, so nothing it
/// governs can win a tie against a kind that is on.
///
/// Editor chrome is skipped, so an overlay drawn over the same tree
/// cannot become something the authored scene snaps to.
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
        // percent-authored node landing on an edge or the centre writes
        // the exact figure whether or not the quarter lines are on.
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

/// The topmost ancestor of `entity`: the root of the scene it is part
/// of, and where the scene's guides are kept.
///
/// A walk up rather than a query for the routed root, which is the same
/// entity: the gesture has already established that the dragged node
/// draws into this panel's camera.
fn scene_root(world: &World, entity: Entity) -> Entity {
    let mut root = entity;
    while let Some(next) = world.get::<ChildOf>(root).map(ChildOf::parent) {
        root = next;
    }
    root
}

/// Every authored node under the same routed root that is not part of
/// the dragged node's family, measured from `origin`.
///
/// The family is the parent, the parent's own children (those are the
/// siblings, which have their own kinds), and the whole selection with
/// everything under it: a node the gesture is carrying cannot be
/// something the gesture lands on.
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
    // descendants with it, so none of them holds still to land on.
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

/// Move or resize what the gesture picked up, every drag event.
///
/// A move carries every selected node on this canvas; a resize carries
/// the primary alone, because the handles are drawn around the primary's
/// rect. The primary is also what snaps, and the rest of the selection
/// moves by the delta it snapped to. See [`UiManipulation`].
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
            // The primary's whole delta, snap included; the rest of the
            // selection moves by it.
            let delta = distance * scale + outcome.nudge;
            let rounding = pixel_rounding(&manipulation.kinds);
            // Only the primary landed on anything, so only the primary
            // may write a landing's exact percentage.
            let exact = outcome.exact_percent();
            for (ordinal, node) in manipulation.nodes.iter().enumerate() {
                // Only the primary is resized; the rest of a selection
                // is carried along by the move.
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

/// Move the edges `edges` names by `delta`, in authored pixels.
///
/// The rect is `(left, top, width, height)`. A move slides both offsets
/// and leaves the size alone; a resize moves one or two edges and lets
/// the opposite ones stay where they are, which is why the near edges
/// take the delta out of the size again.
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
/// [`MIN_NODE_SIZE`] on either axis, with the origin held back by
/// however much the size had to be floored.
///
/// The floor alone is not enough. Dragging a left or top handle past the
/// opposite edge keeps moving `left`/`top` with the cursor while the
/// width bottoms out, so the node walks off across the canvas a single
/// pixel wide. The edge not being dragged has to hold still, so the
/// origin stops exactly [`MIN_NODE_SIZE`] short of it.
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

/// How far the gesture's dragged edges have to move to land on a
/// neighbour, or on the canvas's pixel grid when no neighbour is near.
///
/// The moving geometry is the whole rect for a move and the dragged
/// edge or corner for a resize, so a resize snaps the edge under the
/// cursor and leaves the opposite one alone, and both answer the same
/// lattice.
///
/// # One switch
///
/// Both kinds of snapping are decided by `magnet` once, at the top:
/// [`CanvasSnap::enabled`] inverted by Ctrl for the length of the
/// gesture. Consulting the individual kinds a second time further down
/// would make Ctrl mean "edges only" and give the master's off state two
/// meanings depending on which kind of snap was near.
///
/// `grid` is the lattice in authored pixels; `scale` is authored pixels
/// per pointer pixel, which is what turns [`EDGE_SNAP_PIXELS`] into a
/// radius the candidates can be measured against.
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
    // The grid only has a say on an axis no neighbour claimed; rounding
    // an edge landing afterwards would take it straight back off.
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

/// Name the line an axis landed on.
///
/// A resize collapses the moving rect to the dragged corner, so all
/// three of its lines are the same coordinate and the reported one says
/// nothing about which edge of the node moved. `edge` is that axis of
/// the gesture's handle, and it is what decides: a positive edge moved
/// the far side, a negative one the near side.
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

/// `point` rounded onto a lattice of `grid` authored pixels.
///
/// A non-positive or non-finite grid is no grid at all rather than a
/// division by zero: the value comes off a per-panel view.
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
/// The keyboard half of direct manipulation. It writes through the same
/// two pieces the pointer does, the rect arithmetic of [`drag_edges`]
/// and the scheme projection of [`apply_authored_rect`], so a nudged
/// node keeps the offsets its author wrote and a whole selection moves
/// together.
///
/// # The step
///
/// One authored pixel, or the panel's own canvas grid with Shift held,
/// which is the lattice the header's stepper sets. Not the 3D grid: that
/// is a lattice of world units, and at the editor's default power it
/// rounds an authored pixel to a quarter of one.
///
/// # Separate from the 3D nudge
///
/// The arrow keys are also the editor's 3D nudge, and
/// [`crate::entity_ops::nudge_selected`] translates a `Transform`. A UI
/// node has none: a canvas moves its nodes through `Node`, so the same
/// keys reach a different writer. The selection decides which; see
/// `crate::transform_ops`.
///
/// # One entry per press
///
/// A burst of presses is a burst of history entries, matching the 3D
/// nudge. Nothing coalesces them: a key has no release that marks the
/// end of an edit the way the scrub fields' pointer release does.
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
    // The panel showing the canvas the selection is on, and only while
    // it is being authored: in `Interact` the keys belong to the scene.
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
        let nodes: Vec<GestureNode> = without_selected_ancestors(world, &selected)
            .into_iter()
            .filter_map(|entity| gesture_node(world, entity, host))
            .collect();
        (nodes, step)
    };
    if nodes.is_empty() {
        return false;
    }
    // A node its parent lays out has no offsets to step: a nudge would
    // promote it out of the flow, which is a change to the layout and not
    // the one-pixel move the key asked for. The drag has a rect and a
    // cursor to promote against; a keystroke has neither, so it refuses
    // and says so. The canvas still answers, so the 3D nudge does not
    // take the key instead.
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
        // Kind-blind: a nudge is a keystroke of exactly one step, so
        // nothing it writes is up to what the canvas offers a drag.
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
/// landing on something, and nowhere else.
///
/// Driven off the gesture rather than off the selection: a landing is a
/// property of the drag in progress, so the line comes up with the first
/// drag event that lands and goes on the release, which clears the
/// outcome.
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
            // Above the outline: the line says where the edge came to
            // rest, so the edge must not be drawn over it.
            ZIndex(OVERLAY_Z + 1),
            // A press over the line belongs to the gesture that drew it.
            Pickable::IGNORE,
            ChildOf(line.stage),
        ));
    }
}

/// One line the running gesture wants drawn. Every landing is painted
/// [`tokens::SNAP_HIGHLIGHT`], whatever kind of line it was: the
/// highlight says the drag came to rest, and the line it came to rest
/// against is already drawn in its own right.
struct HighlightLine {
    host: Entity,
    stage: Entity,
    axis: CanvasAxis,
    /// Where the line sits in the stage's logical pixels.
    at: f32,
}

/// The lines the gesture in progress is landing on, in the stage's own
/// logical pixels.
///
/// A candidate is stated from the dragged node's parent, so the
/// canvas position is the landing plus the parent's own corner. Leaving
/// that term out draws the line on the canvas origin's copy of it, which
/// is the same place only while the parent is the root.
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

/// Draw the open scene's guides over every panel showing it.
///
/// Guides are scene data, not a property of a gesture or a selection, so
/// they stand whatever the canvas is doing; what takes them down is the
/// panel leaving [`Viewport2dMode::Edit`], where the stage belongs to the
/// running scene, or the canvas settings saying they are hidden.
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
            // Under the selection chrome: a guide says where to put a
            // node, and the node being placed is what the user is
            // looking at.
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

/// Every guide every panel wants drawn.
///
/// A guide's position is canvas-global authored pixels, and the stage
/// node *is* the canvas, so the whole mapping is the stage's own scale:
/// there is no parent corner to add, unlike a snap landing.
fn guide_lines(
    snap: &CanvasSnap,
    roots: &Query<(Entity, &CanvasGuides), AuthoredUiSceneRoot>,
    hosts: &Query<(Entity, &Viewport2dPanelHost)>,
    stages: &Query<&ComputedNode, With<Scene2dViewport>>,
) -> Vec<WantedGuide> {
    if !snap.show_guides {
        return Vec::new();
    }
    // A malformed document holding several roots picks the lowest
    // entity, the same one the guide operators write.
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

/// The guides the scene carries while `drag` is running, with the
/// dragged line in or out.
///
/// A guide dragged onto another is the other one: two lines on the same
/// pixel are one line the user cannot pull apart again, and the operator
/// that adds a guide refuses the same way.
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

/// The panel a pointer event on a ruler or a guide belongs to, the axis
/// of the line it is about, and which guide of that axis it is, `None`
/// for a ruler, where a drag draws a new one.
///
/// Resolved synchronously so the observer can stop propagation before
/// the event climbs out of the panel and into the dock. Only the primary
/// button and only [`Viewport2dMode::Edit`], like every other gesture on
/// the canvas.
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
/// logical screen pixels: the canvas zoom sizes the stage node rather
/// than scaling the pointer, so the target is the same width at any zoom.
const GUIDE_GRAB_RADIUS: f32 = 1.0;

/// Whether a guide takes a pointer event at `cursor`, or lets it through
/// to the canvas underneath.
///
/// The slab a guide is picked up by is [`GUIDE_HIT_WIDTH`] across, so
/// the line is not a one-pixel target, but only the middle of the slab
/// is the guide's: a press within [`GUIDE_GRAB_RADIUS`] of the drawn
/// line grabs the line, and the pixels either side of that fall through
/// to whatever the canvas has there. A guide drawn along a node's edge
/// -- which is what guides are for -- can therefore still be dragged off
/// the line itself, while the node it marks stays selectable a pixel
/// away, whichever of the two is on top.
///
/// A press on a ruler is never over the canvas at all, so a ruler always
/// takes its own.
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
    // The press off the line went to the canvas, so the drag that
    // follows it belongs to the canvas too.
    if !guide_takes_the_press(&pointer, index.map(|_| target), axis, cursor) {
        return;
    }
    event.propagate(false);
    commands.queue(move |world: &mut World| {
        let started = begin_guide_drag(world, host, axis, index, cursor);
        // A drag that could not be measured leaves nothing half-built
        // behind for the next event to act on.
        world.resource_mut::<GuideManipulation>().active = started;
    });
}

/// Pick a guide up, drawing a new one under the cursor when the drag
/// came off a ruler rather than off a line.
///
/// The new guide goes straight onto the scene: it is what the drag is
/// showing, and the history hears about it once, on release.
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

/// End the guide drag, either keeping the line where the cursor left it
/// or putting the guides back the way the drag found them.
///
/// A release over the guide's own ruler takes it off the canvas: the
/// ruler is where a guide comes from, so it is where one goes back to.
/// Either way the history hears exactly one entry, and none at all when
/// the guides ended up where they started.
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
/// nothing.
///
/// Undo and redo restore the very components a running gesture is
/// editing, and the gesture is still holding the state it started from:
/// its release would write that back over what the history just put
/// there, and record the difference as an edit the user never made. So
/// an undo cancels the gesture first, exactly as Escape does.
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
/// authored pixels.
///
/// Whole pixels at the least: a guide is read back as a figure and typed
/// into the inspector, and a line at 320.37 is one nothing else can be
/// aimed at. With the canvas's magnet on it lands on the panel's own
/// lattice instead, the one a dragged node lands on, and Ctrl inverts
/// that exactly as it inverts a node's drag.
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

/// Whether the cursor has gone past the stage area on the side the ruler
/// for `axis` sits on: the top for a vertical guide, the left for a
/// horizontal one.
///
/// Measured against the area rather than against the ruler, so the
/// answer is the same wherever along the gutter the cursor is, and going
/// past the gutter altogether still counts.
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

/// A handle straddling the edge or corner it drags: offset by half its
/// own size so it sits centred on the outline rather than inside it.
///
/// The border is inside the square, because Bevy measures a `Node` as
/// its border box: the handle stays [`HANDLE_SIZE`] on a side, so its
/// hit area matches what is drawn.
fn handle_node(x: i8, y: i8) -> Node {
    let half = px(-HANDLE_SIZE / 2.0);
    let mut node = Node {
        position_type: PositionType::Absolute,
        width: px(HANDLE_SIZE),
        height: px(HANDLE_SIZE),
        border: UiRect::all(px(OUTLINE_WIDTH)),
        ..default()
    };
    match x {
        -1 => node.left = half,
        1 => node.right = half,
        _ => {
            node.left = percent(50);
            node.margin.left = half;
        }
    }
    match y {
        -1 => node.top = half,
        1 => node.bottom = half,
        _ => {
            node.top = percent(50);
            node.margin.top = half;
        }
    }
    node
}
