//! What the 2D canvas snaps a dragged node to, and the two view toggles
//! that go with it. Project-wide settings, stored in
//! `.jackdaw/settings.json` (see [`crate::project_settings`]) and kept out
//! of the undo snapshot so Ctrl+Z leaves them alone.

use std::path::PathBuf;

use bevy::{ecs::system::SystemParam, prelude::*, ui::UiTargetCamera};
use jackdaw_api::prelude::*;
use serde::{Deserialize, Serialize};

use jackdaw_scene_types::{CanvasGuides, UiSceneRoot};

use crate::commands::{CommandHistory, EditorCommand, SetCanvasGuides};
use crate::prefab::AuthoredUiSceneRoot;
use crate::project::ProjectRoot;
use crate::project_settings::{Section, load_section, store_section};
use crate::sprite_snap::SpriteSnapRules;
use crate::sprite_stage::{Scene2dRoots, world_on_panel};
use crate::ui_stage::{CanvasAxis, axis_of};
use crate::viewport_2d::{
    FrontedViewport, Viewport2dPanelHost, stage_origin, world_to_target_pixel,
};

/// The settings-file key the canvas settings live under.
const CANVAS_SECTION: &str = "canvas";

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<CanvasSnap>().add_systems(
        Update,
        (sync_project_canvas_snap, flag_menu_dirty_on_change),
    );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<CanvasSnapOp>()
        .register_operator::<CanvasRulersOp>()
        .register_operator::<CanvasGuidesOp>()
        .register_operator::<CanvasGameFrameOp>()
        .register_operator::<CanvasGuideAddOp>()
        .register_operator::<CanvasGuideRemoveOp>();
}

/// How near, in guide units, a position has to be to a guide to name it.
pub(crate) const GUIDE_MATCH: f32 = 0.5;

/// One switch on the canvas's snapping: the master, the pixel rounding,
/// or one kind of line a dragged node can land on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasSnapKind {
    /// Whether the canvas snaps at all.
    Enabled,
    /// Round the authored pixels a drag writes to whole numbers.
    Pixel,
    /// The parent's padding-box edges and centre.
    Parent,
    /// The lines of the parent box a percentage names: its quarters and
    /// its thirds.
    PercentLines,
    /// The near and far edges of the dragged node's siblings.
    SiblingSides,
    /// The centres of the dragged node's siblings.
    SiblingCenters,
    /// Nodes elsewhere in the scene, outside the dragged node's family.
    OtherNodes,
    /// The guides pulled off the rulers.
    Guides,
    /// The panel's grid, on a 2D world scene. A UI canvas always rests on
    /// its grid when nothing else is near.
    Grid,
    /// The edges and centres of other sprites, on a 2D world scene.
    SpriteEdges,
}

impl CanvasSnapKind {
    /// Every kind, in the order the canvas offers them.
    pub const ALL: [Self; 10] = [
        Self::Enabled,
        Self::Pixel,
        Self::Parent,
        Self::PercentLines,
        Self::SiblingSides,
        Self::SiblingCenters,
        Self::OtherNodes,
        Self::Guides,
        Self::Grid,
        Self::SpriteEdges,
    ];

    /// The kind `id` names, or `None` when it names none of them.
    pub fn parse(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }

    /// How a caller names this kind, as the `canvas.snap` operator's `kind`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Pixel => "pixel",
            Self::Parent => "parent",
            Self::PercentLines => "percent_lines",
            Self::SiblingSides => "sibling_sides",
            Self::SiblingCenters => "sibling_centers",
            Self::OtherNodes => "other_nodes",
            Self::Guides => "guides",
            Self::Grid => "grid",
            Self::SpriteEdges => "sprite_edges",
        }
    }

    /// How the menu row for this kind reads.
    pub fn label(self) -> &'static str {
        match self {
            Self::Enabled => "Use Snap",
            Self::Pixel => "Use Pixel Snap",
            Self::Parent => "Parent",
            Self::PercentLines => "Percent Lines",
            Self::SiblingSides => "Sibling Sides",
            Self::SiblingCenters => "Sibling Centers",
            Self::OtherNodes => "Other Nodes",
            Self::Guides => "Guides",
            Self::Grid => "Grid",
            Self::SpriteEdges => "Sprite Edges",
        }
    }

    /// Whether this kind has a say on a 2D world scene (`world`) or on a UI
    /// canvas (not `world`).
    pub fn applies(self, world: bool) -> bool {
        match self {
            Self::Enabled | Self::Pixel | Self::Guides => true,
            Self::Grid | Self::SpriteEdges => world,
            Self::Parent
            | Self::PercentLines
            | Self::SiblingSides
            | Self::SiblingCenters
            | Self::OtherNodes => !world,
        }
    }
}

