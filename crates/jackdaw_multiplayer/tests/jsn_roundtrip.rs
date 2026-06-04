//! A `Replication` proxy round-trips through Bevy's structural reflection
//! (de)serialization — the path Jackdaw's `.jsn` save/load uses. Run:
//! `cargo test -p jackdaw_multiplayer --test jsn_roundtrip`

use bevy::prelude::*;
use bevy::reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
use jackdaw_multiplayer::{JackdawMultiplayerTypesPlugin, ReplTarget, Replication};
use serde::de::DeserializeSeed;

#[test]
fn replication_proxy_roundtrips_through_reflection() {
    let mut app = App::new();
    app.add_plugins(JackdawMultiplayerTypesPlugin);
    let registry = app.world().resource::<AppTypeRegistry>().read();

    let original = Replication {
        target: ReplTarget::All,
        interpolated: true,
    };

    let serializer = TypedReflectSerializer::new(&original, &registry);
    let json = serde_json::to_value(&serializer).expect("serialize Replication");

    let registration = registry
        .get(std::any::TypeId::of::<Replication>())
        .expect("Replication registered");
    let de = TypedReflectDeserializer::new(registration, &registry);
    let reflected = de.deserialize(&json).expect("deserialize Replication");

    let roundtripped =
        Replication::from_reflect(reflected.as_partial_reflect()).expect("FromReflect Replication");
    assert_eq!(
        original, roundtripped,
        "Replication must survive the reflect round-trip"
    );
}
