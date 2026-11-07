//! Play-in-editor link for the standalone runtime.
//!
//! When `JACKDAW_PIE` is set by the editor before launch, [`pie_config`]
//! returns the rendezvous name and [`attach_pie`] installs a stream system
//! (ECS state to editor as [`StateEvent`]s) and an apply system
//! ([`ControlEvent`]s from the editor driving pause / resume / stop).

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy::app::AppExit;
use bevy::ecs::archetype::ArchetypeId;
use bevy::ecs::change_detection::Tick;
use bevy::ecs::component::ComponentId;
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::ecs::world::EntityRef;
use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use bevy::time::Virtual;

use jackdaw_pie_protocol::event::{PieChannel, StateEvent, to_bytes};
use jackdaw_pie_protocol::transport::PieTransport;
use jackdaw_pie_protocol::transport_ipc::IpcChannelTransport;
use jackdaw_pie_protocol::{ControlEvent, SnapshotColumn, snapshot_columns};
use jackdaw_scene_types::SceneNodeId;

/// The active PIE link parameters, read from the environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PieConfig {
    /// ipc-channel rendezvous name to connect to.
    pub server: String,
}

impl PieConfig {
    /// Build from the value of `JACKDAW_PIE` (the rendezvous name).
    /// Returns `None` when the variable is absent.
    pub fn from_env_value(server: Option<String>) -> Option<Self> {
        let server = server?;
        Some(PieConfig { server })
    }
}

/// Read the PIE link parameters from the process environment.
pub fn pie_config() -> Option<PieConfig> {
    PieConfig::from_env_value(std::env::var("JACKDAW_PIE").ok())
}

/// Holds the editor link. The ipc transport is `Send` but not `Sync` (its
/// receiver wraps a `Cell`-held file descriptor), so it lives as a `NonSend`
/// resource and the PIE systems run on the main thread.
pub(crate) struct PieTransportRes(pub(crate) Box<dyn PieTransport>);

/// Entity the editor asked to box in the rendered frame, or none.
#[derive(Resource, Default)]
struct HighlightedEntity(Option<Entity>);

/// How long one frame may spend serializing world state for the editor. Work
/// past this waits for the next frame; at least one item goes out each frame
/// unless the link is congested, when none does. The scan that finds the work is outside this: it
/// walks every streamed entity each frame, so it still grows with entity count.
const STREAM_BUDGET: Duration = Duration::from_millis(4);

/// What the editor has been sent, and what it still has to be sent.
#[derive(Resource, Default)]
struct PieStreamState {
    /// A digest of the last value sent, per entity then component. Entities are
    /// here once their `EntitySpawned` went out.
    sent: HashMap<u64, HashMap<ComponentId, u64>>,
    /// Entities waiting for their `EntitySpawned`, oldest first.
    unsent: VecDeque<Entity>,
    /// `unsent` as a set.
    queued: HashSet<Entity>,
    /// Components of sent entities whose value may have changed, oldest first.
    dirty: VecDeque<(Entity, ComponentId)>,
    /// `dirty` as a set, so a component changing every frame holds one place.
    dirty_set: HashSet<(Entity, ComponentId)>,
    /// Whose turn it is when spawns and changes both wait.
    spawn_next: bool,
    /// The tick of the last scan; changes newer than this are not yet queued.
    last_scan: Option<Tick>,
    /// What each archetype streams, resolved once.
    columns: HashMap<ArchetypeId, Arc<[SnapshotColumn]>>,
}

/// Install the PIE transport and the stream / apply systems.
///
/// Kept separate from [`pie_config`] so tests can supply any
/// [`IpcChannelTransport`] directly.
pub fn attach_pie(app: &mut App, transport: IpcChannelTransport) {
    app.insert_resource(crate::pie_frames::spawn_frame_sender_thread(
        transport.lane_sender(jackdaw_pie_protocol::PieChannel::Frames),
    ));
    let link: Box<dyn PieTransport> =
        match jackdaw_pie_protocol::QueuedSendTransport::new(transport) {
            Ok(queued) => Box::new(queued),
            Err(direct) => Box::new(direct),
        };
    install_link(app, link);

    if crate::pie_windowless::windowless_active(app) {
        crate::pie_windowless::setup_windowless(app);
        // Forwarded image-targeted pointer events own the mouse pointer.
        // Disable window-targeted pointer derivation outright so a stray
        // window event can never fight the forwarded stream over the pointer
        // location.
        app.insert_resource(bevy::picking::input::PointerInputSettings {
            is_mouse_enabled: false,
            is_touch_enabled: false,
        });
        // The highlight box draws through the game's own cameras so it lands
        // in the streamed frame. `Gizmos` needs the gizmo plugin, present on
        // the windowless render path but not the headless server, so it is
        // registered only here.
        app.add_systems(Update, draw_highlight);
    }
}

