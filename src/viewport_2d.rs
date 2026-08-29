//! The 2D presentation of a viewport panel: a `Camera2d` rendered into a
//! texture and shown in a UI node, mirroring the 3D presentation's
//! camera-to-texture pattern (see [`crate::viewport`]).
//!
//! One of the two presentations every viewport panel builds; which of them
//! is displayed is the panel's mode, in [`crate::viewport_host`].
//!
//! Holds the stage skeleton, its camera, the teardown that keeps a closed
//! panel from leaking either, and the stage's navigation: an [`Ui2dView`]
//! per panel that scroll and middle-drag move, that `place_stage` turns
//! into the stage node's size and position, and that rides along with the
//! scene tab it was framed for. [`route_ui_roots_to_cameras`] points
//! authored UI roots at this camera and
//! [`size_targets_to_reference`] holds that camera's image at the authored
//! reference size. Selection and the editing overlays live in
//! [`crate::ui_stage`].
//!
//! In [`Viewport2dMode::Interact`], `forward_pointer_into_stage` drives a
//! pointer of the panel's own across the render target, so `bevy_ui`'s
//! picking backend finds the authored widgets there.
//!
//! Framing is a request ([`request_2d_fit`], the `viewport2d.frame`
//! operator) rather than a computation: the two numbers a fit needs are
//! only settled after layout, and [`size_targets_to_reference`] honours
//! the request where it already holds both.

use bevy::{
    asset::uuid::Uuid,
    camera::{NormalizedRenderTarget, RenderTarget, visibility::RenderLayers},
    image::{ImageSampler, ToExtents},
    input::mouse::{MouseMotion, MouseScrollUnit, MouseWheel},
    picking::{
        PickingSystems,
        hover::HoverMap,
        pointer::{Location, PointerId, PointerInput, PointerLocation, PointerPress},
    },
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
    ui::{UiGlobalTransform, UiSystems, UiTargetCamera},
    ui_widgets::observe,
};
use jackdaw_feathers::{
    button::{ButtonProps, ButtonSize, ButtonVariant, button},
    menu_bar::{
        OP_ACTION_PREFIX, SECTION_ACTION_PREFIX, SEPARATOR_ACTION, checked_row, menu_button,
        submenu_row,
    },
    tokens,
};
use jackdaw_scene_types::UiSceneRoot;

use crate::{
    canvas_snap::{CanvasGuidesOp, CanvasRulersOp, CanvasSnap, CanvasSnapKind, CanvasSnapOp},
    prefab::{AuthoredUiSceneRoot, ImportedUiSceneRoot},
    prelude::*,
    selection::Selection,
    ui_stage::{CanvasAxis, stage_to_authored},
    viewport::{
        DEFAULT_VIEWPORT_HEIGHT, DEFAULT_VIEWPORT_WIDTH, InteractionGuards, UiCursorPos,
        ViewportLayerCounter,
    },
};

use crate::viewport_host::{ViewportHost, ViewportMode, ViewportModeIntent};

/// Furthest out the user can zoom, in stage logical pixels per authored
/// pixel.
pub const MIN_ZOOM: f32 = 0.05;
/// Furthest in the user can zoom, in stage logical pixels per authored
/// pixel.
pub const MAX_ZOOM: f32 = 32.0;

/// Zoom factor applied per wheel tick.
const ZOOM_PER_TICK: f32 = 1.1;

/// A pixel wheel event is a fraction of a line tick, matching how the
/// rest of the editor normalises scroll units (see `input_contexts`).
const PIXEL_TICKS: f32 = 0.01;

/// Where the 2D camera sits on Z. The default 2D orthographic projection
/// spans `near = -1000`, so everything the scene draws at Z 0 is in view.
const CAMERA_Z: f32 = 999.0;

/// Draw order of the parking camera. Behind every panel camera, and it
/// never draws anyway (`is_active: false`); distinct so a camera-order
/// collision warning cannot point here.
const PARKING_CAMERA_ORDER: isize = -2;

/// Gap, in stage logical pixels, left between a fitted canvas and each
/// edge of the area it is fitted into.
const FIT_MARGIN: f32 = tokens::SPACING_LG;

/// Marker for a 2D viewport camera. One per viewport panel, so queries
/// that need every 2D camera iterate rather than using `Single<>`.
#[derive(Component)]
pub struct Viewport2dCamera;

/// Marker on the UI node a 2D viewport displays its camera's image in.
/// This node *is* the authored canvas: `place_stage` sizes it to the
/// reference resolution times the view's zoom, so measuring it is how
/// everything downstream recovers the view.
#[derive(Component)]
pub struct Scene2dViewport;

/// Marker on the node a [`Scene2dViewport`] is placed inside: the
/// panel's fixed window onto the canvas, which clips whatever the pan
/// and zoom push outside it.
#[derive(Component)]
pub struct Scene2dStageArea;

/// Marker for the editor's single parking camera. An authored UI scene
/// root points here whenever no 2D viewport panel is open, so it is never
/// handed back to the editor's own window camera. Spawned on demand by
/// `park_ui_scene_roots`; one per session, never per panel.
#[derive(Component)]
pub struct UiSceneParkingCamera;

/// What a 2D viewport panel does with pointer input.
///
/// `Edit` is the authoring mode: clicks select and manipulate the scene
/// being edited, and the live widgets never hear about the pointer.
/// `Interact` hands input to the scene itself so the user can try the UI
/// they are building, and the authoring chrome stands down.
///
/// It sits on [`Viewport2dPanelHost`], one per panel, so two viewports
/// can show the same scene with one being authored and the other tried
/// out. Readers reach it through the host the stage belongs to, never
/// through a resource.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Viewport2dMode {
    #[default]
    Edit,
    Interact,
}

/// How a 2D viewport is framed over the scene it edits.
///
/// The panel's navigation state: `place_stage` derives the stage node's
/// size and position from it, and it travels with a scene tab across a
/// swap.
///
/// **The view moves the stage, not the camera.** Bevy renders UI through
/// a view of its own (`bevy_ui_render::extract_ui_camera_view` builds an
/// orthographic projection straight from the target's viewport rect and
/// parks the view transform at the origin), so a routed UI scene is
/// pinned to its render target and no camera transform can pan or zoom
/// it. The panel's image stays at the authored reference size while the
/// stage node grows and slides around it, so
/// [`target_pixels_per_stage_pixel`] reads the zoom back off the laid-out
/// node and nothing downstream needs a zoom term.
///
/// Two cursor spaces meet here:
///
/// - the view is *driven* in the stage **area**'s logical pixels
///   ([`cursor_area_offset`]). The area is the fixed window the scene
///   moves behind, so an offset measured against it does not change when
///   the view does, which is what makes [`zoom_toward`]'s anchor hold.
///   Raw `MouseMotion` deltas are stage physical pixels and take
///   `ComputedNode::inverse_scale_factor()` alone to get there.
/// - a click is *resolved* in authored pixels
///   ([`cursor_stage_offset`] against the stage **node**, then
///   `ui_stage::stage_to_authored`). The stage node is the canvas and
///   already carries the view, so that path composes the two standing
///   conversions (ui-logical to stage physical, then stage physical to
///   render-target) and needs nothing from [`Ui2dView`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ui2dView {
    /// Authored point shown at the centre of the stage area, in authored
    /// pixels from the centre of the canvas, y up.
    pub pan: Vec2,
    /// Stage logical pixels per authored pixel. `1.0` draws the canvas
    /// at its authored size, whatever the display's scale factor.
    pub zoom: f32,
    /// Lattice a manipulated node lands on, in **authored pixels**.
    ///
    /// Separate from the 3D grid, whose lattice is measured in world
    /// units and cannot express whole authored pixels. It rides on the
    /// view because that is the struct a scene tab captures and restores
    /// (`ViewState::ui_view`).
    pub grid: f32,
}

/// Default [`Ui2dView::grid`]: eight authored pixels, the step UI
/// layouts are usually spaced on, and coarse enough that the magnet is
/// visible the moment it is switched on.
pub const DEFAULT_UI_GRID: f32 = 8.0;

/// Finest [`Ui2dView::grid`] the stepper reaches: a whole authored
/// pixel, which is as fine as a canvas measured in pixels goes.
pub const MIN_UI_GRID: f32 = 1.0;

/// Coarsest [`Ui2dView::grid`] the stepper reaches.
pub const MAX_UI_GRID: f32 = 64.0;

/// `grid` stepped `steps` powers of two, clamped to the ladder's ends.
///
/// The step goes through the power rather than multiplying the value, so
/// a grid set off the ladder (a scripted `viewport2d.grid` of 10) comes
/// back onto it on the first press instead of walking the ladder in tens.
/// Same rule as the 3D grid stepper (`crate::grid_ops`).
pub fn stepped_ui_grid(grid: f32, steps: i32) -> f32 {
    let current = if grid.is_finite() && grid > 0.0 {
        grid
    } else {
        DEFAULT_UI_GRID
    };
    let power = current.log2().round() + steps as f32;
    power.exp2().clamp(MIN_UI_GRID, MAX_UI_GRID)
}

impl Default for Ui2dView {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
            grid: DEFAULT_UI_GRID,
        }
    }
}

/// Authored point under a cursor sitting `area_offset` logical pixels
/// from the centre of the stage area (see [`cursor_area_offset`]), in
/// authored pixels from the centre of the canvas.
///
/// Screen pixels run y-down (UI convention); the view's pan runs y-up,
/// hence the flip.
pub fn world_at(view: Ui2dView, area_offset: Vec2) -> Vec2 {
    view.pan + Vec2::new(area_offset.x, -area_offset.y) / view.zoom
}

/// Zoom by `ticks` wheel steps about the point under the cursor.
///
/// The pan is re-solved so [`world_at`] returns the same authored point
/// before and after: the scene grows and shrinks around the cursor
/// rather than around the centre of the area.
pub fn zoom_toward(view: Ui2dView, area_offset: Vec2, ticks: f32) -> Ui2dView {
    let zoom = (view.zoom * ZOOM_PER_TICK.powf(ticks)).clamp(MIN_ZOOM, MAX_ZOOM);
    let anchor = world_at(view, area_offset);
    Ui2dView {
        pan: anchor - Vec2::new(area_offset.x, -area_offset.y) / zoom,
        zoom,
        ..view
    }
}

