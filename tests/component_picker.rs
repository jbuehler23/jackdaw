//! Picker enumeration coverage. Drives [`enumerate_pickable_components`]
//! directly with a hand-built `TypeRegistry` so filter behaviour
//! can be pinned without spinning up the full editor app.

use std::any::TypeId;
use std::collections::HashSet;

use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use jackdaw::inspector::component_picker::{
    PickableComponent, PickerDenylist, enumerate_pickable_components, fallback_category_for,
    populate_avian_picker_denylist,
};
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
    let pickables =
        enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default());
    assert!(
        find(&pickables, "WithDefault").is_some(),
        "components opting into Default must appear in the picker",
    );
}

#[test]
fn component_without_default_appears() {
    let registry = registry_with_test_types();
    let pickables =
        enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default());
    assert!(
        find(&pickables, "NoDefault").is_some(),
        "components without `#[derive(Default)]` must still reach the picker; \
         `build_reflective_default` walks primitive field defaults",
    );
}

#[test]
fn non_component_reflect_type_is_filtered() {
    let registry = registry_with_test_types();
    let pickables =
        enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default());
    assert!(
        find(&pickables, "NotAComponent").is_none(),
        "reflected types without `ReflectComponent` must not appear",
    );
}

#[test]
fn editor_category_override_sets_category() {
    let registry = registry_with_test_types();
    let pickables =
        enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default());
    let entry = find(&pickables, "CategoriesAsActor").expect("entry present");
    assert_eq!(entry.category, "Actor");
}

#[test]
fn editor_description_override_sets_description() {
    let registry = registry_with_test_types();
    let pickables =
        enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default());
    let entry = find(&pickables, "DescribedExplicitly").expect("entry present");
    assert_eq!(entry.description, "explicit text");
}

#[test]
fn doc_comment_falls_through_as_description() {
    let registry = registry_with_test_types();
    let pickables =
        enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default());
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
    let pickables = enumerate_pickable_components(&registry, &existing, &PickerDenylist::default());
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
    let pickables =
        enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default());
    let entry = find(&pickables, "NoDefault").expect("entry present");
    assert_eq!(
        entry.category, "",
        "no `EditorCategory` attribute means an empty category; \
         the picker UI assigns a fallback group from module path",
    );
}

/// A type whose path is on the denylist must not surface in the picker
/// regardless of what it derives or registers.
#[test]
fn denylisted_path_filters_component() {
    let registry = registry_with_test_types();
    let mut denylist = PickerDenylist::default();
    denylist.deny_path("component_picker::WithDefault");
    let pickables = enumerate_pickable_components(&registry, &HashSet::new(), &denylist);
    assert!(
        find(&pickables, "WithDefault").is_none(),
        "explicitly denylisted type path must drop out of the picker",
    );
    assert!(
        find(&pickables, "NoDefault").is_some(),
        "denylist filtering must be path-scoped, not blanket",
    );
}

/// Prefix denylist entries cover every type under a module path. This
/// is how the avian denylist hides whole solver / mass-cache subtrees.
#[test]
fn denylisted_prefix_filters_component() {
    let registry = registry_with_test_types();
    let mut denylist = PickerDenylist::default();
    denylist.deny_prefix("component_picker::");
    let pickables = enumerate_pickable_components(&registry, &HashSet::new(), &denylist);
    assert!(
        find(&pickables, "WithDefault").is_none(),
        "prefix denylist must catch every type rooted under it",
    );
    assert!(
        find(&pickables, "NoDefault").is_none(),
        "prefix denylist applies to siblings too",
    );
}

/// Avian components don't carry `@EditorCategory` (upstream types),
/// so the picker's fallback maps `avian3d::` and
/// `jackdaw_avian_integration::` paths into the "Avian3d" category.
#[test]
fn avian_paths_fall_back_to_physics_category() {
    assert_eq!(
        fallback_category_for("avian3d::dynamics::rigid_body::RigidBody"),
        Some("Avian3d"),
    );
    assert_eq!(
        fallback_category_for("avian3d::collision::collider::Collider"),
        Some("Avian3d"),
    );
    assert_eq!(
        fallback_category_for("jackdaw_avian_integration::AvianCollider"),
        Some("Avian3d"),
    );
}

/// Non-avian paths get no fallback; their category comes from
/// `@EditorCategory` or remains empty (so the picker uses the
/// Bevy / Game module-path grouping).
#[test]
fn non_avian_paths_have_no_category_fallback() {
    assert_eq!(fallback_category_for("bevy_pbr::StandardMaterial"), None);
    assert_eq!(fallback_category_for("my_game::PlayerSpawn"), None);
    assert_eq!(fallback_category_for("component_picker::WithDefault"), None);
}

/// `populate_avian_picker_denylist` should hide solver / cache types,
/// regardless of registry contents. Asserting against the populated
/// denylist directly keeps the test independent of avian being a test
/// dep.
#[test]
fn avian_denylist_includes_known_internals() {
    let mut denylist = PickerDenylist::default();
    populate_avian_picker_denylist(&mut denylist);

    for path in [
        "avian3d::dynamics::solver::contact::Contact",
        "avian3d::collider_tree::ColliderTree",
        "avian3d::dynamics::rigid_body::mass_properties::components::computed::ComputedAngularInertia",
        "avian3d::dynamics::rigid_body::sleeping::SleepTimer",
        "avian3d::dynamics::integrator::VelocityIntegrationData",
        // Standalone `ColliderConstructor` panics avian's auto-init
        // when added without a mesh; users should pick `AvianCollider`.
        "avian3d::collision::collider::constructor::ColliderConstructor",
    ] {
        assert!(
            denylist.contains(path),
            "expected the avian denylist to hide `{path}`",
        );
    }

    // Sanity: user-facing avian components stay visible, including
    // `ColliderConstructorHierarchy` (descends into Mesh3d children
    // and is the right tool for prop-placement workflows).
    for path in [
        "avian3d::dynamics::rigid_body::RigidBody",
        "avian3d::collision::collider::Collider",
        "avian3d::dynamics::rigid_body::mass_properties::components::Mass",
        "avian3d::collision::collider::constructor::ColliderConstructorHierarchy",
    ] {
        assert!(
            !denylist.contains(path),
            "denylist must not hide user-facing `{path}`",
        );
    }
}

#[test]
fn editor_hidden_marker_filters_component() {
    let registry = registry_with_test_types();
    let pickables =
        enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default());
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
