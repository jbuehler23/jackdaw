//! The 2D presentation of a viewport panel: a `Camera2d` rendered into a
//! texture and shown in a UI node, one of the two presentations every viewport
//! panel builds (the mode lives in [`crate::viewport_host`]).
//!
//! Holds the stage skeleton, its camera, its teardown, and the stage's
//! navigation: an [`Ui2dView`] per panel that scroll and middle-drag move and
//! that rides along with the scene tab it was framed for. Selection and the
//! editing overlays live in [`crate::ui_stage`].

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
    ui::{Checked, UiGlobalTransform, UiSystems, UiTargetCamera},
    ui_widgets::{ValueChange, observe},
};
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_feathers::{
    button::{ButtonProps, ButtonSize, ButtonVariant, button},
    menu_bar::{
        OP_ACTION_PREFIX, SECTION_ACTION_PREFIX, SEPARATOR_ACTION, checked_row, menu_button,
    },
    segmented, tokens,
};
use jackdaw_scene_types::{CanvasGuides, UiSceneRoot};

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

/// Draw order of the parking camera: behind every panel camera, and distinct so
/// a camera-order collision warning cannot point here.
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

/// Marker for the editor's single parking camera. An authored UI scene root
/// points here whenever no 2D viewport panel is open, so it is never handed back
/// to the editor's own window camera.
#[derive(Component)]
pub struct UiSceneParkingCamera;

/// What a 2D viewport panel does with pointer input: `Edit` selects and
/// manipulates the scene being authored, `Interact` hands the pointer to the
/// live widgets. One per panel, on [`Viewport2dPanelHost`].
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Viewport2dMode {
    #[default]
    Edit,
    Interact,
}

/// How a 2D viewport is framed over the scene it edits: `place_stage` derives
/// the stage node's size and position from it, and it travels with a scene tab
/// across a swap.
///
/// The view moves the stage, not the camera. Bevy pins a routed UI scene to its
/// render target and builds its own view from the target's rect, so no camera
/// transform can pan or zoom it. The panel's image stays at the authored
/// reference size while the stage node grows and slides around it, and
/// [`target_pixels_per_stage_pixel`] reads the zoom back off the laid-out node.
///
/// The view is driven in the stage area's logical pixels
/// ([`cursor_area_offset`]); a click is resolved in authored pixels
/// ([`cursor_stage_offset`] against the stage node, then
/// `ui_stage::stage_to_authored`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ui2dView {
    /// Authored point shown at the centre of the stage area, in authored
    /// pixels from the centre of the canvas, y up.
    pub pan: Vec2,
    /// Stage logical pixels per authored pixel. `1.0` draws the canvas
    /// at its authored size, whatever the display's scale factor.
    pub zoom: f32,
    /// Lattice a manipulated node lands on, in authored pixels. Separate from
    /// the 3D grid, which is measured in world units. It rides on the view
    /// because that is what a scene tab captures and restores.
    pub grid: f32,
}

/// Default [`Ui2dView::grid`]: eight authored pixels, the step UI layouts are
/// usually spaced on.
pub const DEFAULT_UI_GRID: f32 = 8.0;

/// Finest [`Ui2dView::grid`] the stepper reaches: a whole authored pixel.
pub const MIN_UI_GRID: f32 = 1.0;

/// Coarsest [`Ui2dView::grid`] the stepper reaches.
pub const MAX_UI_GRID: f32 = 64.0;

/// `grid` stepped `steps` powers of two, clamped to the ladder's ends. The step
/// goes through the power rather than multiplying the value, so a grid set off
/// the ladder comes back onto it on the first press.
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

/// Authored point under a cursor sitting `area_offset` logical pixels from the
/// centre of the stage area (see [`cursor_area_offset`]), in authored pixels
/// from the centre of the canvas. Screen pixels run y-down and the pan y-up.
pub fn world_at(view: Ui2dView, area_offset: Vec2) -> Vec2 {
    view.pan + Vec2::new(area_offset.x, -area_offset.y) / view.zoom
}

/// Zoom by `ticks` wheel steps about the point under the cursor: the pan is
/// re-solved so [`world_at`] returns the same authored point before and after.
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
/// `area`-sized window, centred, with `FIT_MARGIN` to spare on each edge. The
/// usable box is floored at one pixel and the zoom clamped, since this runs on
/// the frame a panel is first laid out and a zoom of zero would divide through
/// every mapping downstream.
pub fn fit_view(view: Ui2dView, reference: UVec2, area: Vec2) -> Ui2dView {
    let canvas = reference.as_vec2().max(Vec2::ONE);
    let usable = (area - Vec2::splat(FIT_MARGIN * 2.0)).max(Vec2::ONE);
    Ui2dView {
        pan: Vec2::ZERO,
        zoom: (usable / canvas).min_element().clamp(MIN_ZOOM, MAX_ZOOM),
        ..view
    }
}

/// Ask every 2D viewport panel to frame the UI scene it is showing. Requested
/// rather than computed here, because the reference size and the area's
/// laid-out size are only settled after layout.
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

/// Render-target pixels per stage physical pixel, which turns a stage
/// measurement into authored pixels. Derived from the laid-out node rather than
/// read off [`Ui2dView`], so no second copy of the zoom can drift. A degenerate
/// stage returns `1.0` rather than an infinity.
pub fn target_pixels_per_stage_pixel(stage_size: Vec2, target_size: UVec2) -> f32 {
    if stage_size.x <= 0.0 || stage_size.y <= 0.0 {
        return 1.0;
    }
    (target_size.as_vec2() / stage_size).min_element()
}

/// Offset of `cursor` (ui-logical) from the centre of a stage node, in the
/// panel's render-target pixels, or `None` when the cursor is outside it.
///
/// The cursor is lifted into physical space rather than the node pushed into
/// logical space: an image render target has a scale factor of 1, so converting
/// the node instead would land the hit test a factor of `scale_factor` off on
/// any high-DPI display.
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

