//! Viewport and window screenshot capture.
//!
//! A viewport capture reads back the panel's existing render target; a window
//! capture takes the whole editor surface, `bevy_ui` chrome included.

use std::path::{Path, PathBuf};

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

use crate::prelude::*;
use crate::viewport::MainViewportCamera;
use crate::viewport_2d::{Ui2dView, Viewport2dPanelHost, world_to_target_pixel};
use crate::viewport_host::{ViewportHost, ViewportMode};

/// Names a PNG the editor writes once the viewport has settled, then exits.
pub const ENV_SHOT: &str = "JACKDAW_SHOT";

/// Frames to let pass after entering the editor before an unattended capture
/// fires; the first frames hold no meshes and the render target is still being
/// resized.
const SETTLE_FRAMES: u32 = 90;

/// Frames an unattended run keeps retrying a capture it could not start before
/// giving up and exiting non-zero.
const CAPTURE_TIMEOUT_FRAMES: u32 = 900;

pub(crate) fn plugin(app: &mut App) {
    if let Some(path) = std::env::var_os(ENV_SHOT) {
        app.insert_resource(ShotProbe {
            path: PathBuf::from(path),
            frames: 0,
            queued: false,
        });
    }
    app.init_resource::<CaptureLog>();
    app.add_systems(
        Update,
        drive_shot_probe.run_if(in_state(crate::AppState::Editor)),
    );
    crate::camera_capture::plugin(app);
}

/// Captures that have reached the disk, by the path they were written to.
///
/// A capture is queued frames before its GPU readback lands, so a caller that
/// needs the file polls here. Entries past [`CaptureLog::CAPACITY`] are
/// evicted oldest first.
#[derive(Resource, Default)]
pub struct CaptureLog {
    landed: bevy::platform::collections::HashMap<PathBuf, (u32, u32)>,
    /// Insertion order, for eviction. Holds the same paths as `landed`.
    order: std::collections::VecDeque<PathBuf>,
}

impl CaptureLog {
    /// Captures remembered before the oldest is dropped.
    pub const CAPACITY: usize = 64;

    /// Record a capture that has reached the disk.
    pub fn record(&mut self, path: PathBuf, size: (u32, u32)) {
        if self.landed.insert(path.clone(), size).is_none() {
            self.order.push_back(path);
        }
        while self.order.len() > Self::CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.landed.remove(&oldest);
            }
        }
    }

    /// The size of the capture written to `path`, if one has landed.
    pub fn size_of(&self, path: &Path) -> Option<(u32, u32)> {
        self.landed.get(path).copied()
    }

    /// Forget `path`, so a later capture there is not answered with this one.
    pub fn forget(&mut self, path: &Path) {
        if self.landed.remove(path).is_some() {
            self.order.retain(|held| held != path);
        }
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ViewportScreenshotOp>();
    ctx.register_menu_entry::<ViewportScreenshotOp>(TopLevelMenu::Tools);
    ctx.register_operator::<WindowScreenshotOp>();
    ctx.register_menu_entry::<WindowScreenshotOp>(TopLevelMenu::Tools);
}

/// The 2D presentation's own capture, registered only where a 2D viewport
/// exists.
pub(crate) fn add_2d_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<Viewport2dScreenshotOp>();
    ctx.register_menu_entry::<Viewport2dScreenshotOp>(TopLevelMenu::Tools);
}

/// The pending one-shot capture requested by [`ENV_SHOT`].
#[derive(Resource)]
struct ShotProbe {
    path: PathBuf,
    frames: u32,
    queued: bool,
}

/// Why a capture could not be started.
#[derive(Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// No viewport panel is open, so there is no camera to capture.
    NoViewport,
    /// A viewport camera exists but does not render to an image.
    NotAnImageTarget,
    /// A world-space region was asked of a capture that is not a 2D view.
    WorldRegionWithoutView,
    /// A world-space region was asked of a UI scene's canvas, which has no
    /// world coordinates.
    WorldRegionOnUiScene,
    /// The region lies wholly outside the captured image.
    EmptyRegion,
}

impl core::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoViewport => write!(f, "no viewport panel is open to capture"),
            Self::NotAnImageTarget => {
                write!(f, "the viewport camera does not render to an image")
            }
            Self::WorldRegionWithoutView => write!(
                f,
                "a world region needs a 2D viewport; use the pixels space for this capture"
            ),
            Self::WorldRegionOnUiScene => write!(
                f,
                "a world region needs a 2D world scene, not a UI scene; use the pixels space \
                 for this capture"
            ),
            Self::EmptyRegion => write!(f, "the region lies outside the captured image"),
        }
    }
}

