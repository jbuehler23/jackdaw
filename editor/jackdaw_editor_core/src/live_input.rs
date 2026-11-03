//! Input capture for the running game: while engaged, editor keyboard and
//! mouse input forwards to the focused game instance over the control
//! channel instead of driving editor keybinds.
//!
//! Engage with a Play-mode click in the Game panel or the header button
//! (`pie.play_input_toggle`); release with Shift+Esc. Plain Esc forwards,
//! so the game's own menus keep working. Every release path synthesizes
//! key-up and button-up events for whatever was held, so the game is never
//! left with stuck input.

use std::collections::HashMap;
use std::collections::HashSet;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::input::mouse::{MouseButton, MouseButtonInput, MouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorMoved, CursorOptions, PrimaryWindow, WindowFocused};
use jackdaw_api::prelude::*;
use jackdaw_pie_protocol::{ControlEvent, PieInputEvent};

use crate::live_frame::LiveFrameStream;

/// Capture state: whether editor input forwards to the game, what is held so
/// release can synthesize ups, and which Game panel node the cursor remaps
/// against.
#[derive(Resource, Default)]
pub struct LiveInputCapture {
    pub active: bool,
    pub(crate) release_requested: bool,
    pub(crate) held_keys: HashMap<KeyCode, Key>,
    pub(crate) held_buttons: HashSet<MouseButton>,
    pub(crate) panel_node: Option<Entity>,
    /// The game's last reported cursor grab state. The cursor mirror reads
    /// these to match the editor window to the game while capturing.
    pub(crate) game_cursor_grabbed: bool,
    pub(crate) game_cursor_visible: bool,
}

/// Events collected this frame, drained by `flush_forwards` and sent to the
/// focused instance.
#[derive(Resource, Default)]
pub(crate) struct PendingForwards(pub(crate) Vec<PieInputEvent>);

/// Engage capture bound to the Game panel surface: input forwards to the
/// focused game until released.
pub(crate) fn engage_capture(world: &mut World, surface: Entity) {
    {
        let mut capture = world.resource_mut::<LiveInputCapture>();
        capture.active = true;
        capture.panel_node = Some(surface);
    }
    world
        .resource_mut::<bevy::input_focus::InputFocus>()
        .clear();
    world
        .resource_mut::<PendingForwards>()
        .0
        .push(PieInputEvent::FocusGained);
    apply_stored_cursor_state(world);
}

/// Forward keyboard and mouse input to the focused game while engaged. The
/// Shift+Esc chord is swallowed and turned into a release request; plain Esc
/// forwards so the game's own menus work.
fn collect_forwards(
    mut capture: ResMut<LiveInputCapture>,
    mut keys: MessageReader<KeyboardInput>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut wheel: MessageReader<MouseWheel>,
    mut motion: MessageReader<MouseMotion>,
    mut cursor: MessageReader<CursorMoved>,
    mut pending: ResMut<PendingForwards>,
    stream: Option<Res<LiveFrameStream>>,
    surfaces: Query<
        (&ComputedNode, &bevy::ui::UiGlobalTransform),
        With<crate::game_panel::GamePanelSurface>,
    >,
) {
    if !capture.active {
        keys.clear();
        buttons.clear();
        wheel.clear();
        motion.clear();
        cursor.clear();
        return;
    }

    for key in keys.read() {
        let pressed = key.state == ButtonState::Pressed;
        if pressed
            && key.key_code == KeyCode::Escape
            && (capture.held_keys.contains_key(&KeyCode::ShiftLeft)
                || capture.held_keys.contains_key(&KeyCode::ShiftRight))
        {
            capture.release_requested = true;
            continue;
        }
        if pressed {
            capture
                .held_keys
                .insert(key.key_code, key.logical_key.clone());
        } else {
            capture.held_keys.remove(&key.key_code);
        }
        pending.0.push(PieInputEvent::Key {
            key: key.key_code,
            logical: key.logical_key.clone(),
            pressed,
            repeat: key.repeat,
        });
    }

    for input in buttons.read() {
        let pressed = input.state == ButtonState::Pressed;
        if pressed {
            capture.held_buttons.insert(input.button);
        } else {
            capture.held_buttons.remove(&input.button);
        }
        pending.0.push(PieInputEvent::MouseButton {
            button: input.button,
            pressed,
        });
    }

    for event in wheel.read() {
        pending.0.push(PieInputEvent::MouseWheel {
            x: event.x,
            y: event.y,
            line_units: event.unit == MouseScrollUnit::Line,
        });
    }

    for event in motion.read() {
        pending
            .0
            .push(PieInputEvent::MouseMotion { delta: event.delta });
    }

    // Cursor positions remap into the streamed frame's pixel space through the
    // bound Game panel surface's letterbox. When the bound surface or the
    // stream size are missing (headless worlds, no game view), the events are
    // dropped rather than forwarded in the wrong space.
    let remap_inputs = capture.panel_node.and_then(|surface| {
        let (computed, transform) = surfaces.get(surface).ok()?;
        let stream_size = stream.as_deref()?.size.as_vec2();
        if stream_size.x < 1.0 || stream_size.y < 1.0 {
            return None;
        }
        let (top_left, panel) = crate::game_panel::surface_remap(computed, transform);
        Some((top_left, panel, stream_size))
    });
    match remap_inputs {
        Some((top_left, panel, stream_size)) => {
            for moved in cursor.read() {
                let Some(position) = crate::game_panel::panel_to_stream(
                    moved.position - top_left,
                    panel,
                    stream_size,
                ) else {
                    continue;
                };
                pending.0.push(PieInputEvent::CursorMoved { position });
            }
        }
        None => cursor.clear(),
    }
}

