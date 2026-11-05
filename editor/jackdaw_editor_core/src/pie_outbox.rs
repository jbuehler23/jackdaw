//! The queue carrying every editor-to-game message of one Live instance.
//!
//! A large `ipc-channel` send blocks until the receiver reads, and the game's
//! main thread can itself be blocked sending its state stream to the editor.
//! So the editor's main thread only queues; a writer thread does the sends, in
//! queue order.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use bevy::log::{debug, warn};
use jackdaw_pie_protocol::{IpcChannelTransport, IpcLaneSender, PieChannel};

/// Queued bytes above which the game is reported as not reading.
const BACKLOG_WARN_BYTES: usize = 64 * 1024 * 1024;

/// The main-thread end of a writer thread.
///
/// Dropping it never blocks. If the game was not marked closing, the writer
/// finishes what is queued, then exits and releases its sender, so the game
/// sees the link close once it has read everything. If the game's process
/// dies, the writer's sends fail and it exits the same way. If the stage was
/// marked closing first, the game will not read the backlog, so it is
/// discarded and the writer exits after its current send.
///
/// The exception is a dead game that left a fragmented send unread: on Linux
/// and other Unix-socket platforms `ipc-channel` holds both ends of that
/// send's dedicated channel for the whole call, so it never fails and the
/// writer stays parked until the editor exits. What stays held is the thread,
/// its file descriptors and the in-flight message; the backlog is freed. It
/// blocks nothing else, and the game is gone, so no live peer waits on its
/// sender.
pub(crate) struct GameOutbox {
    shared: Arc<Shared>,
    /// Set once the game is being torn down, when a failed send is expected.
    closing: Arc<AtomicBool>,
}

struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Vec<u8>>,
    bytes: usize,
    open: bool,
    warned: bool,
}

impl State {
    /// Returns true when this push first crossed the backlog threshold.
    fn push(&mut self, bytes: Vec<u8>) -> bool {
        self.bytes = self.bytes.saturating_add(bytes.len());
        self.queue.push_back(bytes);
        if self.warned || self.bytes <= BACKLOG_WARN_BYTES {
            return false;
        }
        self.warned = true;
        true
    }

    fn pop(&mut self) -> Option<Vec<u8>> {
        let bytes = self.queue.pop_front()?;
        self.bytes = self.bytes.saturating_sub(bytes.len());
        if self.bytes < BACKLOG_WARN_BYTES / 2 {
            self.warned = false;
        }
        Some(bytes)
    }
}

