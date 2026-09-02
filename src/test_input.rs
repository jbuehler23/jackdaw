//! Synthetic pointer and keyboard input, as operators.
//!
//! [`crate::boot_ops`] runs operators with no mouse, which reaches every
//! operator and nothing that only a gesture can do: a drag on the canvas,
//! a click on a menu row, a double click that opens the in-place text
//! editor, a rename typed into a field. Those are the parts of the editor
//! a scripted run could not touch, and they are the parts a usability pass
//! is about.
//!
//! ```text
//! JACKDAW_RUN_OP="input.pointer x=640 y=400 action=click"
//! JACKDAW_RUN_OP="input.pointer space=canvas x=200 y=120 action=drag_to steps=12"
//! JACKDAW_RUN_OP="input.key key=KeyD mods=ctrl; input.text text=Play"
//! ```
//!
//! # The path taken
//!
//! Nothing here triggers a `Pointer<Click>` or a `FocusedInput`. Each
//! operator queues the *window events* winit would have delivered --
//! [`CursorMoved`], [`MouseButtonInput`], [`KeyboardInput`], each written
//! both on its own message stream and on the combined [`WindowEvent`] one,
//! exactly as `bevy_winit` forwards them -- and moves the window's own
//! cursor position with them. Everything downstream then behaves as it
//! does for a user: `bevy_picking`'s `mouse_pick_events` turns them into
//! `PointerInput` for `PointerId::Mouse`, the hover map is rebuilt,
//! `Pointer<Press>` / `Drag` / `Click` come out with their real click
//! counts and drag thresholds, `ButtonInput` picks the buttons and keys
//! up, and [`crate::viewport_2d`]'s forwarding carries the stream onto the
//! canvas's own pointer.
//!
//! # Frames
//!
//! A gesture is not one instant. The queue is a list of *beats*, and one
//! beat is emitted per pass with `frames` frames left to run before the
//! next, so a press lands on a hover the editor has already seen and a
//! drag's steps are separate frames rather than one collapsed jump. That
//! is also why the modifiers a clause names are pressed in a beat of their
//! own ahead of the event they modify: `ButtonInput<KeyCode>` is written
//! in `PreUpdate`, after the picking input pass that would read it.
//!
//! [`crate::boot_ops`] holds its next clause while the queue has anything
//! left, so a script reads as one gesture per clause however many frames
//! the gesture takes.

use std::collections::VecDeque;

use bevy::{
    input::{
        ButtonState,
        keyboard::{Key, KeyboardInput, NativeKey},
        mouse::MouseButtonInput,
    },
    math::DVec2,
    picking::PickingSystems,
    prelude::*,
    window::{PrimaryWindow, WindowEvent},
};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::key_code_from_name;

/// The extension the input operators are registered from.
///
/// A built-in, so it is enabled in every editor launch and in the test
/// harness without a config file saying so. It registers no window, no
/// menu entry and no keybind: an operator here is reached from a script,
/// and a chord that moved the mouse would be a chord no user wants.
pub const EXTENSION_ID: &str = "jackdaw.test_input";

#[derive(Default)]
pub struct TestInputExtension;

impl JackdawExtension for TestInputExtension {
    fn id(&self) -> String {
        EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Synthetic Input".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_operator::<InputPointerOp>()
            .register_operator::<InputKeyOp>()
            .register_operator::<InputTextOp>();
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<SyntheticInput>().add_systems(
        First,
        // Ahead of the pass that turns window events into `PointerInput`,
        // so a beat queued here is this frame's input rather than the next
        // frame's.
        drive_synthetic_input.before(PickingSystems::Input),
    );
}

/// Default steps a `drag_to` is interpolated over.
const DEFAULT_DRAG_STEPS: i64 = 8;

/// Default frames between one beat and the next.
const DEFAULT_FRAMES: i64 = 1;

/// Most steps one drag may be cut into, and most frames one beat may
/// wait. A typo in an environment variable should cost a refused clause,
/// not a session that never advances again.
const MAX_STEPS: i64 = 512;
const MAX_FRAMES: i64 = 600;

/// One window event to deliver, in the form the window system delivers it.
#[derive(Clone, Debug)]
enum Emit {
    /// Move the cursor to a position in window logical pixels.
    Cursor(Vec2),
    Button {
        button: MouseButton,
        state: ButtonState,
    },
    Key {
        key: KeyCode,
        logical: Key,
        text: Option<String>,
        state: ButtonState,
    },
}

/// The events of one frame, and how many frames to let pass afterwards.
#[derive(Clone, Debug)]
struct Beat {
    events: Vec<Emit>,
    frames: u32,
}

/// The gesture still being played out.
///
/// Public so [`crate::boot_ops`] can hold its next clause until the
/// current one's gesture has finished; nothing else reads it.
#[derive(Resource, Default)]
pub struct SyntheticInput {
    beats: VecDeque<Beat>,
    wait: u32,
}

impl SyntheticInput {
    /// Whether every queued beat has been delivered and its frames have
    /// passed.
    pub fn is_idle(&self) -> bool {
        self.beats.is_empty() && self.wait == 0
    }