/// The view that shows the whole of a `reference`-sized canvas inside an
/// `area`-sized window, centred, with `FIT_MARGIN` to spare on each
/// edge.
///
/// The binding axis is whichever runs out first, so the canvas keeps its
/// authored aspect.
///
/// The usable box is floored at one pixel and the zoom clamped to the
/// same range every other zoom path uses: this runs on the frame a panel
/// is first laid out, and a zoom of zero would divide through every
/// mapping downstream. `view.grid` rides through untouched.
pub fn fit_view(view: Ui2dView, reference: UVec2, area: Vec2) -> Ui2dView {
    let canvas = reference.as_vec2().max(Vec2::ONE);
    let usable = (area - Vec2::splat(FIT_MARGIN * 2.0)).max(Vec2::ONE);
    Ui2dView {
        pan: Vec2::ZERO,
        zoom: (usable / canvas).min_element().clamp(MIN_ZOOM, MAX_ZOOM),
        ..view
    }
}

/// Ask every 2D viewport panel to frame the UI scene it is showing.
///
/// The fit is requested rather than computed here because both numbers it
/// needs (the scene's reference size and the panel area's laid-out size)
/// are only settled after layout. [`size_targets_to_reference`] reads
/// both and honours the request there, so a scene that has not spawned
/// yet frames itself when it arrives.
pub fn request_2d_fit(world: &mut World) {
    let mut hosts = world.query::<&mut Viewport2dPanelHost>();
    for mut host in hosts.iter_mut(world) {
        if !host.fit_pending {
            host.fit_pending = true;
        }
    }
}

/// Pan by a cursor drag of `area_delta` logical pixels, dragging the
/// scene along with the cursor.
pub fn pan_by(view: Ui2dView, area_delta: Vec2) -> Ui2dView {
    Ui2dView {
        pan: view.pan + Vec2::new(-area_delta.x, area_delta.y) / view.zoom,
        ..view
    }
}

/// Render-target pixels per stage physical pixel: authored pixels per
/// stage pixel, which is the reciprocal of the view's zoom.
///
/// The stage node displays the panel's image across its whole area, so
/// this factor turns a stage measurement into authored pixels. It is
/// derived from the laid-out node rather than read off [`Ui2dView`]:
/// `place_stage` puts the zoom into the node, so no second copy of the
/// zoom can drift. Both axes agree, because the stage
/// carries the reference aspect exactly; the smaller is taken so the
/// tick after a resize, when they briefly disagree, under-reads rather
/// than over-reads.
///
/// A degenerate stage returns `1.0` rather than an infinity: this runs
/// on every cursor move, including the frame a panel is first laid out.
pub fn target_pixels_per_stage_pixel(stage_size: Vec2, target_size: UVec2) -> f32 {
    if stage_size.x <= 0.0 || stage_size.y <= 0.0 {
        return 1.0;
    }
    (target_size.as_vec2() / stage_size).min_element()
}

/// Offset of `cursor` from the centre of a stage node, in the panel's
/// render-target pixels, or `None` when the cursor is outside it.
///
/// `cursor` is ui-logical, the space `UiCursorPos` reports in.
/// `centre` and `size` are the node's physical values, straight off
/// `UiGlobalTransform::translation` and `ComputedNode::size()`,
/// `inverse_scale_factor` is `ComputedNode::inverse_scale_factor()`, and
/// `target_scale` is [`target_pixels_per_stage_pixel`] for this panel.
///
/// The cursor is lifted into physical space rather than the node pushed
/// into logical space: an image render target has a scale factor of 1, so
/// the authored tree measures in the image's own pixels, and converting
/// the node instead would land the hit test a factor of `scale_factor`
/// off on any high-DPI display or after `view.ui_zoom_in`. The bounds
/// test itself stays in stage pixels, being a question about the node
/// rather than about the image.
pub fn cursor_stage_offset(
    cursor: Vec2,
    centre: Vec2,
    size: Vec2,
    inverse_scale_factor: f32,
    target_scale: f32,
) -> Option<Vec2> {
    let (offset, inside) =
        stage_offset_clamped(cursor, centre, size, inverse_scale_factor, target_scale);
    inside.then_some(offset)
}

/// [`cursor_stage_offset`]'s mapping without its refusal: the offset
/// clamped to the stage's edge, and whether the cursor was really on it.
///
/// A gesture the scene is already holding has to be followed off the
/// canvas (see `forward_pointer_into_stage`), so the edge stands in for
/// wherever the cursor went.
pub fn stage_offset_clamped(
    cursor: Vec2,
    centre: Vec2,
    size: Vec2,
    inverse_scale_factor: f32,
    target_scale: f32,
) -> (Vec2, bool) {
    let offset = cursor / inverse_scale_factor - centre;
    let half = size / 2.0;
    let inside = offset.x.abs() <= half.x && offset.y.abs() <= half.y;
    (offset.clamp(-half, half) * target_scale, inside)
}

/// [`cursor_stage_offset`]'s mapping with neither its refusal nor its
/// clamp: where the cursor is over the canvas, whether or not it is on
/// it.
///
/// What a gesture placing a line on the canvas reads. Those run off the
/// edge by design, a guide dragged back onto its ruler to drop it being
/// the whole point of the gesture, and the clamp would answer with the
/// edge for every position past it.
pub fn stage_offset_unbounded(
    cursor: Vec2,
    centre: Vec2,
    inverse_scale_factor: f32,
    target_scale: f32,
) -> Vec2 {
    (cursor / inverse_scale_factor - centre) * target_scale
}

/// Offset of `cursor` from the centre of a panel's stage **area**, in
/// that area's own logical pixels, or `None` when the cursor is outside
/// it.
///
/// The space [`Ui2dView`] is driven in. The area is the fixed window the
/// scene moves behind, so an offset measured against it does not change
/// when the view does, which is what makes [`zoom_toward`]'s anchor hold.
/// The stage node grows with the zoom and slides with the pan, so a zoom
/// step measured against it would move its own anchor.
pub fn cursor_area_offset(
    cursor: Vec2,
    centre: Vec2,
    size: Vec2,
    inverse_scale_factor: f32,
) -> Option<Vec2> {
    let offset = cursor - centre * inverse_scale_factor;
    let half = size * inverse_scale_factor / 2.0;
    (offset.x.abs() <= half.x && offset.y.abs() <= half.y).then_some(offset)
}

/// Sits on the dock-leaf content entity that hosts a 2D viewport panel.
/// Holds the camera the panel draws with, the stage node that displays
/// it, the area that clips and frames that stage, and how the panel is
/// currently framed over the scene, so teardown can clean the camera up
/// and callers can find the stage without walking the panel's children.
#[derive(Component)]
pub struct Viewport2dPanelHost {
    pub camera: Entity,
    pub stage: Entity,
    /// The clipping area the stage is placed inside: the panel's fixed
    /// window onto a canvas that pans and zooms behind it.
    pub area: Entity,
    /// The software pointer `forward_pointer_into_stage` drives across
    /// this panel's render target while the mode is
    /// [`Viewport2dMode::Interact`].
    pub pointer: Entity,
    pub mode: Viewport2dMode,
    /// How the panel is framed. Move it through [`Self::set_view`]
    /// rather than by assignment, so [`Self::view_touched`] stays in
    /// step.
    pub view: Ui2dView,
    /// Whether [`Self::view`] is a framing something chose, rather than
    /// the default it was built with.
    ///
    /// A tab swap captures the panel's framing into `ViewState::ui_view`;
    /// capturing an untouched panel would stamp the default onto the tab
    /// as a chosen framing, after which the tab reads as framed forever
    /// and a fit can never reach it. Only a real move sets it: pan, zoom,
    /// an applied fit, or a framing restored from a tab that had one.
    pub view_touched: bool,
    /// Set when something has asked this panel to frame its scene (a UI
    /// scene was opened, or the user pressed Fit), cleared by
    /// [`size_targets_to_reference`] on the first frame it can compute
    /// the fit. See [`request_2d_fit`].
    ///
    /// A new panel starts with one pending, so it frames the first UI
    /// scene it is given whether or not anything asked: a restored
    /// session opens its tabs before the dock has built a leaf, so the
    /// panel can arrive after the scene did. A tab that was framed
    /// withdraws this on restore ([`crate::scenes::swap`]'s
    /// `apply_view_state`), so the default never outranks a framing the
    /// user chose.
    pub fit_pending: bool,
    /// Size of the camera's render-target image, in its own pixels.
    /// Written by [`size_targets_to_reference`]; the reference size of
    /// the active UI scene, or the stage's own size when no UI scene is
    /// open. Cached here so the pan/zoom pass converts stage pixels
    /// without touching `Assets<Image>`.
    pub target_size: UVec2,
}

impl Viewport2dPanelHost {
    /// Frame the panel, recording that its framing is chosen rather than
    /// default. Every writer of the view goes through here, so the two
    /// stay in step.
    pub fn set_view(&mut self, view: Ui2dView) {
        self.view = view;
        self.view_touched = true;
    }

    /// Put the panel back on the default framing, unchosen: what an
    /// incoming tab with no framing of its own leaves behind it.
    pub fn reset_view(&mut self) {
        self.view = Ui2dView::default();
        self.view_touched = false;
    }
}

pub struct Viewport2dPlugin;

