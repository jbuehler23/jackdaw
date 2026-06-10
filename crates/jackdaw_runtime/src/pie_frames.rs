//! Frame capture for the live frame view: renders the active camera rig to an
//! offscreen target, reads it back, and ships frames on the `Frames` channel.
//!
//! A capture camera spawns as a child of the active rig with an identity
//! transform, inheriting the rig pose through transform propagation. Each pace
//! tick activates the camera and attaches a [`Readback`] to the readback
//! entity; the readback copy is encoded after the render graph runs, so the
//! completion carries that tick's render. The completion observer encodes the
//! frame, deactivates the camera, and detaches the [`Readback`], so the scene
//! is rendered and copied once per tick rather than every game frame.

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::time::Real;
use jackdaw_camera_rig::{ActiveCameraRig, CameraRig};
use jackdaw_pie_protocol::event::PieChannel;
use jackdaw_pie_protocol::frame::encode_frame;
use jackdaw_pie_protocol::transport::PieTransport;

use crate::pie::PieTransportRes;

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
    pub camera: Entity,
    pub readback: Entity,
    pub size: UVec2,
    pub seq: u64,
    pub pace: Timer,
    /// Encoded frame awaiting the main-thread send. The readback observer
    /// fires on the render thread during extract, where the non-send ipc
    /// transport must not be touched, so it parks the frame here for
    /// [`flush_captured_frames`].
    pub pending: Option<Vec<u8>>,
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

    let mut rigs = world.query_filtered::<Entity, (With<CameraRig>, With<ActiveCameraRig>)>();
    let Some(rig) = rigs.iter(world).next() else {
        log_start_declined_once(world, "no active camera rig");
        return;
    };

    let size = round_stream_size(width, height);
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
        camera,
        readback,
        size,
        seq: 0,
        pace: Timer::from_seconds(FRAME_INTERVAL, TimerMode::Repeating),
        pending: None,
        warned_bad_len: false,
        warned_dead_camera: false,
    });
    world.insert_resource(StartDeclinedLogged(false));
    info!("PIE frames: streaming at {}x{}", size.x, size.y);
}

/// Tear down an active stream: drop the state, despawn the capture camera and
/// readback entities, and release the target image. Safe to call when nothing
/// is streaming.
pub(crate) fn stop_frame_stream(world: &mut World) {
    let Some(stream) = world.remove_resource::<FrameStream>() else {
        return;
    };
    if let Ok(camera) = world.get_entity_mut(stream.camera) {
        camera.despawn();
    }
    if let Ok(readback) = world.get_entity_mut(stream.readback) {
        readback.despawn();
    }
    world.resource_mut::<Assets<Image>>().remove(&stream.target);
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
    if !stream.pace.tick(time.delta()).just_finished() {
        return;
    }
    let Ok(mut camera) = cameras.get_mut(stream.camera) else {
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

/// Completion observer: encode the readback data as a frame and park it for
/// the main-thread flush, then deactivate the capture camera and detach the
/// [`Readback`] until the next pace tick.
fn on_frame_readback(
    readback: On<ReadbackComplete>,
    stream: Option<ResMut<FrameStream>>,
    mut cameras: Query<&mut Camera, With<FrameCaptureCamera>>,
    mut commands: Commands,
) {
    let Some(mut stream) = stream else {
        return;
    };
    let Ok(mut camera) = cameras.get_mut(stream.camera) else {
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
    stream.pending = Some(frame);
}

/// Send the most recent captured frame to the editor. Runs on the main thread
/// because the ipc transport is a non-send resource pinned to it.
pub(crate) fn flush_captured_frames(world: &mut World) {
    if !world.contains_non_send::<PieTransportRes>() {
        return;
    }
    let Some(mut stream) = world.get_resource_mut::<FrameStream>() else {
        return;
    };
    let Some(frame) = stream.pending.take() else {
        return;
    };
    world
        .non_send_resource_mut::<PieTransportRes>()
        .0
        .send(PieChannel::Frames, &frame);
}

#[cfg(test)]
mod tests {
    use super::*;

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
            (stream.camera, stream.target.clone())
        };
        assert_eq!(
            app.world().entity(camera).get::<ChildOf>().map(|c| c.0),
            Some(rig)
        );
        assert!(app.world().resource::<Assets<Image>>().get(&target).is_some());
        stop_frame_stream(app.world_mut());
        assert!(app.world().get_resource::<FrameStream>().is_none());
        assert!(app.world().get_entity(camera).is_err());
        assert!(app.world().resource::<Assets<Image>>().get(&target).is_none());
    }
}