/// Install the control and stream systems over `transport`. Control drains
/// in `PreUpdate` whatever the virtual clock says, so edits from the editor
/// land while the game is paused.
pub(crate) fn install_link(app: &mut App, transport: Box<dyn PieTransport>) {
    if let Some(switch) = app.world().get_resource::<crate::LeaveParseToSpawn>() {
        switch.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    app.insert_non_send(PieTransportRes(transport));
    app.init_resource::<PieStreamState>();
    app.init_resource::<crate::served_scene::SceneRequests>();
    app.add_observer(crate::served_scene::forget_answer_on_insert);
    app.init_resource::<HighlightedEntity>();
    crate::node_sync::install_node_index(app);
    // Drain control in PreUpdate, before input processing so forwarded editor
    // input lands in `ButtonInput` the same frame.
    app.add_systems(
        PreUpdate,
        (apply_control, stream_cursor_state)
            .chain()
            .before(bevy::input::InputSystems),
    );
    app.add_systems(Update, stream_state);
}

/// The entities whose state streams to the editor. Derived face meshes are
/// left out (the editor regenerates faces from the streamed `Brush`), as are
/// everything the runtime adds for itself ([`crate::RuntimeOnly`]).
fn streamed_entities(world: &mut World) -> Vec<Entity> {
    type StreamFilter = (
        With<Transform>,
        Without<jackdaw_scene_types::DerivedFaceMesh>,
        Without<crate::RuntimeOnly>,
    );
    world
        .query_filtered::<Entity, StreamFilter>()
        .iter(world)
        .collect()
}

/// Stream the scene's ECS state to the editor.
///
/// A scan finds what the editor is behind on from entity membership and
/// component change ticks, reading no component values, so an idle frame costs a
/// walk of the streamed entities. Queued work then goes out until
/// [`STREAM_BUDGET`] is spent, taking turns between changes and spawns:
///   - A component whose tick moved emits `ComponentChanged`, unless its value
///     serializes the same as the one last sent.
///   - An entity not yet sent emits `EntitySpawned` with all its components.
///
/// An entity gone from the streamed set emits `EntityDespawned` straight away.
/// While the link reads congested no queued work goes out. It stays queued, each
/// component in one place, so a component changing every frame holds one place.
/// An entity spawned and despawned while congested can leave its place held until
/// the link clears, so the held work grows with that churn.
fn stream_state(world: &mut World) {
    if !world.contains_non_send::<PieTransportRes>() {
        return;
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let entities = streamed_entities(world);
    let this_scan = world.change_tick();

    let mut state = std::mem::take(&mut *world.resource_mut::<PieStreamState>());
    let mut frames: Vec<Vec<u8>> = Vec::new();

    scan_stream(
        world,
        &registry,
        &entities,
        this_scan,
        &mut state,
        &mut frames,
    );
    let began = Instant::now();
    let mut sent_one = false;
    let congested = world.non_send::<PieTransportRes>().0.congested();
    while !congested && (!sent_one || began.elapsed() < STREAM_BUDGET) {
        if !send_next(world, &registry, &mut state, &mut frames) {
            break;
        }
        sent_one = true;
    }

    state.last_scan = Some(this_scan);
    world.increment_change_tick();
    *world.resource_mut::<PieStreamState>() = state;
    let transport = &mut world.non_send_mut::<PieTransportRes>().0;
    for bytes in &frames {
        transport.send(PieChannel::Reliable, bytes);
    }
}

/// Queue what changed since the last scan and report what went away.
fn scan_stream(
    world: &World,
    registry: &TypeRegistry,
    entities: &[Entity],
    this_scan: Tick,
    state: &mut PieStreamState,
    frames: &mut Vec<Vec<u8>>,
) {
    let current: HashSet<u64> = entities.iter().map(|entity| entity.to_bits()).collect();
    state.sent.retain(|&bits, _| {
        let alive = current.contains(&bits);
        if !alive && let Ok(bytes) = to_bytes(&StateEvent::EntityDespawned { entity: bits }) {
            frames.push(bytes);
        }
        alive
    });
    let last_scan = state.last_scan;
    for &entity in entities {
        if !state.sent.contains_key(&entity.to_bits()) {
            if state.queued.insert(entity) {
                state.unsent.push_back(entity);
            }
            continue;
        }
        let Some(last_scan) = last_scan else {
            continue;
        };
        let entity_ref = world.entity(entity);
        for column in columns_of(world, registry, entity_ref, state).iter() {
            let changed = entity_ref
                .get_change_ticks_by_id(column.id)
                .is_some_and(|ticks| ticks.is_changed(last_scan, this_scan));
            if changed && state.dirty_set.insert((entity, column.id)) {
                state.dirty.push_back((entity, column.id));
            }
        }
    }
}

/// What `entity`'s archetype streams, resolved on first sight of the archetype.
fn columns_of(
    world: &World,
    registry: &TypeRegistry,
    entity: EntityRef,
    state: &mut PieStreamState,
) -> Arc<[SnapshotColumn]> {
    let archetype = entity.archetype();
    Arc::clone(state.columns.entry(archetype.id()).or_insert_with(|| {
        snapshot_columns(world, registry, archetype)
            .into_iter()
            .filter(|column| column.type_path != jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH)
            .collect()
    }))
}

/// Encode one queued item, changes and spawns in turn, so an edit in a running
/// game is not held behind the rest of a large scene and a world that changes more
/// than the budget covers still gets its spawns out. False when nothing was queued.
fn send_next(
    world: &World,
    registry: &TypeRegistry,
    state: &mut PieStreamState,
    frames: &mut Vec<Vec<u8>>,
) -> bool {
    let spawn_turn = state.spawn_next && !state.unsent.is_empty();
    if !spawn_turn && let Some((entity, id)) = state.dirty.pop_front() {
        state.dirty_set.remove(&(entity, id));
        state.spawn_next = true;
        send_change(world, registry, state, entity, id, frames);
        return true;
    }
    let Some(entity) = state.unsent.pop_front() else {
        return false;
    };
    state.spawn_next = false;
    state.queued.remove(&entity);
    send_spawn(world, registry, state, entity, frames);
    true
}

fn send_change(
    world: &World,
    registry: &TypeRegistry,
    state: &mut PieStreamState,
    entity: Entity,
    id: ComponentId,
    frames: &mut Vec<Vec<u8>>,
) {
    let bits = entity.to_bits();
    let Ok(entity_ref) = world.get_entity(entity) else {
        return;
    };
    let columns = columns_of(world, registry, entity_ref, state);
    let Some(column) = columns.iter().find(|column| column.id == id) else {
        return;
    };
    let Some(value) = column.value(entity_ref, registry) else {
        return;
    };
    let digest = value_digest(&value);
    let Some(sent) = state.sent.get_mut(&bits) else {
        return;
    };
    if sent.insert(id, digest) == Some(digest) {
        return;
    }
    let event = StateEvent::ComponentChanged {
        entity: bits,
        type_path: column.type_path.to_string(),
        value,
    };
    if let Ok(bytes) = to_bytes(&event) {
        frames.push(bytes);
    }
}

fn send_spawn(
    world: &World,
    registry: &TypeRegistry,
    state: &mut PieStreamState,
    entity: Entity,
    frames: &mut Vec<Vec<u8>>,
) {
    let Ok(entity_ref) = world.get_entity(entity) else {
        return;
    };
    let columns = columns_of(world, registry, entity_ref, state);
    let mut digests = HashMap::with_capacity(columns.len());
    let mut components = std::collections::HashMap::with_capacity(columns.len());
    for column in columns.iter() {
        if let Some(value) = column.value(entity_ref, registry) {
            digests.insert(column.id, value_digest(&value));
            components.insert(column.type_path.to_string(), value);
        }
    }
    state.sent.insert(entity.to_bits(), digests);
    let event = StateEvent::EntitySpawned {
        entity: jackdaw_pie_protocol::RemoteEntity {
            entity: entity.to_bits(),
            components,
            scene_node_id: entity_ref.get::<SceneNodeId>().map(|node_id| node_id.0),
        },
    };
    if let Ok(bytes) = to_bytes(&event) {
        frames.push(bytes);
    }
}

/// A digest of a serialized value, which is all the stream needs to keep to tell
/// a rewrite of the same value from a change.
fn value_digest(value: &serde_json::Value) -> u64 {
    fn feed(value: &serde_json::Value, hasher: &mut DefaultHasher) {
        match value {
            serde_json::Value::Null => 0u8.hash(hasher),
            serde_json::Value::Bool(flag) => flag.hash(hasher),
            serde_json::Value::Number(number) => number.to_string().hash(hasher),
            serde_json::Value::String(text) => text.hash(hasher),
            serde_json::Value::Array(items) => {
                items.len().hash(hasher);
                for item in items {
                    feed(item, hasher);
                }
            }
            serde_json::Value::Object(fields) => {
                fields.len().hash(hasher);
                for (name, field) in fields {
                    name.hash(hasher);
                    feed(field, hasher);
                }
            }
        }
    }
    let mut hasher = DefaultHasher::new();
    feed(value, &mut hasher);
    hasher.finish()
}

/// Send one state event to the editor, if the link is up.
pub(crate) fn send_state(world: &mut World, event: &StateEvent) {
    match to_bytes(event) {
        Ok(bytes) => {
            if let Some(mut transport) = world.get_non_send_mut::<PieTransportRes>() {
                transport.0.send(PieChannel::Reliable, &bytes);
            }
        }
        Err(err) => error!("PIE: failed to encode a state event: {err}"),
    }
}

/// Apply control commands from the editor.
///
/// `Stop`, or the editor hanging up, writes `AppExit::Success` to tear down
/// the game loop. `Pause` / `Resume` toggle the virtual clock, freezing
/// gameplay systems keyed on `Time<Virtual>`. `SetComponent`,
/// `AddComponent`, `RemoveComponent` apply reflected edits to live entities,
/// and `NodeComponents` to every entity spawned from a scene node.
/// `SceneDocument` answers a scene root waiting on the editor, and
/// `SceneAssets` loads asset definitions an edit that follows will name,
/// `AssetFiles` the asset files the catalog walk at startup could not hold, and
/// `PrefabFiles` the unsaved prefabs read in place of their files.
/// Unknown frames are skipped.
fn apply_control(world: &mut World) {
    if !world.contains_non_send::<PieTransportRes>() {
        return;
    }

    let mut transport = world.non_send_mut::<PieTransportRes>();
    let frames: Vec<Vec<u8>> = transport
        .0
        .drain_received()
        .into_iter()
        .map(|(_, bytes)| bytes)
        .collect();
    // The editor's end closes when it exits by any means, a crash included,
    // and a game left running without it has no one to stop it.
    if transport.0.peer_gone() {
        info!("PIE: the editor closed the link, exiting");
        world.write_message(AppExit::Success);
    }

    for bytes in frames {
        let Ok(event) = jackdaw_pie_protocol::event::from_bytes::<ControlEvent>(&bytes) else {
            continue;
        };
        match event {
            ControlEvent::Stop => {
                world.write_message(AppExit::Success);
            }
            ControlEvent::Pause => {
                world.resource_mut::<Time<Virtual>>().pause();
            }
            ControlEvent::Resume => {
                world.resource_mut::<Time<Virtual>>().unpause();
            }
            ControlEvent::SetComponent {
                entity,
                type_path,
                value,
            } => {
                apply_set_component(world, entity, &type_path, &value);
            }
            ControlEvent::AddComponent {
                entity,
                type_path,
                value,
            } => {
                apply_add_component(world, entity, &type_path, &value);
            }
            ControlEvent::RemoveComponent { entity, type_path } => {
                apply_remove_component(world, entity, &type_path);
            }
            ControlEvent::StartFrameStream { width, height } => {
                crate::pie_frames::start_frame_stream(world, width, height);
            }
            ControlEvent::StopFrameStream => {
                crate::pie_frames::stop_frame_stream(world);
            }
            ControlEvent::Input(event) => apply_input(world, event),
            ControlEvent::NodeComponents { node, set, removed } => {
                crate::node_sync::apply_node_components(world, node, &set, &removed);
            }
            ControlEvent::SceneDocument { request, answer } => {
                crate::served_scene::receive(world, request, answer);
            }
            ControlEvent::SceneAssets { text } => {
                crate::node_sync::load_scene_assets(world, &text);
            }
            ControlEvent::AssetFiles { files } => {
                crate::node_sync::load_asset_files(world, &files);
            }
            ControlEvent::PrefabFiles { files } => {
                crate::play_overlay::load_prefab_files(world, &files);
            }
            ControlEvent::Highlight { entity } => {
                let resolved = entity
                    .map(Entity::from_bits)
                    .filter(|&e| world.get_entity(e).is_ok());
                world.resource_mut::<HighlightedEntity>().0 = resolved;
            }
        }
    }
}

/// World-space `(center, half_extents, rotation)` boxes to outline for the
/// highlighted entity: a box for every mesh in its whole subtree (itself and
/// all descendants), each `Aabb` placed by the owning entity's
/// `GlobalTransform`. A multi-part model (a tree of foliage plus a trunk under
/// it) is fully boxed, not just its top piece. Empty when the subtree has no
/// mesh bounds anywhere (the caller then draws a small fallback cube at the
/// entity position).
fn highlight_boxes(
    entity: Entity,
    transforms: &Query<&GlobalTransform>,
    aabbs: &Query<&bevy::camera::primitives::Aabb>,
    children: &Query<&Children>,
) -> Vec<(Vec3, Vec3, Quat)> {
    fn box_for(
        entity: Entity,
        transforms: &Query<&GlobalTransform>,
        aabbs: &Query<&bevy::camera::primitives::Aabb>,
    ) -> Option<(Vec3, Vec3, Quat)> {
        let gt = transforms.get(entity).ok()?;
        let aabb = aabbs.get(entity).ok()?;
        let (scale, rotation, _) = gt.to_scale_rotation_translation();
        let center = gt.transform_point(Vec3::from(aabb.center));
        let half = Vec3::from(aabb.half_extents) * scale;
        Some((center, half, rotation))
    }

    // Iterative subtree walk with a visited set, so a streamed parent cycle
    // cannot loop forever.
    let mut boxes = Vec::new();
    let mut seen = bevy::platform::collections::HashSet::new();
    let mut stack = vec![entity];
    while let Some(current) = stack.pop() {
        if !seen.insert(current) {
            continue;
        }
        if let Some(b) = box_for(current, transforms, aabbs) {
            boxes.push(b);
        }
        if let Ok(kids) = children.get(current) {
            stack.extend(kids.iter());
        }
    }
    boxes
}

/// Draw a wireframe box around the highlighted entity through the game's own
/// cameras, so the outline appears in the streamed frame. Falls back to a
/// small cube at the entity position when it has no mesh bounds. Clears the
/// highlight when the entity is gone.
fn draw_highlight(
    mut highlighted: ResMut<HighlightedEntity>,
    mut gizmos: Gizmos,
    transforms: Query<&GlobalTransform>,
    aabbs: Query<&bevy::camera::primitives::Aabb>,
    children: Query<&Children>,
) {
    let Some(entity) = highlighted.0 else {
        return;
    };
    let Ok(entity_gt) = transforms.get(entity) else {
        highlighted.0 = None;
        return;
    };
    const HIGHLIGHT_COLOR: Color = Color::srgb(1.0, 0.62, 0.1);
    let boxes = highlight_boxes(entity, &transforms, &aabbs, &children);
    if boxes.is_empty() {
        gizmos.cube(
            Transform::from_translation(entity_gt.translation()).with_scale(Vec3::splat(0.5)),
            HIGHLIGHT_COLOR,
        );
        return;
    }
    for (center, half, rotation) in boxes {
        gizmos.cube(
            Transform {
                translation: center,
                rotation,
                scale: half * 2.0,
            },
            HIGHLIGHT_COLOR,
        );
    }
}

/// Inject one forwarded input event as ordinary bevy input messages aimed at
/// the virtual window, so `ButtonInput`, picking, and UI interaction behave
/// exactly as if a real window had produced them. A game in the windowed
/// fallback has no virtual window and ignores forwarded input.
fn apply_input(world: &mut World, event: jackdaw_pie_protocol::PieInputEvent) {
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, KeyboardFocusLost, KeyboardInput};
    use bevy::input::mouse::{MouseButtonInput, MouseMotion, MouseScrollUnit, MouseWheel};
    use bevy::window::CursorMoved;
    use jackdaw_pie_protocol::PieInputEvent;

    let mut windows =
        world.query_filtered::<Entity, With<crate::pie_windowless::PieVirtualWindow>>();
    let Some(window) = windows.iter(world).next() else {
        return;
    };

    match event {
        PieInputEvent::Key {
            key,
            logical,
            pressed,
            repeat,
        } => {
            let text = if pressed {
                match &logical {
                    Key::Character(text) => Some(text.clone()),
                    _ => None,
                }
            } else {
                None
            };
            world.write_message(KeyboardInput {
                key_code: key,
                logical_key: logical,
                state: if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                },
                text,
                repeat,
                window,
            });
        }
        PieInputEvent::MouseButton { button, pressed } => {
            let state = if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            };
            world.write_message(MouseButtonInput {
                button,
                state,
                window,
            });
            if let Some(pointer_button) = pointer_button(button) {
                let action = if pressed {
                    bevy::picking::pointer::PointerAction::Press(pointer_button)
                } else {
                    bevy::picking::pointer::PointerAction::Release(pointer_button)
                };
                write_pointer(world, window, action);
            }
        }
        PieInputEvent::CursorMoved { position } => {
            let previous = world
                .get::<Window>(window)
                .and_then(Window::physical_cursor_position);
            let delta = previous.map(|prev| position - prev);
            if let Some(mut win) = world.get_mut::<Window>(window) {
                win.set_physical_cursor_position(Some(position.as_dvec2()));
            }
            world.write_message(CursorMoved {
                window,
                position,
                delta,
            });
            write_pointer(
                world,
                window,
                bevy::picking::pointer::PointerAction::Move {
                    delta: delta.unwrap_or(Vec2::ZERO),
                },
            );
        }
        PieInputEvent::MouseMotion { delta } => {
            world.write_message(MouseMotion { delta });
        }
        PieInputEvent::MouseWheel { x, y, line_units } => {
            let unit = if line_units {
                MouseScrollUnit::Line
            } else {
                MouseScrollUnit::Pixel
            };
            world.write_message(MouseWheel {
                unit,
                x,
                y,
                window,
                phase: bevy::input::touch::TouchPhase::Moved,
            });
            write_pointer(
                world,
                window,
                bevy::picking::pointer::PointerAction::Scroll {
                    unit,
                    x,
                    y,
                    phase: bevy::input::touch::TouchPhase::Moved,
                },
            );
        }
        PieInputEvent::FocusGained => {
            if let Some(mut win) = world.get_mut::<Window>(window) {
                win.focused = true;
            }
        }
        PieInputEvent::FocusLost => {
            if let Some(mut win) = world.get_mut::<Window>(window) {
                win.focused = false;
            }
            world.write_message(KeyboardFocusLost);
        }
    }
}

