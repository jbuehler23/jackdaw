//! App-extension the game calls to register networked types + its movement rule
//! without importing lightyear directly.
//!
//! The generic bounds mirror lightyear's own bounds verbatim (see the `where`
//! clauses + comments below); they are load-bearing here: loosening them would
//! fail to compile against lightyear's `register_component` /
//! `add_linear_interpolation` / `InputPlugin`.

use bevy::ecs::component::Mutable;
use bevy::ecs::entity::MapEntities;
use bevy::ecs::system::ScheduleSystem;
use bevy::math::curve::Ease;
use bevy::prelude::*;
use bevy::reflect::{FromReflect, Reflectable};
use core::fmt::Debug;
use lightyear::prelude::input::native::{ActionState, InputMarker, InputPlugin};
use lightyear::prelude::{
    AppComponentExt, AppTriggerExt, Controlled, InterpolationRegistrationExt, NetworkDirection,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// `FixedUpdate` set the game's authoritative movement system runs in.
///
/// lightyear applies inputs in `FixedPreUpdate` (client buffers in
/// `InputSystems::BufferClientInputs`, server populates `ActionState` in
/// `InputSystems::UpdateActionState`, both `FixedPreUpdate`) and buffers the
/// resulting replication later (`FixedPostUpdate` / `PostUpdate`). Plain
/// `FixedUpdate` therefore sits naturally AFTER inputs are applied and BEFORE
/// replication is serialized, which is exactly where authoritative movement must
/// run. `MovementSystems` is public so a game can order its own systems relative to it.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub struct MovementSystems;

/// App-extension the game calls to register networked types + its movement rule
/// without importing lightyear directly.
pub trait MultiplayerAppExt {
    /// Register a component for replication with linear interpolation on remote
    /// clients. Call once per networked component type that should be smoothed.
    ///
    /// On a receiving client the replicated entity is marked `Interpolated` and
    /// `C` is smoothed in place (lightyear stores the raw network value as
    /// `Confirmed<C>` and lerps the live `C` between confirmed states); no
    /// separate entity is spawned. The owning server entity must carry an
    /// `InterpolationTarget` (the auto-spawn bundle adds it).
    ///
    /// Bound mirrors `InterpolationRegistrationExt::add_linear_interpolation`,
    /// whose `C: SyncComponent + Ease` reduces (via the blanket
    /// `impl<T: Component<Mutability = Mutable> + Clone + PartialEq> SyncComponent
    /// for T`) to the bound below, plus `register_component`'s
    /// `Component<Mutability: GetWriteFns<C>> + Serialize + DeserializeOwned`
    /// (satisfied because `Mutable: GetWriteFns<C>` when `C: PartialEq`).
    fn replicate_interpolated<C>(&mut self) -> &mut Self
    where
        C: Component<Mutability = Mutable>
            + Clone
            + PartialEq
            + Ease
            + Serialize
            + DeserializeOwned
            + 'static;

    /// Register a component for replication WITHOUT interpolation (e.g. discrete
    /// state). Call once per such networked component type.
    ///
    /// Bound: `register_component` requires
    /// `Component<Mutability: GetWriteFns<C>> + Serialize + DeserializeOwned`,
    /// and `Mutable: GetWriteFns<C>` holds when `C: PartialEq`. `GetWriteFns` is
    /// a lightyear-internal trait that is not re-exported through any public
    /// prelude, so rather than name it we pin `Mutability = Mutable` (the case for
    /// effectively all replicated game state). A game needing to replicate an
    /// immutable component can call lightyear's `register_component` directly.
    fn replicate<C>(&mut self) -> &mut Self
    where
        C: Component<Mutability = Mutable>
            + Clone
            + PartialEq
            + Serialize
            + DeserializeOwned
            + 'static;

    /// Register the player input type (shipped client->server) and install the
    /// client-side input-marker placement.
    ///
    /// Call on BOTH the client and server apps: lightyear's `InputPlugin<A>`
    /// installs the client-send and server-receive wiring behind feature cfgs, so
    /// the same call sets up whichever side the app is. The extra observer this
    /// adds (`place_input_marker`) only fires on the client (it keys on the
    /// `Controlled` marker the receiver adds to the entity it controls).
    ///
    /// Bound copied verbatim from `impl<A: ...> Plugin for InputPlugin<A>` in
    /// `lightyear_inputs_native-0.26.4/src/plugin.rs:19-32`.
    fn register_input<A>(&mut self) -> &mut Self
    where
        A: Serialize
            + DeserializeOwned
            + Clone
            + PartialEq
            + Send
            + Sync
            + Debug
            + Default
            + 'static
            + MapEntities
            + Reflectable
            + FromReflect;

    /// Add the game's authoritative movement system. It runs in `FixedUpdate` in
    /// the public [`MovementSystems`] (naturally after input is applied, before
    /// replication is buffered; see [`MovementSystems`]).
    fn add_movement_system<M, Marker>(&mut self, system: M) -> &mut Self
    where
        M: IntoScheduleConfigs<ScheduleSystem, Marker>;

    /// Register a type usable as an RPC message in both directions. Call once per
    /// message type on BOTH the client and server apps, AFTER adding the turnkey
    /// plugin (it needs the message registry that `ServerPlugins`/`ClientPlugins`
    /// installs; same ordering rule as [`replicate`](Self::replicate)).
    ///
    /// Send with the [`ClientSender`](crate::ClientSender) /
    /// [`ServerSender`](crate::ServerSender) system params; receive by observing
    /// [`ClientMessage<M>`](crate::ClientMessage) (server) or
    /// [`ServerMessage<M>`](crate::ServerMessage) (client). No lightyear types
    /// appear in game code.
    ///
    /// `Clone` is required because the receive plumbing re-emits the payload as the
    /// layer's own event; RPC payloads are small data, so this is cheap.
    fn register_message<M>(&mut self) -> &mut Self
    where
        M: Event + Serialize + DeserializeOwned + Clone + 'static;
}

impl MultiplayerAppExt for App {
    fn replicate_interpolated<C>(&mut self) -> &mut Self
    where
        C: Component<Mutability = Mutable>
            + Clone
            + PartialEq
            + Ease
            + Serialize
            + DeserializeOwned
            + 'static,
    {
        self.register_component::<C>().add_linear_interpolation();
        self
    }

    fn replicate<C>(&mut self) -> &mut Self
    where
        C: Component<Mutability = Mutable>
            + Clone
            + PartialEq
            + Serialize
            + DeserializeOwned
            + 'static,
    {
        self.register_component::<C>();
        self
    }

    fn register_input<A>(&mut self) -> &mut Self
    where
        A: Serialize
            + DeserializeOwned
            + Clone
            + PartialEq
            + Send
            + Sync
            + Debug
            + Default
            + 'static
            + MapEntities
            + Reflectable
            + FromReflect,
    {
        self.add_plugins(InputPlugin::<A>::default());
        self.add_observer(place_input_marker::<A>);
        self
    }

    fn add_movement_system<M, Marker>(&mut self, system: M) -> &mut Self
    where
        M: IntoScheduleConfigs<ScheduleSystem, Marker>,
    {
        self.add_systems(FixedUpdate, system.in_set(MovementSystems));
        self
    }

    fn register_message<M>(&mut self) -> &mut Self
    where
        M: Event + Serialize + DeserializeOwned + Clone + 'static,
    {
        self.register_event::<M>()
            .add_direction(NetworkDirection::Bidirectional);
        self.add_observer(crate::rpc::rewrap_incoming::<M>);
        self
    }
}

/// Client-side observer: when the replication receiver marks an entity
/// `Controlled` (the entity THIS client owns), attach the input marker +
/// `ActionState` so the game can write input into it and lightyear ships it.
///
/// With prediction OFF there is no `Predicted`-spawn observer to do this, so the
/// layer must place the marker explicitly. Generic over `A`, with the same bound
/// as `register_input` / `InputPlugin<A>`.
fn place_input_marker<A>(add: On<Add, Controlled>, mut commands: Commands)
where
    A: Serialize
        + DeserializeOwned
        + Clone
        + PartialEq
        + Send
        + Sync
        + Debug
        + Default
        + 'static
        + MapEntities
        + Reflectable
        + FromReflect,
{
    commands
        .entity(add.entity)
        .insert((InputMarker::<A>::default(), ActionState::<A>::default()));
}
