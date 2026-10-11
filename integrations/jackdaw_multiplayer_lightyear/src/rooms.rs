use crate::lifecycle::PlayerConnection;
use bevy::prelude::*;
use jackdaw_multiplayer::ZoneId;
use lightyear::prelude::{RoomAllocator, RoomId, Rooms};
use std::collections::HashMap;

/// Maps authored zone ids to the `RoomId` allocated for each. A room gates
/// replication: an entity that belongs to a room is received only by
/// connections that belong to the same room.
#[derive(Resource, Default)]
pub struct ZoneRooms {
    /// zone id -> allocated room id.
    pub by_zone: HashMap<ZoneId, RoomId>,
}

/// Which zone an entity currently lives in (drives room membership). Set at
/// player spawn (`on_client_connected`) and mutated on zone transitions by
/// `move_player_to_zone`.
#[derive(Component, Clone, Debug)]
pub struct CurrentZone(pub ZoneId);

/// Return the `RoomId` for `zone`, allocating one from the `RoomAllocator` the
/// first time the zone is seen. Shared by `join_zone` (initial placement) and
/// `set_zone` (re-membership on transition).
fn ensure_room(rooms: &mut ZoneRooms, allocator: &mut RoomAllocator, zone: &ZoneId) -> RoomId {
    *rooms
        .by_zone
        .entry(zone.clone())
        .or_insert_with(|| allocator.allocate())
}

/// Give both `entity` (a replicated entity) and `sender` (a connection/`ClientOf`
/// link) a `Rooms` membership for `zone`'s room, so the entity is received by that
/// connection. Allocates the zone's room on first use. An entity and a connection
/// see each other only when they share a room.
pub(crate) fn join_zone(
    commands: &mut Commands,
    rooms: &mut ZoneRooms,
    allocator: &mut RoomAllocator,
    zone: &ZoneId,
    entity: Entity,
    sender: Entity,
) {
    let room = ensure_room(rooms, allocator, zone);
    commands.entity(entity).insert(Rooms::single(room));
    commands.entity(sender).insert(Rooms::single(room));
}

/// Move a player (entity + its connection sender) from `old_zone` to `new_zone`
/// by replacing both memberships with the new zone's room. Because `Rooms::single`
/// carries only the new room, inserting it drops the old membership. The client
/// auto-despawns entities it loses visibility on. No-op if `old_zone == new_zone`.
pub fn set_zone(
    commands: &mut Commands,
    rooms: &mut ZoneRooms,
    allocator: &mut RoomAllocator,
    old_zone: &ZoneId,
    new_zone: &ZoneId,
    player: Entity,
    connection: Entity,
) {
    if old_zone == new_zone {
        return;
    }
    let new = ensure_room(rooms, allocator, new_zone);
    commands.entity(player).insert(Rooms::single(new));
    commands.entity(connection).insert(Rooms::single(new));
}

/// Move `player` to `new_zone`, reading its current zone + owning connection from
/// the player's own components. Replaces the room membership (via [`set_zone`])
/// and updates the player's [`CurrentZone`] so subsequent moves start from the
/// right room. No-op if the player is already in `new_zone`.
///
/// This is the world-level entry point a game (or a test) calls to relocate a
/// player across zones without threading `Commands`/`ZoneRooms`/`RoomAllocator` by
/// hand.
pub fn move_player_to_zone(world: &mut World, player: Entity, new_zone: ZoneId) {
    let Some(old_zone) = world.get::<CurrentZone>(player).map(|z| z.0.clone()) else {
        warn!("move_player_to_zone: entity {player:?} has no CurrentZone; ignoring");
        return;
    };
    if old_zone == new_zone {
        return;
    }
    let Some(connection) = world.get::<PlayerConnection>(player).map(|c| c.0) else {
        warn!("move_player_to_zone: entity {player:?} has no PlayerConnection; ignoring");
        return;
    };

    world.resource_scope(|world, mut rooms: Mut<ZoneRooms>| {
        world.resource_scope(|world, mut allocator: Mut<RoomAllocator>| {
            let mut commands = world.commands();
            set_zone(
                &mut commands,
                &mut rooms,
                &mut allocator,
                &old_zone,
                &new_zone,
                player,
                connection,
            );
        });
    });
    // Apply the queued `Rooms` inserts now (we hold `&mut World`), then update the
    // bookkeeping component so a subsequent move starts from `new_zone`.
    world.flush();
    if let Some(mut zone) = world.get_mut::<CurrentZone>(player) {
        zone.0 = new_zone;
    }
}
