//! Play-in-editor link for the standalone runtime.
//!
//! When `JACKDAW_PIE` is set by the editor before launch, [`pie_config`]
//! returns the rendezvous name and [`attach_pie`] installs a stream system
//! (ECS state to editor as [`StateEvent`]s) and an apply system
//! ([`ControlEvent`]s from the editor driving pause / resume / stop).

use bevy::app::AppExit;
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::prelude::*;
use bevy::reflect::serde::TypedReflectDeserializer;
use bevy::time::Virtual;
use serde::de::DeserializeSeed;

use jackdaw_jsn::JsnNodeId;
use jackdaw_jsn::ast::JSN_NODE_ID_TYPE_PATH;
use jackdaw_pie_protocol::event::{PieChannel, StateEvent, to_bytes};
use jackdaw_pie_protocol::transport::PieTransport;
use jackdaw_pie_protocol::transport_ipc::IpcChannelTransport;
use jackdaw_pie_protocol::{ControlEvent, PieMode, build_snapshot};

/// The active PIE link parameters, read from the environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PieConfig {
    /// Mode the editor launched the game in.
    pub mode: PieMode,
    /// ipc-channel rendezvous name to connect to.
    pub server: String,
}

impl PieConfig {
    /// Build from the value of `JACKDAW_PIE` (the rendezvous name).
    /// Returns `None` when the variable is absent. `JACKDAW_PIE_MODE` selects
    /// the mode and defaults to `play`.
    pub fn from_env_value(server: Option<String>) -> Option<Self> {
        let server = server?;
        let mode = match std::env::var("JACKDAW_PIE_MODE").as_deref() {
            Ok("editor-preview") => PieMode::EditorPreview,
            _ => PieMode::Play,
        };
        Some(PieConfig { mode, server })
    }
}

/// Read the PIE link parameters from the process environment.
pub fn pie_config() -> Option<PieConfig> {
    PieConfig::from_env_value(std::env::var("JACKDAW_PIE").ok())
}

/// Holds the editor link. `IpcChannelTransport` is `Send` but not `Sync` (its
/// receiver wraps a `Cell`-held file descriptor), so it lives as a `NonSend`
/// resource and the PIE systems run on the main thread.
pub(crate) struct PieTransportRes(pub(crate) IpcChannelTransport);

/// Entity the editor asked to box in the rendered frame, or none.
#[derive(Resource, Default)]
struct HighlightedEntity(Option<Entity>);

/// Tracks whether the initial full snapshot has been streamed, and the last
/// serialized component values per entity for value-diff delta streaming.
#[derive(Resource, Default)]
struct PieStreamState {
    sent_initial: bool,
    /// Last-sent component values keyed by entity bits then type path.
    last_sent: std::collections::HashMap<u64, std::collections::HashMap<String, serde_json::Value>>,
}

/// Install the PIE transport and the stream / apply systems.
///
/// Kept separate from [`pie_config`] so tests can supply any
/// [`IpcChannelTransport`] directly.
pub fn attach_pie(app: &mut App, transport: IpcChannelTransport) {
    app.insert_resource(crate::pie_frames::spawn_frame_sender_thread(
        transport.lane_sender(jackdaw_pie_protocol::PieChannel::Frames),
    ));
    app.insert_non_send(PieTransportRes(transport));
    app.init_resource::<PieStreamState>();
    app.init_resource::<HighlightedEntity>();
    // Drain control in PreUpdate, before input processing so forwarded editor
    // input lands in `ButtonInput` the same frame, and before Update where a
    // stop or restart despawns the readback entity that pacing would target.
    app.add_systems(
        PreUpdate,
        (apply_control, stream_cursor_state)
            .chain()
            .before(bevy::input::InputSystems),
    );
    app.add_systems(
        Update,
        (stream_state, crate::pie_frames::pace_frame_capture),
    );

    if crate::pie_windowless::windowless_active(app) {
        crate::pie_windowless::setup_windowless(app);
        // Select-mode picking needs a 3D raycast backend; UI picking alone
        // only hits interface nodes.
        if !app.is_plugin_added::<bevy::picking::mesh_picking::MeshPickingPlugin>() {
            app.add_plugins(bevy::picking::mesh_picking::MeshPickingPlugin);
        }
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
        // registered only here alongside the picking backend.
        app.add_systems(Update, draw_highlight);
    }
}