impl core::error::Error for CaptureError {}

/// The coordinates a [`CaptureRegion`] is measured in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionSpace {
    /// World units, y up, resolved through the 2D view the capture shows.
    World,
    /// Image pixels from the top-left corner, y down.
    Pixels,
}

impl RegionSpace {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "world" => Some(Self::World),
            "pixels" => Some(Self::Pixels),
            _ => None,
        }
    }
}

/// The part of a capture to keep. `rect` runs from its minimum corner: the
/// lower-left in world space (y up), the top-left in pixels (y down). A
/// region with no `space` is in world units when the capture shows a 2D view
/// and in pixels otherwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureRegion {
    pub rect: Rect,
    pub space: Option<RegionSpace>,
}

/// What a capture shows, which decides the coordinates a region can use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CaptureSurface {
    /// A 3D view or the whole window: image pixels only.
    Plain,
    /// A 2D world scene seen through `view`, drawn into a `target`-sized image.
    World2d { view: Ui2dView, target: UVec2 },
    /// A UI scene's canvas: the authored pixels, whatever the pan and zoom.
    UiScene,
}

impl CaptureSurface {
    fn of_panel(host: &Viewport2dPanelHost) -> Self {
        if host.shows_ui_scene {
            Self::UiScene
        } else {
            Self::World2d {
                view: host.view,
                target: host.target_size,
            }
        }
    }
}

impl CaptureRegion {
    /// A region from a minimum corner and a size; the size has to be finite
    /// and positive.
    pub fn new(x: f64, y: f64, w: f64, h: f64, space: Option<RegionSpace>) -> Result<Self, String> {
        let finite = [x, y, w, h].iter().all(|v| v.is_finite());
        if !finite || w <= 0.0 || h <= 0.0 {
            return Err("a region needs finite x and y and a positive w and h".to_string());
        }
        let min = Vec2::new(x as f32, y as f32);
        Ok(Self {
            rect: Rect::from_corners(min, min + Vec2::new(w as f32, h as f32)),
            space,
        })
    }

    /// The one parse every entry point goes through: `None` when no part of a
    /// region was given, an error when the parts are incomplete or the space
    /// is not `world` or `pixels`.
    pub fn from_values(
        [x, y, w, h]: [Option<f64>; 4],
        space: Option<&str>,
    ) -> Result<Option<Self>, String> {
        if [x, y, w, h].iter().all(Option::is_none) && space.is_none() {
            return Ok(None);
        }
        let (Some(x), Some(y), Some(w), Some(h)) = (x, y, w, h) else {
            return Err("a region needs x, y, w and h".to_string());
        };
        let space = space
            .map(|text| {
                RegionSpace::parse(text)
                    .ok_or_else(|| "the region space must be `world` or `pixels`".to_string())
            })
            .transpose()?;
        Self::new(x, y, w, h, space).map(Some)
    }

    /// The region an operator was given as `region_x`, `region_y`, `region_w`,
    /// `region_h` and `region_space`.
    pub fn from_params(params: &OperatorParameters) -> Result<Option<Self>, String> {
        Self::from_values(
            ["region_x", "region_y", "region_w", "region_h"].map(|key| params.as_float(key)),
            params.as_str("region_space"),
        )
    }

    /// The region a remote request carries as `{x, y, w, h, space}`.
    pub fn from_json(region: &serde_json::Value) -> Result<Self, String> {
        let space = match region.get("space") {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => Some(
                value
                    .as_str()
                    .ok_or_else(|| "the region space must be `world` or `pixels`".to_string())?,
            ),
        };
        Self::from_values(
            ["x", "y", "w", "h"].map(|key| region.get(key).and_then(serde_json::Value::as_f64)),
            space,
        )?
        .ok_or_else(|| "a region needs x, y, w and h".to_string())
    }

