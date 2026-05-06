//! Picker enumeration coverage. Drives [`enumerate_pickable_components`]
//! directly with a hand-built `TypeRegistry` so filter behaviour
//! can be pinned without spinning up the full editor app.

use std::any::TypeId;
use std::collections::HashSet;

use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use jackdaw::inspector::component_picker::{PickableComponent, enumerate_pickable_components};
use jackdaw_runtime::{EditorCategory, EditorDescription, EditorHidden};

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct WithDefault {
    value: i32,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
struct NoDefault {
    a: bool,
    b: String,
    c: f32,
}

#[derive(Component, Reflect)]
#[reflect(Component, @EditorCategory::new("Actor"))]
struct CategoriesAsActor;

#[derive(Component, Reflect)]
#[reflect(Component, @EditorDescription::new("explicit text"))]
struct DescribedExplicitly;

/// Spawns the player.
#[derive(Component, Reflect)]
#[reflect(Component)]
struct DocCommentDescribed;

#[derive(Reflect, Default)]
#[reflect(Default)]
struct NotAComponent {
    flag: bool,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, @EditorHidden)]
struct HiddenByMarker;

fn registry_with_test_types() -> TypeRegistry {
    let mut registry = TypeRegistry::default();
    registry.register::<WithDefault>();
    registry.register::<NoDefault>();
    registry.register::<CategoriesAsActor>();
    registry.register::<DescribedExplicitly>();
    registry.register::<DocCommentDescribed>();
    registry.register::<NotAComponent>();
    registry.register::<HiddenByMarker>();
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

#[test]
fn editor_hidden_marker_filters_component() {
    let registry = registry_with_test_types();
    let pickables = enumerate_pickable_components(&registry, &HashSet::new());
    assert!(
        find(&pickables, "HiddenByMarker").is_none(),
        "`@EditorHidden` reflect attribute must keep a Component out of the picker",
    );
    // Sanity-check that unmarked components in the same registry
    // remain visible. This guards the regression where
    // `starts_with("jackdaw")` filtered any user crate whose name
    // started with `jackdaw_`. The current marker-based filter
    // must not regress to a path-based heuristic.
    assert!(find(&pickables, "WithDefault").is_some());
    assert!(find(&pickables, "NoDefault").is_some());
}
