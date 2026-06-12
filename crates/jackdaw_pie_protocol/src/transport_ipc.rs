//! `ipc-channel` transport for play-in-editor across two OS processes.
//!
//! The editor calls [`serve`] to create a one-shot rendezvous and passes the
//! returned name to the game via a CLI arg or env var; the game calls
//! [`connect`] to complete the bidirectional setup. Both ends then hold an
//! [`IpcChannelTransport`] carrying `(PieChannel, Vec<u8>)` frames.
//!
//! `ipc-channel` is reliable-ordered on every platform, so
//! [`PieChannel::Unreliable`] frames are
//! delivered reliably here. The channel tag is preserved so transports that do
//! distinguish the two (lightyear, raw sockets) can act on it.

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
                // Nothing more queued, or the peer hung up: stop draining.
                Err(TryRecvError::Empty) | Err(TryRecvError::IpcError(IpcError::Disconnected)) => {
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
        let frame = Frame {
            channel: self.channel,
            bytes,
        };
        if let Err(err) = self.tx.send(frame) {
            warn!(
                "PIE ipc transport: dropping {:?} frame, send failed: {err}",
                self.channel
            );
        }
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
}