impl Plugin for Viewport2dPlugin {
    fn build(&self, app: &mut App) {
        // A panel is built from the dock reconciler, which can run before
        // `crate::viewport`'s plugin has been added at all: the counter is
        // shared, so whichever plugin lands first installs it.
        app.init_resource::<ViewportLayerCounter>()
            .init_resource::<Ui2dPanActive>()
            .add_observer(on_viewport_2d_panel_despawn)
            // Where `bevy_ui` runs its own `viewport_picking`: after the
            // input pass has written this frame's `PointerInput`, and
            // early enough that the forwarded copy is a real input to
            // every picking system downstream of it.
            .add_systems(
                First,
                forward_pointer_into_stage.in_set(PickingSystems::PostInput),
            )
            .add_systems(
                Update,
                (
                    update_viewport_2d_mode_bar,
                    update_viewport_2d_title,
                    update_viewport_2d_zoom_readout,
                    update_viewport_2d_grid_readout,
                ),
            )
            .add_systems(
                Update,
                (viewport_2d_pan_zoom, apply_2d_view)
                    .chain()
                    .in_set(crate::EditorInteractionSystems),
            )
            .add_systems(
                PostUpdate,
                (
                    // Routing must land before layout reads a root's
                    // target camera, or the scene spends a frame laid
                    // out against whatever the default UI camera is.
                    route_ui_roots_to_cameras.before(UiSystems::Prepare),
                    // After layout, so the stage is placed from the
                    // area's measured size and the image is held at the
                    // authored reference size the panel exists to show.
                    size_targets_to_reference.after(UiSystems::PostLayout),
                    // After the placement, so a ruler's marks are
                    // measured against the area the canvas was just laid
                    // into.
                    sync_rulers.after(size_targets_to_reference),
                ),
            );
    }
}

/// Tracks the panel a middle-drag pan started on. While one is running
/// that panel keeps the gesture wherever the pointer goes, the way
/// [`crate::viewport::CameraFlyActive`] keeps a fly session with the
/// viewport it started in.
///
/// Without the latch the pan would be resolved by hover on every frame,
/// so dragging past the panel's edge would drop the gesture mid-motion,
/// or carry it into a second 2D viewport. Panning past the edges is the
/// ordinary way to reach a corner of a canvas larger than its window.
#[derive(Resource, Default)]
pub struct Ui2dPanActive(pub Option<Entity>);

/// Scroll to zoom, middle-drag to pan, for whichever 2D viewport the
/// cursor is over, while that viewport is in [`Viewport2dMode::Edit`].
///
/// Hover is resolved against the panel's stage **area**, not its stage
/// node: under a zoom the stage runs past the area and is clipped by it,
/// so the area is what the cursor is over. The 3D viewport's
/// `ActiveViewport` is left untouched, so a 2D panel cannot steal
/// fly-camera routing from a scene viewport.
///
/// The pan follows [`Ui2dPanActive`] and the zoom follows the cursor: a
/// drag belongs to the panel it started on until the button comes up,
/// while a wheel tick belongs to whatever is under the cursor.
///
/// In [`Viewport2dMode::Interact`] the wheel and the middle drag belong
/// to the scene instead, and [`forward_pointer_into_stage`] hands both
/// over.
///
/// Run it on demand through [`run_2d_pan_zoom`].
fn viewport_2d_pan_zoom(
    cursor: UiCursorPos,
    guards: InteractionGuards,
    mouse: Res<ButtonInput<MouseButton>>,
    hover_map: Res<HoverMap>,
    parents: Query<&ChildOf>,
    mut motion: MessageReader<MouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
    areas: Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dStageArea>>,
    mut hosts: Query<(Entity, &ViewportHost, &mut Viewport2dPanelHost)>,
    mut panning: ResMut<Ui2dPanActive>,
) {
    // Both streams are read whatever becomes of them below, so a reader
    // cannot back up while the cursor is elsewhere or while the scene
    // under it owns these gestures.
    let travelled: Vec2 = motion.read().map(|ev| ev.delta).sum();
    let ticks: f32 = wheel
        .read()
        .map(|ev| match ev.unit {
            MouseScrollUnit::Line => ev.y,
            MouseScrollUnit::Pixel => ev.y * PIXEL_TICKS,
        })
        .sum();

    // The pan ends when the button is up, rather than on the release
    // message alone: a window that lost focus mid-drag never delivers
    // one, and a latch left set would pan on the next stray motion.
    if !mouse.pressed(MouseButton::Middle) {
        panning.0 = None;
    }

    let position = if guards.is_any_interaction_active() {
        None
    } else {
        cursor.get()
    };
    let hovered =
        position.and_then(|position| hovered_area(position, &areas, &hosts, &hover_map, &parents));

    if mouse.just_pressed(MouseButton::Middle) {
        panning.0 = hovered.map(|(entity, _)| entity);
    }

    if travelled != Vec2::ZERO
        && let Some(entity) = panning.0
        && let Ok((_, _, mut host)) = hosts.get_mut(entity)
    {
        // `MouseMotion` deltas are stage physical pixels, one conversion
        // short of the logical pixels `pan_by` is stated in.
        let inverse_scale_factor = areas
            .get(host.area)
            .map_or(1.0, |(computed, _)| computed.inverse_scale_factor());
        let view = pan_by(host.view, travelled * inverse_scale_factor);
        if view != host.view {
            host.set_view(view);
        }
    }

    if ticks != 0.0
        && let Some((entity, offset)) = hovered
        && let Ok((_, _, mut host)) = hosts.get_mut(entity)
    {
        let view = zoom_toward(host.view, offset, ticks);
        if view != host.view {
            host.set_view(view);
        }
    }
}

/// Run the panel navigation pass once, outside the schedule.
///
/// For tests: the plugin runs `viewport_2d_pan_zoom` inside
/// [`crate::EditorInteractionSystems`], which never runs while a test app
/// sits in `AppState::ProjectSelect`. The system itself cannot be made
/// public to be called directly, because two of its parameters are
/// `crate::viewport`'s own.
pub fn run_2d_pan_zoom(world: &mut World) {
    if let Err(error) = world.run_system_cached(viewport_2d_pan_zoom) {
        warn!("2D viewport navigation pass could not run: {error}");
    }
}

/// The panel whose stage area is under `cursor` (ui-logical pixels), and
/// the cursor's offset from that area's centre in the area's logical
/// pixels.
///
/// The [`HoverMap`] and a rect test both have to agree. The hover map is
/// the editor's own hit test, so it accounts for a popup or docked panel
/// sitting over the stage; the rect test turns that hover into the offset
/// [`zoom_toward`] anchors on.
///
/// Panels are walked rather than areas so each hit test uses its own
/// panel's scale factor. First hit wins, and neither a panel showing its 3D
/// world nor one not in [`Viewport2dMode::Edit`] is a hit at all.
fn hovered_area(
    cursor: Vec2,
    areas: &Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dStageArea>>,
    hosts: &Query<(Entity, &ViewportHost, &mut Viewport2dPanelHost)>,
    hover_map: &HoverMap,
    parents: &Query<&ChildOf>,
) -> Option<(Entity, Vec2)> {
    for (entity, panel, host) in hosts.iter() {
        if panel.mode != ViewportMode::TwoD || host.mode != Viewport2dMode::Edit {
            continue;
        }
        let Ok((computed, transform)) = areas.get(host.area) else {
            continue;
        };
        if !entity_is_hovered(host.area, hover_map, parents) {
            continue;
        }
        if let Some(offset) = cursor_area_offset(
            cursor,
            transform.translation,
            computed.size(),
            computed.inverse_scale_factor(),
        ) {
            return Some((entity, offset));
        }
    }
    None
}

/// Push each panel's [`Ui2dView`] onto its camera. The view is the
/// authority; the camera transform and ortho scale are derived, so
/// nothing else should write them.
///
/// This does **not** move the authored UI: Bevy renders UI through a view
/// of its own, so a routed scene ignores this camera (see [`Ui2dView`],
/// and `place_stage` for what does move it). It applies to 2D *world*
/// content drawn into this panel (sprites, gizmos, guides), which does
/// follow the camera, so that content and the authored canvas stay framed
/// alike.
///
/// Public so tests can run it directly: the plugin schedules it inside
/// [`crate::EditorInteractionSystems`], which never runs while a test app
/// sits in `AppState::ProjectSelect`.
pub fn apply_2d_view(
    hosts: Query<&Viewport2dPanelHost>,
    mut cameras: Query<(&mut Transform, &mut Projection), With<Viewport2dCamera>>,
) {
    for host in &hosts {
        let Ok((mut transform, mut projection)) = cameras.get_mut(host.camera) else {
            continue;
        };

        let target = host.view.pan.extend(CAMERA_Z);
        if transform.translation != target {
            transform.translation = target;
        }

        let scale = 1.0 / host.view.zoom;
        if matches!(&*projection, Projection::Orthographic(ortho) if ortho.scale != scale)
            && let Projection::Orthographic(ortho) = &mut *projection
        {
            ortho.scale = scale;
        }
    }
}

