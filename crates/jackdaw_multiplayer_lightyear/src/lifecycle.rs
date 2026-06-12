use crate::rooms::{CurrentZone, ZoneRooms, join_zone};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use jackdaw_multiplayer::{ReplTarget, Replication, SpawnPoint, ZoneId};
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{
    Connected, ControlledBy, Disconnected, InterpolationTarget, Lifetime, LinkOf, NetworkTarget,
    NetworkVisibility, Replicate, ReplicationSender, SendUpdatesMode,
};
use std::time::Duration;

/// Fired (server side) when a client connection completes the handshake. `client`
/// is the connection (`ClientOf`) entity, the natural key for per-connection game
/// state. Lightyear-free: a game observes `On<ClientConnected>` without importing
/// lightyear types.
#[derive(Event)]
pub struct ClientConnected {
    pub client: Entity,
}

/// Fired (server side) when a client connection is lost. `client` is the connection
/// entity (still valid to read here: lightyear inserts `Disconnected` just before
/// despawning it). A game observes `On<ClientDisconnected>` to tear down state.
#[derive(Event)]
pub struct ClientDisconnected {
    pub client: Entity,
}

/// Whether the turnkey server auto-spawns a player on connect or leaves spawning to
/// the game. `OnConnect` (default) preserves the original behavior; `Manual` suppresses
/// the on-connect spawn so a login-gated game spawns on its own signal via
/// [`PlayerSpawner`].
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpawnPolicy {
    /// Spawn a player as soon as a connection completes the handshake (original behavior).
    #[default]
    OnConnect,
    /// Do not auto-spawn; the game calls [`PlayerSpawner::spawn`] when ready.
    Manual,
}

/// Links a server-spawned player entity back to the connection (`ClientOf`) link
/// that owns it. `move_player_to_zone` reads it to re-route the player's sender
/// between rooms on a zone transition; Phase 6 input routing reuses it.
#[derive(Component, Clone, Copy)]
pub(crate) struct PlayerConnection(pub Entity);

/// Choose the scene's default spawn: the empty-tag spawn point if present, else
/// any spawn point. Returns its zone and world position.
fn pick_default_spawn<'a>(
    spawns: impl Iterator<Item = (&'a SpawnPoint, Vec3)>,
) -> Option<(ZoneId, Vec3)> {
    let mut fallback = None;
    for (spawn, pos) in spawns {
        if spawn.tag.is_empty() {
            return Some((spawn.zone.clone(), pos));
        }
        fallback.get_or_insert((spawn.zone.clone(), pos));
    }
    fallback
}

/// Build the full networking bundle for a player owned by `connection`, place it at
/// `pos`, and join it to `zone`'s room. Returns the player entity. Shared by the
/// on-connect auto-spawn and the public [`PlayerSpawner`].
fn spawn_player_bundle(
    commands: &mut Commands,
    rooms: &mut ZoneRooms,
    connection: Entity,
    zone: &ZoneId,
    pos: Vec3,
) -> Entity {
    let player = commands
        .spawn((
            Transform::from_translation(pos),
            Replicate::to_clients(NetworkTarget::All),
            NetworkVisibility,
            ControlledBy {
                owner: connection,
                lifetime: Lifetime::SessionBased,
            },
            InterpolationTarget::to_clients(NetworkTarget::All),
            PlayerConnection(connection),
            CurrentZone(zone.clone()),
        ))
        .id();
    join_zone(commands, rooms, zone, player, connection);
    player
}

/// Spawns a player into a zone on demand: the `Manual`-spawn path (e.g. a game
/// spawning on character-select). Looks up the `SpawnPoint` matching `(zone, tag)`.
#[derive(SystemParam)]
pub struct PlayerSpawner<'w, 's> {
    spawns: Query<'w, 's, (&'static SpawnPoint, &'static GlobalTransform)>,
    rooms: ResMut<'w, ZoneRooms>,
    commands: Commands<'w, 's>,
}

impl PlayerSpawner<'_, '_> {
    /// Spawn a player owned by `connection` at the `SpawnPoint` tagged `tag` in `zone`.
    /// Returns the player entity, or `None` if no matching `SpawnPoint` exists.
    pub fn spawn(&mut self, connection: Entity, zone: &ZoneId, tag: &str) -> Option<Entity> {
        let pos = {
            let (_, gtf) = self
                .spawns
                .iter()
                .find(|(s, _)| s.zone == *zone && s.tag == tag)?;
            gtf.translation()
        };
        Some(spawn_player_bundle(
            &mut self.commands,
            &mut self.rooms,
            connection,
            zone,
            pos,
        ))
    }

    /// Spawn `connection`'s player at the scene's default spawn point (empty tag,
    /// else any), taking the zone and position from that `SpawnPoint`. Returns the
    /// player entity and its spawn world position, or `None` if the scene has no
    /// `SpawnPoint`.
    pub fn spawn_at_default(&mut self, connection: Entity) -> Option<(Entity, Vec3)> {
        let (zone, pos) =
            pick_default_spawn(self.spawns.iter().map(|(s, gtf)| (s, gtf.translation())))?;
        let player =
            spawn_player_bundle(&mut self.commands, &mut self.rooms, connection, &zone, pos);
        Some((player, pos))
    }
}

pub(crate) struct ServerLifecyclePlugin;