/// [`cursor_stage_offset`]'s mapping without its refusal: the offset clamped to
/// the stage's edge, and whether the cursor was really on it. A gesture the
/// scene is already holding has to be followed off the canvas.
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

/// [`cursor_stage_offset`]'s mapping with neither its refusal nor its clamp,
/// for gestures that run off the canvas by design, such as dragging a guide
/// back onto its ruler to drop it.
pub fn stage_offset_unbounded(
    cursor: Vec2,
    centre: Vec2,
    inverse_scale_factor: f32,
    target_scale: f32,
) -> Vec2 {
    (cursor / inverse_scale_factor - centre) * target_scale
}

/// Offset of `cursor` from the centre of a panel's stage area, in that area's
/// own logical pixels, or `None` when the cursor is outside it. The space
/// [`Ui2dView`] is driven in: the area is the fixed window the scene moves
/// behind, which is what makes [`zoom_toward`]'s anchor hold.
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

/// Sits on the dock-leaf content entity that hosts a 2D viewport panel, holding
/// the camera, the stage node, the area that clips it, and how the panel is
/// framed, so teardown and callers need not walk the panel's children.
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
    /// Whether [`Self::view`] is a framing something chose, rather than the
    /// default it was built with. A tab swap captures the panel's framing, and
    /// capturing an untouched panel would stamp the default onto the tab as a
    /// chosen framing that no fit could ever reach.
    pub view_touched: bool,
    /// Set when something has asked this panel to frame its scene, cleared on
    /// the first frame the fit can be computed. See [`request_2d_fit`].
    ///
    /// A new panel starts with one pending, since a restored session can open
    /// its tabs before the dock has built a leaf. A tab that was framed
    /// withdraws this on restore, so the default never outranks a chosen
    /// framing.
    pub fit_pending: bool,
    /// Size of the camera's render-target image, in its own pixels: the
    /// reference size of the active UI scene, or the stage's own size when none
    /// is open. Cached here so the pan/zoom pass need not touch `Assets<Image>`.
    pub target_size: UVec2,
}

impl Viewport2dPanelHost {
    /// Frame the panel, recording that its framing is chosen rather than
    /// default. Every writer of the view goes through here.
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
        // The layer counter is shared with `crate::viewport`, whose plugin may
        // not have been added yet: whichever lands first installs it.
        app.init_resource::<ViewportLayerCounter>()
            .init_resource::<Ui2dPanActive>()
            .add_observer(on_viewport_2d_panel_despawn)
            // Where `bevy_ui` runs its own `viewport_picking`: after this
            // frame's `PointerInput` is written, and early enough that the
            // forwarded copy is a real input to every picking system after it.
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
                    // Before layout reads a root's target camera, or the scene
                    // spends a frame laid out against the default UI camera.
                    route_ui_roots_to_cameras.before(UiSystems::Prepare),
                    // After layout, so the stage is placed from the area's
                    // measured size.
                    size_targets_to_reference.after(UiSystems::PostLayout),
                    // After the placement, so a ruler's marks are measured
                    // against the area the canvas was just laid into.
                    sync_rulers.after(size_targets_to_reference),
                    sync_ruler_guide_marks.after(sync_rulers),
                ),
            );
    }
}

/// Tracks the panel a middle-drag pan started on, so the gesture stays with it
/// wherever the pointer goes. Panning past the panel's edge is the ordinary way
/// to reach a corner of a canvas larger than its window.
#[derive(Resource, Default)]
pub struct Ui2dPanActive(pub Option<Entity>);

/// Scroll to zoom, middle-drag to pan, for whichever 2D viewport the cursor is
/// over, while that viewport is in [`Viewport2dMode::Edit`].
///
/// Hover is resolved against the panel's stage area rather than its stage node,
/// which a zoom pushes past the area. The pan follows [`Ui2dPanActive`] and the
/// zoom follows the cursor.
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

/// The panel whose stage area is under `cursor` (ui-logical pixels), and the
/// cursor's offset from that area's centre in the area's logical pixels.
///
/// The [`HoverMap`] and a rect test both have to agree: the hover map accounts
/// for a popup or docked panel over the stage, the rect test turns that hover
/// into the offset [`zoom_toward`] anchors on.
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

/// Push each panel's [`Ui2dView`] onto its camera, which is where the transform
/// and ortho scale come from.
///
/// This does not move the authored UI, which Bevy renders through a view of its
/// own (`place_stage` moves that). It applies to 2D world content drawn into
/// this panel, so sprites, gizmos and guides stay framed with the canvas.
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

/// Drive each `Interact` panel's own pointer across its render target, so the
/// authored widgets showing there can be used. Bevy's `viewport_picking` does
/// not apply, the stage being an `ImageNode` whose frame child keeps it out of
/// the `HoverMap` that system reads.
///
/// The pointer may enter the scene only while the panel is hovered per the
/// [`HoverMap`] and no editor interaction is in flight, or a rect test alone
/// would send clicks through an open modal into the live scene. The hover map
/// read here is one frame old, so entering takes one pointer input to warm up.
///
/// A press already under way runs to its release wherever the cursor goes, with
/// positions clamped to the stage's edge: dropping it at the boundary would
/// leave the widget latched down with no `Click` ever resolving.
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
    // Only the editor's real pointers, which are the ones on a window. Filtering
    // by target rather than by id also keeps a forwarded input from being
    // forwarded again.
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
        // No primary window to resolve against: a panel camera always targets
        // an image, which normalises on its own.
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

/// Whether one of the editor's real pointers is over `entity` or anything inside
/// it. Custom pointers are skipped, or a panel would be kept hovered by the
/// pointer it is itself driving.
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

/// Point every UI scene root at the camera whose view it belongs in: an authored
/// root at a 2D viewport's camera, an imported one at the 3D viewport's, where a
/// world scene's UI shows as a screen-space overlay.
///
/// Routing is an inserted [`UiTargetCamera`], never a reparent, naming a camera
/// this session spawned that `scene_io` skips on save.
///
/// A root is always routed somewhere: removing the component would hand it back
/// to the editor's own window camera and draw the scene over the editor chrome.
/// A group with no view parks on [`UiSceneParkingCamera`] instead, and both
/// groups are routed in one system so a session cannot spawn two parking
/// cameras in one frame's command queue.
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

