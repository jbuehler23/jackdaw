//! Frame capture for the live frame view: reads back a rendered target and
//! ships frames on the `Frames` channel. Two modes.
//!
//! Windowless mode (no OS window): the game's own cameras already render into
//! the shared `WindowlessTarget` image every frame, so capture attaches a
//! permanent [`Readback`] on that image and the completion observer encodes
//! and ships every readback. There is no capture camera and no pacing; a
//! stream stop only detaches the readback and leaves the image in place.
//!
//! Windowed fallback (the game has its own window): a capture camera spawns
//! as a child of the active rig with an identity transform, inheriting the
//! rig pose through transform propagation. Each pace tick activates the
//! camera and attaches a [`Readback`] to the readback entity; the readback
//! copy is encoded after the render graph runs, so the completion carries
//! that tick's render. The completion observer encodes the frame, deactivates
//! the camera, and detaches the [`Readback`], so the scene is rendered and
//! copied once per tick rather than every game frame.

use std::sync::{Arc, Condvar, Mutex};

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::time::Real;
use jackdaw_camera_rig::{ActiveCameraRig, CameraRig};
use jackdaw_pie_protocol::frame::encode_frame;

/// Single-slot, newest-wins handoff from the render-thread readback observer
/// to the dedicated ipc sender thread. If the sender falls behind, newer
/// frames overwrite the undelivered one, so latency stays bounded instead of
/// queueing.
#[derive(Default)]
pub(crate) struct FrameSlot {
    slot: Mutex<Option<Vec<u8>>>,
    ready: Condvar,
}

impl FrameSlot {
    /// Deposit a frame, replacing any undelivered one, and wake the sender.
    pub(crate) fn deposit(&self, frame: Vec<u8>) {
        let mut guard = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(frame);
        self.ready.notify_one();
    }