/// Drive each `Interact` panel's own pointer across its render target,
/// so the authored widgets showing there can be used.
///
/// Bevy's `viewport_picking` does not apply here: the stage is an
/// [`ImageNode`] rather than a [`bevy::ui::widget::ViewportNode`] (see
/// [`build_viewport_2d_panel`]), and the stage's frame child would keep
/// the stage out of the `HoverMap` that system reads. The mechanism
/// underneath it is enough:
///
/// - a pointer is an entity with a [`PointerId`] and a
///   [`PointerLocation`]. Each panel spawns one with a `Custom` id, the
///   variant Bevy reserves for software-driven pointers.
/// - `bevy_ui`'s picking backend hit-tests every pointer against every
///   camera whose render target matches the pointer's *location target*.
///   Pointing this one at the panel's image picks the authored tree: the
///   authored roots are routed to that camera, and an image target has a
///   scale factor of 1, so the position handed over is in the authored
///   pixels the tree laid out in.
/// - a written [`PointerInput`] turns that hover into events:
///   `PointerInput::receive` moves the pointer, and `pointer_events`
///   replays the action onto whatever the hover map found.
///
/// # What may start, and what may not stop
///
/// Two conditions decide whether the pointer may *enter* the scene: the
/// panel has to be hovered according to the editor's own hit test in the
/// [`HoverMap`], which honours whatever popup or dialog is over the
/// stage; and no editor interaction may be in flight, the same
/// [`InteractionGuards`] the pan/zoom pass checks. Without both, a rect
/// test alone would send clicks straight through an open modal into the
/// live scene.
///
/// The [`HoverMap`] read here is one frame old, because it is written in
/// `PreUpdate` and this runs in `First`, the same lag Bevy's
/// `viewport_picking` works under. Entering the stage therefore takes one
/// pointer input to warm up.
///
/// A press already under way is the exception: once the scene's pointer
/// is down, its stream runs to the release wherever the cursor goes, with
/// positions clamped to the stage's edge. Dropping it at the boundary
/// would leave the widget latched down forever, since `PointerPress`
/// never clears, `pointer_events` keeps the entity in
/// `pressing`/`dragging`, no `Click` ever resolves, and the next entry
/// onto the stage opens mid-drag. Bevy's `viewport_picking` does the same
/// when it re-adds entities found in `PointerState::dragging`.
///
/// # Lifting
///
/// With no press live, the pointer is lifted (location cleared) as soon
/// as the cursor leaves the stage or either condition above fails.
/// Lifting unwinds the aggregate state, so the next `generate_hovermap`
/// finds nothing and `PickingInteraction` falls back to `None`, but it is
/// not a pointer *movement*, so an authored `On<Pointer<Out>>` observer
/// does not fire.
fn forward_pointer_into_stage(
    mut commands: Commands,
    ui_scale: Res<UiScale>,
    guards: InteractionGuards,
    hover_map: Res<HoverMap>,
    parents: Query<&ChildOf>,
    hosts: Query<&Viewport2dPanelHost>,
    stages: Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dViewport>>,
    targets: Query<&RenderTarget, With<Viewport2dCamera>>,
    mut pointers: Query<(&PointerId, &mut PointerLocation, &PointerPress)>,
    mut inputs: MessageReader<PointerInput>,
) {
    // Only the editor's real pointers, which are the ones on a window.
    // Filtering by target rather than by id is also what keeps a
    // forwarded input from being forwarded again.
    let real: Vec<PointerInput> = inputs
        .read()
        .filter(|input| matches!(input.location.target, NormalizedRenderTarget::Window(_)))
        .cloned()
        .collect();
    let blocked = guards.is_any_interaction_active();

    for host in &hosts {
        let Ok((&pointer_id, mut location, press)) = pointers.get_mut(host.pointer) else {
            continue;
        };
        let holding = press.is_any_pressed();
        let may_start = !blocked
            && host.mode == Viewport2dMode::Interact
            && entity_is_hovered(host.stage, &hover_map, &parents);
        if !holding && !may_start {
            location.location = None;
            continue;
        }

        let stage = stages.get(host.stage);
        // No primary window to resolve against: a panel camera always
        // targets an image, which normalises on its own.
        let target = targets
            .get(host.camera)
            .ok()
            .and_then(|target| target.normalize(None));
        let (Ok((computed, transform)), Some(target)) = (stage, target) else {
            location.location = None;
            continue;
        };

        let target_scale = target_pixels_per_stage_pixel(computed.size(), host.target_size);
        for input in &real {
            let (offset, inside) = stage_offset_clamped(
                input.location.position / ui_scale.0,
                transform.translation,
                computed.size(),
                computed.inverse_scale_factor(),
                target_scale,
            );
            if !inside && !holding {
                location.location = None;
                continue;
            }
            let forwarded = Location {
                position: stage_to_authored(offset, host.target_size),
                target: target.clone(),
            };
            location.location = Some(forwarded.clone());
            commands.write_message(PointerInput {
                pointer_id,
                location: forwarded,
                action: input.action,
            });
        }
    }
}

/// Whether one of the editor's real pointers is over `entity` or
/// anything inside it.
///
/// The stage's own frame child covers it, so what lands in the
/// [`HoverMap`] is usually the frame rather than the stage; walking up
/// the ancestors makes either answer the same question. Custom pointers
/// are skipped: one caller decides this *for* a custom pointer hovering
/// the authored tree, and reading it back would keep the panel hovered by
/// its own reflection.
fn entity_is_hovered(entity: Entity, hover_map: &HoverMap, parents: &Query<&ChildOf>) -> bool {
    hover_map
        .iter()
        .filter(|(pointer, _)| !pointer.is_custom())
        .flat_map(|(_, hits)| hits.keys())
        .any(|hovered| {
            core::iter::successors(Some(*hovered), |entity| {
                parents.get(*entity).ok().map(ChildOf::parent)
            })
            .any(|ancestor| ancestor == entity)
        })
}

/// Point every UI scene root at the camera whose view it belongs in: an
/// authored root at a 2D viewport's camera, so its layout resolves against
/// that panel's render target rather than the editor's own window, and an
/// imported one at the 3D viewport's, where a world scene's UI shows as a
/// screen-space overlay.
///
/// The runtime leaves a `UiSceneRoot` unparented, because Bevy only lays
/// out a `Node` tree that starts at an ECS root
/// (`jackdaw_runtime::spawn`), and the editor keeps it that way: routing
/// is an inserted [`UiTargetCamera`], never a reparent. That component
/// names a camera entity this session spawned, so it is on the `scene_io`
/// skip list and never reaches disk.
///
/// The two groups are told apart here rather than in each caller, because
/// a world scene's imported overlay is `UiSceneRoot` without `ChildOf`
/// exactly like the scene a 2D panel edits (see [`AuthoredUiSceneRoot`]),
/// and routing an import to the panel would draw a world scene's HUD on
/// the canvas being edited.
///
/// A root is *always* routed somewhere, even with no view to route it to:
/// removing the component would hand the root back to `DefaultUiCamera`,
/// the editor's own window camera, and the scene would draw itself over
/// the editor chrome. A group with no view parks on
/// [`UiSceneParkingCamera`] instead (see `park_ui_scene_roots`). Both
/// groups are routed in one system so a session needing to park cannot
/// spawn two parking cameras in a single frame's command queue.
///
/// With several panels open one answers for the canvas
/// ([`crate::viewport_host::primary_2d_host`]) and one for the world
/// ([`crate::viewport_host::primary_3d_host`]), matching how `ViewState`
/// captures a single `ui_view`.
pub fn route_ui_roots_to_cameras(
    mut commands: Commands,
    panels: Query<(Entity, &ViewportHost)>,
    hosts: Query<(&Viewport2dPanelHost, &crate::viewport::ViewportPanelHost)>,
    parked: Query<Entity, With<UiSceneParkingCamera>>,
    world_view: Query<Entity, With<crate::viewport::MainViewportCamera>>,
    authored: Query<(Entity, Option<&UiTargetCamera>), AuthoredUiSceneRoot>,
    imported: Query<(Entity, Option<&UiTargetCamera>), ImportedUiSceneRoot>,
    mut images: ResMut<Assets<Image>>,
) {
    if authored.is_empty() && imported.is_empty() {
        // Nothing to route, and so nothing to park either.
        return;
    }

    let panel = crate::viewport_host::primary_2d_host(panels.iter())
        .and_then(|entity| hosts.get(entity).ok())
        .map(|(stage, _)| stage.camera);
    // A panel showing the world, because that is where an imported overlay is
    // drawn. The canvas panel's world camera is off while its mode holds, so
    // routing there would hide the overlay from the panel still showing it.
    let world = crate::viewport_host::primary_3d_host(panels.iter())
        .and_then(|entity| hosts.get(entity).ok())
        .map(|(_, world)| world.camera)
        .or_else(|| world_view.iter().next());
    let parking = ((!authored.is_empty() && panel.is_none())
        || (!imported.is_empty() && world.is_none()))
    .then(|| {
        parked
            .iter()
            .next()
            .unwrap_or_else(|| park_ui_scene_roots(&mut commands, &mut images))
    });

    if let Some(camera) = panel.or(parking) {
        aim_ui_roots(&mut commands, authored.iter(), camera);
    }
    if let Some(camera) = world.or(parking) {
        aim_ui_roots(&mut commands, imported.iter(), camera);
    }
}

/// Insert `camera` as each root's [`UiTargetCamera`], skipping roots that
/// already point at it so the change detection other systems watch stays
/// quiet on an idle frame.
fn aim_ui_roots<'a>(
    commands: &mut Commands,
    roots: impl Iterator<Item = (Entity, Option<&'a UiTargetCamera>)>,
    camera: Entity,
) {
    for (entity, routed) in roots {
        if routed.map(UiTargetCamera::entity) != Some(camera) {
            commands.entity(entity).insert(UiTargetCamera(camera));
        }
    }
}

