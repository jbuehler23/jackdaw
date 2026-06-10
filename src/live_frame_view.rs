//! Composites the streamed game frame behind the Live viewport and hides the
//! mirror's solid geometry so the real render shows through.
//!
//! The viewport's 3D camera renders into an image displayed by the panel's
//! `ViewportNode`. While the game view is active, a 2D backdrop camera at a
//! lower order draws the streamed frame into that same image and the 3D
//! camera stops clearing, so its remaining content (gizmos, overlays)
//! composites on top of the frame. Picking keeps working while meshes are
//! hidden because the select raycast casts against hidden meshes
//! (`RayCastVisibility::Any` in `viewport_select`).
//!
//! While the view is active the viewport camera is also locked to the
//! mirrored game rig's pose (and projection when streamed), so rays cast
//! through the displayed frame hit the mirror where the game camera sees
//! the same geometry. The camera's free-flight pose and projection are
//! stashed on entry and restored on exit.

use std::time::Instant;

use bevy::camera::{RenderTarget, visibility::RenderLayers};
use bevy::prelude::*;
#[cfg(feature = "camera_rig")]
use jackdaw_camera_rig::{ActiveCameraRig, CameraRig};
use jackdaw_pie_protocol::ControlEvent;

use crate::live_frame::LiveFrameStream;
use crate::pie::InstanceKey;
use crate::pie_mirror::PieViewMode;
use crate::viewport::{ActiveViewport, MainViewportCamera, ViewportGrid};

/// Render layer reserved for the frame backdrop camera + sprite. Layer 1 is
/// the material preview and layers 2+ are handed out to per-viewport grids,
/// so the backdrop sits well clear of both.
const BACKDROP_LAYER: usize = 30;

/// Which camera the Live viewport uses.
///
/// `Game` locks the viewport to the game camera and shows the streamed frame
/// as the background; `Free` flies freely over the mirror.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveCameraMode {
    #[default]
    Game,
    Free,
}

/// How long a candidate size must hold steady before it is requested, so
/// panel drags do not spam stream restarts.
const RESIZE_DEBOUNCE_MS: u128 = 250;
/// Ignore size changes smaller than this; the game rounds width up anyway.
const RESIZE_DEAD_ZONE: u32 = 32;
/// Minimum gap between restart attempts while the stream stays stale, so a
/// dead game-side capture (rig despawn on respawn or zone change) is
/// re-requested without flooding the channel.
const RESTART_BACKOFF_MS: u128 = 2000;

/// Size last requested from the focused game, and the debounce window for
/// resize requests.
#[derive(Resource, Default)]
struct StreamRequest {
    /// Size of the last `StartFrameStream` sent; `None` while stopped.
    requested: Option<UVec2>,
    /// Candidate size waiting out the debounce window before it is sent.
    pending: Option<(UVec2, Instant)>,
    /// Instance the bookkeeping refers to; a focus change resets it so the
    /// new instance gets its own start request.
    last_focus: Option<InstanceKey>,
    /// When the last `StartFrameStream` went out; gates stale-stream
    /// restarts to one per [`RESTART_BACKOFF_MS`] window.
    last_start: Option<Instant>,
}

/// Marker for the 2D camera that draws the streamed frame into the viewport
/// camera's render image, beneath the 3D render.
#[derive(Component)]
pub struct FrameBackdropCamera;

/// Marker for the sprite displaying the streamed frame.
#[derive(Component)]
pub struct FrameBackdropSprite;

/// Remembers the visibility an entity had before the game view hid it, so
/// leaving the view restores it exactly.
#[derive(Component)]
pub struct HiddenForGameView(pub Visibility);

/// The viewport camera's clear config, stashed while the game view overrides
/// it with [`ClearColorConfig::None`], and the camera it came from.
#[derive(Resource)]
pub struct StoredClearColor {
    pub camera: Entity,
    pub clear_color: ClearColorConfig,
}

/// The viewport camera's free-flight pose and projection, stashed while the
/// game view locks the camera to the game rig and restored when the view
/// exits.
#[derive(Resource)]
pub struct StoredFreePose {
    pub pose: Transform,
    pub projection: Option<Projection>,
}

/// True when the Live viewport should show the game's own render: Live view,
/// camera locked to the game, and a frame arrived recently enough to trust.
pub fn game_view_active(
    view_mode: &PieViewMode,
    camera_mode: &LiveCameraMode,
    stream: &LiveFrameStream,
) -> bool {
    *view_mode == PieViewMode::Live
        && *camera_mode == LiveCameraMode::Game
        && stream.is_fresh()
        && stream.image.is_some()
}

pub struct LiveFrameViewPlugin;