impl Shared {
    fn new() -> Self {
        Self {
            state: Mutex::new(State {
                open: true,
                ..State::default()
            }),
            ready: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns true when the backlog just crossed the warning threshold.
    fn push(&self, bytes: Vec<u8>) -> bool {
        let crossed = self.lock().push(bytes);
        self.ready.notify_one();
        crossed
    }

    /// Block until a message is queued, or return `None` once closed and empty.
    fn next(&self) -> Option<Vec<u8>> {
        let mut state = self.lock();
        loop {
            if let Some(bytes) = state.pop() {
                return Some(bytes);
            }
            if !state.open {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn close(&self, discard: bool) {
        let dropped = {
            let mut state = self.lock();
            state.open = false;
            if discard {
                state.bytes = 0;
                std::mem::take(&mut state.queue)
            } else {
                VecDeque::new()
            }
        };
        drop(dropped);
        self.ready.notify_all();
    }
}

impl GameOutbox {
    /// Start the writer thread for `transport`'s reliable channel.
    pub(crate) fn new(transport: &IpcChannelTransport) -> io::Result<Self> {
        let (outbox, _writer) = Self::start(transport)?;
        Ok(outbox)
    }

    fn start(transport: &IpcChannelTransport) -> io::Result<(Self, JoinHandle<()>)> {
        let shared = Arc::new(Shared::new());
        let closing = Arc::new(AtomicBool::new(false));
        let lane = transport.lane_sender(PieChannel::Reliable);
        let writer = thread::Builder::new()
            .name("pie-outbox".to_string())
            .spawn({
                let shared = shared.clone();
                let closing = closing.clone();
                move || write_queued(&shared, &lane, &closing)
            })?;
        Ok((Self { shared, closing }, writer))
    }

    /// Queue one encoded message behind everything already queued.
    pub(crate) fn queue(&self, bytes: Vec<u8>) {
        if self.shared.push(bytes) {
            warn!(
                "PIE outbox: over {} MiB queued for the game, which is not reading",
                BACKLOG_WARN_BYTES / (1024 * 1024)
            );
        }
    }

    /// Note that the game is about to be killed, so failed sends are expected
    /// and the backlog is discarded when the outbox is dropped.
    pub(crate) fn mark_closing(&self) {
        self.closing.store(true, Ordering::Release);
    }
}

impl Drop for GameOutbox {
    fn drop(&mut self) {
        self.shared.close(self.closing.load(Ordering::Acquire));
    }
}

fn write_queued(shared: &Shared, lane: &IpcLaneSender, closing: &AtomicBool) {
    let mut failed = false;
    while let Some(bytes) = shared.next() {
        let Err(err) = lane.try_send(bytes) else {
            continue;
        };
        if failed {
            debug!("PIE outbox: dropping a message, send failed: {err}");
        } else if closing.load(Ordering::Acquire) {
            debug!("PIE outbox: message not delivered to a closing game: {err}");
        } else {
            warn!("PIE outbox: dropping a message, send failed: {err}");
        }
        failed = true;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use jackdaw_pie_protocol::{connect, serve};

    use super::*;

    const LARGE: usize = 8 * 1024 * 1024;
    const GUARD: Duration = Duration::from_secs(30);

    fn link() -> (IpcChannelTransport, IpcChannelTransport) {
        let (server, name) = serve().expect("a rendezvous");
        let accept = thread::spawn(move || server.accept());
        let game = connect(&name).expect("the game connects");
        let editor = accept
            .join()
            .expect("accept ran")
            .expect("the editor accepts");
        (editor, game)
    }

    fn finishes(writer: &JoinHandle<()>) -> bool {
        let deadline = Instant::now() + GUARD;
        while Instant::now() < deadline {
            if writer.is_finished() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "relies on the socket buffer of the Unix transport filling up"
    )]
    fn a_writer_stuck_on_a_full_socket_ends_when_the_game_dies() {
        let (editor, game) = link();
        let (outbox, writer) = GameOutbox::start(&editor).expect("a writer");
        for _ in 0..100_000 {
            outbox.queue(vec![0; 1024]);
        }
        drop(outbox);
        drop(editor);
        thread::sleep(Duration::from_millis(200));
        assert!(!writer.is_finished(), "the socket buffer is full");

        drop(game);
        assert!(finishes(&writer), "the send fails once the peer is gone");
    }

    #[test]
    fn queueing_and_dropping_return_whatever_the_writer_is_doing() {
        let (editor, game) = link();
        let (outbox, _writer) = GameOutbox::start(&editor).expect("a writer");
        outbox.queue(vec![0; LARGE]);
        thread::sleep(Duration::from_millis(200));
        drop(game);

        let shared = outbox.shared.clone();
        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            outbox.queue(vec![1; 1024]);
            outbox.mark_closing();
            drop(outbox);
            drop(editor);
            done.send(()).ok();
        });
        finished
            .recv_timeout(GUARD)
            .expect("queueing and closing return while the writer is stuck");
        assert!(shared.lock().queue.is_empty(), "the backlog is freed");
    }

    #[test]
    fn a_closing_outbox_discards_its_backlog_and_a_plain_drop_drains_it() {
        let drained = Shared::new();
        drained.push(vec![1]);
        drained.push(vec![2]);
        drained.close(false);
        assert_eq!(drained.next(), Some(vec![1]));
        assert_eq!(drained.next(), Some(vec![2]));
        assert_eq!(drained.next(), None);

        let discarded = Shared::new();
        discarded.push(vec![1]);
        discarded.push(vec![2]);
        discarded.close(true);
        assert_eq!(discarded.next(), None);
        assert_eq!(discarded.lock().bytes, 0);
    }

    #[test]
    fn the_backlog_warning_fires_once_and_rearms_after_draining() {
        let shared = Shared::new();
        let chunk = BACKLOG_WARN_BYTES / 4;
        let mut crossings = 0;
        for _ in 0..6 {
            if shared.push(vec![0; chunk]) {
                crossings += 1;
            }
        }
        assert_eq!(crossings, 1);

        for _ in 0..5 {
            shared.next();
        }
        assert!(!shared.lock().warned, "below half, so re-armed");
        assert!(!shared.push(vec![0; chunk]));
        assert!(shared.push(vec![0; BACKLOG_WARN_BYTES]));
    }
}