/// Spawn the parking camera: where an authored UI scene root points while no 2D
/// viewport panel is open. Inactive and targeting a 1x1 image, so a parked root
/// resolves against a single pixel and `DefaultUiCamera` never picks it up.
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

/// Hold each panel's render-target image at the active UI scene's reference
/// size, and place the stage over it for the current view.
///
/// This is the reference-resolution contract: Bevy derives a UI root's layout
/// viewport from its target's size, so sizing the image to `reference_size`
/// makes authored layout compute at the resolution it was designed for whatever
/// the dock leaf measures. With no UI scene open the stage fills its area and
/// the panel is a plain 1:1 viewport.
///
/// A pending fit ([`request_2d_fit`]) lands here too, this being the one place
/// holding both the reference size and the area's laid-out size.
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
        // settled. A request outlives a frame with nothing to fit, because
        // opening a scene asks before the scene has spawned.
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

/// Place a panel's stage inside its area for the current view: the canvas at
/// `reference_size * zoom` logical pixels, positioned so the authored point the
/// view is panned to lands at the centre of the area.
///
/// Pan and zoom are this placement, not a camera move (see [`Ui2dView`]).
/// Placing rather than transforming keeps the zoom inside
/// `ComputedNode::size()`, where the cursor mapping and the selection overlay
/// pick it up without naming it.
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

/// Where the canvas's top-left corner sits inside an `area`-sized stage area, in
/// that area's own logical pixels. The one place the view becomes a position,
/// so a ruler's mark and the pixel it names cannot drift apart.
pub fn stage_origin(reference: UVec2, view: Ui2dView, area: Vec2) -> Vec2 {
    let canvas = reference.as_vec2().max(Vec2::ONE);
    let focus = canvas / 2.0 + Vec2::new(view.pan.x, -view.pan.y);
    area / 2.0 - focus * view.zoom
}

/// Thickness of a ruler, in the panel's logical pixels.
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

/// Marks a ruler keeps beyond the one per `RULER_TICK_GAP` its length has room
/// for: the pair at either end that a pan can bring in.
const RULER_MARK_MARGIN: usize = 8;

/// How much room a label needs along the ruler it is written on, which for the
/// left ruler is the box its turned labels sit inside.
const RULER_LABEL_BOX: f32 = 40.0;

/// How far in from the ruler's outer edge a turned label's centre sits,
/// leaving the marks the rest of the gutter.
const RULER_LABEL_INSET: f32 = 6.0;

/// Most marks a ruler `length` logical pixels long draws. Derived from the
/// length rather than fixed, since a fixed count would stop the marks partway
/// across a wide panel.
fn ruler_mark_budget(length: f32) -> usize {
    let per_length = (length / RULER_TICK_GAP).ceil();
    let per_length = if per_length.is_finite() && per_length > 0.0 {
        per_length as usize
    } else {
        0
    };
    per_length + RULER_MARK_MARGIN
}

/// One of the three nodes making up a panel's ruler gutter: the two rulers and
/// the corner between them. Hidden together when the rulers are off.
#[derive(Component, Clone, Copy)]
pub struct CanvasRulerPart {
    pub host: Entity,
}

/// A ruler along one edge of a panel's stage area. `axis` is the axis of the
/// guides pulled off it, so the top ruler measures x and drops
/// [`CanvasAxis::Vertical`] guides.
#[derive(Component, Clone, Copy)]
pub struct CanvasRuler {
    pub host: Entity,
    pub axis: CanvasAxis,
}

/// What one ruler's marks currently read: the authored figure each one names and
/// whether it carries it. A pan changes none of this, so only a different
/// reading respawns the mark nodes.
#[derive(Component, Clone, PartialEq)]
struct RulerMarks(Vec<(f32, bool)>);

/// One node of a ruler's marks: the tick, or the figure beside it.
#[derive(Component, Clone, Copy)]
struct RulerMarkNode {
    /// Which mark of the ruler's reading it draws.
    index: usize,
    /// Whether it is the figure rather than the tick.
    label: bool,
}