/// Spawn the parking camera: where an authored UI scene root points while
/// no 2D viewport panel is open.
///
/// It is inactive, so it draws nothing, and it targets a 1x1 image, so the
/// layout viewport a parked root resolves against collapses to a single
/// pixel. `bevy_camera`'s `camera_system` computes target info for
/// inactive cameras too, so a parked root still resolves against that
/// pixel. Targeting an image also keeps this camera out of
/// `DefaultUiCamera`, which only ever picks window cameras.
///
/// Spawned lazily, on the first frame an unrouted root exists, so a
/// session that never opens a UI scene never pays for it.
fn park_ui_scene_roots(commands: &mut Commands, images: &mut Assets<Image>) -> Entity {
    let mut image = Image::new_fill(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Bgra8UnormSrgb,
        default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    let handle = images.add(image);

    commands
        .spawn((
            UiSceneParkingCamera,
            crate::EditorEntity,
            Camera2d,
            Camera {
                is_active: false,
                order: PARKING_CAMERA_ORDER,
                ..default()
            },
            RenderTarget::Image(handle.into()),
            Msaa::Off,
        ))
        .id()
}

/// Hold each panel's render-target image at the active UI scene's
/// reference size, and place the stage over it for the current view.
///
/// The reference-resolution contract. Bevy derives a UI root's layout
/// viewport from its target camera's physical viewport size
/// (`bevy_ui::update::propagate_ui_target_cameras`), and for an image
/// target that is the image's own size, so sizing the image to
/// `reference_size` makes authored layout compute at the resolution it
/// was designed for: a 50%-wide child of a 1280-wide reference is 640px
/// whatever the dock leaf measures. The stage displays that image at
/// whatever size `place_stage` gives it, so the stage-to-authored mapping
/// is a single scale factor ([`target_pixels_per_stage_pixel`]) that
/// already carries the zoom.
///
/// With no UI scene open there is nothing to hold the image at, so the
/// stage fills its area and the panel is a plain 1:1 viewport.
///
/// A pending fit ([`request_2d_fit`]) lands here too, this being the one
/// place that holds both the scene's reference size and the area's
/// laid-out size.
pub fn size_targets_to_reference(
    roots: Query<&UiSceneRoot, AuthoredUiSceneRoot>,
    mut hosts: Query<&mut Viewport2dPanelHost>,
    targets: Query<&RenderTarget, With<Viewport2dCamera>>,
    mut stages: Query<(&mut Node, &ComputedNode), With<Scene2dViewport>>,
    areas: Query<&ComputedNode, With<Scene2dStageArea>>,
    mut images: ResMut<Assets<Image>>,
) {
    let reference = roots
        .iter()
        .next()
        .map(|root| root.reference_size.max(UVec2::ONE));

    for mut host in &mut hosts {
        let Ok((mut node, computed)) = stages.get_mut(host.stage) else {
            continue;
        };

        // The area's logical size, because a `Node` is written in logical
        // pixels and so is the view's zoom.
        let area = areas
            .get(host.area)
            .map(|area| area.size() * area.inverse_scale_factor())
            .unwrap_or(Vec2::ZERO);

        // The first point in the frame where both numbers a fit needs are
        // settled. A request outlives a frame with nothing to fit,
        // because opening a scene asks before the scene has spawned.
        if host.fit_pending
            && let Some(reference) = reference
            && area.x > 0.0
            && area.y > 0.0
        {
            let fitted = fit_view(host.view, reference, area);
            host.set_view(fitted);
            host.fit_pending = false;
        }
        let view = host.view;

        let target_size = match reference {
            Some(reference) => {
                if let Ok(RenderTarget::Image(target)) = targets.get(host.camera)
                    && let Some(mut image) = images.get_mut(&target.handle)
                    && image.size() != reference
                {
                    image.resize(reference.to_extents());
                }
                reference
            }
            None => computed.size().as_uvec2().max(UVec2::ONE),
        };
        if host.target_size != target_size {
            host.target_size = target_size;
        }

        place_stage(&mut node, reference, view, area);
    }
}

/// Place a panel's stage inside its area for the current view: the
/// canvas at `reference_size * zoom` logical pixels, positioned so the
/// authored point the view is panned to lands at the centre of the area.
///
/// Pan and zoom are this placement; the camera cannot do it (see
/// [`Ui2dView`]). Placing here rather than with a `UiTransform` keeps the
/// zoom inside `ComputedNode::size()`, where
/// [`target_pixels_per_stage_pixel`] and the cursor mapping and selection
/// overlay downstream of it pick it up without naming it. The area clips
/// whatever falls outside, so zooming in grows
/// the canvas past its window instead of scaling anything the cursor math
/// has to know about.
///
/// With no UI scene open there is no canvas to place, so the stage fills
/// its area and the panel is a plain 1:1 viewport.
fn place_stage(node: &mut Node, reference: Option<UVec2>, view: Ui2dView, area: Vec2) {
    let (left, top, width, height) = match reference {
        Some(reference) => {
            let canvas = reference.as_vec2().max(Vec2::ONE);
            let size = canvas * view.zoom;
            let top_left = stage_origin(reference, view, area);
            (
                px(top_left.x),
                px(top_left.y),
                px(size.x.max(1.0)),
                px(size.y.max(1.0)),
            )
        }
        None => (px(0), px(0), percent(100), percent(100)),
    };

    if node.position_type != PositionType::Absolute {
        node.position_type = PositionType::Absolute;
    }
    if node.left != left {
        node.left = left;
    }
    if node.top != top {
        node.top = top;
    }
    if node.width != width {
        node.width = width;
    }
    if node.height != height {
        node.height = height;
    }
}

/// Where the canvas's top-left corner sits inside an `area`-sized stage
/// area, in that area's own logical pixels.
///
/// The one place the view becomes a position: the stage node is placed
/// there, and the rulers measure their marks from it, so a mark and the
/// pixel it names cannot drift apart.
///
/// The view pans in authored pixels from the canvas centre, y up; layout
/// places from the canvas's top-left, y down, hence the flip.
pub fn stage_origin(reference: UVec2, view: Ui2dView, area: Vec2) -> Vec2 {
    let canvas = reference.as_vec2().max(Vec2::ONE);
    let focus = canvas / 2.0 + Vec2::new(view.pan.x, -view.pan.y);
    area / 2.0 - focus * view.zoom
}

/// Thickness of a ruler, in the panel's logical pixels. It comes off the
/// stage area the way the header does, so the area stays the one node
/// the stage is placed inside and measured against.
pub const RULER_SIZE: f32 = 18.0;

/// Authored pixels between labelled marks, before the ruler coarsens the
/// step to keep the labels apart.
const RULER_LABEL_STEP: f32 = 100.0;

/// Closest two labelled marks may come, in the ruler's logical pixels.
const RULER_LABEL_GAP: f32 = 40.0;

/// Closest two unlabelled marks may come before the ruler stops drawing
/// them at all.
const RULER_TICK_GAP: f32 = 4.0;

/// How far a labelled mark reaches into the ruler from the stage edge.
const RULER_LABEL_TICK: f32 = 6.0;

/// How far an unlabelled mark reaches in.
const RULER_TICK: f32 = 3.0;

/// Most marks one ruler draws. A canvas zoomed all the way out is
/// hundreds of thousands of authored pixels wide, and every mark is a
/// node.
const RULER_MARK_LIMIT: usize = 512;

/// One of the three nodes making up a panel's ruler gutter: the two
/// rulers and the corner between them. Hidden together when the canvas
/// settings say the rulers are off, which gives the gutter back to the
/// stage area.
#[derive(Component, Clone, Copy)]
pub struct CanvasRulerPart {
    pub host: Entity,
}

/// A ruler along one edge of a panel's stage area.
///
/// `axis` is the axis of the guides pulled off it: the ruler along the
/// top measures x and drops [`CanvasAxis::Vertical`] guides, the one
/// down the left measures y and drops horizontal ones.
#[derive(Component, Clone, Copy)]
pub struct CanvasRuler {
    pub host: Entity,
    pub axis: CanvasAxis,
}

/// What one ruler's marks were last drawn for. The marks are rebuilt
/// when this changes and left alone when it does not, so panning and
/// zooming is the only thing that respawns them.
#[derive(Component, Clone, Copy, PartialEq)]
struct RulerMarks {
    /// Where the canvas's origin sits along the ruler.
    origin: f32,
    zoom: f32,
    /// How far the ruler runs, in its own logical pixels.
    length: f32,
}

/// One mark on a ruler.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RulerMark {
    /// How far along the ruler it sits, in the ruler's logical pixels.
    pub at: f32,
    /// The authored pixel it names.
    pub authored: f32,
    /// Whether it carries its figure as a label.
    pub labelled: bool,
}

/// The marks a ruler `length` logical pixels long draws, given the
/// canvas origin sitting `origin` pixels along it at `zoom`.
///
/// Labels every hundred authored pixels, or a coarser multiple of a
/// hundred once that step would bring two labels within forty pixels of
/// each other, with nine unlabelled marks between each pair while those
/// stay four pixels apart.
pub fn ruler_marks(origin: f32, zoom: f32, length: f32) -> Vec<RulerMark> {
    let mut marks = Vec::new();
    if !zoom.is_finite() || zoom <= 0.0 || !origin.is_finite() || length <= 0.0 {
        return marks;
    }
    let label_step = ruler_label_step(zoom);
    let tick_step = label_step / 10.0;
    let (step, per_label) = if tick_step * zoom >= RULER_TICK_GAP {
        (tick_step, 10)
    } else {
        (label_step, 1)
    };

    let mut index = (-origin / (zoom * step)).ceil() as i64;
    while marks.len() < RULER_MARK_LIMIT {
        let authored = index as f32 * step;
        let at = origin + authored * zoom;
        if at > length {
            break;
        }
        marks.push(RulerMark {
            at,
            authored,
            labelled: index.rem_euclid(per_label) == 0,
        });
        index += 1;
    }
    marks
}

/// Authored pixels between labels at `zoom`: the hundred the ruler wants,
/// walked up the 1-2-5 ladder until two labels stand apart.
fn ruler_label_step(zoom: f32) -> f32 {
    let ladder = [2.0, 2.5, 2.0];
    let mut step = RULER_LABEL_STEP;
    let mut rung = 0;
    while step * zoom < RULER_LABEL_GAP && step < 1.0e6 {
        step *= ladder[rung % ladder.len()];
        rung += 1;
    }
    step
}

