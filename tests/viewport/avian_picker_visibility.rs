//! The avian-physics picker: the user-facing physics components surface from
//! `enumerate_pickable_components` in the "Avian3d" category while the
//! internals stay hidden.

use crate::util;

use std::collections::HashSet;

use bevy::ecs::reflect::AppTypeRegistry;
use jackdaw::inspector::component_picker::{
    PickableComponent, PickerDenylist, enumerate_pickable_components,
    populate_avian_picker_denylist,
};
use jackdaw::project_types::ProjectTypes;
use jackdaw::type_metadata::TypeMetadata;

fn enumerate(app: &mut bevy::prelude::App) -> Vec<PickableComponent> {
    let mut denylist = PickerDenylist::default();
    populate_avian_picker_denylist(&mut denylist);
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let metadata = app.world().resource::<TypeMetadata>().clone();
    let project_types = app.world().resource::<ProjectTypes>().clone();
    let registry = registry.read();
    enumerate_pickable_components(
        &registry,
        &HashSet::new(),
        &denylist,
        &metadata,
        &project_types,
    )
}

fn find<'a>(pickables: &'a [PickableComponent], path: &str) -> Option<&'a PickableComponent> {
    pickables.iter().find(|p| p.type_path_full == path)
}

/// `AvianCollider` is the editor wrapper users pick to attach a collider.
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

/// Picking `AvianCollider` auto-adds `RigidBody` through `#[require(RigidBody)]`,
/// but it must be pickable directly too, to switch a static body to dynamic.
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
