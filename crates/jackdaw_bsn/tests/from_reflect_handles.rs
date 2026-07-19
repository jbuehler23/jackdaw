//! Regression: an asset `Handle` whose `AssetId` is Uuid-based must not
//! descend into its `AssetId`'s `uuid::Uuid` when converted to BSN. Before
//! the fix, a handle of a type without a registered `ReflectHandle` fell
//! through the handle short-circuit into the reflect walk, which reached
//! the raw `Uuid` and emitted a "no BSN representation" warning while
//! storing the uuid's Debug form - noise on every world->BSN sync.

use bevy::asset::{Asset, Handle, uuid_handle};
use bevy::reflect::{Reflect, TypeRegistry};

use jackdaw_bsn::component_to_bsn_patch;

const HANDLE_UUID: &str = "8e6c3d2a-5b14-4f9e-9a77-c01d54a3b681";

#[derive(Asset, Reflect)]
struct DummyMaterial;

#[derive(Reflect)]
struct HasHandle {
    material: Handle<DummyMaterial>,
}

#[test]
fn uuid_based_handle_does_not_leak_into_bsn() {
    let component = HasHandle {
        material: uuid_handle!("8e6c3d2a-5b14-4f9e-9a77-c01d54a3b681"),
    };

    // An empty registry: `Handle<DummyMaterial>` has no `ReflectHandle`
    // type data, exactly the situation that made the walk descend into the
    // AssetId and reach the raw uuid.
    let registry = TypeRegistry::new();
    let patch = component_to_bsn_patch(&component, &registry);

    let rendered = format!("{patch:?}");
    assert!(
        !rendered.contains(HANDLE_UUID),
        "the handle's uuid leaked into the BSN patch instead of being \
         short-circuited: {rendered}"
    );
}
