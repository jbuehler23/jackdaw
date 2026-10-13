//! The per-frame evaluation of every action.

use core::time::Duration;

use bevy::input::keyboard::Key;
use bevy::input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel};
use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use crate::{
    ActionEvents, ActionOf, ActionSettings, ActionValue, Actions, Binding, Bindings, Cancel,
    Complete, Condition, ContextActivity, ContextPriority, Fire, InputContext, InputSources,
    ModKeys, ScrollTicks, Start, TriggerState,
};

/// The default gap allowed between the two presses of a double click, in
/// seconds.
pub const DEFAULT_DOUBLE_CLICK_WINDOW: f32 = 0.3;

/// Pixels of a pixel-unit scroll that make one wheel tick.
const PIXELS_PER_TICK: f32 = 100.0;

/// Whether a context was active when last evaluated.
#[derive(Component, Debug, Default)]
pub(crate) struct ContextRuntime {
    was_active: bool,
}

/// What a binding remembers between frames.
#[derive(Component, Debug)]
pub(crate) struct BindingState {
    /// Whether the binding's input counted last frame.
    active: bool,
    /// Waiting for the input to be released before it may count.
    needs_reset: bool,
    held_since: Option<Duration>,
    first_press_at: Option<Duration>,
}

impl Default for BindingState {
    fn default() -> Self {
        Self {
            active: false,
            needs_reset: true,
            held_since: None,
            first_press_at: None,
        }
    }
}

impl BindingState {
    fn clear(&mut self) {
        self.active = false;
        self.held_since = None;
        self.first_press_at = None;
    }
}

/// What a binding hides from later actions when its action consumes input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Trigger {
    Key(KeyCode),
    /// A letter matched by the character the layout types.
    Letter(char),
    Mouse(MouseButton),
    Wheel,
    Motion,
}

/// One binding's input this frame, before any other binding is considered.
#[derive(Debug, Clone, Copy)]
struct Reading {
    trigger: Trigger,
    held: bool,
    just_pressed: bool,
    just_released: bool,
    value: Vec2,
}

/// The input this frame, read once.
struct Devices<'a> {
    keys: Option<&'a ButtonInput<KeyCode>>,
    logical: Option<&'a ButtonInput<Key>>,
    mouse: Option<&'a ButtonInput<MouseButton>>,
    motion: Vec2,
    scroll: Vec2,
    sources: InputSources,
    /// Whether the keyboard is typing Latin letters, so letter shortcuts can
    /// follow the layout. Otherwise they fall back to the physical key.
    latin_layout: bool,
}

impl Devices<'_> {
    fn mods_held(&self, mod_keys: ModKeys) -> bool {
        if mod_keys.is_empty() {
            return true;
        }
        if !self.sources.keyboard {
            return false;
        }
        let Some(keys) = self.keys else {
            return false;
        };
        mod_keys.keys().all(|pair| keys.any_pressed(pair))
    }

    fn read(&self, binding: Binding) -> Reading {
        let idle = |trigger| Reading {
            trigger,
            held: false,
            just_pressed: false,
            just_released: false,
            value: Vec2::ZERO,
        };
        match binding {
            Binding::Keyboard { key, mod_keys } => {
                if let Some(letter) = layout_letter(key, mod_keys)
                    && self.latin_layout
                    && let Some(logical) = self.logical
                {
                    let matches = |pressed: &Key| {
                        matches!(pressed, Key::Character(text)
                            if text.chars().count() == 1
                                && text.chars().all(|c| c.eq_ignore_ascii_case(&letter)))
                    };
                    let trigger = Trigger::Letter(letter);
                    if !self.sources.keyboard {
                        return idle(trigger);
                    }
                    let held = logical.get_pressed().any(matches);
                    return Reading {
                        trigger,
                        held,
                        just_pressed: logical.get_just_pressed().any(matches),
                        just_released: logical.get_just_released().any(matches),
                        value: if held { Vec2::X } else { Vec2::ZERO },
                    };
                }
                let trigger = Trigger::Key(key);
                let Some(keys) = self.keys.filter(|_| self.sources.keyboard) else {
                    return idle(trigger);
                };
                let held = keys.pressed(key);
                Reading {
                    trigger,
                    held,
                    just_pressed: keys.just_pressed(key),
                    just_released: keys.just_released(key),
                    value: if held { Vec2::X } else { Vec2::ZERO },
                }
            }
            Binding::MouseButton { button, .. } => {
                let trigger = Trigger::Mouse(button);
                let Some(mouse) = self.mouse.filter(|_| self.sources.mouse) else {
                    return idle(trigger);
                };
                let held = mouse.pressed(button);
                Reading {
                    trigger,
                    held,
                    just_pressed: mouse.just_pressed(button),
                    just_released: mouse.just_released(button),
                    value: if held { Vec2::X } else { Vec2::ZERO },
                }
            }
            Binding::MouseWheel { .. } => {
                let held = self.sources.mouse && self.scroll != Vec2::ZERO;
                Reading {
                    trigger: Trigger::Wheel,
                    held,
                    just_pressed: held,
                    just_released: false,
                    value: if held { self.scroll } else { Vec2::ZERO },
                }
            }
            Binding::MouseMotion { .. } => {
                let held = self.sources.mouse && self.motion != Vec2::ZERO;
                Reading {
                    trigger: Trigger::Motion,
                    held,
                    just_pressed: held,
                    just_released: false,
                    value: if held { self.motion } else { Vec2::ZERO },
                }
            }
        }
    }
}