/// Request release when the editor loses OS focus or the frame stream stops
/// being fresh, so capture never strands the game with held input.
fn auto_release(
    mut capture: ResMut<LiveInputCapture>,
    stream: Option<Res<LiveFrameStream>>,
    mut focus_events: MessageReader<WindowFocused>,
    windows: Query<(), With<PrimaryWindow>>,
) {
    if !capture.active {
        focus_events.clear();
        return;
    }
    let view_ok = stream
        .as_deref()
        .is_some_and(crate::live_frame::LiveFrameStream::is_fresh);
    let lost_os_focus = focus_events
        .read()
        .any(|event| !event.focused && windows.get(event.window).is_ok());
    if lost_os_focus || !view_ok {
        capture.release_requested = true;
    }
}

/// Apply a pending release: disengage, synthesize ups for everything held,
/// signal lost focus, and restore the editor cursor.
pub(crate) fn apply_release_requests(world: &mut World) {
    let (requested, held_keys, held_buttons) = {
        let mut capture = world.resource_mut::<LiveInputCapture>();
        let requested = capture.release_requested && capture.active;
        capture.release_requested = false;
        if !requested {
            return;
        }
        capture.active = false;
        capture.panel_node = None;
        let held_keys = std::mem::take(&mut capture.held_keys);
        let held_buttons = std::mem::take(&mut capture.held_buttons);
        (requested, held_keys, held_buttons)
    };
    if !requested {
        return;
    }

    let mut pending = world.resource_mut::<PendingForwards>();
    for (key, logical) in held_keys {
        pending.0.push(PieInputEvent::Key {
            key,
            logical,
            pressed: false,
            repeat: false,
        });
    }
    for button in held_buttons {
        pending.0.push(PieInputEvent::MouseButton {
            button,
            pressed: false,
        });
    }
    pending.0.push(PieInputEvent::FocusLost);

    restore_editor_cursor(world);
}

/// Send everything collected this frame to the focused game instance.
pub(crate) fn flush_forwards(world: &mut World) {
    let events = std::mem::take(&mut world.resource_mut::<PendingForwards>().0);
    for event in events {
        crate::pie::send_control_to_focused(world, ControlEvent::Input(event));
    }
}

/// Record the game's cursor state and, while captured, mirror it onto the
/// editor's own cursor so mouse-look locks and hides like a native game.
pub(crate) fn note_game_cursor_state(world: &mut World, grabbed: bool, visible: bool) {
    {
        let mut capture = world.resource_mut::<LiveInputCapture>();
        capture.game_cursor_grabbed = grabbed;
        capture.game_cursor_visible = visible;
        if !capture.active {
            return;
        }
    }
    write_editor_cursor(world, grabbed, visible);
}