/// Draw each panel's rulers for the view it is showing, and take the
/// gutter down when the canvas settings say the rulers are off.
///
/// Runs after the stage has been placed, so a ruler measures against the
/// same area size the canvas was laid into.
fn sync_rulers(
    snap: Res<CanvasSnap>,
    roots: Query<&UiSceneRoot, AuthoredUiSceneRoot>,
    hosts: Query<&Viewport2dPanelHost>,
    areas: Query<&ComputedNode, With<Scene2dStageArea>>,
    rulers: Query<(Entity, &CanvasRuler, Option<&RulerMarks>, Option<&Children>)>,
    mut parts: Query<&mut Node, With<CanvasRulerPart>>,
    mut commands: Commands,
) {
    let display = if snap.show_rulers {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in &mut parts {
        if node.display != display {
            node.display = display;
        }
    }
    if !snap.show_rulers {
        return;
    }

    let reference = roots
        .iter()
        .next()
        .map(|root| root.reference_size.max(UVec2::ONE));

    for (entity, ruler, drawn, children) in &rulers {
        let Ok(host) = hosts.get(ruler.host) else {
            continue;
        };
        let area = areas
            .get(host.area)
            .map(|area| area.size() * area.inverse_scale_factor())
            .unwrap_or(Vec2::ZERO);
        let wanted = reference.map(|reference| {
            let origin = stage_origin(reference, host.view, area);
            match ruler.axis {
                CanvasAxis::Vertical => RulerMarks {
                    origin: origin.x,
                    zoom: host.view.zoom,
                    length: area.x,
                },
                CanvasAxis::Horizontal => RulerMarks {
                    origin: origin.y,
                    zoom: host.view.zoom,
                    length: area.y,
                },
            }
        });
        if drawn.copied() == wanted {
            continue;
        }

        for child in children.into_iter().flatten() {
            commands.entity(*child).despawn();
        }
        let Some(wanted) = wanted else {
            commands.entity(entity).remove::<RulerMarks>();
            continue;
        };
        commands.entity(entity).insert(wanted);
        for mark in ruler_marks(wanted.origin, wanted.zoom, wanted.length) {
            commands.entity(entity).with_children(|ruler_node| {
                ruler_node.spawn(ruler_mark(ruler.axis, mark));
                if mark.labelled {
                    ruler_node.spawn(ruler_label(ruler.axis, mark));
                }
            });
        }
    }
}

/// The line one mark draws, growing in from the edge the stage is on.
fn ruler_mark(axis: CanvasAxis, mark: RulerMark) -> impl Bundle {
    let reach = if mark.labelled {
        RULER_LABEL_TICK
    } else {
        RULER_TICK
    };
    let mut node = Node {
        position_type: PositionType::Absolute,
        ..default()
    };
    match axis {
        CanvasAxis::Vertical => {
            node.left = px(mark.at);
            node.bottom = px(0);
            node.width = px(1);
            node.height = px(reach);
        }
        CanvasAxis::Horizontal => {
            node.top = px(mark.at);
            node.right = px(0);
            node.width = px(reach);
            node.height = px(1);
        }
    }
    (
        crate::EditorEntity,
        node,
        BackgroundColor(if mark.labelled {
            tokens::TEXT_SECONDARY
        } else {
            tokens::BORDER_STRONG
        }),
        Pickable::IGNORE,
    )
}

/// The authored figure a labelled mark carries, tucked against the
/// outer edge so the marks themselves stay readable.
fn ruler_label(axis: CanvasAxis, mark: RulerMark) -> impl Bundle {
    let mut node = Node {
        position_type: PositionType::Absolute,
        ..default()
    };
    match axis {
        CanvasAxis::Vertical => {
            node.left = px(mark.at + 2.0);
            node.top = px(1);
        }
        CanvasAxis::Horizontal => {
            node.left = px(2);
            node.top = px(mark.at + 1.0);
        }
    }
    (
        crate::EditorEntity,
        node,
        Text::new(format!("{:.0}", mark.authored)),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Pickable::IGNORE,
    )
}

/// The gutter around a panel's stage area: the corner, the ruler along
/// the top, and the ruler down the left, with the area itself filling
/// what is left.
///
/// The area is untouched by this, still the node the stage is placed
/// inside and measured against, so the cursor mapping, the hover test
/// and the framing all keep reading the same node they always did.
fn build_ruler_frame(world: &mut World, host: Entity, stage_area: Entity) -> Entity {
    let corner = world
        .spawn((
            crate::EditorEntity,
            CanvasRulerPart { host },
            Node {
                width: px(RULER_SIZE),
                height: px(RULER_SIZE),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(tokens::PANEL_HEADER_BG),
        ))
        .id();
    let ruler_x = world
        .spawn((
            crate::EditorEntity,
            CanvasRulerPart { host },
            CanvasRuler {
                host,
                axis: CanvasAxis::Vertical,
            },
            Node {
                height: px(RULER_SIZE),
                flex_grow: 1.0,
                min_width: px(0),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(tokens::PANEL_HEADER_BG),
            Pickable::default(),
        ))
        .id();
    let ruler_y = world
        .spawn((
            crate::EditorEntity,
            CanvasRulerPart { host },
            CanvasRuler {
                host,
                axis: CanvasAxis::Horizontal,
            },
            Node {
                width: px(RULER_SIZE),
                flex_shrink: 0.0,
                min_height: px(0),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(tokens::PANEL_HEADER_BG),
            Pickable::default(),
        ))
        .id();

    let top = world
        .spawn((
            crate::EditorEntity,
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Row,
                flex_shrink: 0.0,
                ..default()
            },
        ))
        .id();
    world.entity_mut(top).add_children(&[corner, ruler_x]);

    let body = world
        .spawn((
            crate::EditorEntity,
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Row,
                flex_grow: 1.0,
                min_height: px(0),
                ..default()
            },
        ))
        .id();
    world.entity_mut(body).add_children(&[ruler_y, stage_area]);

    let frame = world
        .spawn((
            crate::EditorEntity,
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                min_height: px(0),
                ..default()
            },
        ))
        .id();
    world.entity_mut(frame).add_children(&[top, body]);
    frame
}

/// A viewport panel opening on the canvas whatever the current intent says.
/// Both presentations are built either way; see
/// [`crate::viewport_host::build_viewport_panel_in`].
pub fn build_viewport_2d_panel(world: &mut World, parent: Entity) {
    crate::viewport_host::build_viewport_panel_in(
        world,
        parent,
        ViewportModeIntent {
            mode: ViewportMode::TwoD,
            chosen: false,
        },
    );
}

/// Build the panel's 2D presentation: a camera plus a render-target image
/// dedicated to this panel, then a column holding a header row above a
/// [`Scene2dStageArea`], with the [`Scene2dViewport`] stage placed inside it
/// showing the camera's image. Returns the column, which the mode switch shows
/// and hides.
///
/// The despawn observer on `parent` (via [`Viewport2dPanelHost`]) cleans
/// the camera up when the reconciler tears the panel down.
pub(crate) fn build_2d_presentation(world: &mut World, parent: Entity) -> Entity {
    // A render-target image dedicated to this panel. The size is only a
    // starting point: `size_targets_to_reference` holds it at the
    // authored reference size while a UI scene is open, and nothing else
    // ever resizes it.
    let image_handle = {
        let size = Extent3d {
            width: DEFAULT_VIEWPORT_WIDTH,
            height: DEFAULT_VIEWPORT_HEIGHT,
            depth_or_array_layers: 1,
        };
        let mut image = Image::new_fill(
            size,
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Bgra8UnormSrgb,
            default(),
        );
        image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::RENDER_ATTACHMENT;
        image.sampler = ImageSampler::linear();
        world.resource_mut::<Assets<Image>>().add(image)
    };

    // A private render layer per panel, so per-viewport overlays can be
    // drawn to this camera alone. Layer 0 stays in the mask so
    // default-layer scene content still draws here.
    let viewport_layer = world.resource_mut::<ViewportLayerCounter>().next();
    let camera_layers = RenderLayers::from_layers(&[0, viewport_layer]);

    let camera = world
        .spawn((
            Viewport2dCamera,
            crate::EditorEntity,
            Camera2d,
            Camera {
                order: -1,
                ..default()
            },
            RenderTarget::Image(image_handle.clone().into()),
            Msaa::Off,
            camera_layers,
        ))
        .id();

    // The panel's own pointer: what `Interact` mode drives across the
    // camera's image (see `forward_pointer_into_stage`). `Custom` is the
    // id Bevy reserves for software-driven pointers, and it is per panel
    // because two panels can be hovered independently.
    let pointer = world
        .spawn((crate::EditorEntity, PointerId::Custom(Uuid::new_v4())))
        .id();

    // The camera's image is shown with an `ImageNode` rather than a
    // `ViewportNode`: the stage node carries the view's zoom (see
    // `place_stage`), and Bevy's `update_viewport_render_target_size`
    // resizes a `ViewportNode`'s image to its node's computed size, which
    // would reallocate the render target to `reference * zoom` every
    // frame of a zoom gesture, gigabytes of it at the far end of
    // `MAX_ZOOM`. An `ImageNode` stretches the image it is given and
    // never resizes the source. The cost is Bevy's pointer forwarding
    // into the target (`viewport_picking`), which the frame child below
    // would keep out of the hover map in any case, so Interact mode
    // forwards the pointer itself.
    //
    // The frame is an absolutely positioned child rather than a border
    // on the stage itself: `ComputedNode::size()` includes the border, so
    // a border here would put the canvas bounds two pixels off the
    // image bounds and skew every measurement taken from the node.
    let stage = world
        .spawn((
            crate::EditorEntity,
            Scene2dViewport,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                ..default()
            },
            ImageNode::new(image_handle.clone()),
            children![(
                crate::EditorEntity,
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    top: px(0),
                    width: percent(100),
                    height: percent(100),
                    border: UiRect::all(px(1)),
                    ..default()
                },
                // The canvas edge: the line saying where the authored
                // scene stops and the panel begins. A subtle border
                // against a dark scene on a dark letterbox is hard to
                // find.
                BorderColor::all(tokens::BORDER_STRONG),
            )],
        ))
        .id();

    // The panel's fixed window onto the canvas. It clips, because the
    // stage inside it is placed absolutely and runs past its bounds at
    // any zoom that does not fit.
    let stage_area = world
        .spawn((
            crate::EditorEntity,
            Scene2dStageArea,
            Node {
                flex_grow: 1.0,
                min_width: px(0),
                min_height: px(0),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(tokens::WINDOW_BG),
        ))
        .id();
    world.entity_mut(stage_area).add_child(stage);
    let ruler_frame = build_ruler_frame(world, parent, stage_area);

    let column = world
        .spawn((
            ChildOf(parent),
            crate::EditorEntity,
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                overflow: Overflow::clip(),
                border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_LG)),
                ..default()
            },
            BackgroundColor(tokens::PANEL_BG),
            children![viewport_2d_header(parent)],
        ))
        .id();
    world.entity_mut(column).add_child(ruler_frame);

    world.entity_mut(parent).insert(Viewport2dPanelHost {
        camera,
        stage,
        area: stage_area,
        pointer,
        mode: Viewport2dMode::Edit,
        view: Ui2dView::default(),
        view_touched: false,
        fit_pending: true,
        target_size: UVec2::new(DEFAULT_VIEWPORT_WIDTH, DEFAULT_VIEWPORT_HEIGHT),
    });

    column
}

