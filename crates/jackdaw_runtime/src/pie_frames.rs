//! Frame capture for the live frame view: reads back a rendered target and
//! ships frames on the `Frames` channel.
//!
//! Capture only runs in windowless mode (no OS window): the game's own
//! cameras already render into the shared `WindowlessTarget` image every
//! frame, so capture attaches a permanent [`Readback`] on that image and the
//! completion observer encodes and ships every readback. A stream stop only
//! detaches the readback and leaves the image in place. A game with its own
//! window never streams.

use std::sync::{Arc, Condvar, Mutex};

use bevy::prelude::*;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
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
        let mut guard = self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(frame);
        self.ready.notify_one();
    }

    /// Non-blocking take, for tests.
    #[cfg(test)]
    pub(crate) fn try_take(&self) -> Option<Vec<u8>> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    /// Block until a frame is available, then take it.
    fn take_blocking(&self) -> Vec<u8> {
        let mut guard = self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(frame) = guard.take() {
                return frame;
            }
            guard = self
                .ready
                .wait(guard)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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

/// Hard cap on the streamed frame size.
const MAX_DIM: u32 = 1920;
// round_stream_size relies on the cap itself being row-aligned, so clamping
// to MAX_DIM cannot break the multiple-of-64 width invariant.
const _: () = assert!(MAX_DIM.is_multiple_of(64));

/// Live capture state. Present only while a stream is active.
#[derive(Resource)]
pub(crate) struct FrameStream {
    pub readback: Entity,
    pub size: UVec2,
    pub seq: u64,
    /// Limits the malformed-readback warning to a single log line.
    pub warned_bad_len: bool,
}

/// Round the requested size: width up to a multiple of 64 (so a row is
/// 256-byte aligned and readback rows arrive unpadded), both axes clamped.
pub(crate) fn round_stream_size(width: u32, height: u32) -> UVec2 {
    let w = width.clamp(64, MAX_DIM).div_ceil(64) * 64;
    let h = height.clamp(64, MAX_DIM);
    UVec2::new(w, h)
}

/// Set after a start request was declined, so repeated requests log the
/// reason once per streak instead of once per attempt. Cleared when a stream
/// starts.
#[derive(Resource, Default)]
struct StartDeclinedLogged(bool);

fn log_start_declined_once(world: &mut World, reason: &str) {
    let mut logged = world.get_resource_or_insert_with(StartDeclinedLogged::default);
    if !logged.0 {
        info!("PIE frames: {reason}, not streaming");
        logged.0 = true;
    }
}

/// Begin streaming and install the [`FrameStream`] state. An already running
/// stream restarts at the new size. A game with its own window declines,
/// logging once and leaving nothing behind.
pub(crate) fn start_frame_stream(world: &mut World, width: u32, height: u32) {
    stop_frame_stream(world);
    let size = round_stream_size(width, height);
    if world.contains_resource::<crate::pie_windowless::WindowlessTarget>() {
        start_windowless_stream(world, size);
    } else {
        log_start_declined_once(world, "the game has its own window");
    }
}

/// Windowless capture: resize the shared target in place (the handle every
/// camera and the virtual window agree on) and attach a permanent readback.
/// The game renders into the target every frame; stop only detaches the
/// readback and leaves the image in place for the still-running instance.
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
        readback,
        size,
        seq: 0,
        warned_bad_len: false,
    });
    world.insert_resource(StartDeclinedLogged(false));
    info!("PIE frames: windowless streaming at {}x{}", size.x, size.y);
}

/// Tear down an active stream: drop the state and despawn the readback
/// entity. The shared windowless target stays alive for the still-running
/// game. Safe to call when nothing is streaming.
pub(crate) fn stop_frame_stream(world: &mut World) {
    let Some(stream) = world.remove_resource::<FrameStream>() else {
        return;
    };
    if let Ok(readback) = world.get_entity_mut(stream.readback) {
        readback.despawn();
    }
    info!("PIE frames: stream stopped");
}

/// Completion observer: encode the readback data as a frame and hand it to the
/// sender thread.
fn on_frame_readback(
    readback: On<ReadbackComplete>,
    stream: Option<ResMut<FrameStream>>,
    frames: Option<Res<FrameSender>>,
) {
    let Some(mut stream) = stream else {
        return;
    };
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
    fn start_without_windowless_target_is_a_noop() {
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
    fn windowless_stream_keeps_the_image_on_stop() {
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
            assert_eq!(stream.size, UVec2::new(1024, 700));
            (
                stream.readback,
                app.world()
                    .resource::<crate::pie_windowless::WindowlessTarget>()
                    .image
                    .clone(),
            )
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
