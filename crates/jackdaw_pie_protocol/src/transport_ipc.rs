//! `ipc-channel` transport for play-in-editor across two OS processes.
//!
//! The editor calls [`serve`] to create a one-shot rendezvous and passes the
//! returned name to the game via a CLI arg or env var; the game calls
//! [`connect`] to complete the bidirectional setup. Both ends then hold an
//! [`IpcChannelTransport`] carrying `(PieChannel, Vec<u8>)` frames.
//!
//! `ipc-channel` is reliable-ordered on every platform. The channel tag is
//! preserved on the wire so the receiver can route each frame.

use bevy::log::warn;
use ipc_channel::ipc::{self, IpcOneShotServer, IpcReceiver, IpcSender};
use ipc_channel::{IpcError, TryRecvError};
use serde::{Deserialize, Serialize};

use crate::event::PieChannel;
use crate::transport::PieTransport;

/// One message on the wire: a channel tag plus its opaque payload.
#[derive(Serialize, Deserialize)]
struct Frame {
    channel: PieChannel,
    bytes: Vec<u8>,
}

/// Payload sent over the one-shot connection to complete bidirectional setup.
/// The child ships the parent's ends of both one-way pipes so each side ends
/// up with one sender and one receiver aimed at its peer.
#[derive(Serialize, Deserialize)]
struct Bootstrap {
    parent_rx: IpcReceiver<Frame>,
    parent_tx: IpcSender<Frame>,
}

/// A bidirectional `ipc-channel` pipe between the editor and the game.
pub struct IpcChannelTransport {
    tx: IpcSender<Frame>,
    rx: IpcReceiver<Frame>,
    /// Set once a receive reports the peer's ends closed.
    hung_up: bool,
}

/// Editor-side rendezvous waiting for the game to connect. Created by
/// [`serve`]; consume it with [`accept`](IpcServerHandle::accept).
pub struct IpcServerHandle {
    server: IpcOneShotServer<Bootstrap>,
}

impl IpcServerHandle {
    /// Block until the game connects, then complete the bidirectional setup and
    /// return the editor's end of the pipe.
    pub fn accept(self) -> std::io::Result<IpcChannelTransport> {
        let (_, bootstrap) = self.server.accept().map_err(ipc_error_to_io)?;
        Ok(IpcChannelTransport {
            tx: bootstrap.parent_tx,
            rx: bootstrap.parent_rx,
            hung_up: false,
        })
    }
}

/// Editor side: create a one-shot rendezvous. Returns a handle to wait on and
/// the name to hand the game process, which passes it to [`connect`].
pub fn serve() -> std::io::Result<(IpcServerHandle, String)> {
    let (server, name) = IpcOneShotServer::<Bootstrap>::new()?;
    Ok((IpcServerHandle { server }, name))
}

/// Game side: connect to the editor's named rendezvous and complete the
/// bidirectional setup, returning the game's end of the pipe.
pub fn connect(name: &str) -> std::io::Result<IpcChannelTransport> {
    // Two one-way pipes. The child keeps the ends that talk to / hear from the
    // parent, and ships the parent's ends over the one-shot connection.
    let (child_to_parent_tx, child_to_parent_rx) = ipc::channel::<Frame>()?;
    let (parent_to_child_tx, parent_to_child_rx) = ipc::channel::<Frame>()?;

    let bootstrap_tx = IpcSender::connect(name.to_owned())?;
    bootstrap_tx
        .send(Bootstrap {
            parent_rx: child_to_parent_rx,
            parent_tx: parent_to_child_tx,
        })
        .map_err(ipc_error_to_io)?;

    Ok(IpcChannelTransport {
        tx: child_to_parent_tx,
        rx: parent_to_child_rx,
        hung_up: false,
    })
}

impl PieTransport for IpcChannelTransport {
    fn send(&mut self, channel: PieChannel, bytes: &[u8]) {
        let frame = Frame {
            channel,
            bytes: bytes.to_vec(),
        };
        if let Err(err) = self.tx.send(frame) {
            warn!("PIE ipc transport: dropping {channel:?} frame, send failed: {err}");
        }
    }

    fn drain_received(&mut self) -> Vec<(PieChannel, Vec<u8>)> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(frame) => out.push((frame.channel, frame.bytes)),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::IpcError(IpcError::Disconnected)) => {
                    self.hung_up = true;
                    break;
                }
                Err(TryRecvError::IpcError(err)) => {
                    warn!("PIE ipc transport: receive failed: {err}");
                    break;
                }
            }
        }
        out
    }

    fn peer_gone(&self) -> bool {
        self.hung_up
    }
}

/// A cloneable handle that sends on one fixed channel from any thread.
///
/// The frame capture's render-thread observer hands frames to a dedicated
/// sender thread holding one of these, so frame sends never wait for the
/// main thread (the full transport is a non-send resource pinned there).
pub struct IpcLaneSender {
    tx: IpcSender<Frame>,
    channel: PieChannel,
}