/// Map a mouse button onto a picking pointer button; buttons with no pointer
/// equivalent skip the pointer event (the raw message still flows).
fn pointer_button(
    button: bevy::input::mouse::MouseButton,
) -> Option<bevy::picking::pointer::PointerButton> {
    use bevy::input::mouse::MouseButton;
    use bevy::picking::pointer::PointerButton;
    match button {
        MouseButton::Left => Some(PointerButton::Primary),
        MouseButton::Right => Some(PointerButton::Secondary),
        MouseButton::Middle => Some(PointerButton::Middle),
        _ => None,
    }
}

/// Emit a picking pointer event targeting the capture image. Pointer
/// locations are matched against camera render targets, and every camera
/// renders into the image, so window-targeted locations would never hit UI.
/// Skipped when picking is not present (the headless server).
fn write_pointer(world: &mut World, window: Entity, action: bevy::picking::pointer::PointerAction) {
    use bevy::picking::pointer::{Location, PointerId, PointerInput};
    if !world.contains_resource::<bevy::ecs::message::Messages<PointerInput>>() {
        return;
    }
    let Some(target) = world.get_resource::<crate::pie_windowless::WindowlessTarget>() else {
        return;
    };
    let image = target.image.clone();
    let Some(position) = world
        .get::<Window>(window)
        .and_then(Window::physical_cursor_position)
    else {
        return;
    };
    let location = Location {
        target: bevy::camera::NormalizedRenderTarget::Image(image.into()),
        position,
    };
    world.write_message(PointerInput {
        pointer_id: PointerId::Mouse,
        location,
        action,
    });
}