/// Header row above the 2D viewport stage: what is being edited on the
/// left, then the zoom readout, the Fit control and Edit|Interact on the
/// right.
///
/// The title names the scene rather than the panel; the dock tab above it
/// already says "Viewport".
///
/// Sized like the 3D viewport's toolbar (`crate::layout::toolbar`): 30px
/// tall, 1px border, top corners rounded against the stage below, and its
/// own padding on each edge.
fn viewport_2d_header(host: Entity) -> impl Bundle {
    (
        crate::EditorEntity,
        Node {
            width: percent(100),
            height: px(tokens::TOOLBAR_HEIGHT),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect {
                left: px(tokens::TOOLBAR_PADDING_LEFT),
                right: px(tokens::TOOLBAR_PADDING_RIGHT),
                top: px(0),
                bottom: px(0),
            },
            column_gap: px(tokens::TOOLBAR_GAP),
            border: UiRect::all(px(1)),
            border_radius: BorderRadius {
                top_left: px(tokens::TOOLBAR_RADIUS),
                top_right: px(tokens::TOOLBAR_RADIUS),
                bottom_left: px(0),
                bottom_right: px(0),
            },
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(tokens::PANEL_HEADER_BG),
        BorderColor::all(tokens::TOOLBAR_BORDER),
        children![
            (
                Viewport2dTitle,
                Text::new(PANEL_TITLE_FALLBACK),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            ),
            // Eats the slack, putting the controls on the right edge.
            Node {
                flex_grow: 1.0,
                ..default()
            },
            viewport_2d_grid_stepper(host),
            viewport_2d_snap_menu(host),
            (
                Viewport2dZoomReadout { host },
                Text::new(zoom_percent(Ui2dView::default().zoom)),
                TextFont {
                    font_size: tokens::TEXT_SIZE_XS,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            (
                button(
                    ButtonProps::new("Fit")
                        .with_variant(ButtonVariant::Ghost)
                        .with_size(ButtonSize::MD)
                        .call_operator(Viewport2dFrameOp::ID),
                ),
                jackdaw_feathers::tooltip::Tooltip::title(
                    "Fit: zoom so the whole UI scene is in view",
                ),
            ),
            viewport_2d_mode_bar(host),
            crate::viewport_host::viewport_mode_bar(host),
        ],
    )
}

/// The canvas's snapping menu: what a dragged node lands on, whether the
/// rulers and the guides are drawn, and the panel's own grid.
///
/// The rows are built from the world each time the menu asks for them,
/// so every box shows the setting it is about. The kinds and the two
/// view toggles are project-wide and go through their operators; the
/// grid is this panel's own, so its rows call the grid operator with the
/// figure the stepper beside them would move to.
fn viewport_2d_snap_menu(host: Entity) -> impl Bundle {
    (
        menu_button(
            "Snap",
            Icon::Magnet,
            std::sync::Arc::new(move |world: &World| snap_menu_rows(world, host)),
        ),
        jackdaw_feathers::tooltip::Tooltip::title(
            "Snap: what a dragged node lands on, and the canvas's rulers and guides",
        ),
    )
}

/// The rows the Snap menu shows for the world as it stands.
///
/// The master leads, so the switch that governs the menu is the first
/// thing in it; pixel snapping follows, because it is about what a drag
/// writes rather than what it lands on.
fn snap_menu_rows(world: &World, host: Entity) -> Vec<(String, String)> {
    let snap = world
        .get_resource::<CanvasSnap>()
        .copied()
        .unwrap_or_default();
    let kind_row = |kind: CanvasSnapKind| {
        checked_row(
            snap.offers(kind),
            format!("{OP_ACTION_PREFIX}{}?kind={}", CanvasSnapOp::ID, kind.id()),
            kind.label(),
        )
    };
    let grid = world
        .get::<Viewport2dPanelHost>(host)
        .map(|host| host.view.grid)
        .unwrap_or(DEFAULT_UI_GRID);

    let mut rows = vec![
        kind_row(CanvasSnapKind::Enabled),
        kind_row(CanvasSnapKind::Pixel),
    ];
    rows.extend(submenu_row(
        "Smart Snapping",
        CanvasSnapKind::ALL
            .into_iter()
            .filter(|kind| !matches!(kind, CanvasSnapKind::Enabled | CanvasSnapKind::Pixel))
            .map(kind_row),
    ));
    rows.push((SEPARATOR_ACTION.to_string(), String::new()));
    rows.push(checked_row(
        snap.show_rulers,
        format!(
            "{OP_ACTION_PREFIX}{}?on={}",
            CanvasRulersOp::ID,
            !snap.show_rulers
        ),
        "Show Rulers",
    ));
    rows.push(checked_row(
        snap.show_guides,
        format!(
            "{OP_ACTION_PREFIX}{}?on={}",
            CanvasGuidesOp::ID,
            !snap.show_guides
        ),
        "Show Guides",
    ));
    rows.push((
        format!("{SECTION_ACTION_PREFIX}Grid: {}", grid_label(grid)),
        String::new(),
    ));
    for (steps, label) in [(-1, "Finer"), (1, "Coarser")] {
        rows.push((
            format!(
                "{OP_ACTION_PREFIX}{}?size={}",
                Viewport2dGridOp::ID,
                stepped_ui_grid(grid, steps)
            ),
            label.to_string(),
        ));
    }
    rows
}

/// The canvas grid control: finer, the current lattice, coarser.
///
/// Shaped like the 3D toolbar's grid stepper (`crate::layout::toolbar`),
/// and wired like the Edit|Interact bar beside it: the buttons write
/// this panel's own [`Ui2dView`] through an observer rather than
/// dispatching an operator, because the grid is per-panel state and a
/// stepper in one panel's header must never move another panel's. The
/// `viewport2d.grid` operator is how a scripted run says the same thing.
fn viewport_2d_grid_stepper(host: Entity) -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(2),
            flex_shrink: 0.0,
            ..default()
        },
        children![
            viewport_2d_grid_step(host, -1, Icon::Minus, "Finer canvas grid"),
            (
                Viewport2dGridReadout { host },
                Text::new(grid_label(Ui2dView::default().grid)),
                TextFont {
                    font_size: tokens::TEXT_SIZE_XS,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    align_self: AlignSelf::Center,
                    justify_content: JustifyContent::Center,
                    min_width: px(GRID_READOUT_WIDTH),
                    ..default()
                },
                jackdaw_feathers::tooltip::Tooltip::title(
                    "Canvas grid: what a dragged or nudged node lands on, in authored pixels",
                ),
            ),
            viewport_2d_grid_step(host, 1, Icon::Plus, "Coarser canvas grid"),
        ],
    )
}

/// Room the readout keeps so the stepper's buttons do not shuffle
/// sideways as the number goes from one digit to two.
const GRID_READOUT_WIDTH: f32 = 34.0;

/// One end of the grid stepper.
fn viewport_2d_grid_step(
    host: Entity,
    steps: i32,
    icon: Icon,
    tooltip: &'static str,
) -> impl Bundle {
    (
        Viewport2dGridStep { host, steps },
        button(
            ButtonProps::new("")
                .with_variant(ButtonVariant::Ghost)
                .with_size(ButtonSize::IconSM)
                .with_left_icon(icon),
        ),
        jackdaw_feathers::tooltip::Tooltip::title(tooltip),
        observe(
            move |_: On<Pointer<Click>>, mut hosts: Query<&mut Viewport2dPanelHost>| {
                if let Ok(mut panel) = hosts.get_mut(host) {
                    let grid = stepped_ui_grid(panel.view.grid, steps);
                    if grid != panel.view.grid {
                        let view = Ui2dView { grid, ..panel.view };
                        panel.set_view(view);
                    }
                }
            },
        ),
    )
}

/// Marker on one end of the grid stepper, naming the panel it steps and
/// which way. Per panel, like [`Viewport2dModeSegment`], because the
/// grid is per-panel state.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport2dGridStep {
    pub host: Entity,
    /// Powers of two this end moves the grid by: `-1` finer, `1`
    /// coarser.
    pub steps: i32,
}

/// Marker on the header's grid readout, naming the panel it reports for.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport2dGridReadout {
    pub host: Entity,
}

/// A canvas grid as the user reads it: authored pixels, with the unit
/// spelled out so it cannot be mistaken for the zoom percentage beside
/// it.
fn grid_label(grid: f32) -> String {
    format!("{grid:.0} px")
}

/// Report each panel's canvas grid in its header.
fn update_viewport_2d_grid_readout(
    hosts: Query<&Viewport2dPanelHost>,
    mut readouts: Query<(&Viewport2dGridReadout, &mut Text)>,
) {
    for (readout, mut text) in &mut readouts {
        let Ok(host) = hosts.get(readout.host) else {
            continue;
        };
        let label = grid_label(host.view.grid);
        if text.0 != label {
            text.0 = label;
        }
    }
}

/// What the header says when no UI scene is open: the panel's own name,
/// as [`crate::builtin_extensions::ViewportExtension`] registers it.
const PANEL_TITLE_FALLBACK: &str = "Viewport";

/// Marker on the header text naming the UI scene the panel is editing.
#[derive(Component)]
pub struct Viewport2dTitle;

/// Marker on the header's zoom readout, naming the panel it reports for.
///
/// Per panel, like [`Viewport2dModeSegment`], because the zoom is per
/// panel: two viewports on the same scene are framed independently.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport2dZoomReadout {
    pub host: Entity,
}

/// A zoom as the percentage the user reads it at. `1.0` is 100%.
fn zoom_percent(zoom: f32) -> String {
    format!("{:.0}%", zoom * 100.0)
}

/// Name the scene each 2D viewport is editing, falling back to the
/// panel's own name when no UI scene is open.
///
/// The name comes from the scene tab rather than the root entity, so it
/// matches what the tab strip shows.
fn update_viewport_2d_title(
    scenes: Option<Res<crate::scenes::Scenes>>,
    roots: Query<(), AuthoredUiSceneRoot>,
    mut titles: Query<&mut Text, With<Viewport2dTitle>>,
) {
    let name = match roots.iter().next() {
        Some(()) => scenes
            .as_deref()
            .and_then(|scenes| scenes.tabs.get(scenes.active))
            .map(|tab| tab.display_name.as_str())
            .unwrap_or(PANEL_TITLE_FALLBACK),
        None => PANEL_TITLE_FALLBACK,
    };
    for mut text in &mut titles {
        if text.0 != name {
            text.0 = name.to_string();
        }
    }
}