    fn queue(&mut self, beats: Vec<Beat>) {
        self.beats.extend(beats);
    }
}

/// The window a synthetic event is delivered to.
fn primary_window(world: &mut World) -> Option<Entity> {
    let mut windows = world.query_filtered::<Entity, With<PrimaryWindow>>();
    windows.iter(world).next()
}

/// Emit one beat per pass, `frames` frames apart.
fn drive_synthetic_input(world: &mut World) {
    {
        let mut queue = world.resource_mut::<SyntheticInput>();
        if queue.wait > 0 {
            queue.wait -= 1;
            return;
        }
        if queue.beats.is_empty() {
            return;
        }
    }
    let Some(window) = primary_window(world) else {
        return;
    };
    let beat = {
        let mut queue = world.resource_mut::<SyntheticInput>();
        let Some(beat) = queue.beats.pop_front() else {
            return;
        };
        queue.wait = beat.frames;
        beat
    };
    for event in beat.events {
        emit(world, window, event);
    }
}

/// Deliver one event the way `bevy_winit` delivers it: on its own message
/// stream and on the combined [`WindowEvent`] stream that `bevy_picking`
/// reads, with the window's cursor moved to match.
fn emit(world: &mut World, window: Entity, event: Emit) {
    match event {
        Emit::Cursor(logical) => {
            let Some(mut win) = world.get_mut::<Window>(window) else {
                return;
            };
            let scale = win.resolution.scale_factor();
            let physical =
                DVec2::new(f64::from(logical.x), f64::from(logical.y)) * f64::from(scale);
            let last = win.physical_cursor_position();
            win.set_physical_cursor_position(Some(physical));
            let delta = last.map(|last| (physical.as_vec2() - last) / scale);
            let moved = CursorMoved {
                window,
                position: logical,
                delta,
            };
            world.write_message(moved.clone());
            world.write_message(WindowEvent::CursorMoved(moved));
        }
        Emit::Button { button, state } => {
            let input = MouseButtonInput {
                button,
                state,
                window,
            };
            world.write_message(input);
            world.write_message(WindowEvent::MouseButtonInput(input));
        }
        Emit::Key {
            key,
            logical,
            text,
            state,
        } => {
            let input = KeyboardInput {
                key_code: key,
                logical_key: logical,
                state,
                text: text.map(Into::into),
                repeat: false,
                window,
            };
            world.write_message(input.clone());
            world.write_message(WindowEvent::KeyboardInput(input));
        }
    }
}

/// The modifiers a `mods=` list names, in press order.
///
/// Unknown names are dropped with a warning rather than failing the
/// clause, matching how [`crate::boot_ops`] treats a token it cannot
/// parse.
fn parse_mods(spec: Option<&str>) -> Vec<KeyCode> {
    let mut out = Vec::new();
    for name in spec.unwrap_or_default().split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        match name.to_ascii_lowercase().as_str() {
            "shift" => out.push(KeyCode::ShiftLeft),
            "ctrl" | "control" => out.push(KeyCode::ControlLeft),
            "alt" => out.push(KeyCode::AltLeft),
            "super" | "cmd" | "meta" => out.push(KeyCode::SuperLeft),
            other => warn!("input: ignoring unknown modifier {other:?}"),
        }
    }
    out
}