/// Which kinds of line the 2D canvas offers a drag, and whether the rulers
/// and guides are drawn. Everything is on by default except
/// [`CanvasSnapKind::OtherNodes`]. [`CanvasSnap::enabled`] is the canvas's
/// own master, separate from the 3D tools' `SnapSettings`.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasSnap {
    /// Whether the canvas snaps at all. Ctrl inverts it for the length
    /// of a gesture.
    pub enabled: bool,
    /// Round the authored pixels a drag writes to whole numbers.
    pub pixel: bool,
    /// Snap to the parent's padding-box edges and centre.
    pub parent: bool,
    /// Snap to the lines of the parent box a percentage names.
    pub percent_lines: bool,
    /// Snap to sibling edges.
    pub sibling_sides: bool,
    /// Snap to sibling centres.
    pub sibling_centers: bool,
    /// Snap to nodes outside the dragged node's family.
    pub other_nodes: bool,
    /// Snap to the scene's guides.
    pub guides: bool,
    /// Snap a sprite to the panel's grid.
    pub grid: bool,
    /// Snap a sprite to other sprites' edges and centres.
    pub sprite_edges: bool,
    /// Draw the rulers along the top and left of the stage.
    pub show_rulers: bool,
    /// Draw the scene's guides over the stage.
    pub show_guides: bool,
    /// Outline what each pixel canvas camera of a 2D world scene shows.
    pub show_game_frame: bool,
}

impl Default for CanvasSnap {
    fn default() -> Self {
        Self {
            enabled: true,
            pixel: true,
            parent: true,
            percent_lines: true,
            sibling_sides: true,
            sibling_centers: true,
            other_nodes: false,
            guides: true,
            grid: true,
            sprite_edges: true,
            show_rulers: true,
            show_guides: true,
            show_game_frame: true,
        }
    }
}

impl CanvasSnap {
    /// Whether `kind` is on.
    pub fn offers(&self, kind: CanvasSnapKind) -> bool {
        match kind {
            CanvasSnapKind::Enabled => self.enabled,
            CanvasSnapKind::Pixel => self.pixel,
            CanvasSnapKind::Parent => self.parent,
            CanvasSnapKind::PercentLines => self.percent_lines,
            CanvasSnapKind::SiblingSides => self.sibling_sides,
            CanvasSnapKind::SiblingCenters => self.sibling_centers,
            CanvasSnapKind::OtherNodes => self.other_nodes,
            CanvasSnapKind::Guides => self.guides,
            CanvasSnapKind::Grid => self.grid,
            CanvasSnapKind::SpriteEdges => self.sprite_edges,
        }
    }

    /// Whether a drag lands on the scene's guides: they are offered and
    /// drawn. A hidden guide is not a line to aim at.
    pub fn snaps_to_guides(&self) -> bool {
        self.guides && self.show_guides
    }

    /// Whether a gesture snaps at all: the master, inverted by Ctrl for the
    /// length of the gesture. A sprite move follows it too; a sprite rotate
    /// or scale takes Ctrl for its own increments instead.
    pub fn magnet(&self, ctrl: bool) -> bool {
        self.enabled != ctrl
    }