impl Plugin for LiveFrameViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LiveCameraMode>()
            .init_resource::<StreamRequest>()
            .add_systems(
                Update,
                (
                    drive_stream_requests,
                    sync_game_view_active,
                    position_frame_backdrop,
                )
                    .chain(),
            );
        #[cfg(feature = "camera_rig")]
        app.add_systems(
            PostUpdate,
            lock_viewport_camera_to_game.after(bevy::transform::TransformSystems::Propagate),
        );
    }
}

/// What [`drive_stream_requests`] should send this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamAction {
    None,
    Stop,
    Start(UVec2),
}

/// Decide what to send this frame given the want-state and current request
/// bookkeeping. Pure so the debounce windows are testable.
///
/// `want` is `Some((focused instance, viewport size))` while the Live view
/// should be streaming. A `None` stops the stream once; a size change beyond
/// [`RESIZE_DEAD_ZONE`] (or a fresh entry / refocus) starts it after holding
/// steady for [`RESIZE_DEBOUNCE_MS`]. While the size is settled but the
/// stream is not fresh, the start is re-sent once per
/// [`RESTART_BACKOFF_MS`]; game-side start has restart semantics and
/// re-finds the currently active rig, so this recovers a capture that died
/// (rig despawn) as well as a lost initial start.
fn next_stream_action(
    want: Option<(&InstanceKey, UVec2)>,
    stream_is_fresh: bool,
    req: &mut StreamRequest,
    now: Instant,
) -> StreamAction {
    let Some((focus, size)) = want else {
        req.pending = None;
        req.last_focus = None;
        if req.requested.take().is_some() {
            return StreamAction::Stop;
        }
        return StreamAction::None;
    };
    // A refocus invalidates the old request; the new instance never saw it.
    if req.last_focus.as_ref() != Some(focus) {
        req.last_focus = Some(focus.clone());
        req.requested = None;
        req.pending = None;
    }
    if let Some(requested) = req.requested
        && within_dead_zone(requested, size)
    {
        req.pending = None;
        if !stream_is_fresh
            && req
                .last_start
                .is_none_or(|at| now.duration_since(at).as_millis() >= RESTART_BACKOFF_MS)
        {
            req.requested = Some(size);
            req.last_start = Some(now);
            return StreamAction::Start(size);
        }
        return StreamAction::None;
    }
    match req.pending {
        Some((candidate, since)) if within_dead_zone(candidate, size) => {
            if now.duration_since(since).as_millis() >= RESIZE_DEBOUNCE_MS {
                req.requested = Some(size);
                req.pending = None;
                req.last_start = Some(now);
                StreamAction::Start(size)
            } else {
                StreamAction::None
            }
        }
        // No candidate yet, or the size moved again; restart the window.
        _ => {
            req.pending = Some((size, now));
            StreamAction::None
        }
    }
}

/// True when the two sizes differ by at most [`RESIZE_DEAD_ZONE`] on both
/// axes.
fn within_dead_zone(a: UVec2, b: UVec2) -> bool {
    a.x.abs_diff(b.x) <= RESIZE_DEAD_ZONE && a.y.abs_diff(b.y) <= RESIZE_DEAD_ZONE
}

/// Ask the focused game to start, resize, or stop the frame stream based on
/// the current view mode, focused instance, and viewport pixel size.
/// Exclusive because the focused-instance check reads the non-send
/// [`PieSession`](crate::pie::PieSession) and the send path needs the world.
pub fn drive_stream_requests(world: &mut World) {
    let live = world
        .get_resource::<PieViewMode>()
        .copied()
        .unwrap_or_default()
        == PieViewMode::Live;
    let want = if live {
        crate::pie::focused_live_instance(world).zip(viewport_pixel_size(world))
    } else {
        None
    };
    let stream_is_fresh = world
        .get_resource::<LiveFrameStream>()
        .is_some_and(LiveFrameStream::is_fresh);
    let action = {
        let mut req = world.resource_mut::<StreamRequest>();
        next_stream_action(
            want.as_ref().map(|(key, size)| (key, *size)),
            stream_is_fresh,
            &mut req,
            Instant::now(),
        )
    };
    match action {
        StreamAction::None => {}
        StreamAction::Stop => {
            // The focus may already be gone; the send is then a no-op.
            crate::pie::send_control_to_focused(world, ControlEvent::StopFrameStream);
        }
        StreamAction::Start(size) => {
            crate::pie::send_control_to_focused(
                world,
                ControlEvent::StartFrameStream {
                    width: size.x,
                    height: size.y,
                },
            );
        }
    }
}