    /// The pixels of an `image`-sized capture of `surface` this region keeps.
    /// Edges are rounded to whole pixels, keeping at least one.
    pub(crate) fn resolve(
        self,
        surface: CaptureSurface,
        image: UVec2,
    ) -> Result<URect, CaptureError> {
        let space = self.space.unwrap_or(match surface {
            CaptureSurface::World2d { .. } => RegionSpace::World,
            CaptureSurface::Plain | CaptureSurface::UiScene => RegionSpace::Pixels,
        });
        let (min, max) = match space {
            RegionSpace::Pixels => (self.rect.min, self.rect.max),
            RegionSpace::World => match surface {
                CaptureSurface::World2d { view, target } => {
                    let top_left = world_to_target_pixel(
                        view,
                        target,
                        Vec2::new(self.rect.min.x, self.rect.max.y),
                    );
                    let bottom_right = world_to_target_pixel(
                        view,
                        target,
                        Vec2::new(self.rect.max.x, self.rect.min.y),
                    );
                    (top_left, bottom_right)
                }
                CaptureSurface::Plain => return Err(CaptureError::WorldRegionWithoutView),
                CaptureSurface::UiScene => return Err(CaptureError::WorldRegionOnUiScene),
            },
        };
        let low = min.round().as_ivec2();
        let high = max.round().as_ivec2().max(low + IVec2::ONE);
        let lo = low.max(IVec2::ZERO);
        let hi = high.min(image.min(UVec2::splat(i32::MAX as u32)).as_ivec2());
        if hi.x <= lo.x || hi.y <= lo.y {
            Err(CaptureError::EmptyRegion)
        } else {
            Ok(URect {
                min: lo.as_uvec2(),
                max: hi.as_uvec2(),
            })
        }
    }
}

/// A region waiting on its frame, with the panel whose view it is read
/// through. The region is resolved when the frame lands, against the view the
/// panel has then.
#[derive(Clone, Copy)]
struct PendingCrop {
    region: CaptureRegion,
    panel: Option<Entity>,
}

/// Queue a capture of what the first viewport panel is showing, written to
/// `path` as a PNG once the GPU readback lands.
///
/// The surface follows the panel's mode: the world in
/// [`ViewportMode::ThreeD`], the canvas in [`ViewportMode::TwoD`]. With
/// `exit_when_done`, the app exits once the file is written, reporting
/// failure when it did not land.
pub fn queue_capture(
    world: &mut World,
    path: PathBuf,
    exit_when_done: bool,
    region: Option<CaptureRegion>,
) -> Result<(), CaptureError> {
    match viewport_camera(world) {
        Some(camera) => queue_capture_of(world, camera, path, exit_when_done, region),
        None => queue_camera_capture::<MainViewportCamera>(world, path, exit_when_done, region),
    }
}

/// The camera the first viewport panel is showing through.
pub(crate) fn viewport_camera(world: &mut World) -> Option<Entity> {
    let mut panels = world.query::<(Entity, &ViewportHost)>();
    let (panel, mode) = panels.iter(world).next().map(|(e, host)| (e, host.mode))?;
    match mode {
        ViewportMode::ThreeD => world
            .get::<crate::viewport::ViewportPanelHost>(panel)
            .map(|host| host.camera),
        ViewportMode::TwoD => world
            .get::<crate::viewport_2d::Viewport2dPanelHost>(panel)
            .map(|host| host.camera),
    }
}

/// Queue a capture of a 2D viewport panel's render target: the authored UI
/// scene at its reference resolution, without the panel chrome around it.
pub fn queue_2d_capture(
    world: &mut World,
    path: PathBuf,
    exit_when_done: bool,
    region: Option<CaptureRegion>,
) -> Result<(), CaptureError> {
    let camera = primary_2d_camera(world).ok_or(CaptureError::NoViewport)?;
    queue_capture_of(world, camera, path, exit_when_done, region)
}

/// The 2D camera of the panel answering for the canvas.
fn primary_2d_camera(world: &mut World) -> Option<Entity> {
    let mut panels = world.query::<(Entity, &ViewportHost)>();
    let panel = crate::viewport_host::primary_2d_host(panels.iter(world))?;
    world
        .get::<crate::viewport_2d::Viewport2dPanelHost>(panel)
        .map(|host| host.camera)
}

/// Queue a capture of the render target belonging to the first camera marked
/// `C`.
fn queue_camera_capture<C: Component>(
    world: &mut World,
    path: PathBuf,
    exit_when_done: bool,
    region: Option<CaptureRegion>,
) -> Result<(), CaptureError> {
    let mut cameras = world.query_filtered::<Entity, With<C>>();
    let camera = cameras.iter(world).next().ok_or(CaptureError::NoViewport)?;
    queue_capture_of(world, camera, path, exit_when_done, region)
}