/// A guide's position, marked on the ruler it came off, which is the target of
/// the drag that removes it.
#[derive(Component, Clone, Copy)]
pub struct RulerGuideMark {
    /// The panel content entity carrying this stage's
    /// [`Viewport2dPanelHost`].
    pub host: Entity,
    /// Which way the guide runs.
    pub axis: CanvasAxis,
    /// Which guide of that axis it marks.
    pub index: usize,
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

/// The marks a ruler `length` logical pixels long draws, given the canvas origin
/// sitting `origin` pixels along it at `zoom`. Labels every hundred authored
/// pixels, coarsened until two labels stand apart, with nine unlabelled marks
/// between each pair while those stay `RULER_TICK_GAP` apart.
pub fn ruler_marks(origin: f32, zoom: f32, length: f32) -> Vec<RulerMark> {
    let mut marks = Vec::new();
    if !zoom.is_finite() || zoom <= 0.0 || !origin.is_finite() || length <= 0.0 {
        return marks;
    }
    let label_step = ruler_label_step(zoom);
    let tick_step = label_step / 10.0;
    let budget = ruler_mark_budget(length);
    let (step, per_label) = if tick_step * zoom >= RULER_TICK_GAP
        && ((length / (tick_step * zoom)).ceil() as usize) < budget
    {
        (tick_step, 10)
    } else {
        // Over budget, the unlabelled ticks are what goes: a ruler that reads
        // the whole panel is worth more than one that stops partway across.
        (label_step, 1)
    };

    let mut index = (-origin / (zoom * step)).ceil() as i64;
    while marks.len() < budget {
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
    let mut rung = ladder.len();
    loop {
        let finer = step / ladder[(rung - 1) % ladder.len()];
        if finer * zoom < RULER_LABEL_GAP || finer < 1.0 {
            break;
        }
        step = finer;
        rung -= 1;
        if rung == 0 {
            rung = ladder.len();
        }
    }
    step
}

#[cfg(test)]
mod ruler_label_step_tests {
    use super::*;

    #[test]
    fn the_label_step_keeps_labels_apart_and_no_further_apart_than_it_must() {
        for zoom in [0.1_f32, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0] {
            let step = ruler_label_step(zoom);
            assert!(
                step * zoom >= RULER_LABEL_GAP,
                "zoom {zoom}: labels {step} apart come too close",
            );
            let ladder = [2.0, 2.5, 2.0];
            let finer_fits = ladder
                .iter()
                .any(|div| (step / div) * zoom >= RULER_LABEL_GAP && step / div >= 1.0);
            assert!(
                !finer_fits,
                "zoom {zoom}: a finer step than {step} would still fit"
            );
        }
    }

    #[test]
    fn zooming_in_refines_the_labels() {
        assert_eq!(ruler_label_step(4.0), 10.0);
        assert_eq!(ruler_label_step(1.0), 50.0);
        assert_eq!(ruler_label_step(0.5), 100.0);
        assert_eq!(ruler_label_step(0.1), 500.0);
    }
}

/// Draw each panel's rulers for the view it is showing, and take the gutter down
/// when the rulers are off. Runs after the stage has been placed, so a ruler
/// measures against the same area size the canvas was laid into.
fn sync_rulers(
    snap: Res<CanvasSnap>,
    roots: Query<(Entity, &UiSceneRoot), AuthoredUiSceneRoot>,
    hosts: Query<&Viewport2dPanelHost>,
    areas: Query<&ComputedNode, With<Scene2dStageArea>>,
    rulers: Query<(Entity, &CanvasRuler, Option<&RulerMarks>, Option<&Children>)>,
    parts: Query<Entity, With<CanvasRulerPart>>,
    mark_nodes: Query<&RulerMarkNode>,
    mut nodes: Query<&mut Node>,
    mut commands: Commands,
) {
    let display = if snap.show_rulers {
        Display::Flex
    } else {
        Display::None
    };
    for part in &parts {
        if let Ok(mut node) = nodes.get_mut(part)
            && node.display != display
        {
            node.display = display;
        }
    }
    if !snap.show_rulers {
        return;
    }

    // A malformed document holding several roots picks the lowest entity, the
    // same one the guide operators write.
    let reference = roots
        .iter()
        .min_by_key(|(entity, _)| *entity)
        .map(|(_, root)| root.reference_size.max(UVec2::ONE));

    for (entity, ruler, drawn, children) in &rulers {
        let Ok(host) = hosts.get(ruler.host) else {
            continue;
        };
        let area = areas
            .get(host.area)
            .map(|area| area.size() * area.inverse_scale_factor())
            .unwrap_or(Vec2::ZERO);
        let Some(reference) = reference else {
            if drawn.is_some() {
                despawn_ruler_marks(&mut commands, children, &mark_nodes);
                commands.entity(entity).remove::<RulerMarks>();
            }
            continue;
        };
        let origin = stage_origin(reference, host.view, area);
        let (origin, length) = match ruler.axis {
            CanvasAxis::Vertical => (origin.x, area.x),
            CanvasAxis::Horizontal => (origin.y, area.y),
        };
        let marks = ruler_marks(origin, host.view.zoom, length);
        let reading = RulerMarks(
            marks
                .iter()
                .map(|mark| (mark.authored, mark.labelled))
                .collect(),
        );

        // A pan slides every mark and leaves the reading alone, so the nodes
        // are moved rather than respawned every frame of the drag.
        if drawn == Some(&reading) {
            for child in children.into_iter().flatten() {
                let Ok(mark_node) = mark_nodes.get(*child) else {
                    continue;
                };
                let (Some(mark), Ok(mut node)) =
                    (marks.get(mark_node.index), nodes.get_mut(*child))
                else {
                    continue;
                };
                if mark_node.label {
                    place_ruler_label(&mut node, ruler.axis, *mark);
                } else {
                    place_ruler_mark(&mut node, ruler.axis, *mark);
                }
            }
            continue;
        }

        despawn_ruler_marks(&mut commands, children, &mark_nodes);
        commands.entity(entity).insert(reading);
        for (index, mark) in marks.into_iter().enumerate() {
            commands.entity(entity).with_children(|ruler_node| {
                ruler_node.spawn(ruler_mark(ruler.axis, mark, index));
                if mark.labelled {
                    ruler_node.spawn(ruler_label(ruler.axis, mark, index));
                }
            });
        }
    }
}

/// Take a ruler's marks down, leaving whatever else was parented to it
/// where it is.
fn despawn_ruler_marks(
    commands: &mut Commands,
    children: Option<&Children>,
    mark_nodes: &Query<&RulerMarkNode>,
) {
    for child in children.into_iter().flatten() {
        if mark_nodes.contains(*child) {
            commands.entity(*child).despawn();
        }
    }
}

/// Draw every guide on the ruler it was pulled off, so the drag that takes one
/// away has somewhere to aim. The guides are scene data and the rulers are per
/// panel, so every panel showing the scene marks the same set.
fn sync_ruler_guide_marks(
    mut commands: Commands,
    snap: Res<CanvasSnap>,
    scenes: Query<(Entity, &UiSceneRoot, Option<&CanvasGuides>), AuthoredUiSceneRoot>,
    hosts: Query<&Viewport2dPanelHost>,
    areas: Query<&ComputedNode, With<Scene2dStageArea>>,
    rulers: Query<(Entity, &CanvasRuler)>,
    marks: Query<(Entity, &RulerGuideMark)>,
    mut nodes: Query<&mut Node>,
) {
    let scene = scenes
        .iter()
        .min_by_key(|(entity, _, _)| *entity)
        .filter(|_| snap.show_rulers && snap.show_guides);

    let mut wanted: Vec<(Entity, RulerGuideMark, f32)> = Vec::new();
    if let Some((_, root, Some(guides))) = scene {
        let reference = root.reference_size.max(UVec2::ONE);
        for (ruler_entity, ruler) in &rulers {
            let Ok(host) = hosts.get(ruler.host) else {
                continue;
            };
            if host.mode != Viewport2dMode::Edit {
                continue;
            }
            let area = areas
                .get(host.area)
                .map(|area| area.size() * area.inverse_scale_factor())
                .unwrap_or(Vec2::ZERO);
            let origin = stage_origin(reference, host.view, area);
            let (origin, positions) = match ruler.axis {
                CanvasAxis::Vertical => (origin.x, &guides.vertical),
                CanvasAxis::Horizontal => (origin.y, &guides.horizontal),
            };
            for (index, at) in positions.iter().enumerate() {
                wanted.push((
                    ruler_entity,
                    RulerGuideMark {
                        host: ruler.host,
                        axis: ruler.axis,
                        index,
                    },
                    origin + at * host.view.zoom,
                ));
            }
        }
    }

    for (entity, mark) in &marks {
        match wanted.iter().find(|(_, want, _)| {
            want.host == mark.host && want.axis == mark.axis && want.index == mark.index
        }) {
            Some((_, _, at)) => {
                if let Ok(mut node) = nodes.get_mut(entity) {
                    place_ruler_guide_mark(&mut node, mark.axis, *at);
                }
            }
            None => {
                if let Ok(mut entity) = commands.get_entity(entity) {
                    entity.despawn();
                }
            }
        }
    }

    for (ruler, want, at) in wanted {
        if marks.iter().any(|(_, mark)| {
            mark.host == want.host && mark.axis == want.axis && mark.index == want.index
        }) {
            continue;
        }
        let mut node = Node {
            position_type: PositionType::Absolute,
            ..default()
        };
        place_ruler_guide_mark(&mut node, want.axis, at);
        commands.spawn((
            crate::EditorEntity,
            want,
            node,
            BackgroundColor(tokens::GUIDE_LINE),
            Pickable::IGNORE,
            ChildOf(ruler),
        ));
    }
}

/// How far into the gutter a guide's mark reaches, measured from the stage edge
/// the ticks are drawn against: the band the ticks stand in and no further, so
/// a mark is never painted over a label.
const RULER_GUIDE_MARK_REACH: f32 = RULER_LABEL_TICK;

/// How wide a guide's mark on the ruler is, across the ruler's own
/// direction.
const RULER_GUIDE_MARK: f32 = 3.0;

/// Lay a guide's mark across the ruler at `at`, centred on the guide.
fn place_ruler_guide_mark(node: &mut Node, axis: CanvasAxis, at: f32) {
    let half = RULER_GUIDE_MARK / 2.0;
    match axis {
        CanvasAxis::Vertical => {
            node.left = px(at - half);
            node.top = Val::Auto;
            node.bottom = px(0);
            node.width = px(RULER_GUIDE_MARK);
            node.height = px(RULER_GUIDE_MARK_REACH);
        }
        CanvasAxis::Horizontal => {
            node.left = Val::Auto;
            node.right = px(0);
            node.top = px(at - half);
            node.width = px(RULER_GUIDE_MARK_REACH);
            node.height = px(RULER_GUIDE_MARK);
        }
    }
}

/// The line one mark draws, growing in from the edge the stage is on.
fn ruler_mark(axis: CanvasAxis, mark: RulerMark, index: usize) -> impl Bundle {
    let mut node = Node {
        position_type: PositionType::Absolute,
        ..default()
    };
    place_ruler_mark(&mut node, axis, mark);
    (
        crate::EditorEntity,
        RulerMarkNode {
            index,
            label: false,
        },
        node,
        BackgroundColor(if mark.labelled {
            tokens::TEXT_SECONDARY
        } else {
            tokens::BORDER_STRONG
        }),
        Pickable::IGNORE,
    )
}

/// Put a mark's tick where the reading says, in the ruler's own pixels.
fn place_ruler_mark(node: &mut Node, axis: CanvasAxis, mark: RulerMark) {
    let reach = if mark.labelled {
        RULER_LABEL_TICK
    } else {
        RULER_TICK
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
}

/// The authored figure a labelled mark carries, tucked against the outer edge so
/// the marks stay readable. The left ruler is narrower than a four-figure
/// reading, so its labels are turned on their side inside a box of known width
/// rather than whatever the text happened to measure.
fn ruler_label(axis: CanvasAxis, mark: RulerMark, index: usize) -> impl Bundle {
    let mut node = Node {
        position_type: PositionType::Absolute,
        ..default()
    };
    place_ruler_label(&mut node, axis, mark);
    let label = (
        Text::new(format!("{:.0}", mark.authored)),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Pickable::IGNORE,
    );
    (
        crate::EditorEntity,
        RulerMarkNode { index, label: true },
        node,
        UiTransform::from_rotation(match axis {
            CanvasAxis::Vertical => Rot2::IDENTITY,
            CanvasAxis::Horizontal => Rot2::degrees(-90.0),
        }),
        Pickable::IGNORE,
        children![label],
    )
}

/// Put a mark's figure where the reading says.
fn place_ruler_label(node: &mut Node, axis: CanvasAxis, mark: RulerMark) {
    match axis {
        CanvasAxis::Vertical => {
            node.left = px(mark.at + 2.0);
            node.top = px(1);
            node.width = Val::Auto;
            node.height = Val::Auto;
        }
        CanvasAxis::Horizontal => {
            // Turned a quarter turn about its own centre, so the box is placed
            // by where that centre has to end up.
            node.left = px(RULER_LABEL_INSET - RULER_LABEL_BOX / 2.0);
            node.top = px(mark.at - RULER_SIZE / 2.0);
            node.width = px(RULER_LABEL_BOX);
            node.height = px(RULER_SIZE);
            node.justify_content = JustifyContent::Center;
            node.align_items = AlignItems::Center;
        }
    }
}

/// The gutter around a panel's stage area: the corner, the ruler along the top,
/// and the ruler down the left, with the area itself filling what is left. The
/// area is untouched, still the node the stage is placed inside.
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
    // A render-target image dedicated to this panel. The size is only a starting
    // point: `size_targets_to_reference` holds it at the authored reference size
    // while a UI scene is open.
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

    // A private render layer per panel, so per-viewport overlays can be drawn to
    // this camera alone. Layer 0 stays in the mask so default-layer scene
    // content still draws here.
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

    // The panel's own pointer, driven across the camera's image in `Interact`
    // mode. Per panel, because two panels can be hovered independently.
    let pointer = world
        .spawn((crate::EditorEntity, PointerId::Custom(Uuid::new_v4())))
        .id();

    // An `ImageNode` rather than a `ViewportNode`: the stage node carries the
    // view's zoom, and Bevy resizes a `ViewportNode`'s image to its node's
    // computed size, reallocating the render target to `reference * zoom` every
    // frame of a zoom gesture. The cost is Bevy's pointer forwarding, which
    // `forward_pointer_into_stage` does instead.
    //
    // The frame is an absolutely positioned child rather than a border on the
    // stage: `ComputedNode::size()` includes the border, which would put the
    // canvas bounds two pixels off the image bounds.
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
                // The canvas edge: where the authored scene stops and the panel
                // begins.
                BorderColor::all(tokens::BORDER_STRONG),
            )],
        ))
        .id();

    // The panel's fixed window onto the canvas. It clips, because the stage
    // inside it is placed absolutely and runs past its bounds at any zoom that
    // does not fit.
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

