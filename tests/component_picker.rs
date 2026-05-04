//! Picker enumeration coverage. These tests drive
//! [`enumerate_pickable_components`] directly with a hand-built
//! `TypeRegistry` so we can pin filter behaviour without needing
//! to spin up the full editor app.
//!
//! Cases:
//!  * Component with `Default` derive shows up.
//!  * Component WITHOUT `Default` still shows up (the
//!    `build_reflective_default` walker covers it).
//!  * Reflected types that lack `ReflectComponent` are skipped.
//!  * `@EditorCategory("...")` overrides the bucket.
//!  * `@EditorDescription("...")` overrides the doc comment.
//!  * Doc comments fall through as the description when no
//!    `@EditorDescription` is set.
//!  * Components whose `TypeId` is in the `existing_types` set
//!    (already on the entity) drop out.
//!  * Components needing `Box<dyn Trait>` style fields drop out
//!    because the default-builder can't synthesise a value.

use std::any::TypeId;
use std::collections::HashSet;

use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use jackdaw::inspector::component_picker::{PickableComponent, enumerate_pickable_components};
use jackdaw_runtime::{EditorCategory, EditorDescription};

/// Component with the full ceremony (Default derive + reflect
/// data). Stand-in for "user opted into Default".
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct WithDefault {
    value: i32,
}

/// Jan's minimum-viable shape: derive + reflect, NO Default.
#[derive(Component, Reflect)]
#[reflect(Component)]
struct NoDefault {
    a: bool,
    b: String,
    c: f32,
}

/// `EditorCategory` override.
#[derive(Component, Reflect)]
#[reflect(Component, @EditorCategory::new("Actor"))]
struct CategoriesAsActor;

/// `EditorDescription` override.
#[derive(Component, Reflect)]
#[reflect(Component, @EditorDescription::new("explicit text"))]
struct DescribedExplicitly;

/// Description should fall back to the doc comment captured via
/// the `reflect_documentation` feature.
///
/// Spawns the player.
#[derive(Component, Reflect)]
#[reflect(Component)]
struct DocCommentDescribed;

/// Reflected type that is NOT a `Component`; must be filtered out.
#[derive(Reflect, Default)]
#[reflect(Default)]
struct NotAComponent {
    flag: bool,
}

fn registry_with_test_types() -> TypeRegistry {
    let mut registry = TypeRegistry::default();
    registry.register::<WithDefault>();
    registry.register::<NoDefault>();
    registry.register::<CategoriesAsActor>();
    registry.register::<DescribedExplicitly>();
    registry.register::<DocCommentDescribed>();
    registry.register::<NotAComponent>();
    registry
}

fn find<'a>(pickables: &'a [PickableComponent], short_name: &str) -> Option<&'a PickableComponent> {
    pickables.iter().find(|p| p.short_name == short_name)
}

#[test]
fn component_with_default_appears() {
    let registry = registry_with_test_types();
    let pickables = enumerate_pickable_components(&registry, &HashSet::new());
    assert!(
        find(&pickables, "WithDefault").is_some(),
        "components opting into Default must appear in the picker",
    );
}

#[test]
fn component_without_default_appears() {
    let registry = registry_with_test_types();
    let pickables = enumerate_pickable_components(&registry, &HashSet::new());
    assert!(
        find(&pickables, "NoDefault").is_some(),
        "components without `#[derive(Default)]` must still reach the picker; \
         `build_reflective_default` walks primitive field defaults",
    );
}

#[test]
fn non_component_reflect_type_is_filtered() {
    let registry = registry_with_test_types();
    let pickables = enumerate_pickable_components(&registry, &HashSet::new());
    assert!(
        find(&pickables, "NotAComponent").is_none(),
        "reflected types without `ReflectComponent` must not appear",
    );
}

#[test]
fn editor_category_override_sets_category() {
    let registry = registry_with_test_types();
    let pickables = enumerate_pickable_components(&registry, &HashSet::new());
    let entry = find(&pickables, "CategoriesAsActor").expect("entry present");
    assert_eq!(entry.category, "Actor");
}

#[test]
fn editor_description_override_sets_description() {
    let registry = registry_with_test_types();
    let pickables = enumerate_pickable_components(&registry, &HashSet::new());
    let entry = find(&pickables, "DescribedExplicitly").expect("entry present");
    assert_eq!(entry.description, "explicit text");
}

#[test]
fn doc_comment_falls_through_as_description() {
    let registry = registry_with_test_types();
    let pickables = enumerate_pickable_components(&registry, &HashSet::new());
    let entry = find(&pickables, "DocCommentDescribed").expect("entry present");
    assert!(
        entry.description.contains("Spawns the player"),
        "doc comment should populate description when no \
         `EditorDescription` attribute is set; got: {:?}",
        entry.description,
    );
}

#[test]
fn already_on_entity_components_are_filtered() {
    let registry = registry_with_test_types();
    let mut existing = HashSet::new();
    existing.insert(TypeId::of::<WithDefault>());
    let pickables = enumerate_pickable_components(&registry, &existing);
    assert!(
        find(&pickables, "WithDefault").is_none(),
        "components already on the target entity must drop out",
    );
    assert!(
        find(&pickables, "NoDefault").is_some(),
        "filtering should be per-type, not blanket",
    );
}

#[test]
fn category_default_is_empty_when_unset() {
    let registry = registry_with_test_types();
    let pickables = enumerate_pickable_components(&registry, &HashSet::new());
    let entry = find(&pickables, "NoDefault").expect("entry present");
    assert_eq!(
        entry.category, "",
        "no `EditorCategory` attribute means an empty category; \
         the picker UI assigns a fallback group from module path",
    );
}
