use crate::lifecycle::PlayerConnection;
use bevy::prelude::*;
use jackdaw_multiplayer::ZoneId;
use lightyear::prelude::{Room, RoomEvent, RoomTarget};
use std::collections::HashMap;

/// Maps authored zone ids to the lightyear `Room` entity created for each.
#[derive(Resource, Default)]
pub struct ZoneRooms {
    /// zone id -> lightyear `Room` entity.
    pub by_zone: HashMap<ZoneId, Entity>,
}

/// Which zone an entity currently lives in (drives room membership). Set at
/// player spawn (`on_client_connected`) and mutated on zone transitions by
/// `move_player_to_zone`.
#[derive(Component, Clone, Debug)]
pub struct CurrentZone(pub ZoneId);

/// Ensure a lightyear `Room` exists for `zone`, returning its entity. Shared by
/// `join_zone` (initial placement) and `set_zone` (re-membership on transition).
fn ensure_room(commands: &mut Commands, rooms: &mut ZoneRooms, zone: &ZoneId) -> Entity {
    *rooms
        .by_zone
        .entry(zone.clone())
        .or_insert_with(|| commands.spawn(Room::default()).id())
}

/// Ensure a lightyear `Room` exists for `zone`, then place `entity` (a replicated
/// entity) and `sender` (a connection/`ClientOf` link) into it so the entity is
/// visible to that connection. Rooms gate `NetworkVisibility`: an entity is only
/// received by senders that share a room with it.
pub(crate) fn join_zone(
    commands: &mut Commands,
    rooms: &mut ZoneRooms,
    zone: &ZoneId,
    entity: Entity,
    sender: Entity,
) {
    let room = ensure_room(commands, rooms, zone);
    commands.trigger(RoomEvent {
        room,
        target: RoomTarget::AddEntity(entity),
    });
    commands.trigger(RoomEvent {
        room,
        target: RoomTarget::AddSender(sender),
    });
}

/// Move a player (entity + its connection sender) from `old_zone` to `new_zone`.
/// Fires all four [`RoomEvent`]s; the client auto-despawns entities it loses
/// visibility on. No-op if `old_zone == new_zone`.
pub fn set_zone(
    commands: &mut Commands,
    rooms: &mut ZoneRooms,
    old_zone: &ZoneId,
    new_zone: &ZoneId,
    player: Entity,
    connection: Entity,
) {
    if old_zone == new_zone {
        return;
    }
    let old = ensure_room(commands, rooms, old_zone);
    let new = ensure_room(commands, rooms, new_zone);
    commands.trigger(RoomEvent {
        room: old,
        target: RoomTarget::RemoveSender(connection),
    });
    commands.trigger(RoomEvent {
        room: old,
        target: RoomTarget::RemoveEntity(player),
    });
    commands.trigger(RoomEvent {
        room: new,
        target: RoomTarget::AddSender(connection),
    });
    commands.trigger(RoomEvent {
        room: new,
        target: RoomTarget::AddEntity(player),
    });
}

/// Move `player` to `new_zone`, reading its current zone + owning connection from
/// the player's own components. Fires the room re-membership (via [`set_zone`])
/// and updates the player's [`CurrentZone`] so subsequent moves start from the
/// right room. No-op if the player is already in `new_zone`.
///
/// This is the world-level entry point a game (or a test) calls to relocate a
/// player across zones without threading `Commands`/`ZoneRooms` by hand.
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
        let mut commands = world.commands();
        set_zone(
            &mut commands,
            &mut rooms,
            &old_zone,
            &new_zone,
            player,
            connection,
        );
    });
    // Apply the queued `RoomEvent` triggers now (we hold `&mut World`), then
    // update the bookkeeping component so a subsequent move starts from `new_zone`.
    world.flush();
    if let Some(mut zone) = world.get_mut::<CurrentZone>(player) {
        zone.0 = new_zone;
    }
}