/// Header row above the 2D viewport stage: what is being edited on the left,
/// then the zoom readout, the Fit control and Edit|Interact on the right. Sized
/// like the 3D viewport's toolbar.
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

/// The canvas's snapping menu: what a dragged node lands on, whether the rulers
/// and the guides are drawn, and the panel's own grid. The rows are built from
/// the world each time the menu asks for them, so every box shows the setting it
/// is about.
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
        (SEPARATOR_ACTION.to_string(), String::new()),
        (
            format!("{SECTION_ACTION_PREFIX}Smart Snapping"),
            String::new(),
        ),
    ];
    rows.extend(
        CanvasSnapKind::ALL
            .into_iter()
            .filter(|kind| !matches!(kind, CanvasSnapKind::Enabled | CanvasSnapKind::Pixel))
            .map(kind_row),
    );
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
        // Named on this panel: the grid is per panel, so a row in one panel's
        // header must not move another panel's lattice.
        rows.push((
            format!(
                "{OP_ACTION_PREFIX}{}?size={}&panel={}",
                Viewport2dGridOp::ID,
                stepped_ui_grid(grid, steps),
                host.to_bits(),
            ),
            label.to_string(),
        ));
    }
    rows
}

/// The canvas grid control: finer, the current lattice, coarser. The buttons
/// write this panel's own [`Ui2dView`] through an observer rather than
/// dispatching an operator, the grid being per-panel state; `viewport2d.grid` is
/// how a scripted run says the same thing.
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

