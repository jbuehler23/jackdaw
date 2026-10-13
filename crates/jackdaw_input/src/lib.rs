//! Input actions for the editor, evaluated from `bevy_input` each frame.
//!
//! Contexts, actions and bindings are entities, so anything holding a
//! `World` can add or remove them at runtime:
//!
//! - an entity with [`InputContext`] groups actions and decides, through
//!   [`ContextPriority`] and [`ContextActivity`], whether and in which order
//!   they are evaluated;
//! - an [`Action`] belongs to a context through [`ActionOf`] and carries its
//!   [`TriggerState`], [`ActionEvents`] and [`ActionValue`];
//! - a [`Binding`] belongs to an action through [`BindingOf`] and names the
//!   input, with a [`Condition`] deciding when it fires.
//!
//! When two bindings watch the same key and both are held, the one naming
//! more modifiers wins, so `Ctrl+S` silences a plain `S` without either
//! knowing about the other. Letter shortcuts with Ctrl or Super follow the
//! keyboard layout: `Ctrl+Z` is the key that types `z`.
//!
//! An action whose state changes triggers [`Start`], [`Fire`], [`Complete`]
//! or [`Cancel`] on its entity.

mod evaluate;

use bevy::input::InputSystems;
use bevy::prelude::*;
use bitflags::bitflags;

pub use evaluate::DEFAULT_DOUBLE_CLICK_WINDOW;

/// Everything needed to declare contexts, actions and bindings.
pub mod prelude {
    pub use crate::{
        Action, ActionEvents, ActionOf, ActionSettings, ActionSystems, ActionValue, Actions,
        Binding, BindingOf, Bindings, Cancel, Complete, Condition, ContextActivity,
        ContextPriority, Fire, InputActionPlugin, InputContext, InputModKeys, InputSources,
        ModKeys, ScrollTicks, Start, TriggerState, actions, bindings,
    };
}

/// Evaluates every action each frame in [`PreUpdate`], after Bevy's input
/// systems.
pub struct InputActionPlugin;

impl Plugin for InputActionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InputSources>()
            .init_resource::<ScrollTicks>()
            .configure_sets(PreUpdate, ActionSystems.after(InputSystems))
            .add_systems(PreUpdate, evaluate::evaluate.in_set(ActionSystems));
    }
}

/// The system set that evaluates actions. Read action state after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActionSystems;

/// An entity whose actions are evaluated together.
///
/// A component that names a context requires this one, so the context's
/// entity is found by it whatever the marker.
#[derive(Component, Debug, Default, Clone, Copy)]
#[require(ContextPriority, ContextActivity, evaluate::ContextRuntime)]
pub struct InputContext;

/// Contexts are evaluated from the highest priority down, so an action that
/// consumes its input hides it from contexts below. Equal priorities keep no
/// particular order.
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deref)]
pub struct ContextPriority(pub i32);

/// Whether a context's actions are evaluated. An inactive context's actions
/// read as not triggered, and a context that becomes active again ignores
/// keys already held for actions that require a reset.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Deref)]
pub struct ContextActivity(pub bool);

impl ContextActivity {
    pub const ACTIVE: Self = Self(true);
    pub const INACTIVE: Self = Self(false);
}

impl Default for ContextActivity {
    fn default() -> Self {
        Self::ACTIVE
    }
}

/// An action: something bindings trigger.
#[derive(Component, Debug, Default, Clone, Copy)]
#[require(ActionSettings, TriggerState, ActionEvents, ActionValue)]
pub struct Action;

/// The context an [`Action`] belongs to.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[relationship(relationship_target = Actions)]
#[require(Action)]
pub struct ActionOf(pub Entity);

/// The actions of a context, in evaluation order.
#[derive(Component, Debug, Default)]
#[relationship_target(relationship = ActionOf, linked_spawn)]
pub struct Actions(Vec<Entity>);

/// How an action treats its input.
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ActionSettings {
    /// Ignore a binding whose input is already held when the binding appears
    /// or its context becomes active, until that input is released.
    pub require_reset: bool,
    /// While this action is triggered, the inputs that triggered it are hidden
    /// from every action evaluated after it.
    pub consume_input: bool,
}

/// Where an action stands this frame.
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TriggerState {
    #[default]
    None,
    /// A binding has begun but not yet fired, like a held key of a release
    /// binding.
    Ongoing,
    Fired,
}

bitflags! {
    /// What changed about an action this frame.
    #[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ActionEvents: u8 {
        const START = 1;
        const ONGOING = 1 << 1;
        const FIRE = 1 << 2;
        const CANCEL = 1 << 3;
        const COMPLETE = 1 << 4;
    }
}