    /// Non-blocking take, for tests.
    #[cfg(test)]
    pub(crate) fn try_take(&self) -> Option<Vec<u8>> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Block until a frame is available, then take it.
    fn take_blocking(&self) -> Vec<u8> {
        let mut guard = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(frame) = guard.take() {
                return frame;
            }
            guard = self.ready.wait(guard).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// Deposit side of the frame handoff. The paired sender thread owns the
/// lane and ships each deposited frame immediately.
#[derive(Resource, Clone)]
pub(crate) struct FrameSender(pub(crate) Arc<FrameSlot>);

/// Spawn the sender thread. It lives for the rest of the process; the game
/// process is per-session, so teardown is process exit.
pub(crate) fn spawn_frame_sender_thread(lane: jackdaw_pie_protocol::IpcLaneSender) -> FrameSender {
    let slot = Arc::new(FrameSlot::default());
    let sender = FrameSender(Arc::clone(&slot));
    if let Err(err) = std::thread::Builder::new()
        .name("pie-frame-sender".into())
        .spawn(move || {
            loop {
                lane.send(slot.take_blocking());
            }
        })
    {
        bevy::log::error!("PIE frames: could not spawn the sender thread: {err}");
    }
    sender
}

/// Interval between captured frames (about 30 fps).
const FRAME_INTERVAL: f32 = 1.0 / 30.0;
/// Hard cap on the streamed frame size.
const MAX_DIM: u32 = 1920;
// round_stream_size relies on the cap itself being row-aligned, so clamping
// to MAX_DIM cannot break the multiple-of-64 width invariant.
const _: () = assert!(MAX_DIM % 64 == 0);

/// Marker for the offscreen capture camera.
#[derive(Component)]
pub(crate) struct FrameCaptureCamera;

/// Live capture state. Present only while a stream is active.
#[derive(Resource)]
pub(crate) struct FrameStream {
    pub target: Handle<Image>,
    /// The offscreen capture camera that drives the windowed fallback. `None`
    /// in windowless mode, where the game's own cameras render into the shared
    /// target every frame and there is nothing extra to spawn.
    pub camera: Option<Entity>,
    pub readback: Entity,
    pub size: UVec2,
    pub seq: u64,
    /// Paces the windowed fallback to about 30 fps by toggling the capture
    /// camera per tick. `None` in windowless mode, which ships every readback
    /// completion without pacing.
    pub pace: Option<Timer>,
    /// Limits the malformed-readback warning to a single log line.
    pub warned_bad_len: bool,
    /// Limits the dead-capture-camera warning to a single log line.
    pub warned_dead_camera: bool,
}

/// Round the requested size: width up to a multiple of 64 (so a row is
/// 256-byte aligned and readback rows arrive unpadded), both axes clamped.
pub(crate) fn round_stream_size(width: u32, height: u32) -> UVec2 {
    let w = width.clamp(64, MAX_DIM).div_ceil(64) * 64;
    let h = height.clamp(64, MAX_DIM);
    UVec2::new(w, h)
}

/// Set after a start request was declined, so the editor's periodic retry
/// (which re-sends the request every couple of seconds until a camera rig
/// exists, e.g. while the player is still on a login screen) logs the reason
/// once per streak instead of once per attempt. Cleared when a stream starts.
#[derive(Resource, Default)]
struct StartDeclinedLogged(bool);

fn log_start_declined_once(world: &mut World, reason: &str) {
    let mut logged = world.get_resource_or_insert_with(StartDeclinedLogged::default);
    if !logged.0 {
        info!("PIE frames: {reason}, not streaming (will retry quietly)");
        logged.0 = true;
    }
}

/// Begin streaming: build the offscreen target, spawn the capture camera as a
/// child of the active rig, and install the [`FrameStream`] state. An already
/// running stream restarts at the new size. A headless world without an
/// active rig logs and leaves nothing behind.
pub(crate) fn start_frame_stream(world: &mut World, width: u32, height: u32) {
    stop_frame_stream(world);
    let size = round_stream_size(width, height);
    if world.contains_resource::<crate::pie_windowless::WindowlessTarget>() {
        start_windowless_stream(world, size);
    } else {
        start_capture_camera_stream(world, size);
    }
}

/// Windowed fallback: spawn an offscreen capture camera under the active rig,
/// pace it to about 30 fps, and attach the readback per tick. Logs and leaves
/// nothing behind on a headless world without an active rig.
fn start_capture_camera_stream(world: &mut World, size: UVec2) {
    let mut rigs = world.query_filtered::<Entity, (With<CameraRig>, With<ActiveCameraRig>)>();
    let Some(rig) = rigs.iter(world).next() else {
        log_start_declined_once(world, "no active camera rig");
        return;
    };

    let mut image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let Some(mut images) = world.get_resource_mut::<Assets<Image>>() else {
        log_start_declined_once(world, "no image assets");
        return;
    };
    let target = images.add(image);

    let camera = world
        .spawn((
            FrameCaptureCamera,
            Camera3d::default(),
            Camera {
                is_active: false,
                ..default()
            },
            RenderTarget::Image(target.clone().into()),
            Transform::default(),
            ChildOf(rig),
        ))
        .id();

    // The Readback component itself is attached per pace tick; only the
    // completion observer lives on the entity permanently.
    let mut readback = world.spawn_empty();
    readback.observe(on_frame_readback);
    let readback = readback.id();

    world.insert_resource(FrameStream {
        target,
        camera: Some(camera),
        readback,
        size,
        seq: 0,
        pace: Some(Timer::from_seconds(FRAME_INTERVAL, TimerMode::Repeating)),
        warned_bad_len: false,
        warned_dead_camera: false,
    });
    world.insert_resource(StartDeclinedLogged(false));
    info!("PIE frames: streaming at {}x{}", size.x, size.y);
}

/// Windowless capture: resize the shared target in place (the handle every
/// camera and the virtual window agree on) and attach a permanent readback.
/// The game renders into the target every frame, so there is no capture
/// camera and no pacing; stop only detaches the readback and leaves the
/// image in place for the still-running instance.
fn start_windowless_stream(world: &mut World, size: UVec2) {
    let image_handle = {
        let target = world.resource::<crate::pie_windowless::WindowlessTarget>();
        target.image.clone()
    };
    if let Err(err) = world.resource_mut::<Assets<Image>>().insert(
        image_handle.id(),
        crate::pie_windowless::make_target_image(size),
    ) {
        error!("PIE frames: could not resize the windowless target: {err}");
        return;
    }
    world
        .resource_mut::<crate::pie_windowless::WindowlessTarget>()
        .size = size;

    {
        let mut windows =
            world.query_filtered::<&mut Window, With<crate::pie_windowless::PieVirtualWindow>>();
        if let Ok(mut window) = windows.single_mut(world) {
            window.resolution.set_physical_resolution(size.x, size.y);
        }
    }

    let mut readback = world.spawn(Readback::texture(image_handle.clone()));
    readback.observe(on_frame_readback);
    let readback = readback.id();

    world.insert_resource(FrameStream {
        target: image_handle,
        camera: None,
        readback,
        size,
        seq: 0,
        pace: None,
        warned_bad_len: false,
        warned_dead_camera: false,
    });
    world.insert_resource(StartDeclinedLogged(false));
    info!("PIE frames: windowless streaming at {}x{}", size.x, size.y);
}

/// Tear down an active stream: drop the state, despawn the capture camera and
/// readback entities, and release the target image. Safe to call when nothing
/// is streaming.
pub(crate) fn stop_frame_stream(world: &mut World) {
    let Some(stream) = world.remove_resource::<FrameStream>() else {
        return;
    };
    if let Some(camera) = stream.camera
        && let Ok(camera) = world.get_entity_mut(camera)
    {
        camera.despawn();
    }
    if let Ok(readback) = world.get_entity_mut(stream.readback) {
        readback.despawn();
    }
    // The windowless shared target stays alive for the still-running game; only
    // a capture-camera target is private to the stream and freed here.
    let keep_image = world
        .get_resource::<crate::pie_windowless::WindowlessTarget>()
        .is_some_and(|target| target.image == stream.target);
    if !keep_image {
        world.resource_mut::<Assets<Image>>().remove(&stream.target);
    }
    info!("PIE frames: stream stopped");
}

/// Once per pace tick, activate the capture camera and attach the [`Readback`]
/// that copies the target back to the cpu. Ticks on real time so the live view
/// keeps following editor-driven edits while the virtual clock is paused.
pub(crate) fn pace_frame_capture(
    time: Res<Time<Real>>,
    stream: Option<ResMut<FrameStream>>,
    mut cameras: Query<&mut Camera, With<FrameCaptureCamera>>,
    mut commands: Commands,
) {
    let Some(mut stream) = stream else {
        return;
    };
    // Windowless streams have no pace timer: the game renders into the shared
    // target every frame and the permanent readback ships each completion.
    let Some(pace) = stream.pace.as_mut() else {
        return;
    };
    if !pace.tick(time.delta()).just_finished() {
        return;
    }
    let Some(camera_entity) = stream.camera else {
        return;
    };
    let Ok(mut camera) = cameras.get_mut(camera_entity) else {
        if !stream.warned_dead_camera {
            warn!("PIE frames: capture camera is gone, stream is dead until restarted");
            stream.warned_dead_camera = true;
        }
        return;
    };
    camera.is_active = true;
    // try_insert: a stop or restart in this tick may have despawned the
    // readback entity before this command applies.
    commands
        .entity(stream.readback)
        .try_insert(Readback::texture(stream.target.clone()));
}

/// Completion observer: encode the readback data as a frame and hand it to the
/// sender thread, then deactivate the capture camera and detach the
/// [`Readback`] until the next pace tick.
fn on_frame_readback(
    readback: On<ReadbackComplete>,
    stream: Option<ResMut<FrameStream>>,
    frames: Option<Res<FrameSender>>,
    mut cameras: Query<&mut Camera, With<FrameCaptureCamera>>,
    mut commands: Commands,
) {
    let Some(mut stream) = stream else {
        return;
    };
    // The windowed fallback toggles a capture camera and a per-tick readback;
    // windowless mode has neither, so the camera bookkeeping is skipped and the
    // permanent readback stays attached.
    if let Some(camera_entity) = stream.camera {
        let Ok(mut camera) = cameras.get_mut(camera_entity) else {
            // The rig despawned and took the capture camera with it; detach the
            // Readback so the GPU copy stops instead of running every frame.
            if let Ok(mut readback) = commands.get_entity(stream.readback) {
                readback.remove::<Readback>();
            }
            return;
        };
        // A completion arriving after the camera was deactivated is a leftover
        // copy from an already-shipped capture; drop it.
        if !camera.is_active {
            return;
        }
        camera.is_active = false;
        commands.entity(stream.readback).remove::<Readback>();
    }

    let data = &readback.event().data;
    let expected = stream.size.x as usize * stream.size.y as usize * 4;
    if data.len() != expected {
        if !stream.warned_bad_len {
            warn!(
                "PIE frames: readback returned {} bytes, expected {expected}; dropping frames",
                data.len()
            );
            stream.warned_bad_len = true;
        }
        return;
    }
    stream.seq += 1;
    let frame = encode_frame(stream.size.x, stream.size.y, stream.seq, data);
    if let Some(frames) = frames {
        frames.0.deposit(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_slot_is_newest_wins() {
        let slot = FrameSlot::default();
        slot.deposit(vec![1]);
        slot.deposit(vec![2]);
        assert_eq!(slot.try_take(), Some(vec![2]));
        assert_eq!(slot.try_take(), None);
    }

    #[test]
    fn sender_thread_ships_deposited_frames() {
        use jackdaw_pie_protocol::transport::PieTransport;
        use jackdaw_pie_protocol::{PieChannel, connect, serve};
        let (handle, name) = serve().unwrap();
        let game = std::thread::spawn(move || {
            let t = connect(&name).unwrap();
            let sender = spawn_frame_sender_thread(t.lane_sender(PieChannel::Frames));
            sender.0.deposit(vec![7, 7]);
            std::thread::sleep(std::time::Duration::from_millis(300));
        });
        let mut editor = handle.accept().unwrap();
        let mut got = None;
        for _ in 0..100_000 {
            if let Some(m) = editor.drain_received().into_iter().next() {
                got = Some(m);
                break;
            }
            std::thread::yield_now();
        }
        game.join().unwrap();
        assert_eq!(got, Some((PieChannel::Frames, vec![7, 7])));
    }

    #[test]
    fn round_stream_size_aligns_and_clamps() {
        assert_eq!(round_stream_size(1280, 720), UVec2::new(1280, 720));
        assert_eq!(round_stream_size(1000, 700), UVec2::new(1024, 700));
        assert_eq!(round_stream_size(10_000, 10_000), UVec2::new(1920, 1920));
        assert_eq!(round_stream_size(1, 1), UVec2::new(64, 64));
        assert_eq!(round_stream_size(1920, 1920), UVec2::new(1920, 1920));
        assert!(round_stream_size(1919, 1).x <= 1920);
    }

    #[test]
    fn start_without_active_rig_is_a_noop() {
        let mut app = bevy::app::App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        start_frame_stream(app.world_mut(), 640, 480);
        assert!(app.world().get_resource::<FrameStream>().is_none());
    }

    #[test]
    fn stop_without_stream_is_a_noop() {
        let mut app = bevy::app::App::new();
        stop_frame_stream(app.world_mut());
    }

    #[test]
    fn start_and_stop_lifecycle_with_active_rig() {
        use jackdaw_camera_rig::{ActiveCameraRig, CameraRig};
        let mut app = bevy::app::App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        let rig = app
            .world_mut()
            .spawn((CameraRig::default(), ActiveCameraRig, Transform::default()))
            .id();
        start_frame_stream(app.world_mut(), 640, 480);
        let (camera, target) = {
            let stream = app
                .world()
                .get_resource::<FrameStream>()
                .expect("stream installed");
            (
                stream.camera.expect("windowed mode has a capture camera"),
                stream.target.clone(),
            )
        };
        assert_eq!(
            app.world().entity(camera).get::<ChildOf>().map(|c| c.0),
            Some(rig)
        );
        assert!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&target)
                .is_some()
        );
        stop_frame_stream(app.world_mut());
        assert!(app.world().get_resource::<FrameStream>().is_none());
        assert!(app.world().get_entity(camera).is_err());
        assert!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&target)
                .is_none()
        );
    }