/// Re-apply the game's last known cursor state to the editor window. Called
/// when an engage path takes over while the game already holds the cursor, so
/// the editor locks immediately instead of waiting on the next stream event.
fn apply_stored_cursor_state(world: &mut World) {
    let (grabbed, visible) = {
        let capture = world.resource::<LiveInputCapture>();
        (capture.game_cursor_grabbed, capture.game_cursor_visible)
    };
    if grabbed {
        write_editor_cursor(world, grabbed, visible);
    }
}

/// Set the editor primary-window cursor to the given grab and visibility.
/// A no-op in headless worlds with no primary window.
fn write_editor_cursor(world: &mut World, grabbed: bool, visible: bool) {
    let mut cursors = world.query_filtered::<&mut CursorOptions, With<PrimaryWindow>>();
    if let Ok(mut options) = cursors.single_mut(world) {
        options.grab_mode = if grabbed {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        options.visible = visible;
    }
}

/// Release the editor's primary-window cursor: ungrab it and make it visible.
/// A no-op in headless worlds with no primary window.
pub fn restore_editor_cursor(world: &mut World) {
    let mut cursors = world.query_filtered::<&mut CursorOptions, With<PrimaryWindow>>();
    if let Ok(mut options) = cursors.single_mut(world) {
        options.grab_mode = CursorGrabMode::None;
        options.visible = true;
    }
}

/// Engage capture from the header button: bind to the Game panel surface and
/// signal the game it gained focus. Returns without engaging when no surface
/// is present.
fn engage_from_header(world: &mut World) {
    let Some(surface) = world
        .query_filtered::<Entity, With<crate::game_panel::GamePanelSurface>>()
        .iter(world)
        .next()
    else {
        return;
    };
    engage_capture(world, surface);
}

/// Toggle input forwarding to the running game: engage if idle, request
/// release (and flush the synthesized ups) if already capturing.
#[operator(
    id = "pie.play_input_toggle",
    label = "Play Input",
    description = "Forward keyboard and mouse to the running game (Shift+Esc releases).",
    is_available = crate::pie::focused_with_fresh_stream
)]
pub(crate) fn pie_play_input_toggle(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        if world.resource::<LiveInputCapture>().active {
            world.resource_mut::<LiveInputCapture>().release_requested = true;
            apply_release_requests(world);
            flush_forwards(world);
        } else {
            engage_from_header(world);
        }
    });
    OperatorResult::Finished
}

pub struct LiveInputPlugin;

