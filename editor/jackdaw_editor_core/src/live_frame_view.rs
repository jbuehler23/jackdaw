//! Paces the frame-stream start/resize/stop requests sent to the focused
//! game so the streamed frame tracks the Game panel's surface size.
//!
//! The Game panel owns and displays the streamed frame; this module only
//! decides when to ask the game to start, resize, or stop the stream based
//! on the focused instance, the panel's pixel size, and stream freshness.
//! Resize requests debounce so a panel drag does not spam restarts, and a
//! stale stream is re-requested on a backoff so a dead game-side capture
//! recovers without flooding the channel.

use std::time::Instant;

use bevy::prelude::*;
use jackdaw_pie_protocol::ControlEvent;

use crate::live_frame::LiveFrameStream;
use crate::pie::InstanceKey;

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

pub struct LiveFrameViewPlugin;

impl Plugin for LiveFrameViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StreamRequest>()
            .add_systems(Update, drive_stream_requests);
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
/// `want` is `Some((focused instance, panel size))` while the Game panel
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
/// the focused instance and the Game panel's pixel size. Exclusive because
/// the focused-instance check reads the non-send
/// [`PieSession`](crate::pie::PieSession) and the send path needs the world.
pub fn drive_stream_requests(world: &mut World) {
    let want = crate::pie::focused_live_instance(world).zip(game_panel_pixel_size(world));
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

/// Pixel size of the Game panel's letterbox surface, the stream request
/// target. `None` while the panel is closed or collapsed, which stops the
/// stream.
fn game_panel_pixel_size(world: &mut World) -> Option<UVec2> {
    let mut surfaces =
        world.query_filtered::<&ComputedNode, With<crate::game_panel::GamePanelSurface>>();
    let computed = surfaces.iter(world).next()?;
    let size = computed.size();
    (size.x >= 1.0 && size.y >= 1.0).then_some(UVec2::new(size.x as u32, size.y as u32))
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
        assert_eq!(
            next_stream_action(None, true, &mut req, now),
            StreamAction::Stop
        );
        assert_eq!(
            next_stream_action(None, true, &mut req, now),
            StreamAction::None
        );
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
        let at_boundary = t0 + std::time::Duration::from_millis(RESIZE_DEBOUNCE_MS as u64);
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
}