    /// Turn `kind` on or off.
    pub fn set(&mut self, kind: CanvasSnapKind, on: bool) {
        let field = match kind {
            CanvasSnapKind::Enabled => &mut self.enabled,
            CanvasSnapKind::Pixel => &mut self.pixel,
            CanvasSnapKind::Parent => &mut self.parent,
            CanvasSnapKind::PercentLines => &mut self.percent_lines,
            CanvasSnapKind::SiblingSides => &mut self.sibling_sides,
            CanvasSnapKind::SiblingCenters => &mut self.sibling_centers,
            CanvasSnapKind::OtherNodes => &mut self.other_nodes,
            CanvasSnapKind::Guides => &mut self.guides,
            CanvasSnapKind::Grid => &mut self.grid,
            CanvasSnapKind::SpriteEdges => &mut self.sprite_edges,
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

/// Rebuild the top menu bar whenever the canvas settings move, so the View
/// menu's boxes stay current.
fn flag_menu_dirty_on_change(snap: Res<CanvasSnap>, mut dirty: ResMut<crate::MenuBarDirty>) {
    if snap.is_changed() {
        dirty.0 = true;
    }
}

/// Write the canvas settings back to the open project. A run with no
/// project open keeps them for the session and writes nothing.
pub(crate) fn persist(project: Option<&ProjectRoot>, snap: &CanvasSnap) {
    let Some(project) = project else {
        return;
    };
    store_section(&project.root, Section::Key(CANVAS_SECTION), snap);
}

/// Turn one kind of canvas snapping on or off. A preference, so no history
/// entry.
#[operator(
    id = "canvas.snap",
    label = "Set Canvas Snapping",
    description = "Turn one kind of 2D canvas snapping on or off.",
    allows_undo = false,
    params(
        kind(
            String,
            doc = "Which switch: `enabled`, `pixel`, `parent`, `percent_lines`, `sibling_sides`, `sibling_centers`, `other_nodes`, `guides`, `grid` or `sprite_edges`."
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
    let on = params.as_bool("on").unwrap_or(!snap.offers(kind));
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

/// Show or hide the outline of each pixel canvas camera's view.
#[operator(
    id = "canvas.game_frame",
    label = "Show Game Frame",
    description = "Show or hide what each pixel canvas camera of a 2D world scene shows.",
    allows_undo = false,
    params(on(bool, doc = "On or off. Omit to flip whichever way it currently is."))
)]
pub(crate) fn canvas_game_frame(
    params: In<OperatorParameters>,
    mut snap: ResMut<CanvasSnap>,
    project: Option<Res<ProjectRoot>>,
) -> OperatorResult {
    snap.show_game_frame = params.as_bool("on").unwrap_or(!snap.show_game_frame);
    persist(project.as_deref(), &snap);
    OperatorResult::Finished
}

/// Add a guide to the open scene, or remove the one nearest a position.
/// One history entry per edit, and none when nothing changes.
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

/// The guides a scene carries, or `None` for a set with nothing in it: the
/// component goes off the root with its last guide.
pub(crate) fn held(guides: CanvasGuides) -> Option<CanvasGuides> {
    (!guides.horizontal.is_empty() || !guides.vertical.is_empty()).then_some(guides)
}

/// Put `guides` on the root without telling the history, for a gesture
/// showing what it is about to do. [`commit_guides`] records the release.
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

/// What a scene's guides are measured in: a UI canvas's authored pixels from
/// its top-left corner (y down), or a 2D world scene's world units (y up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GuideSpace {
    /// A UI canvas of this reference size.
    Canvas(UVec2),
    /// A 2D world scene, read through each panel's view.
    World,
}

impl GuideSpace {
    /// How positions on `axis` map onto `host`'s render target: position `at`
    /// shows at target pixel `origin + step * at`, with `step` negative where
    /// positions grow against the pixels.
    pub(crate) fn target_axis(self, host: &Viewport2dPanelHost, axis: CanvasAxis) -> (f32, f32) {
        match self {
            Self::Canvas(_) => (0.0, 1.0),
            Self::World => {
                let zero = world_to_target_pixel(host.view, host.target_size, Vec2::ZERO);
                let one = world_to_target_pixel(host.view, host.target_size, Vec2::ONE);
                (axis_of(zero, axis), axis_of(one - zero, axis))
            }
        }
    }

    /// Where a guide at `at` on `axis` shows on `host`'s render target, in
    /// target pixels from its top-left corner.
    pub(crate) fn to_target(self, host: &Viewport2dPanelHost, axis: CanvasAxis, at: f32) -> f32 {
        let (origin, step) = self.target_axis(host, axis);
        origin + step * at
    }

    /// The guide position at target pixel `pixel` on `axis`: the inverse of
    /// [`Self::to_target`].
    pub(crate) fn position_at(
        self,
        host: &Viewport2dPanelHost,
        axis: CanvasAxis,
        pixel: f32,
    ) -> f32 {
        let (origin, step) = self.target_axis(host, axis);
        (pixel - origin) / step
    }

    /// Where position zero sits along `axis`'s ruler inside an `area`-sized
    /// stage area, in its logical pixels, and the signed logical pixels per
    /// position, for a stage drawn at `scale` stage pixels per target pixel.
    /// The guide lines' own mapping, moved to wherever the stage sits.
    pub(crate) fn ruler_axis(
        self,
        host: &Viewport2dPanelHost,
        area: Vec2,
        scale: f32,
        axis: CanvasAxis,
    ) -> (f32, f32) {
        let corner = match self {
            Self::Canvas(reference) => stage_origin(reference, host.view, area),
            Self::World => Vec2::ZERO,
        };
        let (origin, step) = self.target_axis(host, axis);
        (axis_of(corner, axis) + origin * scale, step * scale)
    }

    /// The step a dragged guide lands on, if any. On a UI canvas it is the
    /// panel's grid while the magnet is on, else a whole pixel; on a 2D world
    /// scene it is the grid or the `pixel`-wide sprite pixel a sprite drag
    /// would land on.
    pub(crate) fn lattice(
        self,
        kinds: &CanvasSnap,
        ctrl: bool,
        grid: f32,
        pixel: f32,
    ) -> Option<f32> {
        let magnet = kinds.magnet(ctrl);
        match self {
            Self::Canvas(_) => Some(if magnet && grid > 0.0 && grid.is_finite() {
                grid
            } else {
                1.0
            }),
            Self::World => {
                let rules = SpriteSnapRules::new(kinds, magnet, grid, pixel, 0.0);
                rules.magnet.then_some(rules.grid.or(rules.pixel)).flatten()
            }
        }
    }
}

/// The scenes a panel's guides can belong to.
#[derive(SystemParam)]
pub(crate) struct GuideScene<'w, 's> {
    ui: Query<
        'w,
        's,
        (
            Entity,
            &'static UiSceneRoot,
            Option<&'static UiTargetCamera>,
        ),
        AuthoredUiSceneRoot,
    >,
    world: Scene2dRoots<'w, 's>,
}

impl GuideScene<'_, '_> {
    /// The root whose guides `host` draws and edits, and the space they are
    /// measured in: the open 2D world scene's while the panel shows it, by the
    /// same routing rule the sprite stage takes, else the open UI scene's.
    /// With no panel, the UI scene's while one is open. A malformed document
    /// holding several roots of a kind picks the lowest entity.
    pub(crate) fn open(&self, host: Option<&Viewport2dPanelHost>) -> Option<(Entity, GuideSpace)> {
        let world = self.world.iter().min();
        let shows_world = match host {
            Some(host) => world_on_panel(
                world.is_some(),
                self.ui.iter().filter_map(|(.., routed)| routed),
                host,
            ),
            None => self.ui.is_empty(),
        };
        if shows_world {
            return world.map(|root| (root, GuideSpace::World));
        }
        self.ui
            .iter()
            .min_by_key(|(entity, ..)| *entity)
            .map(|(root, scene, _)| {
                let reference = scene.reference_size.max(UVec2::ONE);
                (root, GuideSpace::Canvas(reference))
            })
    }
}

/// Draw a guide down or across the open UI scene's canvas or 2D world scene.
#[operator(
    id = "canvas.guide.add",
    label = "Add Canvas Guide",
    description = "Add a guide line to the open UI scene's canvas or 2D world scene.",
    allows_undo = false,
    params(
        axis(
            String,
            doc = "`vertical` for a line down the canvas, `horizontal` for one across it."
        ),
        position(
            f64,
            doc = "Where the line sits: authored pixels from a UI canvas's top-left corner, or world units (y up) in a 2D world scene."
        )
    )
)]
pub(crate) fn canvas_guide_add(
    params: In<OperatorParameters>,
    scene: GuideScene,
    fronted: FrontedViewport,
    mut commands: Commands,
) -> OperatorResult {
    let Some((root, axis, position)) = guide_call(&params, &scene, &fronted, "canvas.guide.add")
    else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| edit_guides(world, root, axis, position, true));
    OperatorResult::Finished
}