/// Marker on one end of the grid stepper, naming the panel it steps and which
/// way. Per panel, because the grid is per-panel state.
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

/// A canvas grid as the user reads it: authored pixels, with the unit spelled
/// out so it cannot be mistaken for the zoom percentage beside it.
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

/// Marker on the header's zoom readout, naming the panel it reports for: two
/// viewports on the same scene are framed independently.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport2dZoomReadout {
    pub host: Entity,
}

/// A zoom as the percentage the user reads it at. `1.0` is 100%.
fn zoom_percent(zoom: f32) -> String {
    format!("{:.0}%", zoom * 100.0)
}

/// Name the scene each 2D viewport is editing, falling back to the panel's own
/// name when no UI scene is open. Taken from the scene tab rather than the root
/// entity, so it matches what the tab strip shows.
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
    ctx.register_operator::<Viewport2dGridOp>()
        .register_operator::<Viewport2dZoomOp>();
    ctx.register_operator::<SelectionSelectOp>();
    ctx.register_operator::<SelectionExtendOp>();
    crate::canvas_snap::add_to_extension(ctx);
    // These three hang off this extension's own input context, not the core
    // one: an action belongs to the context instance on the entity registering
    // it, and a chord bound into a context on someone else's entity is
    // evaluated by nothing.
    ctx.bind_operator_host::<CanvasRulersOp>([PresetInput::key("KeyR").shift()]);
    ctx.bind_operator_host::<CanvasGuidesOp>([PresetInput::key("KeyG").shift()]);
    // Home frames the canvas the way it jumps the playhead in the timeline;
    // `viewport_2d_is_current` keeps the two apart.
    ctx.bind_operator_host::<Viewport2dFrameOp>([PresetInput::key("Home")]);

    crate::screenshot::add_2d_to_extension(ctx);
}

/// True while the 2D viewport is the panel the keys belong to: its tab is the
/// active one in some dock leaf, or the cursor is over one of its stages. Hover
/// counts as well, since a side-by-side canvas can be under the cursor without
/// being the focused tab.
fn viewport_2d_is_current(viewport: FrontedViewport) -> bool {
    viewport.is_two_d()
}

/// Which of the two viewports the keyboard belongs to. A `SystemParam` because
/// an availability gate is one system, and the operators that ask are already
/// asking something else.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct FrontedViewport<'w, 's> {
    hosts: Query<'w, 's, &'static Viewport2dPanelHost>,
    hover_map: Res<'w, HoverMap>,
    parents: Query<'w, 's, &'static ChildOf>,
    tree: Res<'w, jackdaw_panels::tree::DockTree>,
    contents: Query<'w, 's, (Entity, &'static jackdaw_panels::area::DockTabContent)>,
    viewports: Query<'w, 's, &'static crate::viewport_host::ViewportHost>,
}