/// Queue a capture of `camera`'s render target, which has to be an image; a
/// camera drawing straight to the window has no texture to read back.
///
/// A `region` is checked here, against the view and image the camera has now,
/// so a refusal reaches the caller; it is resolved again when the frame lands.
fn queue_capture_of(
    world: &mut World,
    camera: Entity,
    path: PathBuf,
    exit_when_done: bool,
    region: Option<CaptureRegion>,
) -> Result<(), CaptureError> {
    let handle = world
        .get::<RenderTarget>(camera)
        .ok_or(CaptureError::NoViewport)?
        .as_image()
        .cloned()
        .ok_or(CaptureError::NotAnImageTarget)?;

    let pending = match region {
        Some(region) => {
            let panel = world
                .query::<(Entity, &Viewport2dPanelHost)>()
                .iter(world)
                .find(|(_, host)| host.camera == camera)
                .map(|(panel, host)| (panel, CaptureSurface::of_panel(host)));
            let surface = panel.map_or(CaptureSurface::Plain, |(_, surface)| surface);
            let image = world
                .get_resource::<Assets<Image>>()
                .and_then(|images| images.get(&handle))
                .map(Image::size)
                .or(match surface {
                    CaptureSurface::World2d { target, .. } => Some(target),
                    CaptureSurface::Plain | CaptureSurface::UiScene => None,
                })
                .ok_or(CaptureError::NotAnImageTarget)?;
            region.resolve(surface, image)?;
            Some(PendingCrop {
                region,
                panel: panel.map(|(panel, _)| panel),
            })
        }
        None => None,
    };
    spawn_capture(
        world,
        Screenshot::image(handle),
        path,
        exit_when_done,
        pending,
    );
    Ok(())
}

/// Queue a capture of the whole primary window, written to `path` as a PNG
/// once the GPU readback lands. A `region` can only be in pixels here.
///
/// With no primary window the observer never fires.
pub fn queue_window_capture(
    world: &mut World,
    path: PathBuf,
    exit_when_done: bool,
    region: Option<CaptureRegion>,
) -> Result<(), CaptureError> {
    let pending = match region {
        Some(region) => {
            let size = world
                .query_filtered::<&Window, With<bevy::window::PrimaryWindow>>()
                .iter(world)
                .next()
                .map_or(UVec2::splat(i32::MAX as u32), Window::physical_size);
            region.resolve(CaptureSurface::Plain, size)?;
            Some(PendingCrop {
                region,
                panel: None,
            })
        }
        None => None,
    };
    spawn_capture(
        world,
        Screenshot::primary_window(),
        path,
        exit_when_done,
        pending,
    );
    Ok(())
}

/// Spawn the [`Screenshot`] entity shared by every capture path. A `crop`
/// keeps only that part of the frame.
fn spawn_capture(
    world: &mut World,
    screenshot: Screenshot,
    path: PathBuf,
    exit_when_done: bool,
    crop: Option<PendingCrop>,
) {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = std::fs::create_dir_all(parent);
    }

    world.spawn(screenshot).observe(
        move |capture: On<ScreenshotCaptured>,
              mut exit: MessageWriter<AppExit>,
              log: Option<ResMut<CaptureLog>>,
              panels: Query<&Viewport2dPanelHost>| {
            // Writing here rather than through bevy's `save_to_disk` keeps
            // write-then-exit in one observer; two observers on an entity have
            // no ordering guarantee between them.
            let kept = crop.map(|pending| {
                let surface = pending
                    .panel
                    .and_then(|panel| panels.get(panel).ok())
                    .map_or(CaptureSurface::Plain, CaptureSurface::of_panel);
                pending.region.resolve(surface, capture.image.size())
            });
            let wrote = match kept {
                Some(Err(err)) => {
                    error!("screenshot: {err}");
                    None
                }
                Some(Ok(rect)) => write_png_cropped(&capture.image, &path, Some(rect)),
                None => write_png_cropped(&capture.image, &path, None),
            };
            if let Some(size) = wrote
                && let Some(mut log) = log
            {
                log.record(path.clone(), size);
            }
            if exit_when_done {
                exit.write(if wrote.is_some() {
                    AppExit::Success
                } else {
                    AppExit::error()
                });
            }
        },
    );
}

/// Encode a captured frame as a PNG on disk. Returns whether it landed.
pub(crate) fn write_png(image: &Image, path: &Path) -> bool {
    write_png_cropped(image, path, None).is_some()
}