/// The logical key and produced text a physical key stands for on a
/// plain US layout.
///
/// A synthetic press has no layout behind it, and the widgets downstream
/// read the *logical* key: `bevy_ui_widgets`' text input inserts
/// `KeyboardInput::text` only for `Key::Character`, and matches
/// `Key::Escape`, `Key::Delete` and the arrows by name. Anything not
/// spelled out here still presses its `KeyCode`, which is what the
/// editor's own keybinds read.
fn logical_key(key: KeyCode, shift: bool) -> (Key, Option<String>) {
    if let Some(character) = character_for(key) {
        let text = if shift {
            character.to_ascii_uppercase().to_string()
        } else {
            character.to_string()
        };
        return (Key::Character(text.clone().into()), Some(text));
    }
    let named = match key {
        KeyCode::Space => return (Key::Space, Some(" ".to_string())),
        KeyCode::Enter | KeyCode::NumpadEnter => Key::Enter,
        KeyCode::Escape => Key::Escape,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Key::Shift,
        KeyCode::ControlLeft | KeyCode::ControlRight => Key::Control,
        KeyCode::AltLeft | KeyCode::AltRight => Key::Alt,
        KeyCode::SuperLeft | KeyCode::SuperRight => Key::Super,
        KeyCode::F1 => Key::F1,
        KeyCode::F2 => Key::F2,
        KeyCode::F3 => Key::F3,
        KeyCode::F4 => Key::F4,
        KeyCode::F5 => Key::F5,
        KeyCode::F6 => Key::F6,
        KeyCode::F7 => Key::F7,
        KeyCode::F8 => Key::F8,
        KeyCode::F9 => Key::F9,
        KeyCode::F10 => Key::F10,
        KeyCode::F11 => Key::F11,
        KeyCode::F12 => Key::F12,
        _ => Key::Unidentified(NativeKey::Unidentified),
    };
    (named, None)
}

/// The character a printable key produces unshifted, or `None` for a key
/// that produces none.
fn character_for(key: KeyCode) -> Option<char> {
    const LETTERS: [(KeyCode, char); 26] = [
        (KeyCode::KeyA, 'a'),
        (KeyCode::KeyB, 'b'),
        (KeyCode::KeyC, 'c'),
        (KeyCode::KeyD, 'd'),
        (KeyCode::KeyE, 'e'),
        (KeyCode::KeyF, 'f'),
        (KeyCode::KeyG, 'g'),
        (KeyCode::KeyH, 'h'),
        (KeyCode::KeyI, 'i'),
        (KeyCode::KeyJ, 'j'),
        (KeyCode::KeyK, 'k'),
        (KeyCode::KeyL, 'l'),
        (KeyCode::KeyM, 'm'),
        (KeyCode::KeyN, 'n'),
        (KeyCode::KeyO, 'o'),
        (KeyCode::KeyP, 'p'),
        (KeyCode::KeyQ, 'q'),
        (KeyCode::KeyR, 'r'),
        (KeyCode::KeyS, 's'),
        (KeyCode::KeyT, 't'),
        (KeyCode::KeyU, 'u'),
        (KeyCode::KeyV, 'v'),
        (KeyCode::KeyW, 'w'),
        (KeyCode::KeyX, 'x'),
        (KeyCode::KeyY, 'y'),
        (KeyCode::KeyZ, 'z'),
    ];
    const DIGITS: [(KeyCode, char); 10] = [
        (KeyCode::Digit0, '0'),
        (KeyCode::Digit1, '1'),
        (KeyCode::Digit2, '2'),
        (KeyCode::Digit3, '3'),
        (KeyCode::Digit4, '4'),
        (KeyCode::Digit5, '5'),
        (KeyCode::Digit6, '6'),
        (KeyCode::Digit7, '7'),
        (KeyCode::Digit8, '8'),
        (KeyCode::Digit9, '9'),
    ];
    LETTERS
        .iter()
        .chain(DIGITS.iter())
        .find(|(code, _)| *code == key)
        .map(|(_, character)| *character)
        .or(match key {
            KeyCode::Minus => Some('-'),
            KeyCode::Equal => Some('='),
            KeyCode::Comma => Some(','),
            KeyCode::Period => Some('.'),
            KeyCode::Slash => Some('/'),
            KeyCode::Semicolon => Some(';'),
            KeyCode::Quote => Some('\''),
            KeyCode::BracketLeft => Some('['),
            KeyCode::BracketRight => Some(']'),
            KeyCode::Backslash => Some('\\'),
            KeyCode::Backquote => Some('`'),
            _ => None,
        })
}