impl Plugin for ServerLifecyclePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<jackdaw_multiplayer::JackdawMultiplayerTypesPlugin>() {
            app.add_plugins(jackdaw_multiplayer::JackdawMultiplayerTypesPlugin);
        }
        app.add_systems(Update, apply_replication_proxies);
        app.add_observer(on_link_add);
        app.add_observer(on_client_connected);
        app.add_observer(on_client_disconnected);
    }
}

/// When the server spawns a per-connection link entity, attach a `ReplicationSender`
/// so it can replicate state to that client. Mirrors lightyear's documented
/// `handle_new_client` pattern (facade `src/lib.rs:116-126`).
fn on_link_add(add: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(add.entity).insert(ReplicationSender::new(
        Duration::from_millis(100),
        SendUpdatesMode::SinceLastAck,
        false,
    ));
}

/// When a connection finishes the netcode handshake, fire `ClientConnected` (always)
/// and, unless `SpawnPolicy::Manual`, auto-spawn the player entity with the full
/// networking bundle and join it to its zone room.
fn on_client_connected(
    add: On<Add, Connected>,
    connections: Query<(), With<ClientOf>>,
    policy: Res<SpawnPolicy>,
    spawns: Query<(&SpawnPoint, &GlobalTransform)>,
    mut rooms: ResMut<ZoneRooms>,
    mut commands: Commands,
) {
    // Only react to server-side connection entities (a `ClientOf` link), not the
    // client app's own `Client` entity which also gains `Connected`.
    if connections.get(add.entity).is_err() {
        return;
    }

    commands.trigger(ClientConnected { client: add.entity });

    if *policy == SpawnPolicy::Manual {
        return;
    }

    let Some((zone, pos)) =
        pick_default_spawn(spawns.iter().map(|(s, gtf)| (s, gtf.translation())))
    else {
        warn!("client connected but the world has no SpawnPoint; no player spawned");
        return;
    };
    spawn_player_bundle(&mut commands, &mut rooms, add.entity, &zone, pos);
}

/// Emit `ClientDisconnected` when a server-side connection is torn down. Lightyear
/// inserts `Disconnected` on the `ClientOf` entity immediately before despawning it
/// (specifically to let observers run), so the entity id is still valid to emit.
fn on_client_disconnected(
    add: On<Add, Disconnected>,
    connections: Query<(), With<ClientOf>>,
    mut commands: Commands,
) {
    // Only server-side connection entities (the client app's own Client entity also
    // gains Disconnected on shutdown).
    if connections.get(add.entity).is_err() {
        return;
    }
    commands.trigger(ClientDisconnected { client: add.entity });
}

/// Translate authored `Replication` proxy components into real lightyear
/// `Replicate` + `NetworkVisibility` (so authored networked props are room-gatable),
/// plus `InterpolationTarget` when requested.
fn apply_replication_proxies(
    mut commands: Commands,
    added: Query<(Entity, &Replication), Added<Replication>>,
) {
    for (e, proxy) in &added {
        let target = match proxy.target {
            ReplTarget::All => NetworkTarget::All,
            ReplTarget::None => NetworkTarget::None,
        };
        commands
            .entity(e)
            .insert((Replicate::to_clients(target), NetworkVisibility));
        if proxy.interpolated {
            commands
                .entity(e)
                .insert(InterpolationTarget::to_clients(NetworkTarget::All));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pick_default_spawn;
    use bevy::math::Vec3;
    use jackdaw_multiplayer::{SpawnPoint, ZoneId};

    #[test]
    fn spawn_at_default_prefers_empty_tag_then_falls_back() {
        let spawns = [
            (
                SpawnPoint {
                    zone: ZoneId::from("arena"),
                    tag: "ring".to_string(),
                },
                Vec3::new(9.0, 0.0, 0.0),
            ),
            (
                SpawnPoint {
                    zone: ZoneId::from("lobby"),
                    tag: String::new(),
                },
                Vec3::new(1.0, 2.0, 3.0),
            ),
        ];
        let chosen = pick_default_spawn(spawns.iter().map(|(s, p)| (s, *p)));
        assert_eq!(
            chosen,
            Some((ZoneId::from("lobby"), Vec3::new(1.0, 2.0, 3.0)))
        );

        let only_named = [(
            SpawnPoint {
                zone: ZoneId::from("arena"),
                tag: "ring".to_string(),
            },
            Vec3::new(9.0, 0.0, 0.0),
        )];
        let chosen = pick_default_spawn(only_named.iter().map(|(s, p)| (s, *p)));
        assert_eq!(
            chosen,
            Some((ZoneId::from("arena"), Vec3::new(9.0, 0.0, 0.0)))
        );
    }

    use super::*;
    use bevy::MinimalPlugins;
    use bevy::state::app::StatesPlugin;
    use jackdaw_multiplayer::{ReplTarget, Replication};
    use lightyear::prelude::Replicate;

    #[test]
    fn proxy_translates_to_real_replicate_with_runtime_present() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(StatesPlugin);
        // Stand up lightyear's server replication runtime (so Replicate's hook
        // is satisfied) + our translation plugin.
        app.add_plugins(lightyear::prelude::server::ServerPlugins {
            tick_duration: core::time::Duration::from_millis(100),
        });
        app.add_plugins(ServerLifecyclePlugin);

        let e = app
            .world_mut()
            .spawn(Replication {
                target: ReplTarget::All,
                interpolated: false,
            })
            .id();
        app.update();
        app.update();

        assert!(
            app.world().entity(e).contains::<Replicate>(),
            "proxy Replication should be translated into a real lightyear Replicate",
        );
    }
}