/// Report each panel's zoom in its header.
fn update_viewport_2d_zoom_readout(
    hosts: Query<&Viewport2dPanelHost>,
    mut readouts: Query<(&Viewport2dZoomReadout, &mut Text)>,
) {
    for (readout, mut text) in &mut readouts {
        let Ok(host) = hosts.get(readout.host) else {
            continue;
        };
        let percent = zoom_percent(host.view.zoom);
        if text.0 != percent {
            text.0 = percent;
        }
    }
}

/// Operators the 2D presentation owns. Registered on
/// [`crate::builtin_extensions::ViewportExtension`] beside the panel that
/// carries them, so a workspace without a viewport does not offer them.
pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<Viewport2dFrameOp>();
    ctx.register_menu_entry::<Viewport2dFrameOp>(TopLevelMenu::View);
    ctx.register_operator::<Viewport2dModeOp>();
    ctx.register_operator::<Viewport2dGridOp>();
    ctx.register_operator::<SelectionSelectOp>();
    crate::canvas_snap::add_to_extension(ctx);
    crate::screenshot::add_2d_to_extension(ctx);
}

/// Frame the UI scene in the 2D viewport: zoom and pan so the whole
/// canvas is in view.
///
/// Every open 2D panel is framed, not just one: the editor routes a
/// single UI scene into every 2D viewport (see
/// [`route_ui_roots_to_cameras`]), and an operator has no panel to be
/// called on. The header's Fit control is a plain operator button,
/// identical to the menu entry and to a scripted run.
#[operator(
    id = "viewport2d.frame",
    label = "Fit 2D View",
    description = "Zoom the 2D viewport so the whole UI scene is in view.",
    allows_undo = false
)]
pub(crate) fn viewport_2d_frame(
    _params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(request_2d_fit);
    OperatorResult::Finished
}

/// Put the 2D viewport into Edit or Interact mode.
///
/// The header's segmented control is a pointer gesture on a node, which a
/// scripted run has no way to perform. This is that control as an
/// operator, so a capture of the panel in either mode is one call rather
/// than a synthesised click.
///
/// Every open panel is moved, the way [`viewport_2d_frame`] frames every
/// one. An operator is not called on a panel -- there is no pointer in it
/// to say which -- so the alternative is picking one arbitrarily, and a
/// scripted run that set the mode on a panel the user could not identify
/// would be worse than one that set it everywhere. The header's own
/// control stays per panel: that gesture does name its panel.
#[operator(
    id = "viewport2d.mode",
    label = "Set 2D Viewport Mode",
    description = "Switch the 2D viewport between authoring the UI scene and trying it.",
    allows_undo = false,
    params(mode(String, doc = "`edit` to author the scene, `interact` to use it."))
)]
pub(crate) fn viewport_2d_mode(
    params: In<OperatorParameters>,
    mut hosts: Query<&mut Viewport2dPanelHost>,
) -> OperatorResult {
    let Some(mode) = params.as_str("mode").and_then(parse_viewport_2d_mode) else {
        warn!("viewport2d.mode: 'mode' must be `edit` or `interact`");
        return OperatorResult::Cancelled;
    };
    if hosts.is_empty() {
        warn!("viewport2d.mode: no 2D viewport panel is open");
        return OperatorResult::Cancelled;
    }
    for mut host in &mut hosts {
        if host.mode != mode {
            host.mode = mode;
        }
    }
    OperatorResult::Finished
}

/// Set the canvas grid the 2D viewport snaps and nudges on.
///
/// The header's stepper for a scripted run. Like `viewport2d.mode` and
/// `viewport2d.frame` it acts on every open panel.
///
/// A size off the power-of-two ladder is taken as given; the header's
/// stepper pulls it back onto the ladder from there. See
/// [`stepped_ui_grid`].
#[operator(
    id = "viewport2d.grid",
    label = "Set 2D Canvas Grid",
    description = "Set the lattice the 2D viewport's gestures land on, in authored pixels.",
    allows_undo = false,
    params(size(f64, doc = "Grid size in authored pixels."))
)]
pub(crate) fn viewport_2d_grid(
    params: In<OperatorParameters>,
    mut hosts: Query<&mut Viewport2dPanelHost>,
) -> OperatorResult {
    let size = params.as_float("size")? as f32;
    if !size.is_finite() || size <= 0.0 {
        warn!("viewport2d.grid: 'size' must be a positive number of authored pixels");
        return OperatorResult::Cancelled;
    }
    if hosts.is_empty() {
        warn!("viewport2d.grid: no 2D viewport panel is open");
        return OperatorResult::Cancelled;
    }
    let grid = size.clamp(MIN_UI_GRID, MAX_UI_GRID);
    for mut host in &mut hosts {
        let view = Ui2dView { grid, ..host.view };
        host.set_view(view);
    }
    OperatorResult::Finished
}

/// The mode a `mode` parameter names, or `None` when it names neither.
fn parse_viewport_2d_mode(mode: &str) -> Option<Viewport2dMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "edit" => Some(Viewport2dMode::Edit),
        "interact" => Some(Viewport2dMode::Interact),
        _ => None,
    }
}

/// Select the authored entity with this name.
///
/// The counterpart to `selection.clear` for a scripted run: selecting on
/// the 2D stage is otherwise a click at an authored pixel, which a
/// headless capture cannot aim. Editor chrome is excluded, so the name
/// resolves against the authored scene alone, and an ambiguous name is
/// refused rather than guessed.
#[operator(
    id = "selection.select",
    label = "Select By Name",
    description = "Select the entity with this name in the current scene.",
    allows_undo = false,
    params(name(
        String,
        doc = "`Name` of the entity to select. Must match exactly one."
    ))
)]
pub(crate) fn selection_select(
    params: In<OperatorParameters>,
    named: Query<(Entity, &Name), Without<crate::EditorEntity>>,
    mut selection: ResMut<Selection>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(wanted) = params.as_str("name").filter(|name| !name.is_empty()) else {
        warn!("selection.select: missing 'name' parameter");
        return OperatorResult::Cancelled;
    };
    let Some(entity) = crate::boot_ops::unique_named_entity(named.iter(), wanted) else {
        warn!("selection.select: '{wanted}' names no entity in this scene, or more than one");
        return OperatorResult::Cancelled;
    };
    selection.select_single(&mut commands, entity);
    OperatorResult::Finished
}

/// Marker on one Edit|Interact segment, naming the panel it drives.
///
/// The panel is carried rather than looked up, because the mode is
/// per-panel state on [`Viewport2dPanelHost`] and a segment in one
/// panel's header must never move another panel's.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport2dModeSegment {
    pub host: Entity,
    pub mode: Viewport2dMode,
}

/// The two-segment Edit|Interact control, built like the Game panel's
/// Play/Select bar (`crate::game_panel::game_mode_bar`).
///
/// Not [`jackdaw_feathers::tab_strip`]: that widget spaces its tabs apart
/// rather than joining them inside one bordered box, and dispatches a
/// named operator with a string parameter. These segments write per-panel
/// state on a specific [`Viewport2dPanelHost`] entity, which no operator
/// parameter carries, so each one holds its own target and observer.
fn viewport_2d_mode_bar(host: Entity) -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            overflow: Overflow::clip(),
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        children![
            viewport_2d_mode_segment(
                host,
                Viewport2dMode::Edit,
                "Edit",
                "Edit: clicks select and move the UI you are authoring",
            ),
            viewport_2d_mode_segment(
                host,
                Viewport2dMode::Interact,
                "Interact",
                "Interact: clicks go to the UI itself, so you can try it",
            ),
        ],
    )
}

/// One clickable segment inside the Edit|Interact control.
fn viewport_2d_mode_segment(
    host: Entity,
    mode: Viewport2dMode,
    label: &'static str,
    tooltip: &'static str,
) -> impl Bundle {
    (
        Viewport2dModeSegment { host, mode },
        Interaction::default(),
        jackdaw_feathers::tooltip::Tooltip::title(tooltip),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
            ..default()
        },
        BackgroundColor(Color::NONE),
        observe(
            move |click: On<Pointer<Click>>,
                  disabled: Query<(), With<bevy::ui::InteractionDisabled>>,
                  mut hosts: Query<&mut Viewport2dPanelHost>| {
                if disabled.contains(click.event_target()) {
                    return;
                }
                if let Ok(mut panel) = hosts.get_mut(host)
                    && panel.mode != mode
                {
                    panel.mode = mode;
                }
            },
        ),
        children![(
            Text::new(label),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
    )
}

/// Highlight the segment matching each panel's current mode.
fn update_viewport_2d_mode_bar(
    hosts: Query<&Viewport2dPanelHost>,
    mut segments: Query<(&Viewport2dModeSegment, &mut BackgroundColor)>,
) {
    for (segment, mut background) in &mut segments {
        let Ok(host) = hosts.get(segment.host) else {
            continue;
        };
        let color = if host.mode == segment.mode {
            tokens::TOOLBAR_ACTIVE_BG
        } else {
            Color::NONE
        };
        if background.0 != color {
            background.0 = color;
        }
    }
}

/// Despawn a 2D viewport panel's camera and its pointer when the panel
/// content entity goes away (panel closed, leaf rebuilt on split,
/// workspace switch). The stage node is a descendant of the panel, so it
/// is torn down with it and needs no explicit cleanup.
pub(crate) fn on_viewport_2d_panel_despawn(
    trigger: On<Despawn, Viewport2dPanelHost>,
    hosts: Query<&Viewport2dPanelHost>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let Ok(host) = hosts.get(entity) else {
        return;
    };
    for owned in [host.camera, host.pointer] {
        if let Ok(mut ec) = commands.get_entity(owned) {
            ec.despawn();
        }
    }
}
