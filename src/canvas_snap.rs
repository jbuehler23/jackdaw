//! What the 2D canvas snaps a dragged node to, and the two view toggles
//! that go with it.
//!
//! Project-wide rather than per tab or per document: the answer to "what
//! do my drags land on" is a way of working, so it lives beside the other
//! project settings in `.jackdaw/settings.json` (see
//! [`crate::project_settings`]) and follows the project rather than the
//! scene. The grid a tab snaps to stays per tab, on `Ui2dView`.
//!
//! Deliberately outside the undo snapshot: Ctrl+Z after a drag puts the
//! node back, and taking the user's snap preferences with it would be a
//! surprise. That is what keeps [`CanvasSnap`] off both
//! `EditorStateSnapshot` and `SnapSettings`.

use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use serde::{Deserialize, Serialize};

use jackdaw_scene_types::CanvasGuides;

use crate::commands::{CommandHistory, EditorCommand, SetCanvasGuides};
use crate::project::ProjectRoot;
use crate::project_settings::{Section, load_section, store_section};
use crate::ui_stage::CanvasAxis;

/// The settings-file key the canvas settings live under.
const CANVAS_SECTION: &str = "canvas";

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<CanvasSnap>()
        .add_systems(Update, sync_project_canvas_snap);
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<CanvasSnapOp>()
        .register_operator::<CanvasRulersOp>()
        .register_operator::<CanvasGuidesOp>()
        .register_operator::<CanvasGuideAddOp>()
        .register_operator::<CanvasGuideRemoveOp>();
}

/// How near, in authored pixels, a position has to be to a guide to name
/// it. Half a pixel: guides are placed by hand and read back as exact
/// numbers, so naming one is naming its position.
pub(crate) const GUIDE_MATCH: f32 = 0.5;

/// One kind of line the canvas offers a dragged node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasSnapKind {
    /// Round the authored pixels a drag writes to whole numbers.
    Pixel,
    /// The parent's padding-box edges and centre.
    Parent,
    /// Quarters of the parent box: 0, 25, 50, 75 and 100 percent.
    PercentLines,
    /// The near and far edges of the dragged node's siblings.
    SiblingSides,
    /// The centres of the dragged node's siblings.
    SiblingCenters,
    /// Nodes elsewhere in the scene, outside the dragged node's family.
    OtherNodes,
    /// The guides pulled off the rulers.
    Guides,
}

impl CanvasSnapKind {
    /// Every kind, in the order the canvas offers them and the menu lists
    /// them.
    pub const ALL: [Self; 7] = [
        Self::Pixel,
        Self::Parent,
        Self::PercentLines,
        Self::SiblingSides,
        Self::SiblingCenters,
        Self::OtherNodes,
        Self::Guides,
    ];

    /// The kind `id` names, or `None` when it names none of them.
    pub fn parse(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }

    /// How a caller names this kind: the `kind` parameter of the
    /// `canvas.snap` operator.
    pub fn id(self) -> &'static str {
        match self {
            Self::Pixel => "pixel",
            Self::Parent => "parent",
            Self::PercentLines => "percent_lines",
            Self::SiblingSides => "sibling_sides",
            Self::SiblingCenters => "sibling_centers",
            Self::OtherNodes => "other_nodes",
            Self::Guides => "guides",
        }
    }

    /// How the menu row for this kind reads.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pixel => "Use Pixel Snap",
            Self::Parent => "Parent",
            Self::PercentLines => "Percent Lines",
            Self::SiblingSides => "Sibling Sides",
            Self::SiblingCenters => "Sibling Centers",
            Self::OtherNodes => "Other Nodes",
            Self::Guides => "Guides",
        }
    }
}

/// Which kinds of line the 2D canvas offers a drag, and whether the
/// rulers and guides are drawn.
///
/// Everything is on out of the box except [`CanvasSnapKind::OtherNodes`],
/// which reaches across the scene and would otherwise pull a node towards
/// something the user cannot see next to it.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasSnap {
    /// Round the authored pixels a drag writes to whole numbers.
    pub pixel: bool,
    /// Snap to the parent's padding-box edges and centre.
    pub parent: bool,
    /// Snap to the quarter lines of the parent box.
    pub percent_lines: bool,
    /// Snap to sibling edges.
    pub sibling_sides: bool,
    /// Snap to sibling centres.
    pub sibling_centers: bool,
    /// Snap to nodes outside the dragged node's family.
    pub other_nodes: bool,
    /// Snap to the scene's guides.
    pub guides: bool,
    /// Draw the rulers along the top and left of the stage.
    pub show_rulers: bool,
    /// Draw the scene's guides over the stage.
    pub show_guides: bool,
}

impl Default for CanvasSnap {
    fn default() -> Self {
        Self {
            pixel: true,
            parent: true,
            percent_lines: true,
            sibling_sides: true,
            sibling_centers: true,
            other_nodes: false,
            guides: true,
            show_rulers: true,
            show_guides: true,
        }
    }
}

