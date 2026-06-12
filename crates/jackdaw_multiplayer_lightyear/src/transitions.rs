//! Authored zone-transition handling.
//!
//! A game authors a [`ZoneTransition`] trigger volume (a box) on an entity in the
//! scene. This module's server-side system tests every player's position against
//! every trigger box each `FixedUpdate`; when a player is inside a trigger that
//! targets a *different* zone, it moves the player into that zone (room
//! re-membership via [`set_zone`]) and repositions it at the destination
//! [`SpawnPoint`]. No physics dependency: the overlap test is a pure AABB check.

use crate::lifecycle::PlayerConnection;
use crate::rooms::{CurrentZone, ZoneRooms, set_zone};
use bevy::prelude::*;
use jackdaw_multiplayer::{SpawnPoint, ZoneTransition};

/// Installs the server-side zone-transition detection system in `FixedUpdate`.
pub(crate) struct ZoneTransitionPlugin;

impl Plugin for ZoneTransitionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, detect_zone_transitions);
    }
}

/// Server-side: when a player's position is inside a [`ZoneTransition`] volume,
/// move them to the destination zone and reposition at the matching spawn.
///
/// The overlap test is exact for a rotated trigger: the player's world position is
/// transformed into the trigger box's local space via the inverse of the trigger's
/// `GlobalTransform` affine, then compared componentwise against `half_extents`.
/// The box is centered on the trigger entity (`half_extents` on each side), so we
/// test `local.abs() <= half_extents` on every axis. A player can sit inside
/// several triggers at once; the first one that targets a *different* zone wins and
/// we stop scanning that player's triggers for the tick.
///
/// Requires every trigger entity to have a populated `GlobalTransform` (authored
/// with a `Transform`; the host app's `TransformPlugin` propagates it). The same
/// holds for spawn points, whose world position becomes the player's new
/// `Transform.translation`.
fn detect_zone_transitions(
    triggers: Query<(&ZoneTransition, &GlobalTransform)>,
    spawns: Query<(&SpawnPoint, &GlobalTransform)>,
    mut players: Query<(Entity, &mut Transform, &mut CurrentZone, &PlayerConnection)>,
    mut rooms: ResMut<ZoneRooms>,
    mut commands: Commands,
) {
    for (entity, mut tf, mut zone, conn) in &mut players {
        for (trigger, ttf) in &triggers {
            // World position of the player expressed in the trigger box's local
            // frame (undoes the trigger's translation + rotation + scale).
            let local = ttf.affine().inverse().transform_point3(tf.translation);
            if !local.abs().cmple(trigger.half_extents).all() {
                continue;
            }
            // Already in the destination zone? Nothing to do for this trigger.
            if zone.0 == trigger.dest_zone {
                continue;
            }
            // Find the destination spawn by (zone, tag); skip the move if the
            // authored target is missing rather than teleporting to the origin.
            if let Some((_, sgtf)) = spawns
                .iter()
                .find(|(s, _)| s.zone == trigger.dest_zone && s.tag == trigger.dest_spawn_tag)
            {
                set_zone(
                    &mut commands,
                    &mut rooms,
                    &zone.0,
                    &trigger.dest_zone,
                    entity,
                    conn.0,
                );
                tf.translation = sgtf.translation();
                zone.0 = trigger.dest_zone.clone();
            } else {
                warn!(
                    "ZoneTransition targets zone {} spawn {:?} but no matching SpawnPoint \
                     exists; player {entity:?} not moved",
                    trigger.dest_zone, trigger.dest_spawn_tag,
                );
            }
            // One transition per player per tick: stop scanning this player's
            // triggers (the move above already changed its zone/position).
            break;
        }
    }
}