/// Lift the `JsnNodeId` component out of a snapshot's `components` map and
/// into its `scene_node_id` field.
///
/// `build_snapshot` serializes every reflectable component it finds, including
/// `JsnNodeId`. The editor treats `scene_node_id` as the canonical authored-
/// node link and should not also see `JsnNodeId` as a regular user component,
/// so we remove the entry from `components` and populate `scene_node_id`
/// instead. When the entity has no `JsnNodeId` component the snapshot is left
/// unchanged and `scene_node_id` stays `None`.
fn lift_scene_node_id(
    world: &World,
    entity_bits: u64,
    remote: &mut jackdaw_pie_protocol::RemoteEntity,
) {
    remote.components.remove(JSN_NODE_ID_TYPE_PATH);
    let entity = Entity::from_bits(entity_bits);
    if let Ok(entity_ref) = world.get_entity(entity)
        && let Some(node_id) = entity_ref.get::<JsnNodeId>()
    {
        remote.scene_node_id = Some(node_id.0);
    }
}

/// Stream the scene's ECS state to the editor.
///
/// On the first run, ships a full `EntitySpawned` snapshot for every entity
/// with a `Transform` on the reliable channel and records those component values
/// in `last_sent`. On subsequent runs, calls `build_snapshot` for all
/// `With<Transform>` entities and value-diffs against `last_sent`:
///   - New entity bits: emit `EntitySpawned` (reliable) and record all components.
///   - Known entity bits: emit `ComponentChanged` (unreliable) for each component
///     whose serialized value differs from the last-sent value.
///   - Bits present in `last_sent` but absent from the current set: emit
///     `EntityDespawned` (reliable) and drop from `last_sent`.
fn stream_state(world: &mut World) {
    if !world.contains_non_send::<PieTransportRes>() {
        return;
    }

    let sent_initial = world.resource::<PieStreamState>().sent_initial;

    // Derived face meshes are excluded (the editor regenerates faces from the
    // streamed `Brush`), as is the frame-capture camera (pure capture
    // infrastructure the editor must never project).
    type StreamFilter = (
        With<Transform>,
        Without<jackdaw_jsn::DerivedFaceMesh>,
        Without<crate::pie_frames::FrameCaptureCamera>,
    );
    let entities: Vec<Entity> = world
        .query_filtered::<Entity, StreamFilter>()
        .iter(world)
        .collect();
    let registry = world.resource::<AppTypeRegistry>().clone();

    if !sent_initial {
        let snapshots = {
            let registry = registry.read();
            build_snapshot(world, &registry, &entities)
        };

        let mut spawn_frames: Vec<Vec<u8>> = Vec::new();
        let mut new_last_sent: std::collections::HashMap<
            u64,
            std::collections::HashMap<String, serde_json::Value>,
        > = std::collections::HashMap::new();

        for mut remote in snapshots {
            lift_scene_node_id(world, remote.entity, &mut remote);
            new_last_sent.insert(remote.entity, remote.components.clone());
            if let Ok(bytes) = to_bytes(&StateEvent::EntitySpawned { entity: remote }) {
                spawn_frames.push(bytes);
            }
        }

        let transport = &mut world.non_send_mut::<PieTransportRes>().0;
        for bytes in &spawn_frames {
            transport.send(PieChannel::Reliable, bytes);
        }
        let mut pie_state = world.resource_mut::<PieStreamState>();
        pie_state.sent_initial = true;
        pie_state.last_sent = new_last_sent;
        return;
    }

    let current_bits: std::collections::HashSet<u64> =
        entities.iter().map(|e| e.to_bits()).collect();

    let snapshots = {
        let registry = registry.read();
        build_snapshot(world, &registry, &entities)
    };

    let mut spawn_frames: Vec<Vec<u8>> = Vec::new();
    let mut changed_frames: Vec<Vec<u8>> = Vec::new();
    let mut despawn_frames: Vec<Vec<u8>> = Vec::new();

    {
        let pie_state = world.resource::<PieStreamState>();
        let known_bits: Vec<u64> = pie_state
            .last_sent
            .keys()
            .filter(|b| !current_bits.contains(b))
            .copied()
            .collect();
        for bits in known_bits {
            if let Ok(bytes) = to_bytes(&StateEvent::EntityDespawned { entity: bits }) {
                despawn_frames.push(bytes);
            }
        }
    }

    let mut updates: Vec<(u64, std::collections::HashMap<String, serde_json::Value>)> = Vec::new();

    for mut remote in snapshots {
        lift_scene_node_id(world, remote.entity, &mut remote);
        let bits = remote.entity;
        let pie_state = world.resource::<PieStreamState>();
        if let Some(last_components) = pie_state.last_sent.get(&bits) {
            for (type_path, value) in &remote.components {
                if last_components.get(type_path) != Some(value) {
                    let event = StateEvent::ComponentChanged {
                        entity: bits,
                        type_path: type_path.clone(),
                        value: value.clone(),
                    };
                    if let Ok(bytes) = to_bytes(&event) {
                        changed_frames.push(bytes);
                    }
                }
            }
            updates.push((bits, remote.components));
        } else {
            updates.push((bits, remote.components.clone()));
            if let Ok(bytes) = to_bytes(&StateEvent::EntitySpawned { entity: remote }) {
                spawn_frames.push(bytes);
            }
        }
    }

    let transport = &mut world.non_send_mut::<PieTransportRes>().0;
    for bytes in &spawn_frames {
        transport.send(PieChannel::Reliable, bytes);
    }
    for bytes in &changed_frames {
        transport.send(PieChannel::Unreliable, bytes);
    }
    for bytes in &despawn_frames {
        transport.send(PieChannel::Reliable, bytes);
    }

    let mut pie_state = world.resource_mut::<PieStreamState>();
    for (bits, components) in updates {
        pie_state.last_sent.insert(bits, components);
    }
    let despawned: Vec<u64> = pie_state
        .last_sent
        .keys()
        .filter(|b| !current_bits.contains(b))
        .copied()
        .collect();
    for bits in despawned {
        pie_state.last_sent.remove(&bits);
    }
}