/// Encode a captured frame as a PNG on disk, keeping only `crop` when given.
/// Returns the size written, or `None` when nothing landed.
pub(crate) fn write_png_cropped(
    image: &Image,
    path: &Path,
    crop: Option<URect>,
) -> Option<(u32, u32)> {
    let dynamic = match image.clone().try_into_dynamic() {
        Ok(dynamic) => dynamic,
        Err(err) => {
            error!("screenshot: cannot convert the captured frame: {err}");
            return None;
        }
    };
    let kept = match crop {
        Some(crop) => {
            let kept = crop.intersect(URect::new(0, 0, dynamic.width(), dynamic.height()));
            if kept.is_empty() {
                error!("screenshot: the region lies outside the captured frame");
                return None;
            }
            dynamic.crop_imm(kept.min.x, kept.min.y, kept.width(), kept.height())
        }
        None => dynamic,
    };
    // Drop alpha: with HDR on it carries brightness rather than opacity.
    let rgb = kept.to_rgb8();
    match rgb.save_with_format(path, ::image::ImageFormat::Png) {
        Ok(()) => {
            info!("screenshot: wrote {}", path.display());
            Some((rgb.width(), rgb.height()))
        }
        Err(err) => {
            error!("screenshot: cannot write {}: {err}", path.display());
            None
        }
    }
}

/// Encode a captured frame as a PNG on disk, cropped to the square at its
/// centre and scaled to `edge` pixels. Returns whether it landed.
pub(crate) fn write_scaled_png(image: &Image, path: &Path, edge: u32) -> bool {
    let dynamic = match image.clone().try_into_dynamic() {
        Ok(dynamic) => dynamic,
        Err(err) => {
            error!("screenshot: cannot convert the captured frame: {err}");
            return false;
        }
    };
    let mut rgb = dynamic.to_rgb8();
    let side = rgb.width().min(rgb.height());
    if side == 0 {
        return false;
    }
    let (x, y) = ((rgb.width() - side) / 2, (rgb.height() - side) / 2);
    let square = ::image::imageops::crop(&mut rgb, x, y, side, side).to_image();
    let scaled = ::image::imageops::resize(
        &square,
        edge.max(1),
        edge.max(1),
        ::image::imageops::FilterType::Triangle,
    );
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        error!("screenshot: cannot make {}: {err}", parent.display());
        return false;
    }
    match scaled.save_with_format(path, ::image::ImageFormat::Png) {
        Ok(()) => true,
        Err(err) => {
            error!("screenshot: cannot write {}: {err}", path.display());
            false
        }
    }
}

/// Count settle frames for an unattended run, then fire exactly one capture,
/// retrying until `CAPTURE_TIMEOUT_FRAMES` runs out.
fn drive_shot_probe(world: &mut World) {
    let Some(probe) = world.get_resource::<ShotProbe>() else {
        return;
    };
    if probe.queued {
        return;
    }
    let frames = probe.frames + 1;
    world.resource_mut::<ShotProbe>().frames = frames;
    if frames < SETTLE_FRAMES {
        return;
    }

    let path = world.resource::<ShotProbe>().path.clone();
    match queue_capture(world, path, true, None) {
        Ok(()) => world.resource_mut::<ShotProbe>().queued = true,
        Err(err) if frames > SETTLE_FRAMES + CAPTURE_TIMEOUT_FRAMES => {
            error!("{ENV_SHOT}: {err}");
            // `queued` doubles as "nothing left to do", so latching it keeps
            // a failed capture from re-emitting AppExit every frame.
            world.resource_mut::<ShotProbe>().queued = true;
            world.write_message(AppExit::error());
        }
        Err(_) => {}
    }
}

/// Capture the viewport to a PNG: the world it is showing, or the canvas.
#[operator(
    id = "viewport.screenshot",
    label = "Screenshot Viewport",
    description = "Save what the viewport is showing to a PNG file, or the part of it named \
                   by the region values.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Where to write the PNG. Defaults to a timestamped file in the project."
        ),
        region_x(f64, doc = "Left (pixels) or minimum x (world) of the part to keep."),
        region_y(
            f64,
            doc = "Top (pixels, y down) or minimum y (world, y up) of the part to keep."
        ),
        region_w(
            f64,
            doc = "Width of the part to keep. All four region values go together."
        ),
        region_h(f64, doc = "Height of the part to keep."),
        region_space(
            String,
            doc = "`pixels` (image pixels from the top-left) is the only choice for a 3D view or \
                   a UI scene. `world` (the 2D scene's units, y up) is the default when the \
                   viewport shows a 2D world scene. The region is clamped to the image and refused when nothing is left."
        )
    )
)]
pub(crate) fn viewport_screenshot(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    run_capture(
        "viewport.screenshot",
        "viewport",
        &params,
        world,
        queue_capture,
    )
}