impl ActionEvents {
    /// The events of a change from `previous` to `current`.
    pub fn new(previous: TriggerState, current: TriggerState) -> Self {
        use TriggerState as S;
        match (previous, current) {
            (S::None, S::None) => Self::empty(),
            (S::None, S::Ongoing) => Self::START | Self::ONGOING,
            (S::None, S::Fired) => Self::START | Self::FIRE,
            (S::Ongoing, S::None) => Self::CANCEL,
            (S::Ongoing | S::Fired, S::Ongoing) => Self::ONGOING,
            (S::Ongoing | S::Fired, S::Fired) => Self::FIRE,
            (S::Fired, S::None) => Self::COMPLETE,
        }
    }
}

/// The value the triggering bindings read this frame: `1` on X for a button,
/// the scroll ticks for the wheel, the motion delta for the mouse.
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Deref)]
pub struct ActionValue(pub Vec2);

impl ActionValue {
    pub fn as_bool(self) -> bool {
        self.0 != Vec2::ZERO
    }

    pub fn as_axis2d(self) -> Vec2 {
        self.0
    }
}

macro_rules! action_event {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(EntityEvent, Debug, Clone, Copy)]
        pub struct $name {
            /// The action.
            #[event_target]
            pub action: Entity,
            /// The context the action belongs to.
            pub context: Entity,
            pub value: ActionValue,
        }
    };
}

action_event!(
    /// An action left [`TriggerState::None`].
    Start
);
action_event!(
    /// An action is [`TriggerState::Fired`] this frame.
    Fire
);
action_event!(
    /// An action went from fired back to [`TriggerState::None`].
    Complete
);
action_event!(
    /// An action went from ongoing back to [`TriggerState::None`] without firing.
    Cancel
);

bitflags! {
    /// Modifier keys a binding needs held, either side counting.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ModKeys: u8 {
        const CONTROL = 1;
        const SHIFT = 1 << 1;
        const ALT = 1 << 2;
        const SUPER = 1 << 3;
    }
}

impl ModKeys {
    /// The two keys of each modifier.
    pub fn keys(self) -> impl Iterator<Item = [KeyCode; 2]> {
        [
            (Self::CONTROL, [KeyCode::ControlLeft, KeyCode::ControlRight]),
            (Self::SHIFT, [KeyCode::ShiftLeft, KeyCode::ShiftRight]),
            (Self::ALT, [KeyCode::AltLeft, KeyCode::AltRight]),
            (Self::SUPER, [KeyCode::SuperLeft, KeyCode::SuperRight]),
        ]
        .into_iter()
        .filter(move |(flag, _)| self.contains(*flag))
        .map(|(_, keys)| keys)
    }
}

/// The input a binding reads.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[require(Condition, evaluate::BindingState)]
pub enum Binding {
    Keyboard {
        key: KeyCode,
        mod_keys: ModKeys,
    },
    MouseButton {
        button: MouseButton,
        mod_keys: ModKeys,
    },
    /// The scroll wheel, in [`ScrollTicks`].
    MouseWheel {
        mod_keys: ModKeys,
    },
    MouseMotion {
        mod_keys: ModKeys,
    },
}

impl Binding {
    pub fn mouse_wheel() -> Self {
        Self::MouseWheel {
            mod_keys: ModKeys::empty(),
        }
    }

    pub fn mouse_motion() -> Self {
        Self::MouseMotion {
            mod_keys: ModKeys::empty(),
        }
    }

    pub fn mod_keys(self) -> ModKeys {
        match self {
            Self::Keyboard { mod_keys, .. }
            | Self::MouseButton { mod_keys, .. }
            | Self::MouseWheel { mod_keys }
            | Self::MouseMotion { mod_keys } => mod_keys,
        }
    }
}

impl From<KeyCode> for Binding {
    fn from(key: KeyCode) -> Self {
        Self::Keyboard {
            key,
            mod_keys: ModKeys::empty(),
        }
    }
}

impl From<MouseButton> for Binding {
    fn from(button: MouseButton) -> Self {
        Self::MouseButton {
            button,
            mod_keys: ModKeys::empty(),
        }
    }
}

/// Adds modifier keys to an input.
pub trait InputModKeys {
    fn with_mod_keys(self, mod_keys: ModKeys) -> Binding;
}