impl IpcLaneSender {
    /// Send one payload on this lane. A send failure (peer hung up) is
    /// logged once per call and otherwise dropped, matching the transport.
    pub fn send(&self, bytes: Vec<u8>) {
        if let Err(err) = self.try_send(bytes) {
            warn!(
                "PIE ipc transport: dropping {:?} frame, send failed: {err}",
                self.channel
            );
        }
    }

    /// Send one payload on this lane, leaving a failure to the caller.
    pub fn try_send(&self, bytes: Vec<u8>) -> std::io::Result<()> {
        let frame = Frame {
            channel: self.channel,
            bytes,
        };
        self.tx.send(frame).map_err(ipc_error_to_io)
    }
}

/// An [`IpcChannelTransport`] whose reliable sends go out on their own thread.
///
/// A socket write blocks while the peer is not reading, and an editor out of view
/// reads once a second. The game's main thread sends state each frame, so writing
/// from it would tie the game's frame rate to the editor's.
///
/// The queue itself has no bound: every send is kept, since some events only go
/// once. [`PieTransport::congested`] tells the game to hold what it can resend, so
/// an editor that stopped reading costs about `SEND_BACKLOG_LIMIT` of resendable
/// state. Despawns and scene requests are queued past it, since they go once.
pub struct QueuedSendTransport {
    link: IpcChannelTransport,
    queue: std::sync::mpsc::Sender<Vec<u8>>,
    backlog: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    limit: usize,
}

/// Bytes queued for the editor past which [`QueuedSendTransport`] reads congested.
/// Far more than a healthy editor is ever behind by, and small enough not to
/// matter to a game left running against one stopped in a debugger.
const SEND_BACKLOG_LIMIT: usize = 64 * 1024 * 1024;

impl QueuedSendTransport {
    /// Start the sending thread, handing `link` back if the thread cannot start.
    pub fn new(link: IpcChannelTransport) -> Result<Self, IpcChannelTransport> {
        Self::with_limit(link, SEND_BACKLOG_LIMIT)
    }

    fn with_limit(link: IpcChannelTransport, limit: usize) -> Result<Self, IpcChannelTransport> {
        use std::sync::atomic::Ordering;
        let lane = link.lane_sender(PieChannel::Reliable);
        let (queue, queued) = std::sync::mpsc::channel::<Vec<u8>>();
        let backlog = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sent = backlog.clone();
        let started = std::thread::Builder::new()
            .name("pie-state-sender".into())
            .spawn(move || {
                while let Ok(bytes) = queued.recv() {
                    let length = bytes.len();
                    lane.send(bytes);
                    sent.fetch_sub(length, Ordering::Relaxed);
                }
            });
        match started {
            Ok(_) => Ok(Self {
                link,
                queue,
                backlog,
                limit,
            }),
            Err(err) => {
                warn!("PIE ipc transport: could not start the sending thread: {err}");
                Err(link)
            }
        }
    }
}

impl PieTransport for QueuedSendTransport {
    fn send(&mut self, channel: PieChannel, bytes: &[u8]) {
        use std::sync::atomic::Ordering;
        if channel != PieChannel::Reliable {
            self.link.send(channel, bytes);
            return;
        }
        self.backlog.fetch_add(bytes.len(), Ordering::Relaxed);
        if self.queue.send(bytes.to_vec()).is_err() {
            self.backlog.fetch_sub(bytes.len(), Ordering::Relaxed);
            self.link.send(channel, bytes);
        }
    }

    fn drain_received(&mut self) -> Vec<(PieChannel, Vec<u8>)> {
        self.link.drain_received()
    }

    fn peer_gone(&self) -> bool {
        self.link.peer_gone()
    }

    fn congested(&self) -> bool {
        self.backlog.load(std::sync::atomic::Ordering::Relaxed) > self.limit
    }
}

impl IpcChannelTransport {
    /// Clone the underlying sender, fixed to one channel. `IpcSender` is
    /// `Send + Clone`, so the handle can live on another thread.
    pub fn lane_sender(&self, channel: PieChannel) -> IpcLaneSender {
        IpcLaneSender {
            tx: self.tx.clone(),
            channel,
        }
    }
}

