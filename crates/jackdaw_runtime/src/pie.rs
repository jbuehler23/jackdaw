//! Play-in-editor link for the standalone runtime.
//!
//! When the game binary is launched with `--jackdaw-pie`, the editor passes the
//! name of an `ipc-channel` rendezvous it is listening on. [`pie_args`] reads
//! that out of the process arguments, [`JackdawPlugin`](crate::JackdawPlugin)
//! connects, and [`attach_pie`] installs two `Update` systems:
//!
//! - the stream system snapshots the scene's ECS state and ships it to the
//!   editor as [`StateEvent`]s, and
//! - the apply system drains [`ControlEvent`]s from the editor and drives the
//!   simulation's run state (pause / resume / stop).
//!
//! The transport reading and the system wiring are kept apart so [`attach_pie`]
//! can be exercised with a directly-supplied transport in tests.

use bevy::app::AppExit;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use bevy::time::Virtual;

use jackdaw_pie_protocol::event::{PieChannel, StateEvent, to_bytes};
use jackdaw_pie_protocol::transport::PieTransport;
use jackdaw_pie_protocol::transport_ipc::IpcChannelTransport;
use jackdaw_pie_protocol::{ControlEvent, PieMode, build_snapshot};

/// Type path the stream pulls out of a per-entity snapshot for the
/// `ComponentChanged` payload. Mirrors the loader's `TRANSFORM_TYPE_PATH`.
const TRANSFORM_TYPE_PATH: &str = "bevy_transform::components::transform::Transform";

/// Parsed `--jackdaw-pie` launch arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PieArgs {
    /// Mode the editor launched the game in.
    pub mode: PieMode,
    /// Name of the editor's ipc-channel rendezvous to connect to.
    pub server: String,
}

/// Read PIE launch arguments from the process command line.
///
/// Returns `Some` only when `--jackdaw-pie` is present. `--mode` parses
/// `play` / `editor-preview` and defaults to [`PieMode::Play`]; `--server`
/// supplies the rendezvous name (empty when omitted, which makes the later
/// connect fail and log rather than panic).
pub fn pie_args() -> Option<PieArgs> {
    args_from(std::env::args().skip(1))
}

