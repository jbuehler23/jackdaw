use std::sync::mpsc::{Receiver, Sender, channel};

use crate::event::PieChannel;

/// A byte pipe between an editor client and a game server, abstracting the
/// wire (lightyear, local socket, etc.). The protocol layer serializes
/// `PieEvent`s to bytes (`event::to_bytes`) and routes them by channel.
pub trait PieTransport {
    /// Queue `bytes` for delivery on `channel`.
    fn send(&mut self, channel: PieChannel, bytes: &[u8]);
    /// Take all messages received since the last call.
    fn drain_received(&mut self) -> Vec<(PieChannel, Vec<u8>)>;
}

/// In-process transport pair for tests: whatever one end sends the other
/// receives. Production code uses the lightyear/socket transports.
pub struct LoopbackTransport {
    outgoing: Sender<(PieChannel, Vec<u8>)>,
    incoming: Receiver<(PieChannel, Vec<u8>)>,
}

impl LoopbackTransport {
    pub fn pair() -> (Self, Self) {
        let (tx_a, rx_b) = channel();
        let (tx_b, rx_a) = channel();
        (
            Self {
                outgoing: tx_a,
                incoming: rx_a,
            },
            Self {
                outgoing: tx_b,
                incoming: rx_b,
            },
        )
    }
}

impl PieTransport for LoopbackTransport {
    fn send(&mut self, channel: PieChannel, bytes: &[u8]) {
        let _ = self.outgoing.send((channel, bytes.to_vec()));
    }

    fn drain_received(&mut self) -> Vec<(PieChannel, Vec<u8>)> {
        self.incoming.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{ControlEvent, PieChannel, to_bytes};

    #[test]
    fn loopback_delivers_sent_bytes() {
        let (mut a, mut b) = LoopbackTransport::pair();
        let msg = to_bytes(&ControlEvent::Stop).unwrap();
        a.send(PieChannel::Reliable, &msg);
        let got = b.drain_received();
        assert_eq!(got, vec![(PieChannel::Reliable, msg)]);
        assert!(a.drain_received().is_empty());
    }
}