/// Mirror the virtual window's cursor options to the editor (mouse-look
/// grabs) so the editor can lock its own cursor while input capture is
/// engaged. Runs only when the options changed; absent in the windowed
/// fallback (no virtual window).
fn stream_cursor_state(
    options: Query<
        &bevy::window::CursorOptions,
        (
            With<crate::pie_windowless::PieVirtualWindow>,
            Changed<bevy::window::CursorOptions>,
        ),
    >,
    transport: Option<NonSendMut<PieTransportRes>>,
) {
    let Some(mut transport) = transport else {
        return;
    };
    let Ok(options) = options.single() else {
        return;
    };
    let event = StateEvent::CursorState {
        grabbed: !matches!(options.grab_mode, bevy::window::CursorGrabMode::None),
        visible: options.visible,
    };
    if let Ok(bytes) = to_bytes(&event) {
        transport.0.send(PieChannel::Reliable, &bytes);
    }
}

/// Deserialize `value` for `type_path` and apply it to an existing component on the entity.
/// Logs and skips on unknown type path, missing entity, or deserialize error.
fn apply_set_component(
    world: &mut World,
    entity_bits: u64,
    type_path: &str,
    value: &serde_json::Value,
) {
    let decoded = match crate::node_sync::decode_component(world, type_path, value) {
        Ok(decoded) => decoded,
        Err(reason) => {
            warn!("PIE: SetComponent: {reason}");
            return;
        }
    };
    let entity = Entity::from_bits(entity_bits);
    let Ok(entity_ref) = world.get_entity(entity) else {
        warn!("PIE: SetComponent: entity {entity_bits} not found");
        return;
    };
    if !decoded.reflect.contains(entity_ref) {
        warn!("PIE: SetComponent: entity {entity_bits} does not have `{type_path}`");
        return;
    }
    crate::node_sync::write_component(world, entity, &decoded);
}