impl FrontedViewport<'_, '_> {
    /// Whether the canvas is what the user is looking at.
    pub(crate) fn is_two_d(&self) -> bool {
        self.hosts
            .iter()
            .any(|host| entity_is_hovered(host.area, &self.hover_map, &self.parents))
            || focused_viewport_mode(
                &self.hover_map,
                &self.parents,
                &self.tree,
                &self.contents,
                &self.viewports,
            ) == Some(crate::viewport_host::ViewportMode::TwoD)
    }

    /// Whether the 3D world is what the user is looking at: anything but the
    /// canvas, so a workspace with no viewport open still answers yes.
    pub(crate) fn is_three_d(&self) -> bool {
        !self.is_two_d()
    }
}

/// True while the 3D world is the one the keyboard belongs to. The single-letter
/// transform and brush chords mean nothing on a UI canvas, where a user typing a
/// name with no field focused would otherwise arm one of them.
pub(crate) fn three_d_world_is_current(viewport: FrontedViewport) -> bool {
    viewport.is_three_d()
}

/// Which viewport panel a press belongs to, as the mode that panel is showing.
///
/// The answer has to name one panel, not one mode: a workspace can front a
/// viewport in each of two leaves, and asking each mode separately whether some
/// fronted viewport is showing it answers yes twice. So the viewport under the
/// cursor, else the fronted viewport when the workspace fronts only one, else
/// no panel at all.
pub(crate) fn focused_viewport_mode(
    hover_map: &HoverMap,
    parents: &Query<&ChildOf>,
    tree: &jackdaw_panels::tree::DockTree,
    contents: &Query<(Entity, &jackdaw_panels::area::DockTabContent)>,
    viewports: &Query<&crate::viewport_host::ViewportHost>,
) -> Option<crate::viewport_host::ViewportMode> {
    if let Some(mode) = hovered_viewport_mode(hover_map, parents, contents, viewports) {
        return Some(mode);
    }
    let mut fronted = fronted_viewport_hosts(tree, contents)
        .filter_map(|entity| viewports.get(entity).ok().map(|host| host.mode));
    let first = fronted.next()?;
    fronted.all(|mode| mode == first).then_some(first)
}

/// The mode of the viewport panel the cursor is inside, if it is inside one.
fn hovered_viewport_mode(
    hover_map: &HoverMap,
    parents: &Query<&ChildOf>,
    contents: &Query<(Entity, &jackdaw_panels::area::DockTabContent)>,
    viewports: &Query<&crate::viewport_host::ViewportHost>,
) -> Option<crate::viewport_host::ViewportMode> {
    hover_map
        .iter()
        .filter(|(pointer, _)| !pointer.is_custom())
        .flat_map(|(_, hits)| hits.keys())
        .find_map(|&hovered| {
            // The walk stops at the first panel it reaches: a hit inside some
            // other panel is not a hit inside the viewport beside it.
            core::iter::successors(Some(hovered), |entity| {
                parents.get(*entity).ok().map(ChildOf::parent)
            })
            .find_map(|ancestor| {
                contents
                    .contains(ancestor)
                    .then(|| viewports.get(ancestor).ok().map(|host| host.mode))
            })
            .flatten()
        })
}

/// The content entity of every viewport panel whose tab is the active one
/// in its dock leaf.
fn fronted_viewport_hosts<'a>(
    tree: &'a jackdaw_panels::tree::DockTree,
    contents: &'a Query<(Entity, &jackdaw_panels::area::DockTabContent)>,
) -> impl Iterator<Item = Entity> + 'a {
    tree.leaves()
        .filter_map(|(_, leaf)| {
            let active = leaf.active?;
            leaf.windows
                .iter()
                .find(|tab| tab.id == active)
                .filter(|tab| tab.window_id == crate::viewport::VIEWPORT_WINDOW_ID)
                .map(|tab| tab.id)
        })
        .flat_map(move |tab| {
            contents
                .iter()
                .filter(move |(_, content)| content.tab_id == tab)
                .map(|(entity, _)| entity)
        })
}

/// Frame the UI scene in the 2D viewport: zoom and pan so the whole canvas is in
/// view. Every open 2D panel is framed, an operator having no panel to be called
/// on.
#[operator(
    id = "viewport2d.frame",
    label = "Fit 2D View",
    description = "Zoom the 2D viewport so the whole UI scene is in view.",
    allows_undo = false,
    is_available = viewport_2d_is_current
)]
pub(crate) fn viewport_2d_frame(
    _params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(request_2d_fit);
    OperatorResult::Finished
}

/// Put the 2D viewport into Edit or Interact mode: the header's segmented
/// control as an operator, so a scripted run need not synthesise a click. Moves
/// every open panel, since an operator names none.
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

/// Set the canvas grid the 2D viewport snaps and nudges on, on every open panel.
/// A size off the power-of-two ladder is taken as given; the header's stepper
/// pulls it back onto the ladder from there. See [`stepped_ui_grid`].
#[operator(
    id = "viewport2d.grid",
    label = "Set 2D Canvas Grid",
    description = "Set the lattice the 2D viewport's gestures land on, in authored pixels.",
    allows_undo = false,
    params(
        size(f64, doc = "Grid size in authored pixels."),
        panel(
            i64,
            doc = "Bits of the panel content Entity to set. Omit to set every open 2D panel."
        )
    )
)]
pub(crate) fn viewport_2d_grid(
    params: In<OperatorParameters>,
    mut hosts: Query<(Entity, &mut Viewport2dPanelHost)>,
) -> OperatorResult {
    let size = params.as_float("size")? as f32;
    if !size.is_finite() || size <= 0.0 {
        warn!("viewport2d.grid: 'size' must be a positive number of authored pixels");
        return OperatorResult::Cancelled;
    }
    let open: Vec<Entity> = hosts.iter().map(|(entity, _)| entity).collect();
    if open.is_empty() {
        warn!("viewport2d.grid: no 2D viewport panel is open to set the grid on");
        return OperatorResult::Cancelled;
    }
    let wanted = named_panels(&params, "viewport2d.grid", &open);
    let grid = size.clamp(MIN_UI_GRID, MAX_UI_GRID);
    for (entity, mut host) in &mut hosts {
        if !wanted.contains(&entity) {
            continue;
        }
        let view = Ui2dView { grid, ..host.view };
        host.set_view(view);
    }
    OperatorResult::Finished
}

