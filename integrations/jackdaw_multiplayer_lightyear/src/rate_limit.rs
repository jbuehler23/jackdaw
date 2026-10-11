//! Per-connection inbound-RPC rate-limiting for the turnkey server. The cap is
//! enforced at the single inbound choke point (`rpc::rewrap_incoming`): every
//! inbound RPC, across all message types, increments a per-connection counter for
//! the current 1-second window; messages over the cap are dropped (never surfaced
//! to the game). Configured by `JackdawMultiplayerServerPlugin::max_msgs_per_sec`.

use bevy::prelude::*;
use std::collections::HashMap;

/// Inbound-RPC cap per connection per second. `None` = unlimited. Inserted by the
/// server plugin from `JackdawMultiplayerServerPlugin::max_msgs_per_sec`.
#[derive(Resource)]
pub(crate) struct RpcRateLimit(pub Option<u32>);

/// Per-connection inbound count for the current window. Cleared by `reset_rpc_counts`.
#[derive(Resource, Default)]
pub(crate) struct RpcInboundCounts(pub HashMap<Entity, u32>);

/// Repeating 1-second window timer.
#[derive(Resource)]
pub(crate) struct RpcWindow(pub Timer);

/// Record an inbound message from `client` and report whether it is within the cap.
/// Unlimited (always `true`) when the cap is `None`.
pub(crate) fn allow_inbound(
    limit: &RpcRateLimit,
    counts: &mut RpcInboundCounts,
    client: Entity,
) -> bool {
    let Some(max) = limit.0 else {
        return true;
    };
    let n = counts.0.entry(client).or_insert(0);
    *n += 1;
    *n <= max
}

/// Clear all per-connection counts once per second (fixed window). Disconnected
/// entries are dropped here too, so no per-disconnect cleanup is needed.
pub(crate) fn reset_rpc_counts(
    time: Res<Time>,
    mut window: ResMut<RpcWindow>,
    mut counts: ResMut<RpcInboundCounts>,
) {
    if window.0.tick(time.delta()).just_finished() {
        counts.0.clear();
    }
}
