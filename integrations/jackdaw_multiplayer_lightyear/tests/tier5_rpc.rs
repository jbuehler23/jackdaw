//! Tier 5: turnkey RPC. A client sends a request; the server receives it knowing
//! the sender connection entity and replies to that client; the client receives
//! the reply. Later tasks add broadcast + directed-isolation tests to this file.
//!
//! Run: `cargo test -p jackdaw_multiplayer_lightyear --test tier5_rpc`
//!
//! Game-facing code here (the `on_ping` / `on_pong` observers and the
//! `ClientSender`/`ServerSender` calls) imports NO lightyear types - that is the
//! point of the layer. The only lightyear import is `EventSender`, used purely as
//! a test-harness readiness probe (a real game gates sends on its own UI/connection
//! state instead).
use bevy::MinimalPlugins;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use jackdaw_multiplayer_lightyear::{
    ClientMessage, ClientSender, JackdawMultiplayerClientPlugin, JackdawMultiplayerServerPlugin,
    MultiplayerAppExt, ServerMessage, ServerSender,
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
struct SentPing(bool);

#[derive(Resource, Default)]
struct PongsReceived(Vec<u32>);

#[derive(Resource, Default)]
struct LastPingFrom(Option<Entity>);

/// Build a server that replies to every `Ping` with a `Pong` directed at the
/// sender, recording the sender entity in `LastPingFrom`.
fn build_reply_server(addr: std::net::SocketAddr) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.add_plugins(JackdawMultiplayerServerPlugin {
        bind: addr,
        ..Default::default()
    });
    // Register AFTER the plugin (needs the message registry from ServerPlugins).
    app.register_message::<Ping>().register_message::<Pong>();
    app.init_resource::<LastPingFrom>();
    app.add_observer(on_ping);
    app
}

fn on_ping(
    ev: On<ClientMessage<Ping>>,
    mut tx: ServerSender<Pong>,
    mut last: ResMut<LastPingFrom>,
) {
    last.0 = Some(ev.client);
    tx.send_to(ev.client, Pong(ev.message.0));
}

/// Build a client. When `send_ping` is true it sends one `Ping(42)` the first
/// frame it is connected.
fn build_client(addr: std::net::SocketAddr, id: u64, send_ping: bool) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.add_plugins(JackdawMultiplayerClientPlugin {
        server: addr,
        client_id: id,
        tick: std::time::Duration::from_millis(50),
    });
    app.register_message::<Ping>().register_message::<Pong>();
    app.init_resource::<SentPing>();
    app.init_resource::<PongsReceived>();
    app.add_observer(on_pong);
    if send_ping {
        app.add_systems(Update, send_ping_once);
    }
    app
}

fn on_pong(ev: On<ServerMessage<Pong>>, mut got: ResMut<PongsReceived>) {
    got.0.push(ev.message.0);
}

/// Send exactly one `Ping(42)` the first frame the client connection's
/// `EventSender<Ping>` exists (i.e. once connected). `With<EventSender<Ping>>` is
/// a filter only (no data access), so it does not conflict with `ClientSender`'s
/// `&mut EventSender<Ping>`.
fn send_ping_once(
    mut sent: ResMut<SentPing>,
    mut tx: ClientSender<Ping>,
    ready: Query<(), With<EventSender<Ping>>>,
) {
    if sent.0 || ready.is_empty() {
        return;
    }
    tx.send(Ping(42));
    sent.0 = true;
}

#[test]
fn directed_round_trip() {
    let addr = free_addr();
    let mut server = build_reply_server(addr);
    let mut client = build_client(addr, 1, true);

    // Manually-driven apps must finish + cleanup so plugins whose wiring lives in
    // `Plugin::finish()` are installed; `App::update()` does not drive the
    // finish/cleanup lifecycle. The RPC send/receive systems (lightyear's message
    // trigger plumbing) are built in `MessagePlugin::finish` from the registered
    // message set, so without this the triggers never flow.
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

    assert!(ok, "client did not receive its directed Pong(42) reply");
    assert!(
        server.world().resource::<LastPingFrom>().0.is_some(),
        "server never resolved the sender connection entity"
    );
}

#[derive(Resource, Default)]
struct DidBroadcast(bool);

/// Build a server that broadcasts one `Pong(7)` to all clients the first frame at
/// least two are connected. It still registers `Ping` (protocols must match) but
/// ignores it (no `on_ping` observer).
fn build_broadcast_server(addr: std::net::SocketAddr) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.add_plugins(JackdawMultiplayerServerPlugin {
        bind: addr,
        ..Default::default()
    });
    app.register_message::<Ping>().register_message::<Pong>();
    app.init_resource::<DidBroadcast>();
    app.add_systems(Update, broadcast_when_two);
    app
}

/// `With<EventSender<Pong>>` is a filter only, so it does not conflict with
/// `ServerSender`'s `&mut EventSender<Pong>`.
fn broadcast_when_two(
    mut done: ResMut<DidBroadcast>,
    mut tx: ServerSender<Pong>,
    conns: Query<(), With<EventSender<Pong>>>,
) {
    if done.0 || conns.iter().count() < 2 {
        return;
    }
    tx.broadcast(Pong(7));
    done.0 = true;
}

#[test]
fn broadcast_reaches_all() {
    let addr = free_addr();
    let mut server = build_broadcast_server(addr);
    let mut a = build_client(addr, 1, false);
    let mut b = build_client(addr, 2, false);

    // See directed_round_trip for why finish + cleanup are required here.
    for app in [&mut server, &mut a, &mut b] {
        app.finish();
        app.cleanup();
    }

    let mut ok = false;
    for _ in 0..800 {
        server.update();
        a.update();
        b.update();
        let got_a = a.world().resource::<PongsReceived>().0.contains(&7);
        let got_b = b.world().resource::<PongsReceived>().0.contains(&7);
        if got_a && got_b {
            ok = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert!(ok, "broadcast did not reach both clients");
}

#[test]
fn directed_is_isolated() {
    let addr = free_addr();
    let mut server = build_reply_server(addr); // replies only to whoever pings
    let mut a = build_client(addr, 1, true); // sends Ping(42)
    let mut b = build_client(addr, 2, false); // never sends - must receive nothing

    // See directed_round_trip for why finish + cleanup are required here.
    for app in [&mut server, &mut a, &mut b] {
        app.finish();
        app.cleanup();
    }

    let mut a_ok = false;
    for _ in 0..800 {
        server.update();
        a.update();
        b.update();
        if a.world().resource::<PongsReceived>().0.contains(&42) {
            a_ok = true;
            // Keep stepping so any erroneous delivery to B would have time to
            // arrive, then assert below that none did.
            for _ in 0..120 {
                server.update();
                a.update();
                b.update();
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert!(a_ok, "client A did not receive its directed Pong(42)");
    assert!(
        b.world().resource::<PongsReceived>().0.is_empty(),
        "client B wrongly received a directed message: {:?}",
        b.world().resource::<PongsReceived>().0
    );
}