/// Set the 2D viewport's zoom, in stage logical pixels per authored pixel, on
/// one named panel or on every open one. A stated zoom stands a pending fit
/// down, which would otherwise land next frame over the framing just asked for.
#[operator(
    id = "viewport2d.zoom",
    label = "Set 2D Canvas Zoom",
    description = "Set the 2D viewport's zoom, in stage pixels per authored pixel.",
    allows_undo = false,
    params(
        zoom(f64, doc = "Stage logical pixels per authored pixel."),
        panel(
            i64,
            doc = "Bits of the panel content Entity to zoom. Omit to zoom every open 2D panel."
        )
    )
)]
pub(crate) fn viewport_2d_zoom(
    params: In<OperatorParameters>,
    mut hosts: Query<(Entity, &mut Viewport2dPanelHost)>,
) -> OperatorResult {
    let zoom = params.as_float("zoom")? as f32;
    if !zoom.is_finite() || zoom <= 0.0 {
        warn!(
            "viewport2d.zoom: 'zoom' must be a positive number of stage pixels per authored pixel"
        );
        return OperatorResult::Cancelled;
    }
    let open: Vec<Entity> = hosts.iter().map(|(entity, _)| entity).collect();
    if open.is_empty() {
        warn!("viewport2d.zoom: no 2D viewport panel is open to zoom");
        return OperatorResult::Cancelled;
    }
    let wanted = named_panels(&params, "viewport2d.zoom", &open);
    let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    for (entity, mut host) in &mut hosts {
        if !wanted.contains(&entity) {
            continue;
        }
        let view = Ui2dView { zoom, ..host.view };
        host.set_view(view);
        host.fit_pending = false;
    }
    OperatorResult::Finished
}

/// The panels of `open` a `panel` parameter names. A value naming no open panel
/// is the same answer as no parameter at all.
fn named_panels(params: &OperatorParameters, operator: &str, open: &[Entity]) -> Vec<Entity> {
    let Some(bits) = params.as_int("panel") else {
        return open.to_vec();
    };
    match Entity::try_from_bits(bits as u64).filter(|entity| open.contains(entity)) {
        Some(entity) => vec![entity],
        None => {
            warn!("{operator}: 'panel' names no open 2D panel; every one is used");
            open.to_vec()
        }
    }
}

/// The mode a `mode` parameter names, or `None` when it names neither.
fn parse_viewport_2d_mode(mode: &str) -> Option<Viewport2dMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "edit" => Some(Viewport2dMode::Edit),
        "interact" => Some(Viewport2dMode::Interact),
        _ => None,
    }
}

/// Select the authored entity with this name. Editor chrome is excluded, so the
/// name resolves against the authored scene alone, and an ambiguous name is
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

/// Add the authored entity with this name to the selection, which
/// `selection.select` would replace. The scripted stand-in for Ctrl-click.
#[operator(
    id = "selection.extend",
    label = "Add To Selection By Name",
    description = "Add the entity with this name to the current selection.",
    allows_undo = false,
    params(name(String, doc = "`Name` of the entity to add. Must match exactly one."))
)]
pub(crate) fn selection_extend(
    params: In<OperatorParameters>,
    named: Query<(Entity, &Name), Without<crate::EditorEntity>>,
    mut selection: ResMut<Selection>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(wanted) = params.as_str("name").filter(|name| !name.is_empty()) else {
        warn!("selection.extend: missing 'name' parameter");
        return OperatorResult::Cancelled;
    };
    let Some(entity) = crate::boot_ops::unique_named_entity(named.iter(), wanted) else {
        warn!("selection.extend: '{wanted}' names no entity in this scene, or more than one");
        return OperatorResult::Cancelled;
    };
    selection.extend(&mut commands, entity);
    OperatorResult::Finished
}

/// Marker on one Edit|Interact segment, naming the panel it drives, so a segment
/// in one panel's header never moves another panel's mode.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport2dModeSegment {
    pub host: Entity,
    pub mode: Viewport2dMode,
}

/// The two-segment Edit|Interact control. Not [`jackdaw_feathers::tab_strip`],
/// which spaces its tabs apart and dispatches a named operator: these segments
/// write per-panel state on a specific [`Viewport2dPanelHost`] entity, which no
/// operator parameter carries.
fn viewport_2d_mode_bar(host: Entity) -> impl Bundle {
    (
        segmented::segmented_bar(),
        observe(
            move |change: On<ValueChange<Entity>>,
                  segments: Query<&Viewport2dModeSegment>,
                  mut hosts: Query<&mut Viewport2dPanelHost>| {
                let Ok(segment) = segments.get(change.value) else {
                    return;
                };
                if let Ok(mut panel) = hosts.get_mut(host)
                    && panel.mode != segment.mode
                {
                    panel.mode = segment.mode;
                }
            },
        ),
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
        jackdaw_feathers::tooltip::Tooltip::title(tooltip),
        segmented::segment(label),
    )
}

/// Highlight the segment matching each panel's current mode.
fn update_viewport_2d_mode_bar(
    hosts: Query<&Viewport2dPanelHost>,
    mut segments: Query<(
        Entity,
        &Viewport2dModeSegment,
        &mut BackgroundColor,
        Has<Checked>,
    )>,
    mut commands: Commands,
) {
    for (entity, segment, mut background, checked) in &mut segments {
        let Ok(host) = hosts.get(segment.host) else {
            continue;
        };
        let active = host.mode == segment.mode;
        let color = segmented::segment_background(active);
        if background.0 != color {
            background.0 = color;
        }
        if checked != active {
            segmented::set_segment_checked(&mut commands, entity, active);
        }
    }
}

/// Despawn a 2D viewport panel's camera and its pointer when the panel content
/// entity goes away. The stage node is a descendant of the panel, so it is torn
/// down with it.
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
