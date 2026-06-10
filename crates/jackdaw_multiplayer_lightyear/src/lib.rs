//! Default lightyear backend for Jackdaw multiplayer: the turnkey runtime.
//! A game adds `JackdawMultiplayerServerPlugin` (headless) and `JackdawMultiplayerClientPlugin`;
//! the layer owns transport, connection lifecycle, player auto-spawn, room-based
//! interest management, input plumbing, movement scheduling, and authored zone
//! transitions. The game writes only its `Input` type, a movement system, and
//! `replicate::<C>()` per networked component type.
//!
//! Built on lightyear 0.26 with prediction OFF (server-authoritative + interpolation).

mod camera_gate;
mod client;
mod lifecycle;
mod rate_limit;
mod registration;
mod rooms;
mod rpc;
mod server;
mod transitions;

pub use client::JackdawMultiplayerClientPlugin;
pub use lifecycle::{ClientConnected, ClientDisconnected, PlayerSpawner, SpawnPolicy};
pub use registration::{MovementSystems, MultiplayerAppExt};
pub use rooms::{CurrentZone, ZoneRooms, move_player_to_zone, set_zone};
pub use rpc::{ClientMessage, ClientSender, RpcCommandsExt, ServerMessage, ServerSender};
pub use server::{HostedZones, JackdawMultiplayerServerPlugin};

// Re-export the native-input types a game's movement system + client input need, so
// games depend on this layer rather than `lightyear` directly. `ActionState` carries
// the player's input (read server-side in the movement system); `InputMarker` marks
// the local player's input entity (queried client-side to write input).
pub use lightyear::prelude::input::native::{ActionState, InputMarker};

// Re-export the local-player marker so a game can find the entity it controls (e.g. to
// attach a camera) without importing `lightyear`. Lightyear inserts `Controlled` on the
// client's own replicated player entity.
pub use lightyear::prelude::Controlled;

/// Netcode protocol id; client + server must agree or the handshake is rejected.
pub const PROTOCOL_ID: u64 = 0x_4A41_434B_4D50_5631; // "JACKMPV1"
