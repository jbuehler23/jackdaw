//! Play-in-editor link for the standalone runtime.
//!
//! When `JACKDAW_PIE` is set by the editor before launch, [`pie_config`]
//! returns the rendezvous name and [`attach_pie`] installs an apply system for
//! the [`ControlEvent`]s the editor sends. The game answers with
//! [`StateEvent`]s for scene requests, cursor state and refused spawns.

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::time::Virtual;

use jackdaw_pie_protocol::ControlEvent;
use jackdaw_pie_protocol::event::{PieChannel, StateEvent, to_bytes};
use jackdaw_pie_protocol::transport::PieTransport;
use jackdaw_pie_protocol::transport_ipc::IpcChannelTransport;

use crate::node_structure::{StructureChange, apply_structure_change};

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

/// What the editor asked to box in the rendered frame.
#[derive(Resource, Default)]
struct Highlight {
    /// The scene node, or none.
    node: Option<u64>,
    /// The entities of `node` this frame.
    entities: Vec<Entity>,
}

/// Install the PIE transport and the apply system.
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
        app.add_systems(Update, (resolve_highlight, draw_highlight).chain());
    }
}

/// Install the control systems over `transport`. Control drains
/// in `PreUpdate` whatever the virtual clock says, so edits from the editor
/// land while the game is paused.
pub(crate) fn install_link(app: &mut App, transport: Box<dyn PieTransport>) {
    if let Some(switch) = app.world().get_resource::<crate::LeaveParseToSpawn>() {
        switch.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    app.insert_non_send(PieTransportRes(transport));
    app.init_resource::<crate::served_scene::SceneRequests>();
    app.add_observer(crate::served_scene::forget_answer_on_insert);
    app.init_resource::<Highlight>();
    crate::node_sync::install_node_index(app);
    // Drain control in PreUpdate, before input processing so forwarded editor
    // input lands in `ButtonInput` the same frame.
    app.add_systems(
        PreUpdate,
        (apply_control, stream_cursor_state)
            .chain()
            .before(bevy::input::InputSystems),
    );
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
/// gameplay systems keyed on `Time<Virtual>`. `NodeComponents` applies
/// reflected edits to every entity spawned from a scene node.
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
            ControlEvent::SpawnNode {
                node,
                parent,
                follows,
                scene,
                dir,
                bsn,
            } => {
                apply_structure_change(
                    world,
                    StructureChange::Spawn {
                        node,
                        parent,
                        follows,
                        scene,
                        dir,
                        bsn,
                    },
                );
            }
            ControlEvent::DespawnNode { node, scene } => {
                apply_structure_change(world, StructureChange::Despawn { node, scene });
            }
            ControlEvent::ReparentNode {
                node,
                parent,
                follows,
                scene,
            } => {
                apply_structure_change(
                    world,
                    StructureChange::Reparent {
                        node,
                        parent,
                        follows,
                        scene,
                    },
                );
            }
            ControlEvent::Highlight { node } => {
                world.resource_mut::<Highlight>().node = node;
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

    // Iterative subtree walk with a visited set, so a parent cycle
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

/// Find this frame's entities of the highlighted node. A node with none is boxed
/// again once they appear.
fn resolve_highlight(world: &mut World) {
    let entities = world
        .resource::<Highlight>()
        .node
        .map(|node| crate::node_structure::unit_holders(world, node))
        .unwrap_or_default();
    world.resource_mut::<Highlight>().entities = entities;
}

/// Draw a wireframe box around every entity of the highlighted node through the
/// game's own cameras, so the outline appears in the streamed frame. An entity
/// with no mesh bounds gets a small cube at its position.
fn draw_highlight(
    highlight: Res<Highlight>,
    mut gizmos: Gizmos,
    transforms: Query<&GlobalTransform>,
    aabbs: Query<&bevy::camera::primitives::Aabb>,
    children: Query<&Children>,
) {
    const HIGHLIGHT_COLOR: Color = Color::srgb(1.0, 0.62, 0.1);
    for &entity in &highlight.entities {
        let Ok(entity_gt) = transforms.get(entity) else {
            continue;
        };
        let boxes = highlight_boxes(entity, &transforms, &aabbs, &children);
        if boxes.is_empty() {
            gizmos.cube(
                Transform::from_translation(entity_gt.translation()).with_scale(Vec3::splat(0.5)),
                HIGHLIGHT_COLOR,
            );
            continue;
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::AppExit;
    use jackdaw_pie_protocol::event::{ControlEvent, PieChannel, to_bytes};
    use jackdaw_pie_protocol::{IpcChannelTransport, connect, serve};
    use jackdaw_scene_types::SceneNodeId;

    /// A `Stop` from the editor makes the game write `AppExit`.
    #[test]
    fn stops_on_control() {
        let (mut editor, game) = jackdaw_pie_protocol::LoopbackTransport::pair();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        install_link(&mut app, Box::new(game));

        app.update();
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());

        editor.send(
            PieChannel::Reliable,
            &to_bytes(&ControlEvent::Stop).expect("encode"),
        );
        app.update();
        assert!(!app.world().resource::<Messages<AppExit>>().is_empty());
    }

    /// `Highlight` names a scene node, and the box follows the entities spawned
    /// from it as they come and go.
    #[test]
    fn highlight_follows_the_entities_of_its_node() {
        let (mut editor, game) = jackdaw_pie_protocol::LoopbackTransport::pair();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        install_link(&mut app, Box::new(game));
        app.add_systems(Update, resolve_highlight);
        let first = app.world_mut().spawn(SceneNodeId(7)).id();
        let second = app.world_mut().spawn(SceneNodeId(7)).id();
        app.world_mut().spawn(SceneNodeId(8));
        let send = |editor: &mut jackdaw_pie_protocol::LoopbackTransport, node| {
            let event = ControlEvent::Highlight { node };
            editor.send(PieChannel::Reliable, &to_bytes(&event).expect("encode"));
        };

        send(&mut editor, Some(7));
        app.update();
        assert_eq!(
            app.world().resource::<Highlight>().entities,
            vec![first, second]
        );

        app.world_mut().despawn(first);
        app.update();
        assert_eq!(app.world().resource::<Highlight>().entities, vec![second]);

        let later = app.world_mut().spawn(SceneNodeId(7)).id();
        app.update();
        assert_eq!(
            app.world().resource::<Highlight>().entities,
            vec![second, later]
        );

        send(&mut editor, None);
        app.update();
        assert!(app.world().resource::<Highlight>().entities.is_empty());
    }

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
}
