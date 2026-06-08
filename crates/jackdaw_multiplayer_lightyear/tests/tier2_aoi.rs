//! Tier 2: the critical area-of-interest proof. Two clients connect into the
//! SAME zone and each must SEE the other player (room-scoped replication: the
//! visibility trifecta `Replicate(All) + NetworkVisibility + shared Room`). Then
//! one player is moved to a different zone and both clients must lose sight of
//! the cross-zone player (the room cull / auto-despawn).
//!
//! Run: `cargo test -p jackdaw_multiplayer_lightyear --test tier2_aoi`
use bevy::MinimalPlugins;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use jackdaw_multiplayer::{SpawnPoint, ZoneId};
use jackdaw_multiplayer_lightyear::{
    JackdawMultiplayerClientPlugin, JackdawMultiplayerServerPlugin, move_player_to_zone,
};
use lightyear::prelude::{AppComponentExt, Controlled, ControlledBy};
use serde::{Deserialize, Serialize};

/// Test-only replicated marker so the clients can identify "player" entities.
/// The server inserts it on every auto-spawned player (via the observer below);
/// it replicates to the clients alongside the rest of the player bundle.
#[derive(Component, Serialize, Deserialize, Clone, PartialEq, Debug, Default, Reflect)]
struct PlayerMarker;

fn free_addr() -> std::net::SocketAddr {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let a = s.local_addr().unwrap();
    drop(s);
    a
}

/// Build a client app with a distinct netcode id, registering `PlayerMarker` so
/// it deserializes the replicated marker.
fn make_client(addr: std::net::SocketAddr, client_id: u64) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.add_plugins(JackdawMultiplayerClientPlugin {
        server: addr,
        client_id,
        tick: std::time::Duration::from_millis(50),
    });
    app.register_component::<PlayerMarker>();
    app
}

/// Count player entities a client can see that it does NOT control — i.e. the
/// OTHER players. The controlled entity gets a `Controlled` marker on receipt;
/// everyone else's replicated player does not.
fn other_players(app: &mut App) -> usize {
    let mut q = app
        .world_mut()
        .query_filtered::<(), (With<PlayerMarker>, Without<Controlled>)>();
    q.iter(app.world()).count()
}

fn tick(server: &mut App, c1: &mut App, c2: &mut App) {
    server.update();
    c1.update();
    c2.update();
    std::thread::sleep(std::time::Duration::from_millis(5));
}

#[test]
fn two_clients_see_each_other_then_cross_zone_culls() {
    let addr = free_addr();

    // ---- server: one app hosting two zones; both clients spawn into zone 1 ----
    let mut server = App::new();
    server.add_plugins((MinimalPlugins, StatesPlugin));
    server.add_plugins(JackdawMultiplayerServerPlugin {
        bind: addr,
        ..Default::default()
    });
    server.register_component::<PlayerMarker>();
    // The auto-spawn picks the empty-tag spawn point; zone 1 is the starter.
    server.world_mut().spawn((
        Transform::default(),
        SpawnPoint {
            zone: ZoneId::from("1"),
            tag: String::new(),
        },
    ));
    // Zone 2 is a real place to move into (not strictly required for the cull,
    // but keeps the destination a genuine authored zone).
    server.world_mut().spawn((
        Transform::default(),
        SpawnPoint {
            zone: ZoneId::from("2"),
            tag: String::new(),
        },
    ));
    // Tag every auto-spawned player with PlayerMarker. `ControlledBy` is inserted
    // exactly once per player in the auto-spawn bundle, so this fires per player.
    server.add_observer(|add: On<Add, ControlledBy>, mut commands: Commands| {
        commands.entity(add.entity).insert(PlayerMarker);
    });

    // ---- two clients, DISTINCT ids ----
    let mut c1 = make_client(addr, 1);
    let mut c2 = make_client(addr, 2);

    // Manually-driven apps (no `App::run`) must finish + cleanup their plugins,
    // otherwise plugins whose wiring lives in `Plugin::finish()` are never
    // installed. lightyear builds its replication BUFFER system (the one that
    // actually serializes spawns to send) in `ReplicationSendPlugin::finish()`,
    // so without this no entity ever replicates. `App::update()` (unlike
    // `App::run()`) does not drive the finish/cleanup lifecycle.
    for app in [&mut server, &mut c1, &mut c2] {
        app.finish();
        app.cleanup();
    }

    // ---- (a) both clients must come to see the OTHER player ----
    let mut saw_each_other = false;
    for _ in 0..1000 {
        tick(&mut server, &mut c1, &mut c2);
        if other_players(&mut c1) >= 1 && other_players(&mut c2) >= 1 {
            saw_each_other = true;
            break;
        }
    }
    assert!(
        saw_each_other,
        "each client should see the other player in the shared zone \
         (c1 sees {}, c2 sees {})",
        other_players(&mut c1),
        other_players(&mut c2),
    );
    // Sanity: the server actually spawned two players.
    let server_players = {
        let mut q = server
            .world_mut()
            .query_filtered::<(), With<PlayerMarker>>();
        q.iter(server.world()).count()
    };
    assert_eq!(server_players, 2, "server should have spawned two players");

    // ---- (b) move ONE player to zone 2; the cross-zone view must cull ----
    // Pick any one server-side player and relocate it. `move_player_to_zone`
    // reads its CurrentZone (1) + owning connection and fires the four
    // RoomEvents: the moved player's sender leaves room 1 / joins room 2, and
    // the moved player's entity leaves room 1 / joins room 2.
    let moved = {
        let mut q = server
            .world_mut()
            .query_filtered::<Entity, With<PlayerMarker>>();
        q.iter(server.world()).next().expect("a player to move")
    };
    move_player_to_zone(server.world_mut(), moved, ZoneId::from("2"));

    // After the cull both directions lose visibility:
    //  - the moved client no longer shares a room with the unmoved player, and
    //  - the unmoved client no longer shares a room with the moved player.
    // So the TOTAL count of "other players" across both client worlds must reach
    // 0 (each client auto-despawns the player it can no longer see). Asserting
    // the sum proves the cull fired in both directions without depending on a
    // fragile server-entity → netcode-id mapping.
    let mut culled = false;
    let mut last = (usize::MAX, usize::MAX);
    for _ in 0..1000 {
        tick(&mut server, &mut c1, &mut c2);
        last = (other_players(&mut c1), other_players(&mut c2));
        if last.0 + last.1 == 0 {
            culled = true;
            break;
        }
    }
    assert!(
        culled,
        "after a cross-zone move neither client should still see a player from \
         the other zone (c1 still sees {}, c2 still sees {})",
        last.0, last.1,
    );
}