impl Plugin for LiveInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LiveInputCapture>()
            .init_resource::<PendingForwards>()
            .add_systems(
                Update,
                (
                    collect_forwards,
                    auto_release,
                    apply_release_requests,
                    flush_forwards,
                )
                    .chain(),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};

    fn capture_app() -> App {
        let mut app = App::new();
        app.add_message::<KeyboardInput>();
        app.add_message::<bevy::input::mouse::MouseButtonInput>();
        app.add_message::<bevy::input::mouse::MouseWheel>();
        app.add_message::<bevy::input::mouse::MouseMotion>();
        app.add_message::<bevy::window::CursorMoved>();
        app.init_resource::<LiveInputCapture>();
        app.init_resource::<PendingForwards>();
        app
    }

    #[test]
    fn shift_esc_requests_release_and_swallows_the_esc() {
        let mut app = capture_app();
        {
            let mut capture = app.world_mut().resource_mut::<LiveInputCapture>();
            capture.active = true;
            capture.held_keys.insert(KeyCode::ShiftLeft, Key::Shift);
            capture
                .held_keys
                .insert(KeyCode::KeyW, Key::Character("w".into()));
            capture
                .held_buttons
                .insert(bevy::input::mouse::MouseButton::Left);
        }
        let window = app.world_mut().spawn_empty().id();
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Escape,
            logical_key: Key::Escape,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window,
        });
        app.world_mut().run_system_cached(collect_forwards).unwrap();

        let capture = app.world().resource::<LiveInputCapture>();
        assert!(capture.release_requested, "the chord requests release");
        let pending = app.world().resource::<PendingForwards>();
        assert!(
            !pending.0.iter().any(|e| matches!(
                e,
                jackdaw_pie_protocol::PieInputEvent::Key {
                    key: KeyCode::Escape,
                    ..
                }
            )),
            "the release chord itself is never forwarded"
        );
    }

    #[test]
    fn release_synthesizes_ups_for_everything_held() {
        let mut app = capture_app();
        {
            let mut capture = app.world_mut().resource_mut::<LiveInputCapture>();
            capture.active = true;
            capture.release_requested = true;
            capture
                .held_keys
                .insert(KeyCode::KeyW, Key::Character("w".into()));
            capture
                .held_buttons
                .insert(bevy::input::mouse::MouseButton::Left);
        }
        app.world_mut()
            .run_system_cached(apply_release_requests)
            .unwrap();

        let capture = app.world().resource::<LiveInputCapture>();
        assert!(!capture.active);
        assert!(capture.held_keys.is_empty() && capture.held_buttons.is_empty());
        let pending = &app.world().resource::<PendingForwards>().0;
        use jackdaw_pie_protocol::PieInputEvent;
        assert!(pending.iter().any(|e| matches!(
            e,
            PieInputEvent::Key {
                key: KeyCode::KeyW,
                pressed: false,
                ..
            }
        )));
        assert!(pending.iter().any(|e| matches!(
            e,
            PieInputEvent::MouseButton {
                button: bevy::input::mouse::MouseButton::Left,
                pressed: false
            }
        )));
        assert!(matches!(pending.last(), Some(PieInputEvent::FocusLost)));
    }

    #[test]
    fn note_game_cursor_state_mirrors_onto_the_editor_when_active() {
        use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
        let mut world = World::new();
        world.init_resource::<LiveInputCapture>();
        let window = world.spawn((PrimaryWindow, CursorOptions::default())).id();

        // Inactive: only records, does not touch the editor cursor.
        note_game_cursor_state(&mut world, true, false);
        {
            let capture = world.resource::<LiveInputCapture>();
            assert!(capture.game_cursor_grabbed);
            assert!(!capture.game_cursor_visible);
        }
        assert_eq!(
            world.get::<CursorOptions>(window).unwrap().grab_mode,
            CursorGrabMode::None,
            "inactive capture must not grab the editor cursor"
        );

        // Active: mirrors grab + visibility onto the editor window.
        world.resource_mut::<LiveInputCapture>().active = true;
        note_game_cursor_state(&mut world, true, false);
        let options = world.get::<CursorOptions>(window).unwrap();
        assert_eq!(options.grab_mode, CursorGrabMode::Locked);
        assert!(!options.visible);
    }

    #[test]
    fn engage_from_panel_binds_the_surface_and_queues_focus() {
        let mut world = World::new();
        world.init_resource::<LiveInputCapture>();
        world.init_resource::<PendingForwards>();
        world.init_resource::<bevy::input_focus::InputFocus>();
        let surface = world.spawn_empty().id();

        engage_capture(&mut world, surface);

        let capture = world.resource::<LiveInputCapture>();
        assert!(capture.active);
        assert_eq!(capture.panel_node, Some(surface));
        assert!(matches!(
            world.resource::<PendingForwards>().0.last(),
            Some(jackdaw_pie_protocol::PieInputEvent::FocusGained)
        ));
    }

    #[test]
    fn plain_keys_forward_and_track_held_state() {
        let mut app = capture_app();
        app.world_mut().resource_mut::<LiveInputCapture>().active = true;
        let window = app.world_mut().spawn_empty().id();
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::KeyW,
            logical_key: Key::Character("w".into()),
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window,
        });
        app.world_mut().run_system_cached(collect_forwards).unwrap();
        assert!(
            app.world()
                .resource::<LiveInputCapture>()
                .held_keys
                .contains_key(&KeyCode::KeyW)
        );
        assert_eq!(app.world().resource::<PendingForwards>().0.len(), 1);
    }
}
