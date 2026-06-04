//! Tier 4: the authored zone-transition proof. A player auto-spawns in zone 1 at
//! the empty-tag spawn, which is *inside* a `ZoneTransition` trigger volume that
//! targets (zone 2, tag "gate"). On the next server `FixedUpdate` the server-side
//! `detect_zone_transitions` system fires: it re-routes the player's room
//! membership (zone 1 → zone 2) and repositions the player at the zone-2 "gate"
//! spawn. We assert the server-side player ends in zone 2 at the gate position.
//!
//! This exercises the whole Phase 6 surface: AABB overlap detection (no physics),
//! `set_zone` re-membership, and authored-spawn repositioning.
//!
//! Run: `cargo test -p jackdaw_multiplayer_lightyear --test tier4_transition`
use bevy::MinimalPlugins;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use jackdaw_multiplayer::{SpawnPoint, ZoneTransition};
use jackdaw_multiplayer_lightyear::{
    CurrentZone, JackdawMultiplayerClientPlugin, JackdawMultiplayerServerPlugin,
};

/// The zone-1 starter spawn (where the player auto-spawns) AND the location of the
/// trigger volume — so the freshly spawned player is immediately inside it.
const P1: Vec3 = Vec3::new(10.0, 0.0, -5.0);
/// The zone-2 "gate" destination spawn the player is repositioned to.
const P2: Vec3 = Vec3::new(-40.0, 3.0, 100.0);

fn free_addr() -> std::net::SocketAddr {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let a = s.local_addr().unwrap();
    drop(s);
    a
}

#[test]
fn authored_trigger_moves_player_to_destination_zone_and_spawn() {
    let addr = free_addr();

    // ---- server: hosts zones 1 + 2, plus a trigger volume at P1 → zone 2/"gate" ----
    let mut server = App::new();
    // `MinimalPlugins` does NOT include `TransformPlugin`, so without it `Transform`
    // would never propagate into `GlobalTransform` (it would stay identity/zero) and
    // the authored trigger/spawn *world* positions the transition system + lifecycle
    // read would all be zero. We deliberately do NOT add `TransformPlugin` here:
    // `JackdawMultiplayerServerPlugin` adds it itself (idempotently), so this test proves
    // the turnkey layer self-provides it on a headless server.
    server.add_plugins((MinimalPlugins, StatesPlugin));
    server.add_plugins(JackdawMultiplayerServerPlugin {
        bind: addr,
        ..Default::default()
    });
    // Zone-1 default spawn: the player auto-spawns here (empty tag).
    server.world_mut().spawn((
        Transform::from_translation(P1),
        SpawnPoint {
            zone: 1,
            tag: String::new(),
        },
    ));
    // Zone-2 "gate" spawn: the transition destination.
    server.world_mut().spawn((
        Transform::from_translation(P2),
        SpawnPoint {
            zone: 2,
            tag: "gate".to_string(),
        },
    ));
    // The trigger volume, centered at P1 (so the just-spawned player is inside it).
    // half_extents = 2.0 on each axis; the player sits at P1's center → local ≈ 0.
    server.world_mut().spawn((
        Transform::from_translation(P1),
        ZoneTransition {
            dest_zone: 2,
            dest_spawn_tag: "gate".to_string(),
            half_extents: Vec3::splat(2.0),
        },
    ));

    // ---- one client ----
    let mut client = App::new();
    // Likewise no explicit `TransformPlugin`: `JackdawMultiplayerClientPlugin` adds it
    // itself (idempotently).
    client.add_plugins((MinimalPlugins, StatesPlugin));
    client.add_plugins(JackdawMultiplayerClientPlugin {
        server: addr,
        client_id: 1,
        tick: std::time::Duration::from_millis(50),
    });

    // Manually-driven apps must finish + cleanup so plugins whose wiring lives in
    // `Plugin::finish()` (lightyear's replication buffer / netcode) are installed.
    for app in [&mut server, &mut client] {
        app.finish();
        app.cleanup();
    }

    // Budget: connect + handshake + auto-spawn (with CurrentZone) + one FixedUpdate
    // where the trigger fires. ~1000 ticks is ample.
    let mut transitioned = false;
    let mut last_zone: Option<u64> = None;
    let mut last_pos: Option<Vec3> = None;
    for _ in 0..1000 {
        server.update();
        client.update();
        std::thread::sleep(std::time::Duration::from_millis(5));

        // Inspect the (single) server-side player. Only auto-spawned players carry
        // `CurrentZone`, so this query targets the player entity.
        let mut q = server.world_mut().query::<(&CurrentZone, &Transform)>();
        if let Some((zone, tf)) = q.iter(server.world()).next() {
            last_zone = Some(zone.0);
            last_pos = Some(tf.translation);
            if zone.0 == 2 && tf.translation.distance(P2) < 1.0e-3 {
                transitioned = true;
                break;
            }
        }
    }

    assert!(
        transitioned,
        "the authored trigger should move the player into zone 2 at the gate spawn \
         {P2} (player ended in zone {last_zone:?} at {last_pos:?})",
    );
}