/// Pixel size of the viewport the game view targets: the camera the view
/// bound while it is up, else the hovered viewport's camera, else the first
/// one (the same choice `enter_game_view` makes). Reads the camera's
/// render-target image, which the panel's `ViewportNode` keeps sized to the
/// node in physical pixels.
fn viewport_pixel_size(world: &mut World) -> Option<UVec2> {
    let bound = world
        .get_resource::<StoredClearColor>()
        .map(|stored| stored.camera);
    let hovered = world
        .get_resource::<ActiveViewport>()
        .and_then(|active| active.camera);
    let camera = {
        let mut cameras = world.query_filtered::<Entity, With<MainViewportCamera>>();
        bound
            .filter(|&entity| cameras.get(world, entity).is_ok())
            .or(hovered.filter(|&entity| cameras.get(world, entity).is_ok()))
            .or_else(|| cameras.iter(world).next())
    }?;
    let RenderTarget::Image(target) = world.get::<RenderTarget>(camera)? else {
        return None;
    };
    let handle = target.handle.clone();
    let image = world.get_resource::<Assets<Image>>()?.get(&handle)?;
    let size = image.size();
    (size.x > 0 && size.y > 0).then_some(size)
}

/// Enter, maintain, or exit the game view based on the current view mode and
/// stream freshness. Exclusive so one pass can swap the backdrop entities,
/// the clear config, and scene visibility together.
pub fn sync_game_view_active(world: &mut World) {
    let active = {
        let view_mode = world
            .get_resource::<PieViewMode>()
            .copied()
            .unwrap_or_default();
        let camera_mode = world
            .get_resource::<LiveCameraMode>()
            .copied()
            .unwrap_or_default();
        world
            .get_resource::<LiveFrameStream>()
            .is_some_and(|stream| game_view_active(&view_mode, &camera_mode, stream))
    };
    let backdrop_exists = {
        let mut backdrops = world.query_filtered::<(), With<FrameBackdropCamera>>();
        backdrops.iter(world).next().is_some()
    };
    if active {
        if backdrop_exists {
            // If the viewport camera was despawned (panel rebuild while the
            // view was active), the stashed clear color is gone or points at
            // a dead entity. Re-bind by tearing down and re-entering.
            let camera_gone = world
                .get_resource::<StoredClearColor>()
                .map_or(true, |stored| world.get_entity(stored.camera).is_err());
            if camera_gone {
                exit_game_view(world);
                enter_game_view(world);
            } else {
                // Live entities keep spawning while the view is up; hide each
                // newcomer the frame it arrives.
                hide_scene_geometry(world);
            }
        } else {
            enter_game_view(world);
        }
    } else if backdrop_exists {
        exit_game_view(world);
    }
}

/// Spawn the backdrop camera + sprite into the viewport camera's render
/// image, stop that camera from clearing over them, and hide the scene's
/// solid geometry.
fn enter_game_view(world: &mut World) {
    let Some(frame_image) = world.resource::<LiveFrameStream>().image.clone() else {
        return;
    };

    // The hovered viewport's camera when there is one, else the first
    // viewport camera. The backdrop draws into that camera's render image.
    let hovered = world
        .get_resource::<ActiveViewport>()
        .and_then(|active| active.camera);
    let viewport_camera = {
        let mut cameras = world.query_filtered::<Entity, With<MainViewportCamera>>();
        hovered
            .filter(|&entity| cameras.get(world, entity).is_ok())
            .or_else(|| cameras.iter(world).next())
    };

    let Some(camera_entity) = viewport_camera else {
        return;
    };

    // Stash the camera's free-flight pose and projection. The lock overwrites
    // both every frame while the view is active; exit puts them back.
    if let Some(pose) = world.get::<Transform>(camera_entity).copied() {
        let projection = world.get::<Projection>(camera_entity).cloned();
        world.insert_resource(StoredFreePose { pose, projection });
    }

    let backdrop_camera = world
        .spawn((
            FrameBackdropCamera,
            crate::EditorEntity,
            Camera2d,
            Camera {
                order: -2,
                ..default()
            },
            Msaa::Off,
            RenderLayers::layer(BACKDROP_LAYER),
        ))
        .id();

    // Share the viewport camera's render image so the frame composites
    // beneath its render, then stop that camera from clearing over it.
    if let Some(target) = world.get::<RenderTarget>(camera_entity).cloned() {
        world.entity_mut(backdrop_camera).insert(target);
    }
    let stashed = world.get_mut::<Camera>(camera_entity).map(|mut camera| {
        let prior = camera.clear_color;
        camera.clear_color = ClearColorConfig::None;
        prior
    });
    if let Some(clear_color) = stashed {
        world.insert_resource(StoredClearColor {
            camera: camera_entity,
            clear_color,
        });
    }

    world.spawn((
        FrameBackdropSprite,
        crate::EditorEntity,
        Sprite {
            image: frame_image,
            ..default()
        },
        RenderLayers::layer(BACKDROP_LAYER),
    ));

    hide_scene_geometry(world);
}

