//! Turnkey RPC surface over lightyear: typed client<->server messaging with zero
//! raw lightyear in game code. Two one-way messages paired by type (no request-id
//! correlation): a client sends a request; the server handles it knowing the
//! sender connection entity and replies to that one client.
//!
//! Built on lightyear's trigger/event API (`register_event` + `RemoteEvent<M>` +
//! `EventSender<M>`), all carried on one reliable channel the layer owns
//! (`RpcChannel`). Registration lives on `MultiplayerAppExt::register_message`.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use lightyear::prelude::{EventSender, PeerId, PeerMetadata, RemoteEvent};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// The single reliable, ordered channel every turnkey RPC rides. Private to the
/// layer; games never name it. (lightyear blanket-impls `Channel` for any
/// `Send + Sync + 'static` type, so a bare unit struct is a valid channel.)
pub(crate) struct RpcChannel;

/// Server-side receive event: a message arrived FROM a client. Observe with
/// `On<ClientMessage<M>>`. `client` is the connection entity (where per-connection
/// game state such as a session lives); `message` is the payload.
#[derive(Event)]
pub struct ClientMessage<M: Event> {
    pub client: Entity,
    pub message: M,
}

/// Client-side receive event: a message arrived FROM the server. Observe with
/// `On<ServerMessage<M>>`.
#[derive(Event)]
pub struct ServerMessage<M: Event> {
    pub message: M,
}

/// Client->server sender. Use on the client app: take `mut tx: ClientSender<MyMsg>`
/// in a client system and call `tx.send(msg)`.
#[derive(SystemParam)]
pub struct ClientSender<'w, 's, M: Event + Serialize + DeserializeOwned> {
    senders: Query<'w, 's, &'static mut EventSender<M>>,
}

impl<M: Event + Serialize + DeserializeOwned> ClientSender<'_, '_, M> {
    /// Send `msg` to the server on the reliable RPC channel. If the client is not
    /// yet connected (no `EventSender` exists) the message is dropped (not buffered)
    /// and a warning is logged.
    pub fn send(&mut self, msg: M) {
        match self.senders.single_mut() {
            Ok(mut sender) => sender.trigger::<RpcChannel>(msg),
            Err(_) => warn!(
                "ClientSender::send: no client EventSender<{}> (not connected yet?)",
                core::any::type_name::<M>()
            ),
        }
    }
}

/// Server->client sender. Use on the server app: take `mut tx: ServerSender<MyMsg>`
/// in a server system or observer and call `tx.send_to(client, msg)`.
#[derive(SystemParam)]
pub struct ServerSender<'w, 's, M: Event + Serialize + DeserializeOwned> {
    senders: Query<'w, 's, &'static mut EventSender<M>>,
}

impl<M: Event + Serialize + DeserializeOwned> ServerSender<'_, '_, M> {
    /// Send `msg` to one client by its connection entity (e.g.
    /// `ClientMessage::client`). If that entity has no `EventSender` (e.g. already
    /// disconnected) the message is dropped (not buffered) and a warning is logged.
    pub fn send_to(&mut self, client: Entity, msg: M) {
        match self.senders.get_mut(client) {
            Ok(mut sender) => sender.trigger::<RpcChannel>(msg),
            Err(_) => warn!(
                "ServerSender::send_to: no EventSender<{}> on entity {client:?}",
                core::any::type_name::<M>()
            ),
        }
    }

    /// Send `msg` to every connected client. Clones the payload once per
    /// connection.
    pub fn broadcast(&mut self, msg: M)
    where
        M: Clone,
    {
        for mut sender in &mut self.senders {
            sender.trigger::<RpcChannel>(msg.clone());
        }
    }
}