impl<T: Into<Binding>> InputModKeys for T {
    fn with_mod_keys(self, extra: ModKeys) -> Binding {
        match self.into() {
            Binding::Keyboard { key, mod_keys } => Binding::Keyboard {
                key,
                mod_keys: mod_keys | extra,
            },
            Binding::MouseButton { button, mod_keys } => Binding::MouseButton {
                button,
                mod_keys: mod_keys | extra,
            },
            Binding::MouseWheel { mod_keys } => Binding::MouseWheel {
                mod_keys: mod_keys | extra,
            },
            Binding::MouseMotion { mod_keys } => Binding::MouseMotion {
                mod_keys: mod_keys | extra,
            },
        }
    }
}

/// When a binding fires.
#[derive(Component, Debug, Default, Clone, Copy, PartialEq)]
pub enum Condition {
    /// Every frame the input is held.
    #[default]
    Down,
    /// The frame the input is pressed.
    Press,
    /// The frame the input is released; ongoing while held.
    Release,
    /// Released within `max_secs` of the press; ongoing until then.
    Tap { max_secs: f32 },
    /// The second press within `window_secs` of the first.
    DoubleClick { window_secs: f32 },
    /// One wheel tick up, or down, in a frame; several ticks fire once.
    ScrollTick { up: bool },
}

impl Condition {
    /// A double click within the default window.
    pub const DOUBLE_CLICK: Self = Self::DoubleClick {
        window_secs: DEFAULT_DOUBLE_CLICK_WINDOW,
    };
}

/// The action a [`Binding`] belongs to.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[relationship(relationship_target = Bindings)]
pub struct BindingOf(pub Entity);

/// The bindings of an action.
#[derive(Component, Debug, Default)]
#[relationship_target(relationship = BindingOf, linked_spawn)]
pub struct Bindings(Vec<Entity>);

macro_rules! entity_list {
    ($name:ident) => {
        impl $name {
            pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
                self.0.iter().copied()
            }

            pub fn len(&self) -> usize {
                self.0.len()
            }

            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl<'a> IntoIterator for &'a $name {
            type Item = Entity;
            type IntoIter = core::iter::Copied<core::slice::Iter<'a, Entity>>;

            fn into_iter(self) -> Self::IntoIter {
                self.0.iter().copied()
            }
        }
    };
}

entity_list!(Actions);
entity_list!(Bindings);

/// Which devices bindings may read. Turning the keyboard off also releases
/// every modifier, as when a text field owns the keys.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputSources {
    pub keyboard: bool,
    pub mouse: bool,
}

impl Default for InputSources {
    fn default() -> Self {
        Self {
            keyboard: true,
            mouse: true,
        }
    }
}

/// This frame's scroll in wheel ticks: a line counts one, a hundred pixels
/// count one.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Deref)]
pub struct ScrollTicks(pub Vec2);

/// Something [`bindings!`] can spawn as a binding.
pub trait IntoBindingBundle {
    type Bundle: Bundle;
    fn into_binding_bundle(self) -> Self::Bundle;
}

impl IntoBindingBundle for Binding {
    type Bundle = Binding;
    fn into_binding_bundle(self) -> Binding {
        self
    }
}

impl IntoBindingBundle for KeyCode {
    type Bundle = Binding;
    fn into_binding_bundle(self) -> Binding {
        self.into()
    }
}

impl IntoBindingBundle for MouseButton {
    type Bundle = Binding;
    fn into_binding_bundle(self) -> Binding {
        self.into()
    }
}

impl<B: Into<Binding>> IntoBindingBundle for (B, Condition) {
    type Bundle = (Binding, Condition);
    fn into_binding_bundle(self) -> (Binding, Condition) {
        (self.0.into(), self.1)
    }
}

#[doc(hidden)]
pub mod __private {
    pub use bevy::ecs::spawn::{Spawn, SpawnRelated};
}

/// Spawns bindings for an action: `bindings![KeyCode::KeyM, (MouseButton::Left, Condition::Press)]`.
#[macro_export]
macro_rules! bindings {
    ($($binding:expr),* $(,)?) => {
        <$crate::Bindings as $crate::__private::SpawnRelated>::spawn((
            $($crate::__private::Spawn($crate::IntoBindingBundle::into_binding_bundle($binding)),)*
        ))
    };
}

/// Spawns actions for a context: `actions![(MyAction, bindings![KeyCode::Escape])]`.
#[macro_export]
macro_rules! actions {
    ($($action:expr),* $(,)?) => {
        <$crate::Actions as $crate::__private::SpawnRelated>::spawn((
            $($crate::__private::Spawn($action),)*
        ))
    };
}
