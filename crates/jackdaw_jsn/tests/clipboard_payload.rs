use jackdaw_jsn::format::{ClipboardPayload, JsnAssets, JsnEntity};

#[test]
fn payload_round_trip_serde() {
    let payload = ClipboardPayload {
        entities: vec![JsnEntity {
            parent: None,
            components: Default::default(),
        }],
        assets: JsnAssets::default(),
    };
    let json = serde_json::to_string(&payload).unwrap();
    let back: ClipboardPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(back.entities.len(), 1);
}

#[test]
fn payload_defaults_to_no_assets() {
    let payload = ClipboardPayload {
        entities: vec![],
        assets: JsnAssets::default(),
    };
    let json = serde_json::to_string(&payload).unwrap();
    // The serialized form should be deserializable even if assets is empty.
    let back: ClipboardPayload = serde_json::from_str(&json).unwrap();
    assert!(back.entities.is_empty());
}