/// The `KeyCode` a character is typed on, for a plain US layout.
///
/// The logical key and the text carry the character itself, so a
/// character with no key of its own still types; this is only what
/// `ButtonInput<KeyCode>` sees, which is what the editor's own chords
/// read.
fn key_code_for(character: char) -> KeyCode {
    let lower = character.to_ascii_lowercase();
    if lower == ' ' {
        return KeyCode::Space;
    }
    for candidate in ALL_TYPING_KEYS {
        if character_for(*candidate) == Some(lower) {
            return *candidate;
        }
    }
    KeyCode::Unidentified(bevy::input::keyboard::NativeKeyCode::Unidentified)
}

/// Every key [`character_for`] answers for, so [`key_code_for`] can run
/// the map backwards without a second copy of it.
const ALL_TYPING_KEYS: &[KeyCode] = &[
    KeyCode::KeyA,
    KeyCode::KeyB,
    KeyCode::KeyC,
    KeyCode::KeyD,
    KeyCode::KeyE,
    KeyCode::KeyF,
    KeyCode::KeyG,
    KeyCode::KeyH,
    KeyCode::KeyI,
    KeyCode::KeyJ,
    KeyCode::KeyK,
    KeyCode::KeyL,
    KeyCode::KeyM,
    KeyCode::KeyN,
    KeyCode::KeyO,
    KeyCode::KeyP,
    KeyCode::KeyQ,
    KeyCode::KeyR,
    KeyCode::KeyS,
    KeyCode::KeyT,
    KeyCode::KeyU,
    KeyCode::KeyV,
    KeyCode::KeyW,
    KeyCode::KeyX,
    KeyCode::KeyY,
    KeyCode::KeyZ,
    KeyCode::Digit0,
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
    KeyCode::Minus,
    KeyCode::Equal,
    KeyCode::Comma,
    KeyCode::Period,
    KeyCode::Slash,
    KeyCode::Semicolon,
    KeyCode::Quote,
    KeyCode::BracketLeft,
    KeyCode::BracketRight,
    KeyCode::Backslash,
    KeyCode::Backquote,
];

/// Beats pressing every modifier in `mods`, or `None` when there are
/// none.
fn mod_beats(mods: &[KeyCode], state: ButtonState, frames: u32) -> Option<Beat> {
    if mods.is_empty() {
        return None;
    }
    let shift = mods.contains(&KeyCode::ShiftLeft);
    Some(Beat {
        events: mods
            .iter()
            .map(|key| {
                let (logical, text) = logical_key(*key, shift);
                Emit::Key {
                    key: *key,
                    logical,
                    text,
                    state,
                }
            })
            .collect(),
        frames,
    })
}

/// Wrap `gesture` in the press and release of the modifiers it is held
/// under.
fn with_mods(mods: &[KeyCode], frames: u32, gesture: Vec<Beat>) -> Vec<Beat> {
    let mut beats = Vec::with_capacity(gesture.len() + 2);
    beats.extend(mod_beats(mods, ButtonState::Pressed, frames));
    beats.extend(gesture);
    beats.extend(mod_beats(mods, ButtonState::Released, frames));
    beats
}

/// A count parameter, clamped to something a session can finish.
fn bounded(params: &OperatorParameters, key: &str, default: i64, max: i64) -> u32 {
    let raw = params.as_int(key).unwrap_or(default);
    u32::try_from(raw.clamp(0, max)).unwrap_or(0)
}

/// Where a clause's `x`/`y` land in window logical pixels.
///
/// `space=window` is the default and passes them through.
/// `space=canvas` reads them as authored canvas pixels and maps them
/// through the fronted 2D panel's stage -- the same mapping a guide
/// dragged out of the ruler is landed by, run backwards -- so a script
/// can aim at a node's own coordinates instead of at wherever the panel
/// happens to be on screen.
fn resolve_position(world: &mut World, params: &OperatorParameters) -> Option<Vec2> {
    let x = params.as_float("x")?;
    let y = params.as_float("y")?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a pixel position is an f32 everywhere downstream"
    )]
    let point = Vec2::new(x as f32, y as f32);
    match params.as_str("space").unwrap_or("window") {
        "window" => Some(point),
        "canvas" => {
            let mut panels = world.query::<(Entity, &crate::viewport_host::ViewportHost)>();
            let panel = crate::viewport_host::primary_2d_host(panels.iter(world))?;
            let cursor = crate::ui_stage::canvas_to_cursor(world, panel, point)?;
            Some(cursor * world.resource::<UiScale>().0)
        }
        other => {
            warn!("input.pointer: unknown space {other:?}; expected window or canvas");
            None
        }
    }
}

