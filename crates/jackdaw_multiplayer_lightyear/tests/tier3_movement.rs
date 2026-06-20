//! Tier 3: the server-authoritative movement + interpolation proof. A client
//! writes an input; lightyear ships it to the server; the server's authoritative
//! movement system advances that player's position; the moved player replicates
//! to the OTHER client, which smooths it via interpolation.
//!
//! This exercises the whole Phase 5 surface end-to-end (`register_input` for the
//! input plugin + client marker placement, `replicate_interpolated` for
//! replication + linear interpolation, `add_movement_system` for authoritative
//! movement in `FixedUpdate`) with prediction OFF, so movement is purely
//! server-side and remote clients see the result interpolated, never predicted.
//!
//! Run: `cargo test -p jackdaw_multiplayer_lightyear --test tier3_movement`
use bevy::MinimalPlugins;
use bevy::math::curve::{Curve, Ease};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use jackdaw_multiplayer::{SpawnPoint, ZoneId};
use jackdaw_multiplayer_lightyear::{
    JackdawMultiplayerClientPlugin, JackdawMultiplayerServerPlugin, MultiplayerAppExt,
};
use lightyear::prelude::ControlledBy;
use lightyear::prelude::Interpolated;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use serde::{Deserialize, Serialize};

/// The game's player input. Native input requires the full
/// `Serialize + DeserializeOwned + Clone + PartialEq + Debug + Default + Reflect`
/// set plus a `MapEntities` impl (empty here - the input carries no entities).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default, Reflect)]
struct TestInput {
    dir: Vec3,
}

impl bevy::ecs::entity::MapEntities for TestInput {
    fn map_entities<M: bevy::ecs::entity::EntityMapper>(&mut self, _mapper: &mut M) {}
}

/// The networked, interpolated position. A bare newtype over `Vec3` so the test
/// owns its own replicated component (and a test observer can stamp it onto each
/// auto-spawned player) without touching the layer's player bundle.
#[derive(Component, Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default, Reflect)]
struct Pos(Vec3);

/// `add_linear_interpolation` requires `Pos: Ease`. `Vec3` already implements
/// `Ease` (linear lerp); we delegate to it and re-wrap, so the interpolation
/// system lerps the inner vector between confirmed states.
impl Ease for Pos {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        let inner = Vec3::interpolating_curve_unbounded(start.0, end.0);
        inner.map(Pos)
    }
}

/// Server-authoritative movement: read the (networked) `ActionState<TestInput>`
/// the client shipped, integrate it into `Pos`. Runs in `FixedUpdate` (placed
/// there by `add_movement_system`), which is after lightyear applies inputs in
/// `FixedPreUpdate` and before it buffers replication.
fn move_stub(time: Res<Time>, mut q: Query<(&ActionState<TestInput>, &mut Pos)>) {
    for (action, mut pos) in &mut q {
        pos.0 += action.0.dir * time.delta_secs();
    }
}

/// Client-side: drive the controlled player in +X by writing into the
/// `ActionState` that `register_input`'s observer placed on the `Controlled`
/// entity. Inert until input is synced; harmless to run every frame before then.
fn write_input(mut q: Query<&mut ActionState<TestInput>, With<InputMarker<TestInput>>>) {
    for mut action in &mut q {
        action.0.dir = Vec3::X;
    }
}

fn free_addr() -> std::net::SocketAddr {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let a = s.local_addr().unwrap();
    drop(s);
    a
}

/// Build a client app: distinct netcode id, the input type + interpolated `Pos`
/// registered so it ships input and deserializes/smooths the replicated position.
fn make_client(addr: std::net::SocketAddr, client_id: u64) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.add_plugins(JackdawMultiplayerClientPlugin {
        server: addr,
        client_id,
        tick: std::time::Duration::from_millis(50),
    });
    app.register_input::<TestInput>();
    app.replicate_interpolated::<Pos>();
    app
}

fn tick(server: &mut App, c1: &mut App, c2: &mut App) {
    server.update();
    c1.update();
    c2.update();
    std::thread::sleep(std::time::Duration::from_millis(5));
}

