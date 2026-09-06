//! Opaque reflected values on the write and read paths.
//!
//! A type bevy reflects as opaque has no structure for the BSN walk to descend
//! into, so it lands in the `Debug` fallback: stored as a quoted string that
//! nothing reads back. `SmolStr` is one, and
//! `bevy_feathers::theme::ThemeToken` wraps one, so a token that does not
//! round-trip is a widget that reloads unstyled.
//!
//! The failure mode is silent: the write path succeeds, the read path declines,
//! and the only symptom is a colour reverting on load. These tests turn that
//! into a test failure, including for a `smol_str` version whose reflect impl
//! changes shape.

use bevy::ecs::component::Component;
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::ecs::world::World;
use bevy::reflect::{Reflect, TypeRegistry};

use jackdaw_bsn::{BsnPatch, BsnValue, apply_component_patch, component_to_bsn_patch};
use smol_str::SmolStr;

/// The shape of `ThemeToken`: a tuple struct whose only field is a `SmolStr`.
/// Neither `Default` nor `Clone` type data is registered, matching
/// `bevy_feathers`' `#[reflect(Component, Clone)]`, so the applier takes its
/// no-default path.
#[derive(Component, Reflect, PartialEq, Debug)]
#[reflect(Component)]
struct Token(SmolStr);

#[test]
fn a_smol_str_payload_emits_as_a_plain_string() {
    let registry = TypeRegistry::new();
    let patch = component_to_bsn_patch(&Token(SmolStr::new("feathers.button.bg")), &registry);

    let BsnPatch::TupleStruct(data) = &patch else {
        panic!("a tuple struct emits as a tuple-struct patch, got {patch:?}");
    };
    assert_eq!(
        data.values.as_slice(),
        [BsnValue::String("feathers.button.bg".to_string())],
        "the token emits as its text, not as its Debug form",
    );
}

#[test]
fn a_smol_str_payload_reads_back_as_the_same_token() {
    let mut world = World::new();
    world.init_resource::<AppTypeRegistry>();
    world
        .resource::<AppTypeRegistry>()
        .write()
        .register::<Token>();

    let patch = {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        component_to_bsn_patch(&Token(SmolStr::new("feathers.button.bg")), &registry)
    };

    let entity = world.spawn_empty().id();
    apply_component_patch(&mut world, entity, &patch);

    assert_eq!(
        world.get::<Token>(entity),
        Some(&Token(SmolStr::new("feathers.button.bg"))),
        "the token survived the round trip as a token, not as a quoted Debug string",
    );
}

/// A component the applier can neither default nor rebuild. Without the guard
/// it reaches `ReflectComponent::insert` with a value bevy cannot make
/// concrete, which takes the editor down mid-load.
#[derive(Component, Reflect)]
#[reflect(Component, from_reflect = false)]
struct Unbuildable(SmolStr);

#[test]
fn a_component_that_cannot_be_rebuilt_is_skipped_not_fatal() {
    let mut world = World::new();
    world.init_resource::<AppTypeRegistry>();
    world
        .resource::<AppTypeRegistry>()
        .write()
        .register::<Unbuildable>();

    let patch = {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        component_to_bsn_patch(&Unbuildable(SmolStr::new("x")), &registry)
    };

    let entity = world.spawn_empty().id();
    apply_component_patch(&mut world, entity, &patch);

    assert!(
        world.get::<Unbuildable>(entity).is_none(),
        "a component with no default and no FromReflect is declined, not forced",
    );
}