/// The pointer's current position in window logical pixels, for a
/// gesture that starts where the cursor already is.
fn current_position(world: &mut World) -> Option<Vec2> {
    let window = primary_window(world)?;
    world.get::<Window>(window)?.cursor_position()
}

fn pointer_button(name: &str) -> Option<MouseButton> {
    match name {
        "primary" | "left" => Some(MouseButton::Left),
        "secondary" | "right" => Some(MouseButton::Right),
        "middle" => Some(MouseButton::Middle),
        other => {
            warn!("input.pointer: unknown button {other:?}");
            None
        }
    }
}

/// Move, click or drag the mouse.
#[operator(
    id = "input.pointer",
    label = "Synthetic Pointer",
    description = "Drive the mouse: move, press, release, click, double click or drag.",
    allows_undo = false,
    params(
        x(f64, doc = "Horizontal position, in the space `space` names."),
        y(f64, doc = "Vertical position, in the space `space` names."),
        space(
            String,
            doc = "`window` (default) for window logical pixels, `canvas` for authored \
                   pixels on the 2D panel's canvas."
        ),
        action(
            String,
            doc = "move, press, release, click, dblclick, drag_to or rest. \
                   Defaults to move."
        ),
        button(String, doc = "primary (default), secondary or middle."),
        mods(
            String,
            doc = "Comma list of shift, ctrl, alt, super held for the gesture."
        ),
        steps(
            i64,
            doc = "Moves a drag_to is cut into, or beats a rest lasts. Defaults to 8."
        ),
        frames(i64, doc = "Frames between one event and the next. Defaults to 1.")
    )
)]
pub(crate) fn input_pointer(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let params = params.0;
    commands.queue(move |world: &mut World| {
        let frames = bounded(&params, "frames", DEFAULT_FRAMES, MAX_FRAMES);
        let steps = bounded(&params, "steps", DEFAULT_DRAG_STEPS, MAX_STEPS).max(1);
        let mods = parse_mods(params.as_str("mods"));
        let Some(button) = pointer_button(params.as_str("button").unwrap_or("primary")) else {
            return;
        };
        let action = params.as_str("action").unwrap_or("move").to_string();

        let press = Beat {
            events: vec![Emit::Button {
                button,
                state: ButtonState::Pressed,
            }],
            frames,
        };
        let release = Beat {
            events: vec![Emit::Button {
                button,
                state: ButtonState::Released,
            }],
            frames,
        };

        let gesture = if action == "rest" {
            // Beats with nothing in them: the cursor stays where it is and
            // the frames pass. A move to the same position would not do --
            // a `CursorMoved` of zero delta is still a move, and a menu
            // reads one as the pointer stirring on the row it rests on.
            vec![
                Beat {
                    events: Vec::new(),
                    frames,
                };
                usize::try_from(steps).unwrap_or(1)
            ]
        } else if action == "drag_to" {
            let Some(from) = current_position(world) else {
                warn!("input.pointer: drag_to has no cursor position to start from");
                return;
            };
            let Some(to) = resolve_position(world, &params) else {
                warn!("input.pointer: drag_to needs x and y");
                return;
            };
            let mut beats = vec![press];
            for step in 1..=steps {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a drag is at most MAX_STEPS steps"
                )]
                let t = step as f32 / steps as f32;
                beats.push(Beat {
                    events: vec![Emit::Cursor(from.lerp(to, t))],
                    frames,
                });
            }
            beats.push(release);
            beats
        } else {
            let Some(to) = resolve_position(world, &params) else {
                warn!("input.pointer: {action} needs x and y");
                return;
            };
            let move_to = Beat {
                events: vec![Emit::Cursor(to)],
                frames,
            };
            match action.as_str() {
                "move" => vec![move_to],
                "press" => vec![move_to, press],
                "release" => vec![move_to, release],
                "click" => vec![move_to, press, release],
                "dblclick" => vec![move_to, press.clone(), release.clone(), press, release],
                other => {
                    warn!("input.pointer: unknown action {other:?}");
                    return;
                }
            }
        };

        world
            .resource_mut::<SyntheticInput>()
            .queue(with_mods(&mods, frames, gesture));
    });
    OperatorResult::Finished
}