/// Take away the guide nearest a position.
#[operator(
    id = "canvas.guide.remove",
    label = "Remove Canvas Guide",
    description = "Remove the guide nearest a position on the open UI scene's canvas or 2D world scene.",
    allows_undo = false,
    params(
        axis(
            String,
            doc = "`vertical` for a line down the canvas, `horizontal` for one across it."
        ),
        position(
            f64,
            doc = "Where to look: authored pixels from a UI canvas's top-left corner, or world units (y up) in a 2D world scene."
        )
    )
)]
pub(crate) fn canvas_guide_remove(
    params: In<OperatorParameters>,
    scene: GuideScene,
    fronted: FrontedViewport,
    mut commands: Commands,
) -> OperatorResult {
    let Some((root, axis, position)) = guide_call(&params, &scene, &fronted, "canvas.guide.remove")
    else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| edit_guides(world, root, axis, position, false));
    OperatorResult::Finished
}

/// What both guide operators need out of a call, or `None` with a reason
/// warned when the call names none of it.
fn guide_call(
    params: &OperatorParameters,
    scene: &GuideScene,
    fronted: &FrontedViewport,
    id: &str,
) -> Option<(Entity, CanvasAxis, f32)> {
    let Some(axis) = params.as_str("axis").and_then(CanvasAxis::parse) else {
        warn!("{id}: 'axis' must be `vertical` or `horizontal`");
        return None;
    };
    let Some(position) = params.as_float("position") else {
        warn!("{id}: 'position' must be a number");
        return None;
    };
    let Some((root, _)) = scene.open(fronted.two_d_host()) else {
        warn!("{id}: no UI or 2D world scene is open to draw a guide on");
        return None;
    };
    Some((root, axis, position as f32))
}
