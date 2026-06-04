//! Tier 6: turnkey additions — Commands-based sends, connection-lifecycle events,
//! and per-connection inbound rate-limiting.
//!
//! Run: `cargo test -p jackdaw_multiplayer_lightyear --test tier6_turnkey_additions`
use bevy::MinimalPlugins;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use jackdaw_multiplayer_lightyear::{
    ClientMessage, JackdawMultiplayerClientPlugin, JackdawMultiplayerServerPlugin,
    MultiplayerAppExt, RpcCommandsExt, ServerMessage,
};
use lightyear::prelude::EventSender;
use serde::{Deserialize, Serialize};

fn free_addr() -> std::net::SocketAddr {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let a = s.local_addr().unwrap();
    drop(s);
    a
}

#[derive(Event, Serialize, Deserialize, Clone)]
struct Ping(u32);

#[derive(Event, Serialize, Deserialize, Clone)]
struct Pong(u32);

#[derive(Resource, Default)]
struct PongsReceived(Vec<u32>);

#[derive(Resource, Default)]
struct SentOnce(bool);

#[derive(Resource, Default)]
struct DidBroadcast(bool);

// Server reply via Commands (no ServerSender param).
fn on_ping_reply_cmd(ev: On<ClientMessage<Ping>>, mut commands: Commands) {
    let client = ev.client;
    let v = ev.message.0;
    commands.server_send_to(client, Pong(v));
}

fn on_pong(ev: On<ServerMessage<Pong>>, mut got: ResMut<PongsReceived>) {
    got.0.push(ev.message.0);
}

// Client send via Commands once connected.
fn client_send_ping_once(
    mut sent: ResMut<SentOnce>,
    ready: Query<(), With<EventSender<Ping>>>,
    mut commands: Commands,
) {
    if sent.0 || ready.is_empty() {
        return;
    }
    commands.client_send(Ping(42));
    sent.0 = true;
}

fn make_client(addr: std::net::SocketAddr, id: u64) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.add_plugins(JackdawMultiplayerClientPlugin {
        server: addr,
        client_id: id,
        tick: std::time::Duration::from_millis(50),
    });
    app.register_message::<Ping>().register_message::<Pong>();
    app.init_resource::<PongsReceived>();
    app.add_observer(on_pong);
    app
}