/// Press, release or tap one key.
#[operator(
    id = "input.key",
    label = "Synthetic Key",
    description = "Press, release or tap a key, optionally under modifiers.",
    allows_undo = false,
    params(
        key(
            String,
            doc = "Key name, as the keybind dialog spells it: KeyC, Escape, Delete."
        ),
        mods(
            String,
            doc = "Comma list of shift, ctrl, alt, super held for the key."
        ),
        action(
            String,
            doc = "press, release or tap (press then release). Defaults to tap."
        ),
        frames(i64, doc = "Frames between one event and the next. Defaults to 1.")
    )
)]
pub(crate) fn input_key(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let params = params.0;
    commands.queue(move |world: &mut World| {
        let frames = bounded(&params, "frames", DEFAULT_FRAMES, MAX_FRAMES);
        let Some(name) = params.as_str("key") else {
            warn!("input.key: no `key=` to press");
            return;
        };
        let Some(key) = key_code_from_name(name) else {
            warn!("input.key: {name:?} names no key");
            return;
        };
        let mods = parse_mods(params.as_str("mods"));
        let shift = mods.contains(&KeyCode::ShiftLeft);
        let (logical, text) = logical_key(key, shift);
        let beat = |state| Beat {
            events: vec![Emit::Key {
                key,
                logical: logical.clone(),
                text: text.clone(),
                state,
            }],
            frames,
        };
        let gesture = match params.as_str("action").unwrap_or("tap") {
            "press" => vec![beat(ButtonState::Pressed)],
            "release" => vec![beat(ButtonState::Released)],
            "tap" => vec![beat(ButtonState::Pressed), beat(ButtonState::Released)],
            other => {
                warn!("input.key: unknown action {other:?}");
                return;
            }
        };
        world
            .resource_mut::<SyntheticInput>()
            .queue(with_mods(&mods, frames, gesture));
    });
    OperatorResult::Finished
}

/// Type a string, one key at a time.
#[operator(
    id = "input.text",
    label = "Synthetic Typing",
    description = "Type a string into whatever holds the keyboard, one key press at a time.",
    allows_undo = false,
    params(
        text(
            String,
            doc = "What to type. A clause carries no spaces, so `_` and `%20` both type one."
        ),
        frames(i64, doc = "Frames between one key and the next. Defaults to 1.")
    )
)]
pub(crate) fn input_text(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let params = params.0;
    commands.queue(move |world: &mut World| {
        let frames = bounded(&params, "frames", DEFAULT_FRAMES, MAX_FRAMES);
        let Some(raw) = params.as_str("text") else {
            warn!("input.text: no `text=` to type");
            return;
        };
        let typed = unescape_spaces(raw);
        if typed.is_empty() {
            return;
        }
        let mut beats = Vec::with_capacity(typed.chars().count() * 2);
        for character in typed.chars() {
            let key = key_code_for(character);
            let text = character.to_string();
            let logical = if character == ' ' {
                Key::Space
            } else {
                Key::Character(text.clone().into())
            };
            for state in [ButtonState::Pressed, ButtonState::Released] {
                beats.push(Beat {
                    events: vec![Emit::Key {
                        key,
                        logical: logical.clone(),
                        text: Some(text.clone()),
                        state,
                    }],
                    frames,
                });
            }
        }
        world.resource_mut::<SyntheticInput>().queue(beats);
    });
    OperatorResult::Finished
}