impl CanvasSnap {
    /// Whether `kind` is on.
    pub fn enabled(&self, kind: CanvasSnapKind) -> bool {
        match kind {
            CanvasSnapKind::Pixel => self.pixel,
            CanvasSnapKind::Parent => self.parent,
            CanvasSnapKind::PercentLines => self.percent_lines,
            CanvasSnapKind::SiblingSides => self.sibling_sides,
            CanvasSnapKind::SiblingCenters => self.sibling_centers,
            CanvasSnapKind::OtherNodes => self.other_nodes,
            CanvasSnapKind::Guides => self.guides,
        }
    }

    /// Turn `kind` on or off.
    pub fn set(&mut self, kind: CanvasSnapKind, on: bool) {
        let field = match kind {
            CanvasSnapKind::Pixel => &mut self.pixel,
            CanvasSnapKind::Parent => &mut self.parent,
            CanvasSnapKind::PercentLines => &mut self.percent_lines,
            CanvasSnapKind::SiblingSides => &mut self.sibling_sides,
            CanvasSnapKind::SiblingCenters => &mut self.sibling_centers,
            CanvasSnapKind::OtherNodes => &mut self.other_nodes,
            CanvasSnapKind::Guides => &mut self.guides,
        };
        *field = on;
    }
}

/// Load the open project's canvas settings, once per project opened.
fn sync_project_canvas_snap(
    project: Option<Res<ProjectRoot>>,
    mut snap: ResMut<CanvasSnap>,
    mut loaded_root: Local<Option<PathBuf>>,
) {
    let Some(project) = project else {
        return;
    };
    if loaded_root.as_ref() == Some(&project.root) {
        return;
    }
    *loaded_root = Some(project.root.clone());
    *snap = load_section(&project.root, Section::Key(CANVAS_SECTION));
}

/// Write the canvas settings back to the open project. A run with no
/// project open keeps them for the session and writes nothing.
fn persist(project: Option<&ProjectRoot>, snap: &CanvasSnap) {
    let Some(project) = project else {
        return;
    };
    store_section(&project.root, Section::Key(CANVAS_SECTION), snap);
}

/// Turn one kind of canvas snapping on or off.
///
/// Preferences rather than scene data, so no history entry: undo after a
/// drag moves the node back and leaves the user's snapping alone.
#[operator(
    id = "canvas.snap",
    label = "Set Canvas Snapping",
    description = "Turn one kind of 2D canvas snapping on or off.",
    allows_undo = false,
    params(
        kind(
            String,
            doc = "Which kind: `pixel`, `parent`, `percent_lines`, `sibling_sides`, `sibling_centers`, `other_nodes` or `guides`."
        ),
        on(bool, doc = "On or off. Omit to flip whichever way it currently is.")
    )
)]
pub(crate) fn canvas_snap(
    params: In<OperatorParameters>,
    mut snap: ResMut<CanvasSnap>,
    project: Option<Res<ProjectRoot>>,
) -> OperatorResult {
    let Some(kind) = params.as_str("kind").and_then(CanvasSnapKind::parse) else {
        warn!("canvas.snap: 'kind' must name one of the canvas's snap kinds");
        return OperatorResult::Cancelled;
    };
    let on = params.as_bool("on").unwrap_or(!snap.enabled(kind));
    snap.set(kind, on);
    persist(project.as_deref(), &snap);
    OperatorResult::Finished
}

/// Show or hide the canvas rulers.
#[operator(
    id = "canvas.rulers",
    label = "Show Rulers",
    description = "Show or hide the 2D canvas's rulers.",
    allows_undo = false,
    params(on(bool, doc = "On or off. Omit to flip whichever way it currently is."))
)]
pub(crate) fn canvas_rulers(
    params: In<OperatorParameters>,
    mut snap: ResMut<CanvasSnap>,
    project: Option<Res<ProjectRoot>>,
) -> OperatorResult {
    snap.show_rulers = params.as_bool("on").unwrap_or(!snap.show_rulers);
    persist(project.as_deref(), &snap);
    OperatorResult::Finished
}

/// Show or hide the scene's guides.
#[operator(
    id = "canvas.guides",
    label = "Show Guides",
    description = "Show or hide the 2D canvas's guides.",
    allows_undo = false,
    params(on(bool, doc = "On or off. Omit to flip whichever way it currently is."))
)]
pub(crate) fn canvas_guides(
    params: In<OperatorParameters>,
    mut snap: ResMut<CanvasSnap>,
    project: Option<Res<ProjectRoot>>,
) -> OperatorResult {
    snap.show_guides = params.as_bool("on").unwrap_or(!snap.show_guides);
    persist(project.as_deref(), &snap);
    OperatorResult::Finished
}