#[test]
fn commands_round_trip() {
    let addr = free_addr();

    let mut server = App::new();
    server.add_plugins((MinimalPlugins, StatesPlugin));
    server.add_plugins(JackdawMultiplayerServerPlugin {
        bind: addr,
        ..Default::default()
    });
    server.register_message::<Ping>().register_message::<Pong>();
    server.add_observer(on_ping_reply_cmd);

    let mut client = make_client(addr, 1);
    client.init_resource::<SentOnce>();
    client.add_systems(Update, client_send_ping_once);

    for app in [&mut server, &mut client] {
        app.finish();
        app.cleanup();
    }

    let mut ok = false;
    for _ in 0..600 {
        server.update();
        client.update();
        if client.world().resource::<PongsReceived>().0.contains(&42) {
            ok = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        ok,
        "Commands client_send -> server_send_to round-trip failed"
    );
}

// Server broadcasts via Commands once two clients are connected.
fn broadcast_when_two_cmd(
    mut done: ResMut<DidBroadcast>,
    conns: Query<(), With<EventSender<Pong>>>,
    mut commands: Commands,
) {
    if done.0 || conns.iter().count() < 2 {
        return;
    }
    commands.server_broadcast(Pong(7));
    done.0 = true;
}

#[test]
fn commands_broadcast() {
    let addr = free_addr();

    let mut server = App::new();
    server.add_plugins((MinimalPlugins, StatesPlugin));
    server.add_plugins(JackdawMultiplayerServerPlugin {
        bind: addr,
        ..Default::default()
    });
    server.register_message::<Ping>().register_message::<Pong>();
    server.init_resource::<DidBroadcast>();
    server.add_systems(Update, broadcast_when_two_cmd);

    let mut a = make_client(addr, 1);
    let mut b = make_client(addr, 2);

    for app in [&mut server, &mut a, &mut b] {
        app.finish();
        app.cleanup();
    }

    let mut ok = false;
    for _ in 0..800 {
        server.update();
        a.update();
        b.update();
        let ga = a.world().resource::<PongsReceived>().0.contains(&7);
        let gb = b.world().resource::<PongsReceived>().0.contains(&7);
        if ga && gb {
            ok = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(ok, "Commands server_broadcast did not reach both clients");
}

use jackdaw_multiplayer_lightyear::{ClientConnected, ClientDisconnected};

#[derive(Resource, Default)]
struct Connects(Vec<Entity>);

#[derive(Resource, Default)]
struct Disconnects(Vec<Entity>);

fn rec_connect(ev: On<ClientConnected>, mut r: ResMut<Connects>) {
    r.0.push(ev.client);
}

fn rec_disconnect(ev: On<ClientDisconnected>, mut r: ResMut<Disconnects>) {
    r.0.push(ev.client);
}

fn connect_event_server(addr: std::net::SocketAddr) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.add_plugins(JackdawMultiplayerServerPlugin {
        bind: addr,
        ..Default::default()
    });
    app.init_resource::<Connects>();
    app.init_resource::<Disconnects>();
    app.add_observer(rec_connect);
    app.add_observer(rec_disconnect);
    app
}

// Minimal client with no registered messages — protocol agrees with connect_event_server.
fn make_plain_client(addr: std::net::SocketAddr, id: u64) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.add_plugins(JackdawMultiplayerClientPlugin {
        server: addr,
        client_id: id,
        tick: std::time::Duration::from_millis(50),
    });
    app
}

#[test]
fn client_connected_fires() {
    let addr = free_addr();
    let mut server = connect_event_server(addr);
    let mut client = make_plain_client(addr, 1);

    for app in [&mut server, &mut client] {
        app.finish();
        app.cleanup();
    }

    let mut got = None;
    for _ in 0..600 {
        server.update();
        client.update();
        if let Some(&e) = server.world().resource::<Connects>().0.first() {
            got = Some(e);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(got.is_some(), "ClientConnected did not fire");
}

#[test]
fn client_disconnected_fires() {
    let addr = free_addr();
    let mut server = connect_event_server(addr);
    let mut client = make_plain_client(addr, 1);

    for app in [&mut server, &mut client] {
        app.finish();
        app.cleanup();
    }

    let mut conn = None;
    for _ in 0..600 {
        server.update();
        client.update();
        if let Some(&e) = server.world().resource::<Connects>().0.first() {
            conn = Some(e);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let conn = conn.expect("client never connected");

    // Force a clean disconnect from the client.
    let link = {
        let mut q = client
            .world_mut()
            .query_filtered::<Entity, With<lightyear::prelude::Client>>();
        q.single(client.world()).expect("client link entity")
    };
    client
        .world_mut()
        .trigger(lightyear::prelude::Disconnect { entity: link });

    let mut ok = false;
    for _ in 0..600 {
        client.update();
        server.update();
        if server.world().resource::<Disconnects>().0.contains(&conn) {
            ok = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        ok,
        "ClientDisconnected did not fire for the connection entity"
    );
}

#[derive(Resource, Default)]
struct PingsReceived(u32);

fn count_pings(_ev: On<ClientMessage<Ping>>, mut n: ResMut<PingsReceived>) {
    n.0 += 1;
}

fn client_flood_once(
    mut sent: ResMut<SentOnce>,
    ready: Query<(), With<EventSender<Ping>>>,
    mut commands: Commands,
) {
    if sent.0 || ready.is_empty() {
        return;
    }
    for i in 0..10 {
        commands.client_send(Ping(i));
    }
    sent.0 = true;
}

#[test]
fn rate_limit_drops_over_cap() {
    let addr = free_addr();

    let mut server = App::new();
    server.add_plugins((MinimalPlugins, StatesPlugin));
    server.add_plugins(JackdawMultiplayerServerPlugin {
        bind: addr,
        max_msgs_per_sec: Some(3),
        ..Default::default()
    });
    server.register_message::<Ping>().register_message::<Pong>();
    server.init_resource::<PingsReceived>();
    server.add_observer(count_pings);

    let mut client = make_client(addr, 1);
    client.init_resource::<SentOnce>();
    client.add_systems(Update, client_flood_once);

    for app in [&mut server, &mut client] {
        app.finish();
        app.cleanup();
    }

    // The client floods 10 Pings in one burst. Reach the cap, then confirm the
    // remainder are dropped — all within the first 1-second window.
    let mut hit_cap = false;
    for _ in 0..400 {
        server.update();
        client.update();
        if server.world().resource::<PingsReceived>().0 >= 3 {
            hit_cap = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(hit_cap, "server never received up to the cap");

    // Keep stepping briefly (still inside the window) so the dropped remainder would
    // have arrived if the limiter let them through.
    for _ in 0..40 {
        server.update();
        client.update();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(
        server.world().resource::<PingsReceived>().0,
        3,
        "rate limiter let more than the cap through in one window"
    );
}