/// Observer installed once per registered type by `register_message::<M>()`. Maps
/// lightyear's `RemoteEvent<M>` into the layer's lightyear-free `ClientMessage<M>`
/// (server side) or `ServerMessage<M>` (client side), self-selecting by sender so
/// the same registration serves both apps:
/// - `from == PeerId::Server`: we are the client; emit `ServerMessage`.
/// - otherwise: we are the server; resolve the sender `PeerId` to its connection
///   `Entity` via `PeerMetadata` and emit `ClientMessage`.
///
/// Also the single server-inbound choke point, so it enforces the optional
/// per-connection rate cap (`rate_limit`): an over-cap message is dropped here (no
/// `ClientMessage` emitted). The rate-limit resources exist only on the server app,
/// so the check is a no-op on the client.
pub(crate) fn rewrap_incoming<M: Event + Clone>(
    ev: On<RemoteEvent<M>>,
    peers: Option<Res<PeerMetadata>>,
    limit: Option<Res<crate::rate_limit::RpcRateLimit>>,
    counts: Option<ResMut<crate::rate_limit::RpcInboundCounts>>,
    mut commands: Commands,
) {
    if ev.from == PeerId::Server {
        commands.trigger(ServerMessage {
            message: ev.trigger.clone(),
        });
        return;
    }

    let Some(peers) = peers else {
        return;
    };
    let Some(&client) = peers.mapping.get(&ev.from) else {
        return;
    };

    // Server inbound: enforce the per-connection rate cap if configured. The
    // resources exist only on the server app (Option = None on the client).
    if let (Some(limit), Some(mut counts)) = (limit, counts)
        && !crate::rate_limit::allow_inbound(&limit, &mut counts, client)
    {
        return; // dropped: over cap this window
    }

    commands.trigger(ClientMessage {
        client,
        message: ev.trigger.clone(),
    });
}

/// `Commands`-based RPC sends, usable anywhere you hold `&mut Commands` (generic
/// helpers, observers, async-polling systems), complementing the [`ClientSender`] /
/// [`ServerSender`] system params. Each method enqueues a command that triggers the
/// connection's `EventSender` on the reliable RPC channel.
pub trait RpcCommandsExt {
    /// Server -> one client (its connection entity). Mirrors [`ServerSender::send_to`].
    fn server_send_to<M: Event + Serialize + DeserializeOwned>(&mut self, client: Entity, msg: M);
    /// Server -> every connected client. Mirrors [`ServerSender::broadcast`].
    fn server_broadcast<M: Event + Serialize + DeserializeOwned + Clone>(&mut self, msg: M);
    /// Client -> server. Mirrors [`ClientSender::send`].
    fn client_send<M: Event + Serialize + DeserializeOwned>(&mut self, msg: M);
}

impl RpcCommandsExt for Commands<'_, '_> {
    fn server_send_to<M: Event + Serialize + DeserializeOwned>(&mut self, client: Entity, msg: M) {
        self.queue(
            move |world: &mut World| match world.get_mut::<EventSender<M>>(client) {
                Some(mut sender) => sender.trigger::<RpcChannel>(msg),
                None => warn!(
                    "server_send_to: no EventSender<{}> on entity {client:?}",
                    core::any::type_name::<M>()
                ),
            },
        );
    }

    fn server_broadcast<M: Event + Serialize + DeserializeOwned + Clone>(&mut self, msg: M) {
        self.queue(move |world: &mut World| {
            let mut q = world.query::<&mut EventSender<M>>();
            for mut sender in q.iter_mut(world) {
                sender.trigger::<RpcChannel>(msg.clone());
            }
        });
    }

    fn client_send<M: Event + Serialize + DeserializeOwned>(&mut self, msg: M) {
        self.queue(move |world: &mut World| {
            let mut q = world.query::<&mut EventSender<M>>();
            match q.single_mut(world) {
                Ok(mut sender) => sender.trigger::<RpcChannel>(msg),
                Err(_) => warn!(
                    "client_send: no unique client EventSender<{}> (not connected yet?)",
                    core::any::type_name::<M>()
                ),
            }
        });
    }
}
