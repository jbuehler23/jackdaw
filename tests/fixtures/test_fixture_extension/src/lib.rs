//! Minimal cdylib fixture for `tests/dylib_loading.rs`.
//!
//! Registers one operator. No windows, no BEI input contexts. The
//! integration tests exercise the loader's own job (open, ABI check,
//! catalog entry, handle retention); cross-boundary component
//! dispatch needs the `dylib` feature plus `jackdaw_sdk` for type
//! coherence and lives in a separate harness.

use bevy::prelude::*;
use jackdaw_api::export_extension;
use jackdaw_api::prelude::*;

pub struct TestFixtureExtension;

impl JackdawExtension for TestFixtureExtension {
    fn id(&self) -> String {
        "test_fixture".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_operator::<SpawnMarkerOp>();
    }
}

#[operator(id = "test_fixture.spawn_marker", label = "Spawn Marker")]
fn spawn_marker(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.spawn(Name::new("test_fixture_marker"));
    OperatorResult::Finished
}

export_extension!(TestFixtureExtension);
