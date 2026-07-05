//! Tier 1: a client connects and the server auto-spawns a player with the full
//! networking bundle.
//!
//! Run: `cargo test -p jackdaw_multiplayer_lightyear --test tier1_connect_spawn`
use bevy::MinimalPlugins;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use jackdaw_multiplayer::{SpawnPoint, ZoneId};
use jackdaw_multiplayer_lightyear::{
    JackdawMultiplayerClientPlugin, JackdawMultiplayerServerPlugin,
};
use lightyear::prelude::{ControlledBy, Replicate, Rooms};

fn free_addr() -> std::net::SocketAddr {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let a = s.local_addr().unwrap();
    drop(s);
    a
}

#[test]
fn client_connects_and_player_is_spawned() {
    let addr = free_addr();
    let mut server = App::new();
    server.add_plugins((MinimalPlugins, StatesPlugin));
    server.add_plugins(JackdawMultiplayerServerPlugin {
        bind: addr,
        ..Default::default()
    });
    server.world_mut().spawn((
        Transform::default(),
        SpawnPoint {
            zone: ZoneId::from("1"),
            tag: String::new(),
        },
    ));

    let mut client = App::new();
    client.add_plugins((MinimalPlugins, StatesPlugin));
    client.add_plugins(JackdawMultiplayerClientPlugin {
        server: addr,
        client_id: 0,
        tick: std::time::Duration::from_millis(50),
    });

    // Manually-driven apps must finish + cleanup so plugins whose wiring lives in
    // `Plugin::finish()` are installed: lightyear's replicon channel bridge inserts
    // `RepliconChannelMap` there, and its server packet systems read that resource
    // every update. `App::update()` does not drive the finish/cleanup lifecycle.
    for app in [&mut server, &mut client] {
        app.finish();
        app.cleanup();
    }

    let mut spawned = false;
    for _ in 0..600 {
        server.update();
        client.update();
        let mut q = server
            .world_mut()
            .query_filtered::<Entity, (With<Replicate>, With<ControlledBy>, With<Rooms>)>();
        if q.iter(server.world()).count() >= 1 {
            spawned = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        spawned,
        "server should auto-spawn a player after the client connects"
    );
}