/// Hide every scene mesh (and each viewport's grid) not already hidden by
/// the game view, remembering its prior visibility for restore. Idempotent:
/// already-hidden entities carry [`HiddenForGameView`] and are skipped.
fn hide_scene_geometry(world: &mut World) {
    let mut targets: Vec<(Entity, Visibility)> = {
        let mut meshes = world.query_filtered::<(Entity, &Visibility), (
            With<Mesh3d>,
            Without<crate::EditorEntity>,
            Without<HiddenForGameView>,
        )>();
        meshes
            .iter(world)
            .map(|(entity, visibility)| (entity, *visibility))
            .collect()
    };
    let grids: Vec<Entity> = {
        let mut cameras = world.query_filtered::<&ViewportGrid, With<MainViewportCamera>>();
        cameras.iter(world).map(|grid| grid.0).collect()
    };
    for grid in grids {
        if world.get::<HiddenForGameView>(grid).is_some() {
            continue;
        }
        if let Some(visibility) = world.get::<Visibility>(grid) {
            targets.push((grid, *visibility));
        }
    }
    for (entity, prior) in targets {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.insert((Visibility::Hidden, HiddenForGameView(prior)));
        }
    }
}

/// Tear down the backdrop, restore the stashed clear config, and restore
/// every hidden entity to the visibility it had before the view activated.
fn exit_game_view(world: &mut World) {
    let backdrops: Vec<Entity> = {
        let mut backdrops = world.query_filtered::<Entity, Or<(
            With<FrameBackdropCamera>,
            With<FrameBackdropSprite>,
        )>>();
        backdrops.iter(world).collect()
    };
    for entity in backdrops {
        if let Ok(entity_mut) = world.get_entity_mut(entity) {
            entity_mut.despawn();
        }
    }
    let stored_free = world.remove_resource::<StoredFreePose>();
    if let Some(stored) = world.remove_resource::<StoredClearColor>() {
        if let Some(mut camera) = world.get_mut::<Camera>(stored.camera) {
            camera.clear_color = stored.clear_color;
        }
        // Hand the free-flight pose and projection back to the camera the
        // view was bound to. If that camera died (panel rebuild mid-view),
        // they have nowhere to go; re-entry stashes the replacement camera's
        // own state.
        if let Some(free) = stored_free
            && let Ok(mut entity) = world.get_entity_mut(stored.camera)
        {
            entity.insert(free.pose);
            if let Some(projection) = free.projection
                && entity.get::<Projection>().is_some()
            {
                entity.insert(projection);
            }
        }
    }
    let hidden: Vec<(Entity, Visibility)> = {
        let mut hidden = world.query::<(Entity, &HiddenForGameView)>();
        hidden
            .iter(world)
            .map(|(entity, stored)| (entity, stored.0))
            .collect()
    };
    for (entity, prior) in hidden {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.insert(prior);
            entity_mut.remove::<HiddenForGameView>();
        }
    }
}

/// Lock the viewport camera the game view bound (recorded in
/// [`StoredClearColor`]) to the mirrored game rig's pose while the view is
/// active, so rays cast through the displayed frame hit the mirror where the
/// game camera sees the same geometry. Quad-view layouts have several
/// viewport cameras; only the bound one follows the rig.
///
/// Runs in `PostUpdate` after transform propagation and writes both the
/// local and global transforms, the same dual-write the rig driver performs
/// game-side, so the current frame already renders from the game pose. The
/// free-flight camera controller writes earlier in the frame and is
/// deterministically overridden while the lock holds. The two queries both
/// touch `GlobalTransform` and `Projection` but are disjoint through the
/// `MainViewportCamera` filter, so no `ParamSet` is needed.
#[cfg(feature = "camera_rig")]
pub fn lock_viewport_camera_to_game(
    view_mode: Option<Res<PieViewMode>>,
    camera_mode: Option<Res<LiveCameraMode>>,
    stream: Option<Res<LiveFrameStream>>,
    stored: Option<Res<StoredClearColor>>,
    rigs: Query<
        (&GlobalTransform, Option<&Projection>),
        (
            With<CameraRig>,
            With<ActiveCameraRig>,
            Without<MainViewportCamera>,
        ),
    >,
    mut cameras: Query<
        (&mut Transform, &mut GlobalTransform, Option<&mut Projection>),
        With<MainViewportCamera>,
    >,
) {
    let view_mode = view_mode.as_deref().copied().unwrap_or_default();
    let camera_mode = camera_mode.as_deref().copied().unwrap_or_default();
    let Some(stream) = stream else {
        return;
    };
    if !game_view_active(&view_mode, &camera_mode, &stream) {
        return;
    }
    let Some(stored) = stored else {
        return;
    };
    let Ok((rig_global, rig_projection)) = rigs.single() else {
        return;
    };
    let Ok((mut transform, mut global, projection)) = cameras.get_mut(stored.camera) else {
        return;
    };
    *transform = rig_global.compute_transform();
    *global = *rig_global;
    if let Some(rig_projection) = rig_projection
        && let Some(mut projection) = projection
    {
        let mut next = rig_projection.clone();
        // The rig's aspect ratio belongs to the game window. The overlay
        // renders into the viewport panel and must keep the panel's own
        // aspect, or everything it draws shears sideways against the
        // displayed frame; the height-fitted backdrop shares the vertical
        // field of view, which is what alignment actually needs.
        if let (Projection::Perspective(next), Projection::Perspective(current)) =
            (&mut next, &*projection)
        {
            next.aspect_ratio = current.aspect_ratio;
        }
        *projection = next;
    }
}