/// [`pie_args`] split from `std::env` so the parsing is unit-testable.
fn args_from(args: impl Iterator<Item = String>) -> Option<PieArgs> {
    let args: Vec<String> = args.collect();
    if !args.iter().any(|a| a == "--jackdaw-pie") {
        return None;
    }

    let mut mode = PieMode::Play;
    let mut server = String::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--mode" => {
                if let Some(value) = args.get(i + 1) {
                    mode = match value.as_str() {
                        "editor-preview" => PieMode::EditorPreview,
                        _ => PieMode::Play,
                    };
                    i += 1;
                }
            }
            "--server" => {
                if let Some(value) = args.get(i + 1) {
                    server = value.clone();
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    Some(PieArgs { mode, server })
}

/// Holds the editor link. `IpcChannelTransport` is `Send` but not `Sync` (its
/// receiver wraps a `Cell`-held file descriptor), so it lives as a `NonSend`
/// resource and the PIE systems run on the main thread.
struct PieTransportRes(IpcChannelTransport);

/// Tracks whether the initial full snapshot has been streamed yet.
#[derive(Resource, Default)]
struct PieStreamState {
    sent_initial: bool,
}

/// Install the PIE transport and the stream / apply systems.
///
/// Separate from [`pie_args`] so tests can drive it with any
/// [`IpcChannelTransport`] (for example one from
/// [`connect`](jackdaw_pie_protocol::connect) against an in-test editor).
pub fn attach_pie(app: &mut App, transport: IpcChannelTransport) {
    app.insert_non_send_resource(PieTransportRes(transport));
    app.init_resource::<PieStreamState>();
    app.add_systems(Update, (stream_state, apply_control));
}

/// Stream the scene's ECS state to the editor.
///
/// The first run ships a full `EntitySpawned` snapshot for every entity with a
/// `Transform` on the reliable channel. Each later run reports `Transform`
/// mutations as `ComponentChanged` on the unreliable channel and entities that
/// lost their `Transform` as `EntityDespawned` on the reliable channel.
fn stream_state(
    world: &mut World,
    state: &mut SystemState<(
        Query<Entity, Changed<Transform>>,
        RemovedComponents<Transform>,
    )>,
) {
    // The transport is a `NonSend` resource; bail quietly if it is gone.
    if !world.contains_non_send::<PieTransportRes>() {
        return;
    }

    let sent_initial = world.resource::<PieStreamState>().sent_initial;

    if !sent_initial {
        let entities: Vec<Entity> = world
            .query_filtered::<Entity, With<Transform>>()
            .iter(world)
            .collect();
        let registry = world.resource::<AppTypeRegistry>().clone();
        let frames: Vec<Vec<u8>> = {
            let registry = registry.read();
            build_snapshot(world, &registry, &entities)
                .into_iter()
                .filter_map(|entity| to_bytes(&StateEvent::EntitySpawned { entity }).ok())
                .collect()
        };

        let transport = &mut world.non_send_resource_mut::<PieTransportRes>().0;
        for bytes in &frames {
            transport.send(PieChannel::Reliable, bytes);
        }
        world.resource_mut::<PieStreamState>().sent_initial = true;
        return;
    }

    // Collect changed / removed ids first, then drop the query borrow so
    // `build_snapshot` can take `&World`.
    let (changed, removed): (Vec<Entity>, Vec<Entity>) = {
        let (changed_query, mut removed_reader) = state.get(world);
        let changed = changed_query.iter().collect();
        let removed = removed_reader.read().collect();
        (changed, removed)
    };

    let registry = world.resource::<AppTypeRegistry>().clone();
    let mut changed_frames: Vec<Vec<u8>> = Vec::new();
    {
        let registry = registry.read();
        for entity in changed {
            let Some(mut snapshot) = build_snapshot(world, &registry, &[entity]).pop() else {
                continue;
            };
            let Some(value) = snapshot.components.remove(TRANSFORM_TYPE_PATH) else {
                continue;
            };
            let event = StateEvent::ComponentChanged {
                entity: entity.to_bits(),
                type_path: TRANSFORM_TYPE_PATH.to_string(),
                value,
            };
            if let Ok(bytes) = to_bytes(&event) {
                changed_frames.push(bytes);
            }
        }
    }

    let removed_frames: Vec<Vec<u8>> = removed
        .into_iter()
        .filter_map(|entity| {
            to_bytes(&StateEvent::EntityDespawned {
                entity: entity.to_bits(),
            })
            .ok()
        })
        .collect();

    let transport = &mut world.non_send_resource_mut::<PieTransportRes>().0;
    for bytes in &changed_frames {
        transport.send(PieChannel::Unreliable, bytes);
    }
    for bytes in &removed_frames {
        transport.send(PieChannel::Reliable, bytes);
    }
}

/// Apply control commands from the editor.
///
/// `Stop` writes `AppExit::Success` so the game loop tears down; `Pause` /
/// `Resume` toggle the virtual clock so gameplay systems keyed on
/// `Time<Virtual>` freeze and thaw. Unknown frames are ignored.
fn apply_control(
    mut transport: NonSendMut<PieTransportRes>,
    mut app_exit: MessageWriter<AppExit>,
    mut virtual_time: ResMut<Time<Virtual>>,
) {
    for (_, bytes) in transport.0.drain_received() {
        let Ok(event) = jackdaw_pie_protocol::event::from_bytes::<ControlEvent>(&bytes) else {
            continue;
        };
        match event {
            ControlEvent::Stop => {
                app_exit.write(AppExit::Success);
            }
            ControlEvent::Pause => virtual_time.pause(),
            ControlEvent::Resume => virtual_time.unpause(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::AppExit;
    use jackdaw_pie_protocol::event::{ControlEvent, PieChannel, to_bytes};
    use jackdaw_pie_protocol::{IpcChannelTransport, connect, serve};

    /// Build a headless app wired with the PIE systems and a single
    /// `Name` + `Transform` entity, returning the spawned entity.
    fn headless_pie_app(transport: IpcChannelTransport) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.register_type::<Name>();
        app.register_type::<Transform>();
        let entity = app
            .world_mut()
            .spawn((Name::new("pie-probe"), Transform::from_xyz(1.0, 2.0, 3.0)))
            .id();
        attach_pie(&mut app, transport);
        (app, entity)
    }

    /// Full round-trip in one process: the app streams its initial snapshot to
    /// an in-test "editor" over ipc-channel, and a `Stop` from the editor makes
    /// the app write `AppExit`.
    #[test]
    fn streams_spawn_and_stops_on_control() {
        let (handle, name) = serve().expect("serve");

        // Editor side runs on a worker thread: accept the connection, collect
        // received frames, then send Stop once the spawn frame has arrived.
        let editor = std::thread::spawn(move || {
            let mut editor = handle.accept().expect("accept");
            let mut received: Vec<StateEvent> = Vec::new();
            for _ in 0..100_000 {
                for (_, bytes) in editor.drain_received() {
                    if let Ok(event) = jackdaw_pie_protocol::event::from_bytes::<StateEvent>(&bytes)
                    {
                        received.push(event);
                    }
                }
                if !received.is_empty() {
                    break;
                }
                std::thread::yield_now();
            }
            editor.send(
                PieChannel::Reliable,
                &to_bytes(&ControlEvent::Stop).expect("encode"),
            );
            // Keep the editor end alive until the app has had a chance to read
            // the Stop frame; the app side signals completion by joining.
            std::thread::sleep(std::time::Duration::from_millis(200));
            received
        });

        let transport = connect(&name).expect("connect");
        let (mut app, entity) = headless_pie_app(transport);

        // A few updates: the first streams the snapshot; later ones pick up the
        // Stop frame from the editor.
        let mut exited = false;
        for _ in 0..50 {
            app.update();
            if !app.world().resource::<Messages<AppExit>>().is_empty() {
                exited = true;
                break;
            }
            std::thread::yield_now();
        }

        let received = editor.join().expect("editor thread");

        let spawned: Vec<&StateEvent> = received
            .iter()
            .filter(|e| matches!(e, StateEvent::EntitySpawned { .. }))
            .collect();
        assert!(
            spawned
                .iter()
                .any(|e| matches!(e, StateEvent::EntitySpawned { entity: re } if re.entity == entity.to_bits())),
            "editor should receive an EntitySpawned for the probe entity; got {received:?}"
        );
        assert!(
            exited,
            "app should write AppExit after the editor sends Stop"
        );
    }

    #[test]
    fn pie_args_requires_flag() {
        assert!(args_from(std::iter::empty()).is_none());
        let parsed = args_from(
            ["--jackdaw-pie", "--mode", "play", "--server", "rv-123"]
                .into_iter()
                .map(str::to_owned),
        );
        assert_eq!(
            parsed,
            Some(PieArgs {
                mode: PieMode::Play,
                server: "rv-123".to_string(),
            })
        );
    }

    #[test]
    fn pie_args_parses_editor_preview_and_defaults() {
        let preview = args_from(
            [
                "--jackdaw-pie",
                "--mode",
                "editor-preview",
                "--server",
                "rv",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("present");
        assert_eq!(preview.mode, PieMode::EditorPreview);

        // `--mode` omitted defaults to Play; `--server` omitted leaves it empty.
        let defaulted =
            args_from(["--jackdaw-pie"].into_iter().map(str::to_owned)).expect("present");
        assert_eq!(defaulted.mode, PieMode::Play);
        assert!(defaulted.server.is_empty());
    }
}