/// The spaces a clause cannot carry, put back.
///
/// A `JACKDAW_RUN_OP` clause splits on whitespace and has no quoting, so
/// a value cannot contain a space (see [`crate::boot_ops`]). Both the
/// underscore and the percent escape spell one here; an underscore that
/// is meant literally is written `%5f`.
pub fn unescape_spaces(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(character) = chars.next() {
        if character != '%' {
            out.push(if character == '_' { ' ' } else { character });
            continue;
        }
        let hex: String = chars.clone().take(2).collect();
        match u8::from_str_radix(&hex, 16) {
            Ok(byte) if hex.len() == 2 => {
                chars.next();
                chars.next();
                out.push(char::from(byte));
            }
            _ => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_underscore_and_a_percent_escape_both_spell_a_space() {
        assert_eq!(unescape_spaces("New_Game"), "New Game");
        assert_eq!(unescape_spaces("New%20Game"), "New Game");
        assert_eq!(unescape_spaces("Play"), "Play");
    }

    /// A literal underscore is still reachable, so a node named with one
    /// can be typed.
    #[test]
    fn a_percent_escape_reaches_the_underscore_itself() {
        assert_eq!(unescape_spaces("snake%5fcase"), "snake_case");
    }

    /// A stray percent is text, not a failed clause.
    #[test]
    fn an_incomplete_escape_stays_a_percent() {
        assert_eq!(unescape_spaces("100%"), "100%");
        assert_eq!(unescape_spaces("50%z9"), "50%z9");
    }

    #[test]
    fn modifier_names_map_to_keys_and_unknown_ones_are_dropped() {
        assert_eq!(
            parse_mods(Some("shift,ctrl,alt")),
            vec![KeyCode::ShiftLeft, KeyCode::ControlLeft, KeyCode::AltLeft]
        );
        assert_eq!(parse_mods(Some("Shift")), vec![KeyCode::ShiftLeft]);
        assert!(parse_mods(Some("banana")).is_empty());
        assert!(parse_mods(None).is_empty());
    }

    /// The logical key is what a text field reads, so a letter has to
    /// carry its character and its text, and Shift has to reach the
    /// capital.
    #[test]
    fn a_letter_carries_the_character_a_text_field_inserts() {
        assert_eq!(
            logical_key(KeyCode::KeyP, false),
            (Key::Character("p".into()), Some("p".to_string()))
        );
        assert_eq!(
            logical_key(KeyCode::KeyP, true),
            (Key::Character("P".into()), Some("P".to_string()))
        );
        assert_eq!(logical_key(KeyCode::Escape, false).0, Key::Escape);
        assert_eq!(logical_key(KeyCode::Delete, false).0, Key::Delete);
        assert_eq!(logical_key(KeyCode::ArrowUp, false).0, Key::ArrowUp);
        assert_eq!(logical_key(KeyCode::Space, false).1, Some(" ".to_string()));
    }

    /// Every key that types a character is reachable from that
    /// character, so `input.text` presses the `KeyCode` the editor's own
    /// chords read rather than an unidentified one.
    #[test]
    fn every_typing_key_round_trips_through_its_character() {
        for key in ALL_TYPING_KEYS {
            let character = character_for(*key).expect("a typing key produces a character");
            assert_eq!(key_code_for(character), *key, "{key:?}");
        }
        assert_eq!(key_code_for(' '), KeyCode::Space);
        assert_eq!(key_code_for('P'), KeyCode::KeyP);
    }

    /// Modifiers are pressed a beat ahead of what they modify, because
    /// `ButtonInput<KeyCode>` is written after the picking input pass
    /// that a same-frame pointer event goes through.
    #[test]
    fn modifiers_bracket_the_gesture_in_beats_of_their_own() {
        let gesture = vec![Beat {
            events: vec![Emit::Cursor(Vec2::ZERO)],
            frames: 1,
        }];
        let held = with_mods(&[KeyCode::ShiftLeft], 1, gesture.clone());
        assert_eq!(held.len(), 3);
        assert!(matches!(
            held[0].events.as_slice(),
            [Emit::Key {
                state: ButtonState::Pressed,
                ..
            }]
        ));
        assert!(matches!(held[1].events.as_slice(), [Emit::Cursor(_)]));
        assert!(matches!(
            held[2].events.as_slice(),
            [Emit::Key {
                state: ButtonState::Released,
                ..
            }]
        ));
        assert_eq!(with_mods(&[], 1, gesture).len(), 1);
    }

    #[test]
    fn a_count_parameter_is_clamped_to_something_a_session_can_finish() {
        let mut params = OperatorParameters::default();
        params.insert(
            "steps".to_string(),
            jackdaw_scene_types::PropertyValue::Int(100_000),
        );
        assert_eq!(
            bounded(&params, "steps", DEFAULT_DRAG_STEPS, MAX_STEPS),
            512
        );
        params.insert(
            "steps".to_string(),
            jackdaw_scene_types::PropertyValue::Int(-4),
        );
        assert_eq!(bounded(&params, "steps", DEFAULT_DRAG_STEPS, MAX_STEPS), 0);
        assert_eq!(
            bounded(
                &OperatorParameters::default(),
                "steps",
                DEFAULT_DRAG_STEPS,
                MAX_STEPS
            ),
            8
        );
    }
}