    #[test]
    fn windowless_stream_has_no_camera_and_keeps_the_image_on_stop() {
        let mut app = bevy::app::App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        crate::pie_windowless::install_windowless_world(app.world_mut());

        start_frame_stream(app.world_mut(), 1000, 700);
        let (readback, target) = {
            let stream = app
                .world()
                .get_resource::<FrameStream>()
                .expect("stream installed");
            assert!(
                stream.camera.is_none(),
                "no capture camera in windowless mode"
            );
            assert!(stream.pace.is_none(), "no pace timer in windowless mode");
            assert_eq!(stream.size, UVec2::new(1024, 700));
            (stream.readback, stream.target.clone())
        };
        assert!(
            app.world().entity(readback).contains::<Readback>(),
            "readback attached permanently"
        );
        let image_size = app
            .world()
            .resource::<Assets<Image>>()
            .get(&target)
            .unwrap()
            .size();
        assert_eq!(image_size, UVec2::new(1024, 700));
        let mut windows = app
            .world_mut()
            .query_filtered::<&Window, With<crate::pie_windowless::PieVirtualWindow>>();
        let window = windows.single(app.world()).unwrap();
        assert_eq!(window.physical_width(), 1024);
        assert_eq!(window.physical_height(), 700);

        stop_frame_stream(app.world_mut());
        assert!(app.world().get_resource::<FrameStream>().is_none());
        assert!(app.world().get_entity(readback).is_err());
        assert!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&target)
                .is_some(),
            "windowless target image survives a stream stop"
        );
    }
}