/// Deserialize `value` for `type_path` and insert (or replace) a component on the entity.
/// Logs and skips on unknown type path, missing entity, or deserialize error.
fn apply_add_component(
    world: &mut World,
    entity_bits: u64,
    type_path: &str,
    value: &serde_json::Value,
) {
    let decoded = match crate::node_sync::decode_component(world, type_path, value) {
        Ok(decoded) => decoded,
        Err(reason) => {
            warn!("PIE: AddComponent: {reason}");
            return;
        }
    };
    let entity = Entity::from_bits(entity_bits);
    if world.get_entity(entity).is_err() {
        warn!("PIE: AddComponent: entity {entity_bits} not found");
        return;
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    decoded.reflect.insert(
        &mut world.entity_mut(entity),
        decoded.value.as_ref(),
        &registry,
    );
}

/// Remove a component identified by `type_path` from an entity.
/// Logs and skips on unknown type path or missing entity; silently ignores absent components.
fn apply_remove_component(world: &mut World, entity_bits: u64, type_path: &str) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry_guard = registry.read();

    let Some(registration) = registry_guard.get_with_type_path(type_path) else {
        warn!("PIE: RemoveComponent: unknown type path `{type_path}`");
        return;
    };
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        warn!("PIE: RemoveComponent: `{type_path}` has no ReflectComponent");
        return;
    };

    let entity = Entity::from_bits(entity_bits);
    if world.get_entity(entity).is_err() {
        warn!("PIE: RemoveComponent: entity {entity_bits} not found");
        return;
    }

    reflect_component.remove(&mut world.entity_mut(entity));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::AppExit;
    use jackdaw_pie_protocol::event::{ControlEvent, PieChannel, to_bytes};
    use jackdaw_pie_protocol::{IpcChannelTransport, connect, serve};
    use jackdaw_scene_types::SceneNodeId;

    /// Headless app with PIE systems and a single `Name` + `Transform` entity.
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

    #[test]
    fn the_cameras_the_runtime_adds_for_itself_never_stream_to_the_editor() {
        let mut world = World::new();
        let authored = world.spawn(Transform::default()).id();
        world.spawn((Transform::default(), crate::FallbackCamera2d));
        world.spawn((Transform::default(), crate::RuntimeOnly));

        assert_eq!(streamed_entities(&mut world), vec![authored]);
    }

    /// Build a minimal app with `SceneNodeId` registered, spawn one entity that
    /// carries the id and one without, run one stream tick, and verify:
    ///  - the entity with `SceneNodeId(42)` produces `EntitySpawned` with
    ///    `scene_node_id == Some(42)` and no `SceneNodeId` key in `components`.
    ///  - the entity without `SceneNodeId` produces `EntitySpawned` with
    ///    `scene_node_id == None`.
    #[test]
    fn scene_node_id_lifted_out_of_components() {
        const JSN_NODE_ID_TYPE_PATH: &str = jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH;

        let (handle, rendezvous) = serve().expect("serve");

        let editor = std::thread::spawn(move || {
            let mut editor = handle.accept().expect("accept");
            let mut received: Vec<StateEvent> = Vec::new();
            for _ in 0..500_000 {
                for (_, bytes) in editor.drain_received() {
                    if let Ok(event) = jackdaw_pie_protocol::event::from_bytes::<StateEvent>(&bytes)
                    {
                        received.push(event);
                    }
                }
                let spawn_count = received
                    .iter()
                    .filter(|e| matches!(e, StateEvent::EntitySpawned { .. }))
                    .count();
                if spawn_count >= 2 {
                    break;
                }
                std::thread::yield_now();
            }
            editor.send(
                PieChannel::Reliable,
                &to_bytes(&ControlEvent::Stop).expect("encode"),
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
            received
        });

        let transport = connect(&rendezvous).expect("connect");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.register_type::<Name>();
        app.register_type::<Transform>();
        app.register_type::<SceneNodeId>();

        let with_id = app
            .world_mut()
            .spawn((Transform::from_xyz(1.0, 0.0, 0.0), SceneNodeId(42)))
            .id();
        let without_id = app
            .world_mut()
            .spawn(Transform::from_xyz(2.0, 0.0, 0.0))
            .id();

        attach_pie(&mut app, transport);

        for _ in 0..50 {
            app.update();
            if !app.world().resource::<Messages<AppExit>>().is_empty() {
                break;
            }
            std::thread::yield_now();
        }

        let received = editor.join().expect("editor thread");

        let find_spawned = |bits: u64| -> Option<&jackdaw_pie_protocol::RemoteEntity> {
            for event in &received {
                if let StateEvent::EntitySpawned { entity: re } = event
                    && re.entity == bits
                {
                    return Some(re);
                }
            }
            None
        };

        let re_with = find_spawned(with_id.to_bits())
            .expect("EntitySpawned not received for entity with SceneNodeId");
        assert_eq!(
            re_with.scene_node_id,
            Some(42),
            "scene_node_id should be Some(42) for entity carrying SceneNodeId(42)"
        );
        assert!(
            !re_with.components.contains_key(JSN_NODE_ID_TYPE_PATH),
            "SceneNodeId must not appear in the components map; keys: {:?}",
            re_with.components.keys().collect::<Vec<_>>()
        );

        let re_without = find_spawned(without_id.to_bits())
            .expect("EntitySpawned not received for entity without SceneNodeId");
        assert_eq!(
            re_without.scene_node_id, None,
            "scene_node_id should be None for entity without SceneNodeId"
        );
    }

    /// Round-trip: app streams its initial snapshot to an in-test editor, and
    /// a `Stop` from the editor causes the app to write `AppExit`.
    #[test]
    fn streams_spawn_and_stops_on_control() {
        let (handle, name) = serve().expect("serve");

        // Worker thread simulates the editor: accept, collect frames, send Stop.
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
            // Hold the connection open until the app has read the Stop frame.
            std::thread::sleep(std::time::Duration::from_millis(200));
            received
        });

        let transport = connect(&name).expect("connect");
        let (mut app, entity) = headless_pie_app(transport);

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

    /// An editor that goes away without sending `Stop`, as a crash does,
    /// still takes the game down.
    #[test]
    #[cfg_attr(
        windows,
        ignore = "ipc-channel on Windows keeps a copy of a transferred sender in the receiving \
                  process, so the editor end closes only when that process exits"
    )]
    fn exits_when_the_editor_hangs_up() {
        let (handle, name) = serve().expect("serve");
        let editor = std::thread::spawn(move || handle.accept().expect("accept"));
        let transport = connect(&name).expect("connect");
        let (mut app, _) = headless_pie_app(transport);
        let editor = editor.join().expect("editor thread");

        app.update();
        assert!(
            app.world().resource::<Messages<AppExit>>().is_empty(),
            "a connected editor keeps the game running"
        );

        drop(editor);
        let mut exited = false;
        for _ in 0..200 {
            app.update();
            if !app.world().resource::<Messages<AppExit>>().is_empty() {
                exited = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(exited, "app should write AppExit once the editor is gone");
    }

    /// After the initial `EntitySpawned`, mutating `Name` (a non-`Transform`
    /// reflectable component) must produce a `ComponentChanged` whose
    /// `type_path` identifies `Name`.
    #[test]
    fn streams_name_delta_after_mutation() {
        const NAME_TYPE_PATH: &str = "bevy_ecs::name::Name";

        let (handle, rendezvous) = serve().expect("serve");

        // Editor thread: accept connection, collect all events until it sees
        // a ComponentChanged for Name, then send Stop.
        let editor = std::thread::spawn(move || {
            let mut editor = handle.accept().expect("accept");
            let mut received: Vec<StateEvent> = Vec::new();

            // Spin until we see a ComponentChanged for Name or give up.
            for _ in 0..500_000 {
                for (_, bytes) in editor.drain_received() {
                    if let Ok(event) = jackdaw_pie_protocol::event::from_bytes::<StateEvent>(&bytes)
                    {
                        received.push(event);
                    }
                }
                let has_name_delta = received.iter().any(|e| {
                    matches!(e, StateEvent::ComponentChanged { type_path, .. }
                        if type_path == NAME_TYPE_PATH)
                });
                if has_name_delta {
                    break;
                }
                std::thread::yield_now();
            }

            editor.send(
                PieChannel::Reliable,
                &to_bytes(&ControlEvent::Stop).expect("encode"),
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
            received
        });

        let transport = connect(&rendezvous).expect("connect");
        let (mut app, entity) = headless_pie_app(transport);

        // First update: initial snapshot is sent.
        app.update();

        // Mutate `Name` to trigger a delta on the next update.
        app.world_mut()
            .entity_mut(entity)
            .insert(Name::new("pie-probe-renamed"));

        // Drive updates until the app sees Stop.
        for _ in 0..50 {
            app.update();
            if !app.world().resource::<Messages<AppExit>>().is_empty() {
                break;
            }
            std::thread::yield_now();
        }

        let received = editor.join().expect("editor thread");

        assert!(
            received.iter().any(|e| {
                matches!(e, StateEvent::ComponentChanged { type_path, .. }
                    if type_path == NAME_TYPE_PATH)
            }),
            "editor should receive a ComponentChanged for Name after mutation; got {received:?}"
        );
    }

    /// A game with its link on loopback and `count` entities, every entity's
    /// `Name` a long string so one spawn is measurable work.
    fn streaming_app(count: usize) -> (App, jackdaw_pie_protocol::LoopbackTransport) {
        let (editor, game) = jackdaw_pie_protocol::LoopbackTransport::pair();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.register_type::<Name>();
        app.register_type::<Transform>();
        install_link(&mut app, Box::new(game));
        for index in 0..count {
            app.world_mut().spawn((
                Name::new(format!("{index}{}", "x".repeat(4096))),
                Transform::default(),
            ));
        }
        (app, editor)
    }

    fn state_events(editor: &mut impl PieTransport) -> Vec<StateEvent> {
        editor
            .drain_received()
            .into_iter()
            .filter_map(|(_, bytes)| jackdaw_pie_protocol::event::from_bytes(&bytes).ok())
            .collect()
    }

    /// The first frame after a large scene spawns must not serialize all of it.
    #[test]
    fn a_large_world_reaches_the_editor_over_several_frames() {
        const COUNT: usize = 5000;
        let (mut app, mut editor) = streaming_app(COUNT);

        app.update();
        let first = state_events(&mut editor).len();
        assert!(first > 0 && first < COUNT, "one frame sent {first} spawns");

        let mut total = first;
        for _ in 0..100_000 {
            if total == COUNT {
                break;
            }
            app.update();
            total += state_events(&mut editor).len();
        }
        assert_eq!(total, COUNT);
    }

    /// A transport the test can report as congested.
    struct CongestedTransport {
        inner: jackdaw_pie_protocol::LoopbackTransport,
        congested: std::rc::Rc<std::cell::Cell<bool>>,
    }

    impl PieTransport for CongestedTransport {
        fn send(&mut self, channel: PieChannel, bytes: &[u8]) {
            self.inner.send(channel, bytes);
        }
        fn drain_received(&mut self) -> Vec<(PieChannel, Vec<u8>)> {
            self.inner.drain_received()
        }
        fn congested(&self) -> bool {
            self.congested.get()
        }
    }

    /// An editor that stopped reading holds up what can wait, not what cannot, and
    /// the stream picks up where it left off, each changing component once.
    #[test]
    fn a_congested_link_pauses_the_stream_but_not_a_despawn() {
        let (mut editor, game) = jackdaw_pie_protocol::LoopbackTransport::pair();
        let congested = std::rc::Rc::new(std::cell::Cell::new(false));
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.register_type::<Transform>();
        install_link(
            &mut app,
            Box::new(CongestedTransport {
                inner: game,
                congested: congested.clone(),
            }),
        );
        let moved = app.world_mut().spawn(Transform::default()).id();
        let doomed = app.world_mut().spawn(Transform::default()).id();
        app.update();
        app.update();
        assert_eq!(state_events(&mut editor).len(), 2, "both spawned");

        congested.set(true);
        let late = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().despawn(doomed);
        for x in 0..10 {
            app.world_mut()
                .get_mut::<Transform>(moved)
                .expect("it")
                .translation
                .x = x as f32;
            app.update();
        }
        let held = state_events(&mut editor);
        assert!(
            matches!(held.as_slice(), [StateEvent::EntityDespawned { .. }]),
            "{held:?}"
        );

        congested.set(false);
        app.update();
        app.update();
        let resumed = state_events(&mut editor);
        let changes = resumed
            .iter()
            .filter(|event| matches!(event, StateEvent::ComponentChanged { .. }))
            .count();
        assert_eq!(changes, 1, "{resumed:?}");
        assert!(resumed.iter().any(|event| matches!(
            event,
            StateEvent::EntitySpawned { entity } if entity.entity == late.to_bits()
        )));
    }

    /// More changes each frame than the budget covers never drain, and spawns
    /// still reach the editor.
    #[test]
    fn sustained_changes_do_not_starve_a_spawn() {
        const CHANGING: usize = 4000;
        const LATE: usize = 20;
        let (mut app, mut editor) = streaming_app(CHANGING);
        for _ in 0..100_000 {
            app.update();
            let spawned = state_events(&mut editor).len();
            if spawned == 0 {
                break;
            }
        }
        for index in 0..LATE {
            app.world_mut()
                .spawn((Name::new(format!("late {index}")), Transform::default()));
        }

        let mut late_spawns = 0;
        for frame in 0..200 {
            let mut transforms = app.world_mut().query::<&mut Transform>();
            for mut transform in transforms.iter_mut(app.world_mut()) {
                transform.translation.x = frame as f32 + 1.0;
            }
            app.update();
            late_spawns += state_events(&mut editor)
                .iter()
                .filter(|event| matches!(event, StateEvent::EntitySpawned { .. }))
                .count();
            if late_spawns == LATE {
                return;
            }
        }
        panic!("{late_spawns} of {LATE} spawns went out");
    }

    /// Once the editor is caught up, a frame sends only what changed: nothing at
    /// all when idle, and the one component that moved otherwise.
    #[test]
    fn a_caught_up_stream_sends_only_changes() {
        let (mut app, mut editor) = streaming_app(3);
        let entity = {
            let mut query = app.world_mut().query_filtered::<Entity, With<Name>>();
            query.iter(app.world()).next().expect("an entity")
        };
        for _ in 0..50 {
            app.update();
        }
        let spawns = state_events(&mut editor);
        assert_eq!(spawns.len(), 3, "{spawns:?}");

        app.update();
        assert!(
            state_events(&mut editor).is_empty(),
            "an idle frame is silent"
        );

        app.world_mut()
            .get_mut::<Transform>(entity)
            .expect("a transform")
            .translation
            .x = 4.0;
        app.update();
        let events = state_events(&mut editor);
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(matches!(
            &events[0],
            StateEvent::ComponentChanged { entity: bits, type_path, .. }
                if *bits == entity.to_bits() && type_path.ends_with("Transform")
        ));

        let same = *app.world().get::<Transform>(entity).expect("a transform");
        app.world_mut()
            .get_mut::<Transform>(entity)
            .expect("a transform")
            .set_if_neq(same);
        *app.world_mut()
            .get_mut::<Transform>(entity)
            .expect("a transform") = same;
        app.update();
        assert!(
            state_events(&mut editor).is_empty(),
            "writing the same value is not a change"
        );

        app.world_mut().despawn(entity);
        app.update();
        let events = state_events(&mut editor);
        assert_eq!(
            events,
            vec![StateEvent::EntityDespawned {
                entity: entity.to_bits()
            }]
        );
    }

    #[test]
    fn injected_input_drives_button_input() {
        use bevy::input::keyboard::{Key, KeyCode};
        use jackdaw_pie_protocol::PieInputEvent;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::input::InputPlugin));
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        crate::pie_windowless::install_windowless_world(app.world_mut());

        apply_input(
            app.world_mut(),
            PieInputEvent::Key {
                key: KeyCode::KeyW,
                logical: Key::Character("w".into()),
                pressed: true,
                repeat: false,
            },
        );
        app.update();
        assert!(
            app.world()
                .resource::<bevy::input::ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );

        apply_input(app.world_mut(), PieInputEvent::FocusLost);
        app.update();
        assert!(
            !app.world()
                .resource::<bevy::input::ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW),
            "focus loss clears held keys"
        );
    }

    #[test]
    fn injected_cursor_updates_window_and_pointer() {
        use jackdaw_pie_protocol::PieInputEvent;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::input::InputPlugin));
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        app.add_message::<bevy::picking::pointer::PointerInput>();
        crate::pie_windowless::install_windowless_world(app.world_mut());

        apply_input(
            app.world_mut(),
            PieInputEvent::CursorMoved {
                position: Vec2::new(100.0, 50.0),
            },
        );

        let mut windows = app
            .world_mut()
            .query_filtered::<&Window, With<crate::pie_windowless::PieVirtualWindow>>();
        let window = windows.single(app.world()).unwrap();
        assert_eq!(
            window.physical_cursor_position(),
            Some(Vec2::new(100.0, 50.0))
        );

        let target_handle = app
            .world()
            .resource::<crate::pie_windowless::WindowlessTarget>()
            .image
            .clone();
        let messages = app
            .world()
            .resource::<bevy::ecs::message::Messages<bevy::picking::pointer::PointerInput>>();
        let mut cursor = messages.get_cursor();
        let event = cursor
            .read(messages)
            .next()
            .expect("one pointer event injected");
        match &event.location.target {
            bevy::camera::NormalizedRenderTarget::Image(image) => {
                assert_eq!(image.handle, target_handle);
            }
            other => panic!("expected an image pointer target, got {other:?}"),
        }
    }

    /// `highlight_boxes` takes `Query` params, so it is exercised through a
    /// tiny one-shot system that forwards a target entity into it and returns
    /// the box count. A mesh-bearing entity with one mesh-bearing child yields
    /// two boxes; an entity with no `Aabb` anywhere yields zero.
    #[test]
    fn highlight_boxes_covers_self_and_children() {
        use bevy::camera::primitives::Aabb;

        fn count_boxes(
            target: In<Entity>,
            transforms: Query<&GlobalTransform>,
            aabbs: Query<&Aabb>,
            children: Query<&Children>,
        ) -> usize {
            highlight_boxes(*target, &transforms, &aabbs, &children).len()
        }

        let mut world = World::new();

        let child = world
            .spawn((
                GlobalTransform::default(),
                Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0)),
            ))
            .id();
        let parent = world
            .spawn((
                GlobalTransform::default(),
                Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0)),
            ))
            .add_child(child)
            .id();

        // A grandchild mesh (a tree trunk under foliage under the root) must
        // also be boxed, so the whole subtree is outlined, not just the top.
        let grandchild = world
            .spawn((
                GlobalTransform::default(),
                Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0)),
            ))
            .id();
        world.entity_mut(child).add_child(grandchild);

        let count = world
            .run_system_cached_with(count_boxes, parent)
            .expect("one-shot system runs");
        assert_eq!(
            count, 3,
            "self plus child plus grandchild, the whole subtree"
        );

        // An entity with a transform but no Aabb (and no children) yields no
        // boxes, so the draw system falls back to a small cube.
        let no_mesh = world.spawn(GlobalTransform::default()).id();
        let count = world
            .run_system_cached_with(count_boxes, no_mesh)
            .expect("one-shot system runs");
        assert_eq!(count, 0, "no mesh bounds anywhere yields no boxes");
    }

    /// The `Highlight` apply path resolves valid entity bits and rejects bits
    /// for an entity that no longer exists, the same guard `draw_highlight`
    /// relies on to clear a stale highlight. Asserted on `apply_control`'s
    /// resolve so the test stays headless (no `Gizmos`).
    #[test]
    fn highlight_resolves_live_bits_and_drops_dead_ones() {
        let mut world = World::new();
        world.init_resource::<HighlightedEntity>();
        let live = world.spawn_empty().id();
        let dead = world.spawn_empty().id();
        let dead_bits = dead.to_bits();
        world.despawn(dead);

        let resolve = |world: &World, bits: Option<u64>| -> Option<Entity> {
            bits.map(Entity::from_bits)
                .filter(|&e| world.get_entity(e).is_ok())
        };

        assert_eq!(resolve(&world, Some(live.to_bits())), Some(live));
        assert_eq!(resolve(&world, Some(dead_bits)), None);
        assert_eq!(resolve(&world, None), None);
    }

    /// A node edit lands on every entity of the node while the virtual clock
    /// is paused: control drains every frame regardless of `Time<Virtual>`.
    #[test]
    fn node_components_apply_while_paused() {
        use jackdaw_pie_protocol::LoopbackTransport;

        let (mut editor, game) = LoopbackTransport::pair();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.register_type::<Transform>();
        app.register_type::<SceneNodeId>();
        install_link(&mut app, Box::new(game));
        let first = app
            .world_mut()
            .spawn((SceneNodeId(7), Transform::default()))
            .id();
        let second = app
            .world_mut()
            .spawn((SceneNodeId(7), Transform::default()))
            .id();
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        app.update();

        let event = ControlEvent::NodeComponents {
            node: 7,
            set: vec![(
                "bevy_transform::components::transform::Transform".to_string(),
                serde_json::json!({
                    "translation": [3.0, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0]
                }),
            )],
            removed: Vec::new(),
        };
        editor.send(PieChannel::Reliable, &to_bytes(&event).expect("encode"));
        app.update();

        assert!(app.world().resource::<Time<Virtual>>().is_paused());
        for entity in [first, second] {
            let transform = app.world().get::<Transform>(entity).expect("a transform");
            assert_eq!(transform.translation.x, 3.0);
        }
    }

    #[test]
    fn config_present_only_with_env() {
        assert!(PieConfig::from_env_value(None).is_none());
        let cfg = PieConfig::from_env_value(Some("rv-123".to_string())).unwrap();
        assert_eq!(cfg.server, "rv-123");
    }

    /// `SetComponent` sent from the editor updates the game world's `Transform`.
    #[test]
    fn set_component_applies_transform() {
        const TRANSFORM_PATH: &str = "bevy_transform::components::transform::Transform";

        let (handle, rendezvous) = serve().expect("serve");

        let editor = std::thread::spawn(move || {
            let mut editor = handle.accept().expect("accept");

            // Wait until we receive an EntitySpawned so we know the entity bits.
            let mut entity_bits: Option<u64> = None;
            for _ in 0..500_000 {
                for (_, bytes) in editor.drain_received() {
                    if let Ok(jackdaw_pie_protocol::StateEvent::EntitySpawned { entity: re }) =
                        jackdaw_pie_protocol::event::from_bytes(&bytes)
                    {
                        entity_bits = Some(re.entity);
                    }
                }
                if entity_bits.is_some() {
                    break;
                }
                std::thread::yield_now();
            }
            let bits = entity_bits.expect("never received EntitySpawned");

            // Build a Transform JSON value matching what TypedReflectSerializer produces.
            // Vec3 serializes as [x, y, z]; Quat as [x, y, z, w].
            let new_translation = serde_json::json!({
                "translation": [10.0, 20.0, 30.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0]
            });

            let set_event = ControlEvent::SetComponent {
                entity: bits,
                type_path: TRANSFORM_PATH.to_string(),
                value: new_translation,
            };
            editor.send(PieChannel::Reliable, &to_bytes(&set_event).expect("encode"));

            // Give the game time to apply, then stop it.
            std::thread::sleep(std::time::Duration::from_millis(100));
            editor.send(
                PieChannel::Reliable,
                &to_bytes(&ControlEvent::Stop).expect("encode"),
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
        });

        let transport = connect(&rendezvous).expect("connect");
        let (mut app, entity) = headless_pie_app(transport);

        for _ in 0..200 {
            app.update();
            if !app.world().resource::<Messages<AppExit>>().is_empty() {
                break;
            }
            std::thread::yield_now();
        }

        editor.join().expect("editor thread");

        let transform = app
            .world()
            .entity(entity)
            .get::<Transform>()
            .expect("entity should still have Transform");
        assert!(
            (transform.translation.x - 10.0).abs() < 1e-4,
            "x should be 10.0, got {}",
            transform.translation.x
        );
        assert!(
            (transform.translation.y - 20.0).abs() < 1e-4,
            "y should be 20.0, got {}",
            transform.translation.y
        );
        assert!(
            (transform.translation.z - 30.0).abs() < 1e-4,
            "z should be 30.0, got {}",
            transform.translation.z
        );
    }
}
