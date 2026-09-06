//! Connect a widget to game state without writing a system for it.
//!
//! A binding names where to read and where to write. Bindings are authored on
//! the widget as a [`Bindings`] component and travel in the scene document with
//! everything else about it, so a game that adds [`JackdawBindPlugin`] gets a
//! scene that arrives already wired.
//!
//! # Vocabulary
//!
//! - **Path** ([`BindPath`]): where a binding reads or writes.
//!   `Health.current` names a field of a component on the context entity;
//!   `Res(AudioSettings).master` names a field of a resource. The field half is
//!   a reflect path, so `Node.margin.left` reaches as deep as it reads.
//! - **Context** ([`BindContext`]): the entity component paths read from. A
//!   widget without one inherits the nearest above it.
//! - **Via**: a registered function the reads pass through on the way in.
//! - **Kinds** ([`Binding`]): `Field` drives one of the widget's own fields,
//!   `Text` fills in a sentence, `Visible` shows or hides it, `Value` keeps a
//!   slider, checkbox or text input in step, and `Action` sends an event when
//!   the widget is activated.
//!
//! `jackdaw_runtime` keeps a loaded scene's roots as ECS roots, so a
//! [`BindContext`] inserted on the entity the load hands back is above nothing.
//! Mark the document's root with a component of the game's own and insert the
//! context on the entity carrying it once the scene has spawned.
//!
//! # Value and marker writes
//!
//! Which component a `Value` binding writes is decided by the value: a number
//! goes to `SliderValue`, a bool to `Checked`, a string to whatever
//! [`ValueTextTarget`] names. `two_way` decides only the direction back, so an
//! editable text input wants `two_way: true`.
//!
//! A `Field` binding whose write path names a component and no field takes a
//! bool: true puts the component on, false takes it off. Reflection has to be
//! able to build it, which means `#[reflect(Component, Default)]`.
//!
//! # What the evaluator does
//!
//! [`evaluate_bindings`] runs once a frame in `PostUpdate`, ahead of UI layout.
//! It writes only when the computed value differs from the one stored, through
//! `bypass_change_detection` with the component flagged by hand, so nothing can
//! wait on `Changed` to learn that a binding fired.
//!
//! Bindings whose sources have not moved are skipped; `Value` and `Text` are
//! exempt, since a click or a keystroke moves the widget without moving the
//! source. What is due is decided before anything is written, so a chain of
//! bindings advances one link per frame. Only the tightest cycle is refused, as
//! [`BindError::SelfCycle`] at resolve time; a longer way round is ended by the
//! equality guard rather than detected.
//!
//! Path splitting and lookup happen once and land in a [`ResolvedBindings`]
//! component beside the [`Bindings`] they came from, so a frame costs one read
//! and one guarded write per binding. Lookups are redone when bindings change,
//! when a [`BindContext`] appears or goes away, and when the widget moves.
//! Evaluating a failed binding is retried every frame; re-resolving one takes
//! the type registry lock and so happens on a slower cadence.
//!
//! # What an action binding can send
//!
//! An `Action` binding fills the event's fields from the paths it names. An
//! `EntityEvent`'s target is the exception: it takes the widget's
//! [`BindContext`], and only when the field is named `entity`. A
//! `#[event_target]` field of another name leaves nothing in the type registry
//! for reflection to find and is refused with
//! [`BindError::UnfillableEntityField`], which rules out `ValueChange` and
//! `MenuEvent`; `Activate` binds. The event must be a struct with named fields,
//! and a widget with no context above it is refused rather than sent
//! `Entity::PLACEHOLDER`.
//!
//! # When a binding is wrong
//!
//! Nothing here panics over an authored mistake. Each failure is a
//! [`BindError`], warned once per widget and binding through [`BindFailures`]
//! and then skipped, and every other binding on the widget still runs. The log
//! is the only place a runtime failure shows up: the editor badges what it can
//! see from the type registry, but does not read [`BindFailures`].

#![deny(missing_docs)]

mod actions;
mod error;
mod evaluate;
mod resolve;
mod types;

use bevy::prelude::*;

pub use error::BindError;
pub use evaluate::{BindReads, ResolvedBindings, apply_via, evaluate_bindings};
pub use resolve::{
    BindValue, WriteValue, read_path, resolve_context, write_path, write_source_path,
};
pub use types::{
    BindContext, BindFailures, BindPath, Binding, Bindings, ParsedPath, ValueTextTarget,
};

/// Where [`evaluate_bindings`] runs, named so it can be ordered against.
///
/// Order against this rather than against the function: an app can register
/// [`evaluate_bindings`] more than once, and bevy refuses to order against a
/// `SystemTypeSet` with more than one instance.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BindEvaluationSystems;

/// Everything a game needs to run the bindings its scenes were authored with:
/// the types, the once-a-frame evaluator, and the observers that carry a
/// widget's activation and value edits back to game state.
///
/// The editor registers those pieces itself, since bindings there evaluate only
/// while a scene is in preview.
pub struct JackdawBindPlugin;

impl Plugin for JackdawBindPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Bindings>()
            .register_type::<Binding>()
            .register_type::<BindPath>()
            .register_type::<BindContext>()
            .init_resource::<BindFailures>()
            .init_resource::<BindReads>()
            .add_systems(
                PostUpdate,
                evaluate_bindings
                    .in_set(BindEvaluationSystems)
                    .before(bevy::ui::UiSystems::Layout),
            )
            .add_observer(actions::on_activate)
            .add_observer(actions::on_value_change_f32)
            .add_observer(actions::on_value_change_bool)
            .add_observer(actions::on_value_change_string);
    }
}