/// Add a guide to the open UI scene, or remove the one nearest a
/// position.
///
/// One history entry per edit, and none at all when the scene already
/// says what the caller asked for: adding a guide where one already sits
/// or removing one from empty canvas changes nothing.
fn edit_guides(world: &mut World, root: Entity, axis: CanvasAxis, position: f32, add: bool) {
    let before = world.get::<CanvasGuides>(root).cloned();
    let mut next = before.clone().unwrap_or_default();
    let lines = match axis {
        CanvasAxis::Vertical => &mut next.vertical,
        CanvasAxis::Horizontal => &mut next.horizontal,
    };
    if add {
        if lines.iter().any(|at| (at - position).abs() <= GUIDE_MATCH) {
            return;
        }
        lines.push(position);
        lines.sort_by(f32::total_cmp);
    } else {
        let nearest = lines
            .iter()
            .enumerate()
            .filter(|(_, at)| (*at - position).abs() <= GUIDE_MATCH)
            .min_by(|(_, a), (_, b)| (*a - position).abs().total_cmp(&(*b - position).abs()))
            .map(|(index, _)| index);
        let Some(index) = nearest else {
            return;
        };
        lines.remove(index);
    }
    record_guides(world, root, before, held(next));
}

/// The guides a scene carries, or `None` for a set with nothing in it.
///
/// The component goes off the root with its last guide, so an empty one
/// never reaches a saved document.
pub(crate) fn held(guides: CanvasGuides) -> Option<CanvasGuides> {
    (!guides.horizontal.is_empty() || !guides.vertical.is_empty()).then_some(guides)
}

/// Put `guides` on the root without telling the history, for a gesture
/// showing what it is about to do. What the history is told is
/// [`commit_guides`], once, when the gesture is released.
pub(crate) fn preview_guides(world: &mut World, root: Entity, guides: Option<CanvasGuides>) {
    let Ok(mut entity) = world.get_entity_mut(root) else {
        return;
    };
    match guides {
        Some(guides) => {
            entity.insert(guides);
        }
        None => {
            entity.remove::<CanvasGuides>();
        }
    }
}

/// Hand the history the guide edit a gesture has already written onto
/// the scene: `before` as the gesture found the component, whatever the
/// root carries now as the edit.
pub(crate) fn commit_guides(world: &mut World, root: Entity, before: Option<CanvasGuides>) {
    let after = world.get::<CanvasGuides>(root).cloned().and_then(held);
    record_guides(world, root, before, after);
}

/// One history entry for a change to the scene's guides, and none at all
/// when the two sides say the same thing.
fn record_guides(
    world: &mut World,
    root: Entity,
    before: Option<CanvasGuides>,
    after: Option<CanvasGuides>,
) {
    if after == before {
        return;
    }
    let mut command = SetCanvasGuides {
        root,
        before,
        after,
    };
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(command));
}

/// The open UI scene's root, or `None` when no UI scene is open. A
/// malformed document holding several picks the lowest entity, so which
/// one is chosen does not follow archetype order.
fn guide_root(roots: &Query<Entity, crate::prefab::AuthoredUiSceneRoot>) -> Option<Entity> {
    roots.iter().min()
}

/// Draw a guide down or across the open UI scene's canvas.
#[operator(
    id = "canvas.guide.add",
    label = "Add Canvas Guide",
    description = "Add a guide line to the open UI scene's canvas.",
    allows_undo = false,
    params(
        axis(
            String,
            doc = "`vertical` for a line down the canvas, `horizontal` for one across it."
        ),
        position(
            f64,
            doc = "Where the line sits, in authored pixels from the canvas's top-left corner."
        )
    )
)]
pub(crate) fn canvas_guide_add(
    params: In<OperatorParameters>,
    roots: Query<Entity, crate::prefab::AuthoredUiSceneRoot>,
    mut commands: Commands,
) -> OperatorResult {
    let Some((root, axis, position)) = guide_call(&params, &roots, "canvas.guide.add") else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| edit_guides(world, root, axis, position, true));
    OperatorResult::Finished
}

/// Take away the guide nearest a position.
#[operator(
    id = "canvas.guide.remove",
    label = "Remove Canvas Guide",
    description = "Remove the guide nearest a position on the open UI scene's canvas.",
    allows_undo = false,
    params(
        axis(
            String,
            doc = "`vertical` for a line down the canvas, `horizontal` for one across it."
        ),
        position(
            f64,
            doc = "Where to look, in authored pixels from the canvas's top-left corner."
        )
    )
)]
pub(crate) fn canvas_guide_remove(
    params: In<OperatorParameters>,
    roots: Query<Entity, crate::prefab::AuthoredUiSceneRoot>,
    mut commands: Commands,
) -> OperatorResult {
    let Some((root, axis, position)) = guide_call(&params, &roots, "canvas.guide.remove") else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| edit_guides(world, root, axis, position, false));
    OperatorResult::Finished
}

/// What both guide operators need out of a call, or `None` with a reason
/// warned when the call names none of it.
fn guide_call(
    params: &OperatorParameters,
    roots: &Query<Entity, crate::prefab::AuthoredUiSceneRoot>,
    id: &str,
) -> Option<(Entity, CanvasAxis, f32)> {
    let Some(axis) = params.as_str("axis").and_then(CanvasAxis::parse) else {
        warn!("{id}: 'axis' must be `vertical` or `horizontal`");
        return None;
    };
    let Some(position) = params.as_float("position") else {
        warn!("{id}: 'position' must be a number of authored pixels");
        return None;
    };
    let Some(root) = guide_root(roots) else {
        warn!("{id}: no UI scene is open to draw a guide on");
        return None;
    };
    Some((root, axis, position as f32))
}
