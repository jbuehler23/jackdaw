//! Connect a widget to game state without writing a system for it.
//!
//! A binding names two places: where to read, and where to write. Bindings are
//! authored on the widget itself, as a [`Bindings`] component holding a list of
//! [`Binding`]s, and they travel in the scene document with everything else
//! about that widget. A game adds [`JackdawBindPlugin`] and the scene arrives
//! already wired.
//!
//! # Vocabulary
//!
//! - **Path** ([`BindPath`]): where a binding reads or writes.
//!   `Health.current` names a field of a component on the context entity;
//!   `Res(AudioSettings).master` names a field of a resource and needs no
//!   context. The type half takes a full type path, or a short name as long
//!   as one registered type answers to it. The field half is a reflect path,
//!   so `Node.margin.left` reaches as deep as it reads.
//! - **Context** ([`BindContext`]): the entity component paths read from.
//!   A widget without one inherits the nearest above it, so a panel names its
//!   subject once and every widget inside it reads from there.
//! - **Via**: a registered function the reads pass through on the way in.
//!   Two health paths go in, one ratio comes out.
//! - **Kinds** ([`Binding`]): `Field` drives one of the widget's own
//!   fields, `Text` fills in a sentence, `Visible` shows or hides it, `Value`
//!   keeps a slider, checkbox or text input in step (both ways when asked
//!   for), and `Action` sends an event when the widget is activated.
//!
//! # Giving a loaded screen its subject
//!
//! `jackdaw_runtime` keeps a loaded scene's roots as ECS roots, where bevy's UI
//! layout needs them, so they are not children of the entity the load hands
//! back, and a [`BindContext`] inserted on that entity is above nothing. Mark
//! the document's root with a component of the game's own and insert the
//! context on the entity carrying that marker once the scene has spawned.
//!
//! # What a `Value` binding keeps in step
//!
//! Which component takes the value is decided by the value: a number goes to
//! `SliderValue`, a bool to `Checked`, a string to the widget's text. The
//! first two are bevy's own and named here directly; text is not, so whoever
//! supplies one says where it lives through [`ValueTextTarget`]. With none
//! named, a string binding has nowhere to land and says so every frame until
//! one is. The lookup is part of resolving, so a target installed late is
//! picked up the next time the binding resolves. A string binding on a widget
//! with no such field, or a number on a widget that has one, is refused by
//! name rather than written somewhere else.
//!
//! `two_way` decides only the direction back: an edit arrives as
//! `bevy_ui_widgets`' `ValueChange<T>` and the observers here write it to the
//! source. A one-way `Value` binding on a text input puts a keystroke back the
//! next frame, so an editable field wants `two_way: true`.
//!
//! # Writing a marker
//!
//! A `Field` binding whose write path names a component and no field
//! (`InteractionDisabled` rather than `Node.width`) takes a bool: true puts
//! the component on, false takes it off. Reflection has to be able to build
//! it, which for a marker means `#[reflect(Component, Default)]`. The equality
//! guard is on presence rather than value, so a marker already on the widget
//! is left alone. Otherwise it is an ordinary `Field`: same reads, same `via`,
//! same change-tick gate.
//!
//! # What the evaluator does
//!
//! [`evaluate_bindings`] runs once a frame in `PostUpdate`, ahead of UI
//! layout. Every binding reads its sources, works out its value, and writes
//! only when that value differs from the one already stored. The write goes
//! through `bypass_change_detection` and flags the component by hand, so a
//! binding that lands on the value already there leaves no change tick behind:
//! code waiting on `Changed` to learn that a binding fired waits forever.
//!
//! The evaluator also watches the change ticks of what a binding reads and
//! skips the ones whose sources have not moved, so a target overwritten from
//! outside is not put back until that binding's own source moves again.
//! `Value` and `Text` bindings are exempt and run every frame: a click or a
//! keystroke moves the widget without moving the source.
//!
//! What is due is decided before anything is written, so a chain of bindings,
//! one reading what another writes, advances one link per frame. Only the
//! tightest cycle is refused: a binding whose write lands on exactly what one
//! of its own reads takes is [`BindError::SelfCycle`] at resolve time. A
//! longer way round is not detected and advances a link a frame like any other
//! chain; the equality guard is what ends it.
//!
//! # How the lookups are kept
//!
//! Splitting a path, finding its type and finding the entity the context names
//! happens once, when the bindings are authored and whenever the scene moves
//! under them, and lands in a [`ResolvedBindings`] component beside the
//! [`Bindings`] it came from. A frame then costs one read and one guarded write
//! per binding.
//!
//! A lookup goes stale on anything that changes what a path points at:
//! bindings inserted or edited, a [`BindContext`] appearing above the widget
//! or being taken away, the widget moving to a new parent or out of the tree,
//! and bindings removed, which drops the lookups with them. Insertions and
//! edits are `Changed` state; removals are not, so they are read from their
//! own queues.
//!
//! A binding that fails is retried at whichever half failed, and the two
//! halves retry at different rates: evaluating costs a read, so it happens
//! every frame and a target that arrives late is picked up on the frame after;
//! re-resolving clones the authored list and takes the type registry lock, so
//! it happens on a cadence of a few dozen frames. A binding that starts
//! working leaves the ledger, so a second failure is reported again.
//!
//! [`ResolvedBindings`] is derived state, never authored state, and holds
//! entity ids, component ids and reflect handles that mean nothing outside the
//! world that produced them, so it is not a reflected type.
//!
//! # What an action binding can send
//!
//! An `Action` binding fills the event's fields from the paths it names and
//! sends it. One field it does not name: an `EntityEvent`'s target, which
//! takes the widget's [`BindContext`].
//!
//! That works for a target field named `entity`, and only that. bevy's
//! `EntityEvent` derive also accepts a field of any name carrying
//! `#[event_target]`, but the attribute leaves nothing behind in the type
//! registry and bevy 0.19 has no `ReflectEntityEvent` to ask at runtime, so
//! reflection cannot tell which field such an event targets; it is refused
//! with [`BindError::UnfillableEntityField`]. That rules out
//! `bevy_ui_widgets`' `ValueChange` and `MenuEvent`, which both name their
//! target field `source`; `Activate` binds. A game that wants a widget to
//! raise something else declares its own event with an `entity` field.
//!
//! The event also has to be a struct with named fields, or it is refused with
//! [`BindError::EventNotNamedStruct`] at resolve time. A widget with no
//! context above it is refused with [`BindError::MissingContext`] rather than
//! sending `Entity::PLACEHOLDER`.
//!
//! # When a binding is wrong
//!
//! Nothing here panics over an authored mistake. A path that names no type, a
//! context that was despawned, a `via` function that hands back a borrow --
//! each is a [`BindError`], warned once per widget and binding through
//! [`BindFailures`] and then skipped. Every other binding on the widget still
//! runs.
//!
//! The log is the only place a runtime failure shows up. The editor badges a
//! row it can see is wrong from the type registry and the project schema, but
//! does not read [`BindFailures`], so a binding that only fails once the game
//! is running says so in the log and not on the row.

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
/// Order against this rather than the function. [`evaluate_bindings`] is a
/// plain `pub fn` a host registers itself, the editor doing so behind its
/// preview toggle, so one app can hold more than one of it, and bevy refuses to
/// order against a `SystemTypeSet` with more than one instance. A set has no
/// such restriction, and both registrations are in it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BindEvaluationSystems;

/// Everything a game needs to run the bindings its scenes were authored
/// with: the types, the once-a-frame evaluator, and the observers that carry
/// a widget's activation and value edits back to game state.
///
/// The editor registers those pieces itself instead of adding this plugin,
/// because bindings there only evaluate while a scene is in preview.
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