/// Map an `ipc-channel` error to `std::io::Error` so public APIs stay
/// free of `ipc-channel` types (aside from [`IpcChannelTransport`] itself).
fn ipc_error_to_io(err: IpcError) -> std::io::Error {
    match err {
        IpcError::Io(io) => io,
        IpcError::Disconnected => {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "ipc peer disconnected")
        }
        IpcError::SerializationError(err) => {
            std::io::Error::new(std::io::ErrorKind::InvalidData, err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::PieChannel;

    /// A peer that stopped reading backs the queue up past its limit, every send
    /// is still kept, and reading again clears the congestion.
    #[test]
    fn a_peer_that_stops_reading_congests_the_queue_until_it_reads() {
        const FRAME: usize = 256 * 1024;
        const FRAMES: usize = 64;
        let (handle, name) = serve().unwrap();
        let game = std::thread::spawn(move || connect(&name).unwrap());
        let mut editor = handle.accept().unwrap();
        let game = game.join().unwrap();

        let mut queued = QueuedSendTransport::with_limit(game, 4 * FRAME)
            .map_err(|_| "no thread")
            .unwrap();
        assert!(!queued.congested());
        let frame = vec![7u8; FRAME];
        for _ in 0..FRAMES {
            queued.send(PieChannel::Reliable, &frame);
        }
        assert!(queued.congested(), "nothing reads what was sent");

        let mut received = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while received < FRAMES {
            assert!(std::time::Instant::now() < deadline, "got {received}");
            received += editor.drain_received().len();
        }
        while queued.congested() {
            assert!(std::time::Instant::now() < deadline, "still congested");
            std::thread::yield_now();
        }
    }

    #[test]
    fn ipc_round_trips_in_both_directions() {
        let (handle, name) = serve().unwrap();
        let child = std::thread::spawn(move || {
            let mut t = connect(&name).unwrap();
            // Echo: wait for a message, send it back on the same channel.
            loop {
                let got = t.drain_received();
                if let Some((ch, bytes)) = got.into_iter().next() {
                    t.send(ch, &bytes);
                    break;
                }
                std::thread::yield_now();
            }
        });

        let mut server = handle.accept().unwrap();
        server.send(PieChannel::Reliable, b"ping");

        let mut echoed = None;
        for _ in 0..100_000 {
            if let Some(m) = server.drain_received().into_iter().next() {
                echoed = Some(m);
                break;
            }
            std::thread::yield_now();
        }

        child.join().unwrap();
        assert_eq!(echoed, Some((PieChannel::Reliable, b"ping".to_vec())));
    }

    /// A peer that is not reading fills the socket, which would block a plain
    /// send. The game's main thread must not feel it.
    #[test]
    fn a_queued_send_does_not_wait_for_a_peer_that_is_not_reading() {
        let (server, name) = serve().expect("a rendezvous");
        let child = std::thread::spawn(move || connect(&name).expect("connected"));
        let mut editor = server.accept().expect("accepted");
        let game = child.join().expect("the connecting thread");
        let mut game = QueuedSendTransport::new(game)
            .ok()
            .expect("a sending thread");

        let (sent, was_sent) = std::sync::mpsc::channel();
        let sender = std::thread::spawn(move || {
            for index in 0..32u8 {
                game.send(PieChannel::Reliable, &vec![index; 1024 * 1024]);
            }
            sent.send(()).expect("the test is waiting");
            game
        });
        was_sent
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("every send returned with nobody reading");
        let _game = sender.join().expect("the sending thread");

        let mut received = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while received.len() < 32 {
            assert!(std::time::Instant::now() < deadline, "{}", received.len());
            received.extend(editor.drain_received());
            std::thread::yield_now();
        }
        for (index, (channel, bytes)) in received.iter().enumerate() {
            assert_eq!(*channel, PieChannel::Reliable);
            assert_eq!(bytes[0] as usize, index, "in order");
        }
    }

    #[test]
    fn lane_sender_sends_from_another_thread() {
        let (handle, name) = serve().unwrap();
        let child = std::thread::spawn(move || {
            let t = connect(&name).unwrap();
            let lane = t.lane_sender(PieChannel::Frames);
            let sender = std::thread::spawn(move || lane.send(vec![1, 2, 3]));
            sender.join().unwrap();
            // Keep the connection alive until the editor side drained.
            std::thread::sleep(std::time::Duration::from_millis(200));
        });
        let mut server = handle.accept().unwrap();
        let mut got = None;
        for _ in 0..100_000 {
            if let Some(m) = server.drain_received().into_iter().next() {
                got = Some(m);
                break;
            }
            std::thread::yield_now();
        }
        child.join().unwrap();
        assert_eq!(got, Some((PieChannel::Frames, vec![1, 2, 3])));
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "ipc-channel on Windows keeps a copy of a transferred sender in the receiving \
                  process, so the editor end closes only when that process exits"
    )]
    fn a_closed_peer_reads_as_gone() {
        let (handle, name) = serve().unwrap();
        let child = std::thread::spawn(move || connect(&name).unwrap());
        let server = handle.accept().unwrap();
        let mut game = child.join().unwrap();
        game.drain_received();
        assert!(!game.peer_gone(), "a live editor end is not gone");

        drop(server);
        let mut gone = false;
        for _ in 0..1000 {
            game.drain_received();
            if game.peer_gone() {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(gone, "the game end sees the editor end close");
    }
}