/// The largest `Pos.x` among the server's players (the player client 1 drives
/// should advance; the other stays put since client 2 writes no input).
fn max_server_pos_x(server: &mut App) -> f32 {
    let mut q = server.world_mut().query::<&Pos>();
    q.iter(server.world())
        .map(|p| p.0.x)
        .fold(f32::NEG_INFINITY, f32::max)
}

/// The largest `Pos.x` among entities client 2 is interpolating (i.e. the moved
/// player, smoothed in place on the entity the receiver marked `Interpolated`).
fn max_interpolated_pos_x(client: &mut App) -> Option<f32> {
    let mut q = client
        .world_mut()
        .query_filtered::<&Pos, With<Interpolated>>();
    q.iter(client.world())
        .map(|p| p.0.x)
        .fold(None, |acc, x| Some(acc.map_or(x, |a: f32| a.max(x))))
}

#[test]
fn networked_input_drives_server_movement_seen_interpolated_by_peer() {
    let addr = free_addr();

    // ---- server: hosts one zone; both clients auto-spawn into it ----
    let mut server = App::new();
    server.add_plugins((MinimalPlugins, StatesPlugin));
    server.add_plugins(JackdawMultiplayerServerPlugin {
        bind: addr,
        ..Default::default()
    });
    server.register_input::<TestInput>();
    server.replicate_interpolated::<Pos>();
    server.add_movement_system(move_stub);
    server.world_mut().spawn((
        Transform::default(),
        SpawnPoint {
            zone: ZoneId::from("1"),
            tag: String::new(),
        },
    ));
    // Stamp `Pos` onto every auto-spawned player. `ControlledBy` is inserted
    // exactly once per player in the auto-spawn bundle, so this fires per player.
    server.add_observer(|add: On<Add, ControlledBy>, mut commands: Commands| {
        commands.entity(add.entity).insert(Pos(Vec3::ZERO));
    });

    // ---- two clients, distinct ids; only client 1 writes input ----
    let mut c1 = make_client(addr, 1);
    c1.add_systems(Update, write_input);
    let mut c2 = make_client(addr, 2);

    // Manually-driven apps must finish + cleanup so plugins whose wiring lives in
    // `Plugin::finish()` (lightyear's replication-buffer system, input send/recv)
    // are installed; `App::update()` does not drive the finish/cleanup lifecycle.
    for app in [&mut server, &mut c1, &mut c2] {
        app.finish();
        app.cleanup();
    }

    // Inputs are inert until `IsSynced<InputTimeline>` (a few frames post-connect),
    // so budget plenty of ticks: connect + spawn + input sync + movement + the
    // replication round-trip + interpolation buffering before either assert holds.
    let mut server_moved = false;
    let mut peer_saw_interpolated = false;
    let mut last_server_x = f32::NEG_INFINITY;
    let mut last_peer_x = None;
    for _ in 0..1500 {
        tick(&mut server, &mut c1, &mut c2);

        last_server_x = max_server_pos_x(&mut server);
        if last_server_x > 0.05 {
            server_moved = true;
        }

        last_peer_x = max_interpolated_pos_x(&mut c2);
        // Assert (ii) only matters once (i) holds: the peer can only interpolate a
        // moved position after the server has actually moved it.
        if server_moved && last_peer_x.is_some_and(|x| x > 0.01) {
            peer_saw_interpolated = true;
            break;
        }
    }

    // ---- (i) server-authoritative movement from the networked input ----
    assert!(
        server_moved,
        "the server should have advanced a player in +X from the input client 1 \
         shipped (max server Pos.x = {last_server_x})",
    );

    // ---- (ii) the OTHER client sees the moved player, interpolated in place ----
    assert!(
        peer_saw_interpolated,
        "client 2 should interpolate the moved player (an entity marked \
         `Interpolated` whose Pos advanced in +X; max interpolated Pos.x = {last_peer_x:?})",
    );
}