/// The letter a shortcut on `key` matches by layout: a letter key chorded
/// with Ctrl or Super and not Alt, which on some layouts types another
/// character.
fn layout_letter(key: KeyCode, mod_keys: ModKeys) -> Option<char> {
    if !mod_keys.intersects(ModKeys::CONTROL | ModKeys::SUPER) || mod_keys.contains(ModKeys::ALT) {
        return None;
    }
    let letter = match key {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        _ => return None,
    };
    Some(letter)
}

/// Where one binding's evaluation leaves it this frame.
struct Evaluated {
    state: TriggerState,
    value: Vec2,
}

/// Apply `condition` to a binding whose input counts this frame or not.
fn apply_condition(
    condition: Condition,
    counts: bool,
    reading: &Reading,
    state: &mut BindingState,
    now: Duration,
) -> TriggerState {
    let pressed = counts && reading.just_pressed;
    let released = state.active && !counts && reading.just_released;
    let result = match condition {
        Condition::Down => {
            if counts {
                TriggerState::Fired
            } else {
                TriggerState::None
            }
        }
        Condition::Press => {
            if pressed {
                TriggerState::Fired
            } else {
                TriggerState::None
            }
        }
        Condition::Release => {
            if counts {
                TriggerState::Ongoing
            } else if released {
                TriggerState::Fired
            } else {
                TriggerState::None
            }
        }
        Condition::Tap { max_secs } => {
            let limit = Duration::from_secs_f32(max_secs);
            if counts {
                let since = *state.held_since.get_or_insert(now);
                if now.saturating_sub(since) <= limit {
                    TriggerState::Ongoing
                } else {
                    TriggerState::None
                }
            } else {
                let tapped = released
                    && state
                        .held_since
                        .is_some_and(|since| now.saturating_sub(since) <= limit);
                state.held_since = None;
                if tapped {
                    TriggerState::Fired
                } else {
                    TriggerState::None
                }
            }
        }
        Condition::DoubleClick { window_secs } => {
            let window = Duration::from_secs_f32(window_secs);
            match state.first_press_at {
                None if pressed => {
                    state.first_press_at = Some(now);
                    TriggerState::Ongoing
                }
                None => TriggerState::None,
                Some(first) if pressed => {
                    if now.saturating_sub(first) <= window {
                        state.first_press_at = None;
                        TriggerState::Fired
                    } else {
                        state.first_press_at = Some(now);
                        TriggerState::Ongoing
                    }
                }
                Some(first) => {
                    if now.saturating_sub(first) > window {
                        state.first_press_at = None;
                        TriggerState::None
                    } else {
                        TriggerState::Ongoing
                    }
                }
            }
        }
        Condition::ScrollTick { up } => {
            let y = reading.value.y;
            if counts && (if up { y > 0.0 } else { y < 0.0 }) {
                TriggerState::Fired
            } else {
                TriggerState::None
            }
        }
    };
    state.active = counts;
    result
}