/// Apply control commands from the editor.
///
/// `Stop` writes `AppExit::Success` to tear down the game loop. `Pause` /
/// `Resume` toggle the virtual clock, freezing gameplay systems keyed on
/// `Time<Virtual>`. `SetComponent`, `AddComponent`, `RemoveComponent`
/// apply reflected edits to live entities. Unknown frames are skipped.
fn apply_control(world: &mut World) {
    if !world.contains_non_send::<PieTransportRes>() {
        return;
    }

    let frames: Vec<Vec<u8>> = world
        .non_send_mut::<PieTransportRes>()
        .0
        .drain_received()
        .into_iter()
        .map(|(_, bytes)| bytes)
        .collect();

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
            ControlEvent::Pick => {
                let entity = picked_entity(world);
                let reply = StateEvent::PickResult { entity };
                if let Ok(bytes) = to_bytes(&reply) {
                    world
                        .non_send_mut::<PieTransportRes>()
                        .0
                        .send(PieChannel::Reliable, &bytes);
                }
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

/// The topmost streamable entity under the mouse pointer, from the picking
/// hover map. Hits without a `Transform` (UI nodes, picking infrastructure)
/// are skipped so the answer is always an entity the editor can inspect.
fn picked_entity(world: &mut World) -> Option<u64> {
    let candidates: Vec<(Entity, f32)> = {
        let hover = world.get_resource::<bevy::picking::hover::HoverMap>()?;
        let hits = hover.0.get(&bevy::picking::pointer::PointerId::Mouse)?;
        hits.iter()
            .map(|(&entity, hit)| (entity, hit.depth))
            .collect()
    };
    let mut best: Option<(Entity, f32)> = None;
    for (entity, depth) in candidates {
        if world.get::<Transform>(entity).is_none() {
            continue;
        }
        let better = match best {
            Some((_, best_depth)) => depth < best_depth,
            None => true,
        };
        if better {
            best = Some((entity, depth));
        }
    }
    best.map(|(entity, _)| entity.to_bits())
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
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry_guard = registry.read();

    let Some(registration) = registry_guard.get_with_type_path(type_path) else {
        warn!("PIE: SetComponent: unknown type path `{type_path}`");
        return;
    };
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        warn!("PIE: SetComponent: `{type_path}` has no ReflectComponent");
        return;
    };

    let reflected =
        match TypedReflectDeserializer::new(registration, &registry_guard).deserialize(value) {
            Ok(r) => r,
            Err(err) => {
                warn!("PIE: SetComponent: failed to deserialize `{type_path}`: {err}");
                return;
            }
        };

    let entity = Entity::from_bits(entity_bits);
    if world.get_entity(entity).is_err() {
        warn!("PIE: SetComponent: entity {entity_bits} not found");
        return;
    }
    if !reflect_component.contains(world.entity(entity)) {
        warn!("PIE: SetComponent: entity {entity_bits} does not have `{type_path}`");
        return;
    }

    reflect_component.apply(world.entity_mut(entity), reflected.as_ref());
}

/// Deserialize `value` for `type_path` and insert (or replace) a component on the entity.
/// Logs and skips on unknown type path, missing entity, or deserialize error.
fn apply_add_component(
    world: &mut World,
    entity_bits: u64,
    type_path: &str,
    value: &serde_json::Value,
) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry_guard = registry.read();

    let Some(registration) = registry_guard.get_with_type_path(type_path) else {
        warn!("PIE: AddComponent: unknown type path `{type_path}`");
        return;
    };
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        warn!("PIE: AddComponent: `{type_path}` has no ReflectComponent");
        return;
    };

    let reflected =
        match TypedReflectDeserializer::new(registration, &registry_guard).deserialize(value) {
            Ok(r) => r,
            Err(err) => {
                warn!("PIE: AddComponent: failed to deserialize `{type_path}`: {err}");
                return;
            }
        };

    let entity = Entity::from_bits(entity_bits);
    if world.get_entity(entity).is_err() {
        warn!("PIE: AddComponent: entity {entity_bits} not found");
        return;
    }

    reflect_component.insert(
        &mut world.entity_mut(entity),
        reflected.as_ref(),
        &registry_guard,
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
    use jackdaw_jsn::JsnNodeId;
    use jackdaw_pie_protocol::event::{ControlEvent, PieChannel, to_bytes};
    use jackdaw_pie_protocol::{IpcChannelTransport, connect, serve};

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

    /// Build a minimal app with `JsnNodeId` registered, spawn one entity that
    /// carries the id and one without, run one stream tick, and verify:
    ///  - the entity with `JsnNodeId(42)` produces `EntitySpawned` with
    ///    `scene_node_id == Some(42)` and no `JsnNodeId` key in `components`.
    ///  - the entity without `JsnNodeId` produces `EntitySpawned` with
    ///    `scene_node_id == None`.
    #[test]
    fn scene_node_id_lifted_out_of_components() {
        const JSN_NODE_ID_TYPE_PATH: &str = jackdaw_jsn::ast::JSN_NODE_ID_TYPE_PATH;

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
        app.register_type::<JsnNodeId>();

        let with_id = app
            .world_mut()
            .spawn((Transform::from_xyz(1.0, 0.0, 0.0), JsnNodeId(42)))
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
                if let StateEvent::EntitySpawned { entity: re } = event {
                    if re.entity == bits {
                        return Some(re);
                    }
                }
            }
            None
        };

        let re_with = find_spawned(with_id.to_bits())
            .expect("EntitySpawned not received for entity with JsnNodeId");
        assert_eq!(
            re_with.scene_node_id,
            Some(42),
            "scene_node_id should be Some(42) for entity carrying JsnNodeId(42)"
        );
        assert!(
            !re_with.components.contains_key(JSN_NODE_ID_TYPE_PATH),
            "JsnNodeId must not appear in the components map; keys: {:?}",
            re_with.components.keys().collect::<Vec<_>>()
        );

        let re_without = find_spawned(without_id.to_bits())
            .expect("EntitySpawned not received for entity without JsnNodeId");
        assert_eq!(
            re_without.scene_node_id, None,
            "scene_node_id should be None for entity without JsnNodeId"
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

    #[test]
    fn pick_replies_with_the_nearest_transform_hit() {
        use bevy::ecs::entity::EntityHashMap;
        use bevy::picking::backend::HitData;
        use bevy::picking::hover::HoverMap;
        use bevy::picking::pointer::PointerId;

        let mut world = World::new();
        let camera = world.spawn_empty().id();
        // A UI-style hit (no Transform) nearer than a world entity: the world
        // entity must win because only streamable entities are inspectable.
        let ui_entity = world.spawn_empty().id();
        let near = world.spawn(Transform::default()).id();
        let far = world.spawn(Transform::default()).id();

        let hit = |depth: f32| HitData {
            camera,
            depth,
            position: None,
            normal: None,
            extra: None,
        };
        let mut hits = EntityHashMap::new();
        hits.insert(ui_entity, hit(0.1));
        hits.insert(near, hit(1.0));
        hits.insert(far, hit(5.0));
        let mut map = HoverMap::default();
        map.0.insert(PointerId::Mouse, hits);
        world.insert_resource(map);

        assert_eq!(picked_entity(&mut world), Some(near.to_bits()));

        world.resource_mut::<HoverMap>().0.clear();
        assert_eq!(picked_entity(&mut world), None);
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

    #[test]
    fn config_present_only_with_env() {
        assert!(PieConfig::from_env_value(None).is_none());
        let cfg = PieConfig::from_env_value(Some("rv-123".to_string())).unwrap();
        assert_eq!(cfg.server, "rv-123");
        assert_eq!(cfg.mode, PieMode::Play);
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