/// Capture what the 2D viewport is showing to a PNG: the authored scene
/// alone, without the stage chrome drawn over it.
#[operator(
    id = "viewport2d.screenshot",
    label = "Screenshot 2D Viewport",
    description = "Save what the 2D viewport is showing to a PNG file, without the selection \
                   chrome, or the part of it named by the region values; `window.screenshot` \
                   includes the chrome.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Where to write the PNG. Defaults to a timestamped file in the project."
        ),
        region_x(f64, doc = "Left (pixels) or minimum x (world) of the part to keep."),
        region_y(
            f64,
            doc = "Top (pixels, y down) or minimum y (world, y up) of the part to keep."
        ),
        region_w(
            f64,
            doc = "Width of the part to keep. All four region values go together."
        ),
        region_h(f64, doc = "Height of the part to keep."),
        region_space(
            String,
            doc = "`world` (the 2D scene's units, y up) is the default for a 2D world scene. \
                   `pixels` (image pixels from the top-left) is the only choice for a UI scene. The region is clamped to the image and refused when nothing is left."
        )
    )
)]
pub(crate) fn viewport_2d_screenshot(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    run_capture(
        "viewport2d.screenshot",
        "viewport-2d",
        &params,
        world,
        queue_2d_capture,
    )
}

/// Parse the region, queue the capture and cancel with a warning to the caller
/// when either is refused.
fn run_capture(
    id: &str,
    prefix: &str,
    params: &OperatorParameters,
    world: &mut World,
    queue: fn(&mut World, PathBuf, bool, Option<CaptureRegion>) -> Result<(), CaptureError>,
) -> OperatorResult {
    let path = match params.as_str("path").filter(|p| !p.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => default_shot_path(world.get_resource::<crate::project::ProjectRoot>(), prefix),
    };
    let region = match CaptureRegion::from_params(params) {
        Ok(region) => region,
        Err(problem) => {
            warn_caller(world, format!("{id}: {problem}"));
            return OperatorResult::Cancelled;
        }
    };
    match queue(world, path, false, region) {
        Ok(()) => OperatorResult::Finished,
        Err(err) => {
            warn_caller(world, format!("{id}: {err}"));
            OperatorResult::Cancelled
        }
    }
}

/// Capture the whole editor window to a PNG, `bevy_ui` chrome included.
#[operator(
    id = "window.screenshot",
    label = "Screenshot Window",
    description = "Save the whole editor window, including all UI panels, to a PNG file.",
    allows_undo = false,
    params(path(
        String,
        doc = "Where to write the PNG. Defaults to a timestamped file in the project."
    ))
)]
pub(crate) fn window_screenshot(
    params: In<OperatorParameters>,
    project: Option<Res<crate::project::ProjectRoot>>,
    mut commands: Commands,
) -> OperatorResult {
    let path = match params.as_str("path").filter(|p| !p.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => default_shot_path(project.as_deref(), "window"),
    };
    commands.queue(move |world: &mut World| {
        if let Err(err) = queue_window_capture(world, path, false, None) {
            warn_caller(world, format!("window.screenshot: {err}"));
        }
    });
    OperatorResult::Finished
}