/// Normalized scroll of this frame's wheel messages.
fn scroll_ticks(wheel: &mut MessageReader<MouseWheel>) -> Vec2 {
    wheel
        .read()
        .map(|event| {
            let delta = Vec2::new(event.x, event.y);
            match event.unit {
                MouseScrollUnit::Line => delta,
                MouseScrollUnit::Pixel => delta / PIXELS_PER_TICK,
            }
        })
        .sum()
}

type ContextQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static ContextPriority,
        &'static ContextActivity,
        &'static mut ContextRuntime,
        Option<&'static Actions>,
    ),
    With<InputContext>,
>;

type ActionQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static ActionSettings,
        &'static mut TriggerState,
        &'static mut ActionEvents,
        &'static mut ActionValue,
        Option<&'static Bindings>,
    ),
    With<ActionOf>,
>;

type BindingQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Binding,
        &'static Condition,
        &'static mut BindingState,
    ),
>;

/// Evaluate every action of every context, highest priority first.
#[expect(
    clippy::too_many_arguments,
    reason = "a system reading each input device"
)]
pub(crate) fn evaluate(
    mut commands: Commands,
    time: Option<Res<Time<Real>>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    logical: Option<Res<ButtonInput<Key>>>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    motion: Option<Res<AccumulatedMouseMotion>>,
    mut wheel: MessageReader<MouseWheel>,
    mut scroll: ResMut<ScrollTicks>,
    sources: Res<InputSources>,
    mut contexts: ContextQuery,
    mut actions: ActionQuery,
    mut bindings: BindingQuery,
) {
    scroll.0 = scroll_ticks(&mut wheel);
    let now = time.map(|time| time.elapsed()).unwrap_or_default();
    let latin_layout = logical.as_deref().is_some_and(|logical| {
        logical
            .get_pressed()
            .chain(logical.get_just_released())
            .any(|key| matches!(key, Key::Character(text) if text.chars().all(|c| c.is_ascii_alphabetic())))
    });
    let devices = Devices {
        keys: keys.as_deref(),
        logical: logical.as_deref(),
        mouse: mouse.as_deref(),
        motion: motion.map(|motion| motion.delta).unwrap_or_default(),
        scroll: scroll.0,
        sources: *sources,
        latin_layout,
    };

    let mut order: Vec<(Entity, i32, bool)> = Vec::new();
    for (context, priority, activity, mut runtime, context_actions) in &mut contexts {
        let active = activity.0;
        let became_active = active && !runtime.was_active;
        runtime.was_active = active;
        order.push((context, priority.0, active));
        let Some(context_actions) = context_actions else {
            continue;
        };
        for action in context_actions.iter() {
            let Ok((settings, _, _, _, action_bindings)) = actions.get(action) else {
                continue;
            };
            let require_reset = settings.require_reset;
            for binding in action_bindings.into_iter().flat_map(Bindings::iter) {
                if let Ok((_, _, _, mut state)) = bindings.get_mut(binding) {
                    if !active {
                        state.clear();
                    } else if became_active && require_reset {
                        state.needs_reset = true;
                    }
                }
            }
        }
    }
    order.sort_by_key(|&(_, priority, _)| core::cmp::Reverse(priority));

    // Every binding of an active context whose input and modifiers are held,
    // so a longer chord on the same input can silence a shorter one.
    let mut held: Vec<(Entity, Trigger, ModKeys)> = Vec::new();
    for &(context, _, active) in &order {
        if !active {
            continue;
        }
        let Ok((.., Some(context_actions))) = contexts.get(context) else {
            continue;
        };
        for action in context_actions.iter() {
            let Ok((.., Some(action_bindings))) = actions.get(action) else {
                continue;
            };
            for binding in action_bindings.iter() {
                let Ok((_, &input, ..)) = bindings.get(binding) else {
                    continue;
                };
                let reading = devices.read(input);
                if reading.held && devices.mods_held(input.mod_keys()) {
                    held.push((binding, reading.trigger, input.mod_keys()));
                }
            }
        }
    }
    let outranked = |binding: Entity, trigger: Trigger, mod_keys: ModKeys| {
        held.iter().any(|&(other, other_trigger, other_mods)| {
            other != binding
                && other_trigger == trigger
                && other_mods != mod_keys
                && other_mods.contains(mod_keys)
        })
    };

    let mut consumed: HashSet<Trigger> = HashSet::default();
    for &(context, _, active) in &order {
        let Ok((.., Some(context_actions))) = contexts.get(context) else {
            continue;
        };
        let context_actions: Vec<Entity> = context_actions.iter().collect();
        for action in context_actions {
            let Ok((settings, mut trigger_state, mut events, mut value, action_bindings)) =
                actions.get_mut(action)
            else {
                continue;
            };
            let settings = *settings;
            let mut result = Evaluated {
                state: TriggerState::None,
                value: Vec2::ZERO,
            };
            let mut contributing: Vec<Trigger> = Vec::new();
            if active {
                for binding in action_bindings.into_iter().flat_map(Bindings::iter) {
                    let Ok((_, &input, &condition, mut state)) = bindings.get_mut(binding) else {
                        continue;
                    };
                    let reading = devices.read(input);
                    let mut counts = reading.held
                        && devices.mods_held(input.mod_keys())
                        && !consumed.contains(&reading.trigger)
                        && !outranked(binding, reading.trigger, input.mod_keys());
                    if settings.require_reset && state.needs_reset {
                        if counts {
                            consumed.insert(reading.trigger);
                            counts = false;
                        } else {
                            state.needs_reset = false;
                        }
                    }
                    let binding_state =
                        apply_condition(condition, counts, &reading, &mut state, now);
                    if binding_state == TriggerState::None {
                        continue;
                    }
                    match binding_state.cmp(&result.state) {
                        core::cmp::Ordering::Less => {}
                        core::cmp::Ordering::Equal => {
                            result.value += reading.value;
                            contributing.push(reading.trigger);
                        }
                        core::cmp::Ordering::Greater => {
                            result = Evaluated {
                                state: binding_state,
                                value: reading.value,
                            };
                            contributing.clear();
                            contributing.push(reading.trigger);
                        }
                    }
                }
            }
            if settings.consume_input && result.state != TriggerState::None {
                consumed.extend(contributing);
            }

            let previous = *trigger_state;
            let new_events = ActionEvents::new(previous, result.state);
            events.set_if_neq(new_events);
            trigger_state.set_if_neq(result.state);
            value.set_if_neq(ActionValue(result.value));

            let action_value = ActionValue(result.value);
            if new_events.contains(ActionEvents::START) {
                commands.trigger(Start {
                    action,
                    context,
                    value: action_value,
                });
            }
            if new_events.contains(ActionEvents::FIRE) {
                commands.trigger(Fire {
                    action,
                    context,
                    value: action_value,
                });
            }
            if new_events.contains(ActionEvents::COMPLETE) {
                commands.trigger(Complete {
                    action,
                    context,
                    value: action_value,
                });
            }
            if new_events.contains(ActionEvents::CANCEL) {
                commands.trigger(Cancel {
                    action,
                    context,
                    value: action_value,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::input::touch::TouchPhase;

    use super::*;
    use crate::{
        ActionOf, BindingOf, ContextActivity, ContextPriority, InputActionPlugin, InputContext,
        InputModKeys,
    };

    #[derive(Resource, Default)]
    struct Fired(Vec<Entity>);

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, InputActionPlugin))
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<Key>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<AccumulatedMouseMotion>()
            .add_message::<MouseWheel>()
            .init_resource::<Fired>()
            .add_observer(|fire: On<Fire>, mut fired: ResMut<Fired>| {
                fired.0.push(fire.action);
            });
        app
    }

    fn context(app: &mut App, priority: i32) -> Entity {
        app.world_mut()
            .spawn((InputContext, ContextPriority(priority)))
            .id()
    }

    fn action(app: &mut App, context: Entity, settings: ActionSettings) -> Entity {
        app.world_mut().spawn((ActionOf(context), settings)).id()
    }

    fn bind(app: &mut App, action: Entity, binding: impl Into<Binding>, condition: Condition) {
        app.world_mut()
            .spawn((binding.into(), condition, BindingOf(action)));
    }

    fn press(app: &mut App, key: KeyCode) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
    }

    fn release(app: &mut App, key: KeyCode) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(key);
    }

    /// Run a frame, returning the actions that fired in it.
    fn frame(app: &mut App) -> Vec<Entity> {
        app.update();
        let fired = core::mem::take(&mut app.world_mut().resource_mut::<Fired>().0);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut().resource_mut::<ButtonInput<Key>>().clear();
        fired
    }

    #[test]
    fn a_press_fires_once_per_press() {
        let mut app = app();
        let ctx = context(&mut app, 0);
        let undo = action(&mut app, ctx, ActionSettings::default());
        bind(&mut app, undo, KeyCode::KeyU, Condition::Press);
        frame(&mut app);

        press(&mut app, KeyCode::KeyU);
        assert_eq!(frame(&mut app), [undo]);
        assert!(frame(&mut app).is_empty(), "holding does not fire again");
        release(&mut app, KeyCode::KeyU);
        frame(&mut app);
        press(&mut app, KeyCode::KeyU);
        assert_eq!(frame(&mut app), [undo]);
    }

    #[test]
    fn the_longer_chord_on_a_key_silences_the_shorter() {
        let mut app = app();
        let ctx = context(&mut app, 0);
        let save = action(&mut app, ctx, ActionSettings::default());
        let scale = action(&mut app, ctx, ActionSettings::default());
        bind(
            &mut app,
            save,
            KeyCode::KeyS.with_mod_keys(ModKeys::CONTROL),
            Condition::Press,
        );
        bind(&mut app, scale, KeyCode::KeyS, Condition::Press);
        frame(&mut app);

        press(&mut app, KeyCode::ControlLeft);
        press(&mut app, KeyCode::KeyS);
        assert_eq!(frame(&mut app), [save]);
        release(&mut app, KeyCode::KeyS);
        release(&mut app, KeyCode::ControlLeft);
        frame(&mut app);

        press(&mut app, KeyCode::KeyS);
        assert_eq!(frame(&mut app), [scale]);
    }

    #[test]
    fn a_consuming_context_hides_its_keys_from_lower_ones() {
        let mut app = app();
        let low = context(&mut app, 0);
        let high = context(&mut app, 10);
        let cancel_selection = action(&mut app, low, ActionSettings::default());
        let cancel_modal = action(
            &mut app,
            high,
            ActionSettings {
                consume_input: true,
                ..default()
            },
        );
        bind(
            &mut app,
            cancel_selection,
            KeyCode::Escape,
            Condition::Press,
        );
        bind(&mut app, cancel_modal, KeyCode::Escape, Condition::Press);
        frame(&mut app);

        press(&mut app, KeyCode::Escape);
        assert_eq!(frame(&mut app), [cancel_modal]);
        release(&mut app, KeyCode::Escape);
        frame(&mut app);

        app.world_mut()
            .entity_mut(high)
            .insert(ContextActivity::INACTIVE);
        press(&mut app, KeyCode::Escape);
        assert_eq!(frame(&mut app), [cancel_selection]);
    }

    #[test]
    fn a_reset_ignores_a_key_held_when_the_context_activates() {
        let mut app = app();
        let modal = context(&mut app, 10);
        app.world_mut()
            .entity_mut(modal)
            .insert(ContextActivity::INACTIVE);
        let axis = action(
            &mut app,
            modal,
            ActionSettings {
                require_reset: true,
                ..default()
            },
        );
        bind(&mut app, axis, KeyCode::KeyX, Condition::Down);
        frame(&mut app);

        press(&mut app, KeyCode::KeyX);
        frame(&mut app);
        app.world_mut()
            .entity_mut(modal)
            .insert(ContextActivity::ACTIVE);
        assert!(frame(&mut app).is_empty(), "X was held before activation");
        release(&mut app, KeyCode::KeyX);
        frame(&mut app);
        press(&mut app, KeyCode::KeyX);
        assert_eq!(frame(&mut app), [axis]);
    }

    #[test]
    fn a_turned_off_keyboard_reads_nothing() {
        let mut app = app();
        let ctx = context(&mut app, 0);
        let tool = action(&mut app, ctx, ActionSettings::default());
        bind(&mut app, tool, KeyCode::KeyQ, Condition::Press);
        frame(&mut app);

        app.world_mut().resource_mut::<InputSources>().keyboard = false;
        press(&mut app, KeyCode::KeyQ);
        assert!(frame(&mut app).is_empty());
    }

    #[test]
    fn a_release_fires_when_the_key_comes_up() {
        let mut app = app();
        let ctx = context(&mut app, 0);
        let deselect = action(&mut app, ctx, ActionSettings::default());
        bind(&mut app, deselect, KeyCode::Escape, Condition::Release);
        frame(&mut app);

        press(&mut app, KeyCode::Escape);
        assert!(frame(&mut app).is_empty());
        release(&mut app, KeyCode::Escape);
        assert_eq!(frame(&mut app), [deselect]);
    }

    #[test]
    fn a_letter_shortcut_follows_the_layout() {
        let mut app = app();
        let ctx = context(&mut app, 0);
        let undo = action(&mut app, ctx, ActionSettings::default());
        bind(
            &mut app,
            undo,
            KeyCode::KeyZ.with_mod_keys(ModKeys::CONTROL),
            Condition::Press,
        );
        frame(&mut app);

        // On AZERTY the key in the QWERTY W position types z.
        press(&mut app, KeyCode::ControlLeft);
        press(&mut app, KeyCode::KeyW);
        app.world_mut()
            .resource_mut::<ButtonInput<Key>>()
            .press(Key::Character("z".into()));
        assert_eq!(frame(&mut app), [undo]);
        app.world_mut()
            .resource_mut::<ButtonInput<Key>>()
            .reset_all();
        release(&mut app, KeyCode::KeyW);
        frame(&mut app);

        // And the key in the QWERTY Z position types w, which is not undo.
        press(&mut app, KeyCode::KeyZ);
        app.world_mut()
            .resource_mut::<ButtonInput<Key>>()
            .press(Key::Character("w".into()));
        assert!(frame(&mut app).is_empty());
    }

    #[test]
    fn a_double_click_fires_on_the_second_press_inside_the_window() {
        let mut state = BindingState::default();
        let reading = |pressed| Reading {
            trigger: Trigger::Mouse(MouseButton::Left),
            held: pressed,
            just_pressed: pressed,
            just_released: !pressed,
            value: Vec2::ZERO,
        };
        let condition = Condition::DOUBLE_CLICK;
        let at = Duration::from_millis;
        assert_eq!(
            apply_condition(condition, true, &reading(true), &mut state, at(0)),
            TriggerState::Ongoing
        );
        apply_condition(condition, false, &reading(false), &mut state, at(50));
        assert_eq!(
            apply_condition(condition, true, &reading(true), &mut state, at(100)),
            TriggerState::Fired
        );
        apply_condition(condition, false, &reading(false), &mut state, at(150));
        assert_eq!(
            apply_condition(condition, true, &reading(true), &mut state, at(200)),
            TriggerState::Ongoing,
            "the next press starts a new pair"
        );
        apply_condition(condition, false, &reading(false), &mut state, at(250));
        assert_ne!(
            apply_condition(condition, true, &reading(true), &mut state, at(700)),
            TriggerState::Fired,
            "a second press after the window is a first press"
        );
    }

    #[test]
    fn a_scroll_tick_fires_once_per_frame_in_its_direction() {
        let mut app = app();
        let ctx = context(&mut app, 0);
        let up = action(&mut app, ctx, ActionSettings::default());
        let down = action(&mut app, ctx, ActionSettings::default());
        bind(
            &mut app,
            up,
            Binding::mouse_wheel(),
            Condition::ScrollTick { up: true },
        );
        bind(
            &mut app,
            down,
            Binding::mouse_wheel(),
            Condition::ScrollTick { up: false },
        );
        frame(&mut app);

        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Pixel,
            x: 0.0,
            y: 250.0,
            window: Entity::PLACEHOLDER,
            phase: TouchPhase::Moved,
        });
        assert_eq!(frame(&mut app), [up]);
        assert_eq!(app.world().resource::<ScrollTicks>().0, Vec2::new(0.0, 2.5));
    }
}
