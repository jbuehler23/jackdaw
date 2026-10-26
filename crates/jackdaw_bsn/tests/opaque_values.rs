//! Opaque reflected values on the write and read paths.
//!
//! A type bevy reflects as opaque has no structure for the BSN walk to descend
//! into. `SmolStr` is one, and `bevy_feathers::theme::ThemeToken` wraps one,
//! so a token that does not round-trip is a widget that reloads unstyled;
//! `Duration`, `Entity`, `char` and the wide integers are others, written
//! through their own serializers.
//!
//! The failure mode is silent: the write path succeeds, the read path declines,
//! and the only symptom is a value reverting on load. These tests turn that
//! into a test failure, including for a `smol_str` version whose reflect impl
//! changes shape.

use bevy::ecs::component::Component;
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::ecs::world::World;
use bevy::prelude::ReflectDefault;
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

/// Opaque values that carry their own serializer, and a set.
#[derive(Component, Reflect, Clone, PartialEq, Debug)]
#[reflect(Component, Default)]
struct Kept {
    cooldown: std::time::Duration,
    spread: std::ops::Range<f32>,
    tags: std::collections::HashSet<String>,
    target: bevy::ecs::entity::Entity,
    glyph: char,
    big: u128,
    count: std::num::NonZeroU64,
}

impl Default for Kept {
    fn default() -> Self {
        Kept {
            cooldown: std::time::Duration::ZERO,
            spread: 0.0..1.0,
            tags: Default::default(),
            target: bevy::ecs::entity::Entity::PLACEHOLDER,
            glyph: 'a',
            big: 0,
            count: std::num::NonZeroU64::MIN,
        }
    }
}

fn kept_registry() -> AppTypeRegistry {
    let registry = AppTypeRegistry::default();
    registry.write().register::<Kept>();
    registry
}

/// The component a save and a reload give back: patch, text, parse, apply.
fn saved_and_reloaded<T: Component + Clone + bevy::reflect::Reflect + bevy::reflect::TypePath>(
    value: &T,
    registry: &AppTypeRegistry,
) -> (Option<T>, String) {
    let patch = component_to_bsn_patch(value, &registry.read());
    let mut ast = jackdaw_bsn::SceneBsnAst::default();
    let node = ast.create_entity_node(vec![patch]);
    let text = jackdaw_bsn::emit_entity(&ast, node);
    let (read, root) = jackdaw_bsn::parse_bsn(&text)
        .unwrap_or_else(|err| panic!("the saved text reads back: {err}\n{text}"));
    let patch = read
        .find_patch_by_type_path(root, T::type_path())
        .and_then(|patch| read.get_patch(patch))
        .cloned()
        .unwrap_or_else(|| panic!("the saved text holds the component:\n{text}"));
    let mut world = World::new();
    world.insert_resource(registry.clone());
    let entity = world.spawn_empty().id();
    apply_component_patch(&mut world, entity, &patch);
    (world.get::<T>(entity).cloned(), text)
}

#[test]
fn opaque_values_with_a_serializer_save_and_reload_as_themselves() {
    let target = World::new().spawn_empty().id();
    let kept = Kept {
        cooldown: std::time::Duration::from_millis(1500),
        spread: -2.5..4.0,
        tags: ["boss".to_string(), "elite".to_string()].into(),
        target,
        glyph: 'Q',
        big: u128::MAX,
        count: std::num::NonZeroU64::new(1 << 40).unwrap(),
    };

    let (reloaded, text) = saved_and_reloaded(&kept, &kept_registry());

    assert_eq!(reloaded, Some(kept), "from:\n{text}");
}

#[test]
fn a_duration_is_written_as_its_fields_not_its_debug_text() {
    let kept = Kept {
        cooldown: std::time::Duration::from_millis(1500),
        ..Kept::default()
    };

    let (_, text) = saved_and_reloaded(&kept, &kept_registry());

    assert!(
        text.contains("secs: 1") && text.contains("nanos: 500000000"),
        "{text}"
    );
    assert!(!text.contains("1.5s"), "{text}");
}

#[test]
fn a_set_is_written_in_a_stable_order() {
    let kept = Kept {
        tags: ["c", "a", "b"].map(String::from).into(),
        ..Kept::default()
    };

    let (_, text) = saved_and_reloaded(&kept, &kept_registry());

    let at = |tag: &str| {
        text.find(&format!("\"{tag}\""))
            .expect("each tag is written")
    };
    assert!(at("a") < at("b") && at("b") < at("c"), "{text}");
}

/// A struct holding a value nothing can write: an `Instant` reflects as
/// opaque with no serializer.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component, Default)]
struct Stamped {
    at: bevy::platform::time::Instant,
    label: String,
}

impl Default for Stamped {
    fn default() -> Self {
        Stamped {
            at: bevy::platform::time::Instant::now(),
            label: String::new(),
        }
    }
}

#[test]
fn a_field_with_no_document_form_is_left_out_instead_of_written_as_debug_text() {
    let registry = AppTypeRegistry::default();
    registry.write().register::<Stamped>();
    let stamped = Stamped {
        label: "kept".to_string(),
        ..Stamped::default()
    };

    let (reloaded, text) = saved_and_reloaded(&stamped, &registry);

    assert!(!text.contains("at:"), "the instant is left out:\n{text}");
    assert_eq!(
        reloaded.map(|stamped| stamped.label).as_deref(),
        Some("kept")
    );
}

#[test]
fn a_value_with_no_document_form_is_refused_when_asked() {
    let registry = AppTypeRegistry::default();
    registry.write().register::<Stamped>();

    let refused =
        BsnValue::try_from_reflect(&bevy::platform::time::Instant::now(), &registry.read());

    assert!(refused.is_err(), "got {refused:?}");
}