/// Fit the backdrop sprite to the viewport's render image by height.
///
/// The backdrop camera shares the viewport camera's render image, whose size
/// tracks the viewport node in physical pixels with a render-target scale
/// factor of 1.0, so 2D world units equal image pixels and the sprite at the
/// origin already sits at the image center.
///
/// Height-fit, not letterbox: with the frame's displayed height equal to the
/// image height, the frame and the locked overlay camera share the same
/// vertical field of view from the same pose, which makes the displayed
/// pixels line up exactly with overlay gizmos and picking rays in both axes.
/// Width differences (the stream's 64 px width rounding, resize transients
/// inside the request dead zone) become thin side bars or a slight side
/// overflow instead of a scale mismatch.
fn position_frame_backdrop(
    stream: Res<LiveFrameStream>,
    images: Res<Assets<Image>>,
    cameras: Query<&RenderTarget, With<FrameBackdropCamera>>,
    mut sprites: Query<&mut Sprite, With<FrameBackdropSprite>>,
) {
    let Ok(mut sprite) = sprites.single_mut() else {
        return;
    };
    let Ok(target) = cameras.single() else {
        return;
    };
    let RenderTarget::Image(image_target) = target else {
        return;
    };
    let Some(target_image) = images.get(&image_target.handle) else {
        return;
    };
    let target_size = target_image.size().as_vec2() / image_target.scale_factor;
    let frame_size = stream.size.as_vec2();
    if target_size.x < 1.0 || target_size.y < 1.0 || frame_size.x < 1.0 || frame_size.y < 1.0 {
        return;
    }
    // The stream re-creates its image asset when the game resizes; keep the
    // sprite pointing at the current one.
    if let Some(handle) = &stream.image
        && sprite.image != *handle
    {
        sprite.image = handle.clone();
    }
    let scale = target_size.y / frame_size.y;
    sprite.custom_size = Some(frame_size * scale);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(config: &str, instance: u32) -> InstanceKey {
        InstanceKey {
            config: config.to_string(),
            instance,
        }
    }

    /// An instant the debounce window has already elapsed against.
    fn later(start: Instant) -> Instant {
        start + std::time::Duration::from_millis(RESIZE_DEBOUNCE_MS as u64 + 10)
    }

    #[test]
    fn stop_is_sent_once_when_want_goes_away() {
        let mut req = StreamRequest {
            requested: Some(UVec2::new(800, 600)),
            pending: None,
            last_focus: Some(key("game", 1)),
            last_start: None,
        };
        let now = Instant::now();
        assert_eq!(next_stream_action(None, true, &mut req, now), StreamAction::Stop);
        assert_eq!(next_stream_action(None, true, &mut req, now), StreamAction::None);
        assert!(req.requested.is_none());
        assert!(req.last_focus.is_none());
    }

    #[test]
    fn no_request_yet_and_no_want_stays_quiet() {
        let mut req = StreamRequest::default();
        assert_eq!(
            next_stream_action(None, true, &mut req, Instant::now()),
            StreamAction::None
        );
    }

    #[test]
    fn first_start_waits_out_the_debounce_window() {
        let mut req = StreamRequest::default();
        let k = key("game", 1);
        let size = UVec2::new(800, 600);
        let t0 = Instant::now();

        assert_eq!(
            next_stream_action(Some((&k, size)), true, &mut req, t0),
            StreamAction::None
        );
        assert_eq!(
            next_stream_action(
                Some((&k, size)),
                true,
                &mut req,
                t0 + std::time::Duration::from_millis(100)
            ),
            StreamAction::None
        );
        // At exactly t0 + 250 ms the >= boundary fires and the action is Start.
        let at_boundary =
            t0 + std::time::Duration::from_millis(RESIZE_DEBOUNCE_MS as u64);
        let mut req_boundary = StreamRequest::default();
        next_stream_action(Some((&k, size)), true, &mut req_boundary, t0);
        assert_eq!(
            next_stream_action(Some((&k, size)), true, &mut req_boundary, at_boundary),
            StreamAction::Start(size),
            "at exactly t0 + RESIZE_DEBOUNCE_MS the action must be Start (>= boundary)"
        );
        assert_eq!(
            next_stream_action(Some((&k, size)), true, &mut req, later(t0)),
            StreamAction::Start(size)
        );
        assert_eq!(req.requested, Some(size));
        assert!(req.pending.is_none());
    }

    #[test]
    fn wiggle_inside_the_dead_zone_never_restarts() {
        let mut req = StreamRequest::default();
        let k = key("game", 1);
        let t0 = Instant::now();
        next_stream_action(Some((&k, UVec2::new(800, 600))), true, &mut req, t0);
        next_stream_action(Some((&k, UVec2::new(800, 600))), true, &mut req, later(t0));
        assert_eq!(req.requested, Some(UVec2::new(800, 600)));

        for (dx, dy) in [(10, 0), (0, 20), (32, 32), (5, 31)] {
            let wiggled = UVec2::new(800 + dx, 600 - dy);
            assert_eq!(
                next_stream_action(Some((&k, wiggled)), true, &mut req, later(later(t0))),
                StreamAction::None
            );
            assert!(req.pending.is_none(), "wiggle must not arm the debounce");
        }
        assert_eq!(req.requested, Some(UVec2::new(800, 600)));

        // A delta of exactly (33, 0) is outside the dead zone and must arm
        // the debounce candidate.
        let outside = UVec2::new(800 + 33, 600);
        next_stream_action(Some((&k, outside)), true, &mut req, later(later(t0)));
        assert!(
            req.pending.is_some(),
            "(33, 0) delta is outside the dead zone and must arm a pending candidate"
        );
    }

    #[test]
    fn real_resize_restarts_after_the_debounce() {
        let mut req = StreamRequest::default();
        let k = key("game", 1);
        let t0 = Instant::now();
        next_stream_action(Some((&k, UVec2::new(800, 600))), true, &mut req, t0);
        next_stream_action(Some((&k, UVec2::new(800, 600))), true, &mut req, later(t0));

        let resized = UVec2::new(1200, 800);
        let t1 = later(later(t0));
        assert_eq!(
            next_stream_action(Some((&k, resized)), true, &mut req, t1),
            StreamAction::None
        );
        assert_eq!(
            next_stream_action(Some((&k, resized)), true, &mut req, later(t1)),
            StreamAction::Start(resized)
        );
        assert_eq!(req.requested, Some(resized));
    }

    #[test]
    fn size_still_moving_keeps_resetting_the_window() {
        let mut req = StreamRequest::default();
        let k = key("game", 1);
        let t0 = Instant::now();
        next_stream_action(Some((&k, UVec2::new(800, 600))), true, &mut req, t0);

        // A drag past the dead zone mid-window restarts the wait from there.
        let t1 = t0 + std::time::Duration::from_millis(100);
        let moved = UVec2::new(900, 600);
        assert_eq!(
            next_stream_action(Some((&k, moved)), true, &mut req, t1),
            StreamAction::None
        );
        assert_eq!(
            next_stream_action(Some((&k, moved)), true, &mut req, later(t0)),
            StreamAction::None,
            "the original window no longer counts"
        );
        assert_eq!(
            next_stream_action(Some((&k, moved)), true, &mut req, later(t1)),
            StreamAction::Start(moved)
        );
    }

    #[test]
    fn refocus_rerequests_for_the_new_instance() {
        let mut req = StreamRequest::default();
        let first = key("game", 1);
        let size = UVec2::new(800, 600);
        let t0 = Instant::now();
        next_stream_action(Some((&first, size)), true, &mut req, t0);
        next_stream_action(Some((&first, size)), true, &mut req, later(t0));
        assert_eq!(req.requested, Some(size));

        // Same size, different instance: the request must go out again.
        let second = key("game", 2);
        let t1 = later(later(t0));
        assert_eq!(
            next_stream_action(Some((&second, size)), true, &mut req, t1),
            StreamAction::None
        );
        assert_eq!(
            next_stream_action(Some((&second, size)), true, &mut req, later(t1)),
            StreamAction::Start(size)
        );
        assert_eq!(req.last_focus, Some(second));
    }

    /// An instant the restart backoff window has already elapsed against.
    fn after_backoff(start: Instant) -> Instant {
        start + std::time::Duration::from_millis(RESTART_BACKOFF_MS as u64)
    }

    #[test]
    fn stale_stream_retries_after_the_backoff_window() {
        let mut req = StreamRequest::default();
        let k = key("game", 1);
        let size = UVec2::new(800, 600);
        let t0 = Instant::now();
        next_stream_action(Some((&k, size)), true, &mut req, t0);
        let t_start = later(t0);
        assert_eq!(
            next_stream_action(Some((&k, size)), true, &mut req, t_start),
            StreamAction::Start(size)
        );

        // Stale inside the backoff window: the start it gated stays the
        // only one.
        let inside = t_start + std::time::Duration::from_millis(RESTART_BACKOFF_MS as u64 - 1);
        assert_eq!(
            next_stream_action(Some((&k, size)), false, &mut req, inside),
            StreamAction::None
        );
        // Still stale past the window: the start goes out again.
        assert_eq!(
            next_stream_action(Some((&k, size)), false, &mut req, after_backoff(t_start)),
            StreamAction::Start(size)
        );
        assert_eq!(req.requested, Some(size));
    }

    #[test]
    fn fresh_stream_never_retries() {
        let mut req = StreamRequest::default();
        let k = key("game", 1);
        let size = UVec2::new(800, 600);
        let t0 = Instant::now();
        next_stream_action(Some((&k, size)), true, &mut req, t0);
        let t_start = later(t0);
        next_stream_action(Some((&k, size)), true, &mut req, t_start);
        assert_eq!(req.requested, Some(size));

        let long_after = after_backoff(after_backoff(after_backoff(t_start)));
        assert_eq!(
            next_stream_action(Some((&k, size)), true, &mut req, long_after),
            StreamAction::None
        );
    }

    #[test]
    fn each_retry_rearms_the_backoff() {
        let mut req = StreamRequest::default();
        let k = key("game", 1);
        let size = UVec2::new(800, 600);
        let t0 = Instant::now();
        next_stream_action(Some((&k, size)), true, &mut req, t0);
        let t_start = later(t0);
        next_stream_action(Some((&k, size)), true, &mut req, t_start);

        let t_retry = after_backoff(t_start);
        assert_eq!(
            next_stream_action(Some((&k, size)), false, &mut req, t_retry),
            StreamAction::Start(size)
        );
        // A second stale check inside the re-armed window stays quiet, so
        // one window holds at most one start.
        let inside = t_retry + std::time::Duration::from_millis(RESTART_BACKOFF_MS as u64 - 1);
        assert_eq!(
            next_stream_action(Some((&k, size)), false, &mut req, inside),
            StreamAction::None
        );
        // The window measured from the retry elapses and fires again.
        assert_eq!(
            next_stream_action(Some((&k, size)), false, &mut req, after_backoff(t_retry)),
            StreamAction::Start(size)
        );
    }

    fn fresh_stream() -> LiveFrameStream {
        let mut s = LiveFrameStream::default();
        s.image = Some(Handle::default());
        s.received_at = Some(std::time::Instant::now());
        s
    }

    #[test]
    fn game_view_requires_live_mode_game_camera_and_fresh_stream() {
        let stale = LiveFrameStream::default();
        assert!(!game_view_active(
            &PieViewMode::Live,
            &LiveCameraMode::Game,
            &stale
        ));
        let s = fresh_stream();
        assert!(game_view_active(&PieViewMode::Live, &LiveCameraMode::Game, &s));
        assert!(!game_view_active(
            &PieViewMode::Scene,
            &LiveCameraMode::Game,
            &s
        ));
        assert!(!game_view_active(
            &PieViewMode::Live,
            &LiveCameraMode::Free,
            &s
        ));
    }

    #[test]
    fn hide_restore_cycle_round_trips() {
        let mut world = World::new();
        world.insert_resource(PieViewMode::Live);
        world.insert_resource(LiveCameraMode::Game);
        world.insert_resource(fresh_stream());

        let mesh = world
            .spawn((Mesh3d(Handle::default()), Visibility::Visible))
            .id();
        let camera = world
            .spawn((
                MainViewportCamera,
                Camera {
                    order: -1,
                    ..default()
                },
                Transform::from_xyz(1.0, 2.0, 3.0),
                Projection::Perspective(PerspectiveProjection {
                    fov: 0.7,
                    ..default()
                }),
            ))
            .id();

        sync_game_view_active(&mut world);

        assert_eq!(world.get::<Visibility>(mesh), Some(&Visibility::Hidden));
        assert_eq!(
            world.get::<HiddenForGameView>(mesh).map(|h| h.0),
            Some(Visibility::Visible)
        );
        assert!(matches!(
            world.get::<Camera>(camera).unwrap().clear_color,
            ClearColorConfig::None
        ));
        assert_eq!(
            world.resource::<StoredFreePose>().pose.translation,
            Vec3::new(1.0, 2.0, 3.0)
        );
        assert!(world.resource::<StoredFreePose>().projection.is_some());
        let backdrop_count = world
            .query_filtered::<(), With<FrameBackdropCamera>>()
            .iter(&world)
            .count();
        assert_eq!(backdrop_count, 1);
        let sprite_count = world
            .query_filtered::<(), With<FrameBackdropSprite>>()
            .iter(&world)
            .count();
        assert_eq!(sprite_count, 1);

        // A mesh spawned while the view is active is swept up next run, and
        // the run stays idempotent (still one backdrop).
        let late_mesh = world
            .spawn((Mesh3d(Handle::default()), Visibility::Visible))
            .id();
        sync_game_view_active(&mut world);
        assert_eq!(
            world.get::<Visibility>(late_mesh),
            Some(&Visibility::Hidden)
        );
        let backdrop_count = world
            .query_filtered::<(), With<FrameBackdropCamera>>()
            .iter(&world)
            .count();
        assert_eq!(backdrop_count, 1);

        // The lock moved the camera and copied the rig's projection while the
        // view was up; exit must put the stashed free-flight state back.
        world.entity_mut(camera).insert((
            Transform::from_xyz(9.0, 9.0, 9.0),
            Projection::Perspective(PerspectiveProjection {
                fov: 1.2,
                ..default()
            }),
        ));

        // Stale stream: the view deactivates and restores everything.
        world.resource_mut::<LiveFrameStream>().received_at = None;
        sync_game_view_active(&mut world);

        assert_eq!(world.get::<Visibility>(mesh), Some(&Visibility::Visible));
        assert!(world.get::<HiddenForGameView>(mesh).is_none());
        assert_eq!(
            world.get::<Visibility>(late_mesh),
            Some(&Visibility::Visible)
        );
        assert!(world.get::<HiddenForGameView>(late_mesh).is_none());
        assert!(matches!(
            world.get::<Camera>(camera).unwrap().clear_color,
            ClearColorConfig::Default
        ));
        let leftover = world
            .query_filtered::<(), Or<(With<FrameBackdropCamera>, With<FrameBackdropSprite>)>>()
            .iter(&world)
            .count();
        assert_eq!(leftover, 0);
        assert!(world.get_resource::<StoredClearColor>().is_none());
        assert!(world.get_resource::<StoredFreePose>().is_none());
        assert_eq!(
            world.get::<Transform>(camera).unwrap().translation,
            Vec3::new(1.0, 2.0, 3.0)
        );
        match world.get::<Projection>(camera).unwrap() {
            Projection::Perspective(perspective) => {
                assert!((perspective.fov - 0.7).abs() < 1e-6);
            }
            other => panic!("expected the stashed perspective projection, got {other:?}"),
        }

        // Inactive with nothing spawned: a further run is a no-op.
        sync_game_view_active(&mut world);
        assert_eq!(world.get::<Visibility>(mesh), Some(&Visibility::Visible));
    }

    #[cfg(feature = "camera_rig")]
    #[test]
    fn lock_copies_rig_pose_onto_viewport_camera() {
        use jackdaw_camera_rig::{ActiveCameraRig, CameraRig};
        let mut app = App::new();
        app.add_plugins(bevy::transform::TransformPlugin);
        app.init_resource::<LiveCameraMode>();
        app.insert_resource(PieViewMode::Live);
        app.insert_resource(fresh_stream());
        app.add_systems(
            PostUpdate,
            lock_viewport_camera_to_game.after(bevy::transform::TransformSystems::Propagate),
        );

        let pose = Transform::from_xyz(3.0, 4.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y);
        // A deliberately odd rig aspect: the lock must copy the fov but keep
        // the viewport camera's own aspect (the game window's shape is not
        // the panel's).
        let rig_projection = Projection::Perspective(PerspectiveProjection {
            fov: 0.5,
            aspect_ratio: 0.25,
            ..default()
        });
        app.world_mut()
            .spawn((CameraRig::default(), ActiveCameraRig, pose, rig_projection));
        let cam = app
            .world_mut()
            .spawn((
                MainViewportCamera,
                Transform::default(),
                Projection::Perspective(PerspectiveProjection {
                    fov: 1.0,
                    aspect_ratio: 2.0,
                    ..default()
                }),
            ))
            .id();
        // The lock follows the camera the game view bound, recorded the same
        // way enter_game_view stashes the clear config.
        app.world_mut().insert_resource(StoredClearColor {
            camera: cam,
            clear_color: ClearColorConfig::Default,
        });
        app.update();

        // Propagation computed the rig's GlobalTransform from its streamed
        // local pose; the lock copied it onto the camera the same frame.
        let got = app
            .world()
            .entity(cam)
            .get::<GlobalTransform>()
            .unwrap()
            .translation();
        assert!((got - Vec3::new(3.0, 4.0, 5.0)).length() < 1e-4);
        let local = app.world().entity(cam).get::<Transform>().unwrap();
        assert!((local.translation - pose.translation).length() < 1e-4);
        match app.world().entity(cam).get::<Projection>().unwrap() {
            Projection::Perspective(perspective) => {
                assert!((perspective.fov - 0.5).abs() < 1e-6, "rig fov copied");
                assert!(
                    (perspective.aspect_ratio - 2.0).abs() < 1e-6,
                    "the viewport's own aspect is preserved, not the rig's"
                );
            }
            other => panic!("expected the rig's perspective projection, got {other:?}"),
        }
    }
}
