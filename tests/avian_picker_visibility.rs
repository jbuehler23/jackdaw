//! Smoke coverage for the avian-physics picker UX. Boots a real
//! `editor_test_app`, walks the live `AppTypeRegistry`, and asserts
//! that the user-facing physics components surface from
//! [`enumerate_pickable_components`] in the "Avian3d" category while
//! the internals stay hidden.
//!
//! Catches regressions where:
//!   - `register_avian_types` stops being called on plugin build,
//!   - the picker denylist over-matches and hides public types,
//!   - the category fallback stops mapping `avian3d::*` to "Avian3d",
//!   - a future avian rename breaks the denylist's literal paths.

use std::collections::HashSet;

use bevy::ecs::reflect::AppTypeRegistry;
use jackdaw::inspector::component_picker::{
    PickableComponent, PickerDenylist, enumerate_pickable_components,
    populate_avian_picker_denylist,
};

mod util;

fn enumerate(app: &mut bevy::prelude::App) -> Vec<PickableComponent> {
    let mut denylist = PickerDenylist::default();
    populate_avian_picker_denylist(&mut denylist);
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    enumerate_pickable_components(&registry, &HashSet::new(), &denylist)
}

fn find<'a>(pickables: &'a [PickableComponent], path: &str) -> Option<&'a PickableComponent> {
    pickables.iter().find(|p| p.type_path_full == path)
}

/// `AvianCollider` is the editor wrapper users pick to attach a
/// collider. If this drops out of the picker, the inspector workflow
/// for adding physics breaks completely.
#[test]
fn avian_collider_wrapper_is_pickable() {
    let mut app = util::editor_test_app();
    let pickables = enumerate(&mut app);

    let entry = find(&pickables, "jackdaw_avian_integration::AvianCollider").unwrap_or_else(|| {
        panic!(
            "`jackdaw_avian_integration::AvianCollider` must surface in the picker; \
             registered = {}, total pickables = {}",
            app.world()
                .resource::<AppTypeRegistry>()
                .read()
                .get_with_type_path("jackdaw_avian_integration::AvianCollider")
                .is_some(),
            pickables.len(),
        )
    });
    assert_eq!(
        entry.category, "Avian3d",
        "AvianCollider should land in the Avian3d category via the avian fallback",
    );
}

/// `RigidBody` is the canonical body type. Picking `AvianCollider`
/// auto-adds it via `#[require(RigidBody)]`, but the user must also
/// be able to pick it directly to switch a static body to dynamic
/// (or vice versa) without going through the wrapper.
#[test]
fn rigid_body_is_pickable() {
    let mut app = util::editor_test_app();
    let pickables = enumerate(&mut app);

    let entry = find(&pickables, "avian3d::dynamics::rigid_body::RigidBody")
        .expect("`avian3d::dynamics::rigid_body::RigidBody` must surface in the picker");
    assert_eq!(entry.category, "Avian3d");
}

/// `ColliderConstructorHierarchy` is the descend-into-children path
/// for Mesh3d trees (Jan's prop-placement use case).
#[test]
fn collider_constructor_hierarchy_is_pickable() {
    let mut app = util::editor_test_app();
    let pickables = enumerate(&mut app);

    find(
        &pickables,
        "avian3d::collision::collider::constructor::ColliderConstructorHierarchy",
    )
    .expect("`ColliderConstructorHierarchy` must surface (denylist must not catch it)");
}

/// Standalone `ColliderConstructor` panics avian's auto-init when
/// added without a mesh. The denylist must keep it out of the picker.
#[test]
fn standalone_collider_constructor_is_hidden() {
    let mut app = util::editor_test_app();
    let pickables = enumerate(&mut app);

    assert!(
        find(
            &pickables,
            "avian3d::collision::collider::constructor::ColliderConstructor",
        )
        .is_none(),
        "standalone `ColliderConstructor` must be denylisted; picking it triggers \
         avian's `init_collider_constructors` which panics on entities without \
         a `Mesh3d` handle",
    );
}
