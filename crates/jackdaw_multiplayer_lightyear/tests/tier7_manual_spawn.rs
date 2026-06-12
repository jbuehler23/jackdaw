//! Tier 7: Manual spawn policy. `Manual` suppresses the on-connect auto-spawn; the
//! game calls `PlayerSpawner::spawn` itself.
//!
//! Run: `cargo test -p jackdaw_multiplayer_lightyear --test tier7_manual_spawn`
use bevy::MinimalPlugins;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use jackdaw_multiplayer::{SpawnPoint, ZoneId};
use jackdaw_multiplayer_lightyear::{
    JackdawMultiplayerClientPlugin, JackdawMultiplayerServerPlugin, PlayerSpawner, SpawnPolicy,
};
use lightyear::prelude::{ControlledBy, NetworkVisibility, Replicate};

fn free_addr() -> std::net::SocketAddr {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let a = s.local_addr().unwrap();
    drop(s);
    a
}

fn manual_server(addr: std::net::SocketAddr) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.add_plugins(JackdawMultiplayerServerPlugin {
        bind: addr,
        spawn_policy: SpawnPolicy::Manual,
        ..Default::default()
    });
    app.world_mut().spawn((
        Transform::default(),
        SpawnPoint {
            zone: ZoneId::from("1"),
            tag: String::new(),
        },
    ));
    app
}

fn client(addr: std::net::SocketAddr) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.add_plugins(JackdawMultiplayerClientPlugin {
        server: addr,
        client_id: 1,
        tick: std::time::Duration::from_millis(50),
    });
    app
}

fn player_count(app: &mut App) -> usize {
    let mut q = app
        .world_mut()
        .query_filtered::<(), (With<Replicate>, With<ControlledBy>, With<NetworkVisibility>)>();
    q.iter(app.world()).count()
}

#[test]
fn manual_policy_suppresses_autospawn() {
    let addr = free_addr();
    let mut server = manual_server(addr);
    let mut client = client(addr);
    for app in [&mut server, &mut client] {
        app.finish();
        app.cleanup();
    }
    for _ in 0..400 {
        server.update();
        client.update();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        player_count(&mut server),
        0,
        "Manual policy must NOT auto-spawn a player on connect"
    );
}

#[derive(Resource, Default)]
struct Spawned(bool);

fn spawn_on_connected(
    ev: On<jackdaw_multiplayer_lightyear::ClientConnected>,
    mut spawner: PlayerSpawner,
    mut done: ResMut<Spawned>,
) {
    if spawner.spawn(ev.client, &ZoneId::from("1"), "").is_some() {
        done.0 = true;
    }
}

#[test]
fn spawner_spawns_under_manual() {
    let addr = free_addr();
    let mut server = manual_server(addr);
    server.init_resource::<Spawned>();
    server.add_observer(spawn_on_connected);
    let mut client = client(addr);
    for app in [&mut server, &mut client] {
        app.finish();
        app.cleanup();
    }
    let mut ok = false;
    for _ in 0..600 {
        server.update();
        client.update();
        if player_count(&mut server) >= 1 {
            ok = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        ok,
        "PlayerSpawner::spawn did not spawn a networked player under Manual policy"
    );
}