/// Where an unnamed capture goes: `screenshots/<prefix>-<timestamp>.png` under
/// the open project, or under the working directory.
fn default_shot_path(project: Option<&crate::project::ProjectRoot>, prefix: &str) -> PathBuf {
    let stamp = crate::timestamps::utc_rfc3339_now().replace(':', "-");
    let dir = project
        .map(|p| p.root.join("screenshots"))
        .unwrap_or_else(|| PathBuf::from("screenshots"));
    dir.join(format!("{prefix}-{stamp}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capturing_without_a_viewport_reports_no_viewport() {
        let mut world = World::new();
        let err = queue_capture(&mut world, PathBuf::from("unused.png"), false, None)
            .expect_err("no viewport camera exists");
        assert_eq!(err, CaptureError::NoViewport);
    }

    #[test]
    fn capturing_a_window_target_reports_not_an_image_target() {
        let mut world = World::new();
        world.spawn((
            MainViewportCamera,
            RenderTarget::Window(bevy::window::WindowRef::Primary),
        ));
        let err = queue_capture(&mut world, PathBuf::from("unused.png"), false, None)
            .expect_err("the target is a window");
        assert_eq!(err, CaptureError::NotAnImageTarget);
    }

    #[test]
    fn default_path_is_a_png_under_the_project() {
        let root = PathBuf::from("/tmp/some-project");
        let project = crate::project::ProjectRoot {
            root: root.clone(),
            config: crate::project::ProjectConfig::default(),
        };
        let path = default_shot_path(Some(&project), "viewport");
        assert!(path.starts_with(root.join("screenshots")), "{path:?}");
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
        // Colons are illegal in Windows paths.
        assert!(!path.to_string_lossy().contains(':'), "{path:?}");
    }

    #[test]
    fn a_capture_that_never_starts_emits_app_exit_only_once() {
        let mut world = World::new();
        world.init_resource::<bevy::ecs::message::Messages<AppExit>>();
        world.insert_resource(ShotProbe {
            path: PathBuf::from("unused.png"),
            frames: SETTLE_FRAMES + CAPTURE_TIMEOUT_FRAMES,
            queued: false,
        });

        for _ in 0..5 {
            drive_shot_probe(&mut world);
        }

        assert_eq!(
            world
                .resource::<bevy::ecs::message::Messages<AppExit>>()
                .len(),
            1,
            "AppExit must be written exactly once, not once per frame",
        );
        assert!(
            world.resource::<ShotProbe>().queued,
            "must latch so drive_shot_probe stops retrying",
        );
    }

    const IMAGE: UVec2 = UVec2::new(100, 100);

    fn world_view() -> CaptureSurface {
        CaptureSurface::World2d {
            view: Ui2dView::default(),
            target: IMAGE,
        }
    }

    fn region(x: f64, y: f64, w: f64, h: f64, space: Option<RegionSpace>) -> CaptureRegion {
        CaptureRegion::new(x, y, w, h, space).expect("a valid region")
    }

    #[test]
    fn a_world_region_resolves_through_the_view_with_y_flipped() {
        let centred = region(-10.0, -10.0, 20.0, 20.0, Some(RegionSpace::World));
        assert_eq!(
            centred.resolve(world_view(), IMAGE),
            Ok(URect::new(40, 40, 60, 60)),
        );

        // World y runs up, so a region above the origin sits in the top of the image.
        let above = region(0.0, 20.0, 10.0, 10.0, Some(RegionSpace::World));
        assert_eq!(
            above.resolve(world_view(), IMAGE),
            Ok(URect::new(50, 20, 60, 30)),
        );
    }

    #[test]
    fn a_world_region_follows_the_pan_and_zoom() {
        let zoomed = CaptureSurface::World2d {
            view: Ui2dView {
                pan: Vec2::new(10.0, 0.0),
                zoom: 2.0,
                ..default()
            },
            target: IMAGE,
        };
        let region = region(10.0, -5.0, 10.0, 10.0, Some(RegionSpace::World));
        assert_eq!(
            region.resolve(zoomed, IMAGE),
            Ok(URect::new(50, 40, 70, 60))
        );
    }

    #[test]
    fn a_pixel_aligned_world_rect_on_an_odd_target_keeps_its_exact_size() {
        let odd = UVec2::new(343, 693);
        let surface = CaptureSurface::World2d {
            view: Ui2dView::default(),
            target: odd,
        };
        let sprite = region(-96.0, -88.0, 192.0, 176.0, Some(RegionSpace::World));
        let kept = sprite.resolve(surface, odd).expect("inside the image");
        assert_eq!((kept.width(), kept.height()), (192, 176));

        let zoomed = CaptureSurface::World2d {
            view: Ui2dView {
                zoom: 2.0,
                ..default()
            },
            target: odd,
        };
        let half = region(-48.0, -44.0, 96.0, 88.0, Some(RegionSpace::World));
        let kept = half.resolve(zoomed, odd).expect("inside the image");
        assert_eq!((kept.width(), kept.height()), (192, 176));
    }

    #[test]
    fn a_region_narrower_than_a_pixel_keeps_one() {
        let sliver = region(10.2, 10.2, 0.1, 0.1, Some(RegionSpace::Pixels));
        assert_eq!(
            sliver.resolve(CaptureSurface::Plain, IMAGE),
            Ok(URect::new(10, 10, 11, 11)),
        );
    }

    #[test]
    fn a_region_is_clamped_to_the_image() {
        let partial = region(40.0, -10.0, 30.0, 20.0, Some(RegionSpace::World));
        assert_eq!(
            partial.resolve(world_view(), IMAGE),
            Ok(URect::new(90, 40, 100, 60)),
        );

        let hanging = region(-20.0, 90.0, 40.0, 40.0, Some(RegionSpace::Pixels));
        assert_eq!(
            hanging.resolve(CaptureSurface::Plain, IMAGE),
            Ok(URect::new(0, 90, 20, 100)),
        );
    }

    #[test]
    fn a_region_with_nothing_inside_the_image_is_refused() {
        let outside = region(200.0, 0.0, 10.0, 10.0, Some(RegionSpace::World));
        assert_eq!(
            outside.resolve(world_view(), IMAGE),
            Err(CaptureError::EmptyRegion),
        );

        let behind = region(-50.0, 10.0, 20.0, 20.0, Some(RegionSpace::Pixels));
        assert_eq!(
            behind.resolve(CaptureSurface::Plain, IMAGE),
            Err(CaptureError::EmptyRegion),
        );
    }

    #[test]
    fn the_default_space_follows_the_surface() {
        let bare = region(10.0, 10.0, 20.0, 20.0, None);
        assert_eq!(
            bare.resolve(CaptureSurface::Plain, IMAGE),
            Ok(URect::new(10, 10, 30, 30)),
        );
        assert_eq!(
            bare.resolve(CaptureSurface::UiScene, IMAGE),
            Ok(URect::new(10, 10, 30, 30)),
        );
        assert_eq!(
            bare.resolve(world_view(), IMAGE),
            Ok(URect::new(60, 20, 80, 40)),
        );
    }

    #[test]
    fn a_world_region_is_refused_off_a_2d_world_scene() {
        let world = region(0.0, 0.0, 10.0, 10.0, Some(RegionSpace::World));
        assert_eq!(
            world.resolve(CaptureSurface::Plain, IMAGE),
            Err(CaptureError::WorldRegionWithoutView),
        );
        assert_eq!(
            world.resolve(CaptureSurface::UiScene, IMAGE),
            Err(CaptureError::WorldRegionOnUiScene),
        );
    }

    #[test]
    fn a_region_needs_finite_values_and_a_positive_size() {
        assert!(CaptureRegion::new(0.0, 0.0, 0.0, 5.0, None).is_err());
        assert!(CaptureRegion::new(0.0, 0.0, 5.0, -1.0, None).is_err());
        assert!(CaptureRegion::new(f64::NAN, 0.0, 5.0, 5.0, None).is_err());
    }

    #[test]
    fn operator_and_remote_regions_follow_the_same_rules() {
        let mut params = OperatorParameters::default();
        assert_eq!(CaptureRegion::from_params(&params), Ok(None));

        params.insert("region_x".to_string(), 1.0.into());
        assert!(CaptureRegion::from_params(&params).is_err());

        params.insert("region_y".to_string(), 2.0.into());
        params.insert("region_w".to_string(), 3.0.into());
        params.insert("region_h".to_string(), 4.0.into());
        params.insert("region_space".to_string(), "pixels".into());
        let expected = region(1.0, 2.0, 3.0, 4.0, Some(RegionSpace::Pixels));
        assert_eq!(CaptureRegion::from_params(&params), Ok(Some(expected)));
        let json = serde_json::json!({ "x": 1, "y": 2, "w": 3, "h": 4, "space": "pixels" });
        assert_eq!(CaptureRegion::from_json(&json), Ok(expected));

        params.insert("region_space".to_string(), "screen".into());
        assert!(CaptureRegion::from_params(&params).is_err());
        params.insert("region_space".to_string(), "".into());
        assert!(CaptureRegion::from_params(&params).is_err());
        let blank = serde_json::json!({ "x": 1, "y": 2, "w": 3, "h": 4, "space": "" });
        assert!(CaptureRegion::from_json(&blank).is_err());
    }

    #[test]
    fn a_space_without_numbers_is_an_error_on_both_paths() {
        let mut params = OperatorParameters::default();
        params.insert("region_space".to_string(), "pixels".into());
        assert!(CaptureRegion::from_params(&params).is_err());
        assert!(CaptureRegion::from_json(&serde_json::json!({ "space": "pixels" })).is_err());
        assert!(CaptureRegion::from_json(&serde_json::json!({})).is_err());
    }
}
