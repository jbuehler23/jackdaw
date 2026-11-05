//! The queue carrying every editor-to-game message of one Live instance.
//!
//! A large `ipc-channel` send blocks until the receiver reads, and the game's
//! main thread can itself be blocked sending its state stream to the editor.
//! So the editor's main thread only queues; a writer thread does the sends, in
//! queue order.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use bevy::log::{debug, warn};
use jackdaw_pie_protocol::{IpcChannelTransport, IpcLaneSender, PieChannel};

/// The main-thread end of a writer thread.
///
/// Dropping it closes the queue and never blocks. The writer finishes what is
/// queued, then exits and releases its sender, so the game sees the link close
/// once it has read everything. If the game's process dies, the writer's sends
/// fail and it exits the same way.
///
/// The exception is a dead game that left a fragmented send unread: on Unix
/// `ipc-channel` holds both ends of that send's dedicated channel for the whole
/// call, so it never fails and the writer stays parked, with its message and
/// whatever was queued behind it, until the editor exits. It blocks nothing
/// else, and the game is gone, so no live peer waits on its sender.
pub(crate) struct GameOutbox {
    queue: Sender<Vec<u8>>,
    /// Set once the game is being torn down, when a failed send is expected.
    closing: Arc<AtomicBool>,
}

impl GameOutbox {
    /// Start the writer thread for `transport`'s reliable channel.
    pub(crate) fn new(transport: &IpcChannelTransport) -> io::Result<Self> {
        let (outbox, _writer) = Self::start(transport)?;
        Ok(outbox)
    }

    fn start(transport: &IpcChannelTransport) -> io::Result<(Self, JoinHandle<()>)> {
        let (queue, queued) = mpsc::channel();
        let closing = Arc::new(AtomicBool::new(false));
        let lane = transport.lane_sender(PieChannel::Reliable);
        let writer = thread::Builder::new()
            .name("pie-outbox".to_string())
            .spawn({
                let closing = closing.clone();
                move || write_queued(&queued, &lane, &closing)
            })?;
        Ok((Self { queue, closing }, writer))
    }

    /// Queue one encoded message behind everything already queued.
    pub(crate) fn queue(&self, bytes: Vec<u8>) {
        if self.queue.send(bytes).is_err() {
            warn!("PIE outbox: dropping a message, the writer thread is gone");
        }
    }

    /// Note that the game is about to be killed, so failed sends are expected.
    pub(crate) fn mark_closing(&self) {
        self.closing.store(true, Ordering::Release);
    }
}

impl Drop for GameOutbox {
    fn drop(&mut self) {
        self.mark_closing();
    }
}

fn write_queued(queued: &Receiver<Vec<u8>>, lane: &IpcLaneSender, closing: &AtomicBool) {
    for bytes in queued {
        if let Err(err) = lane.try_send(bytes) {
            if closing.load(Ordering::Acquire) {
                debug!("PIE outbox: message not delivered to a closing game: {err}");
            } else {
                warn!("PIE outbox: dropping a message, send failed: {err}");
            }
        }
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
    fn a_writer_stuck_in_a_large_send_to_a_dead_game_blocks_nothing() {
        let (editor, game) = link();
        let (outbox, _writer) = GameOutbox::start(&editor).expect("a writer");
        outbox.queue(vec![0; LARGE]);
        thread::sleep(Duration::from_millis(200));
        drop(game);

        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            outbox.queue(vec![1]);
            drop(outbox);
            drop(editor);
            done.send(()).ok();
        });
        finished
            .recv_timeout(GUARD)
            .expect("queueing and closing return while the writer is stuck");
    }
}
