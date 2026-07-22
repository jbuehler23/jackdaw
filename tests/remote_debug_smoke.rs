//! Smoke coverage for the debugger's dock windows: every remote/debug panel is
//! registered as a dock window, carries the "Remote " name prefix, and the
//! Remote Debug workspace exists to hold them. A panel dropping out of the
//! wiring, or the naming convention regressing, fails here.

use jackdaw_panels::{WindowRegistry, WorkspaceRegistry};

mod util;

/// Every dock window the debugger registers.
const DEBUG_WINDOWS: &[&str] = &[
    "jackdaw.remote.entities",
    "jackdaw.remote.inspector",
    "jackdaw.debug.diagnostics",
    "jackdaw.debug.queries",
    "jackdaw.debug.archetypes",
    "jackdaw.debug.schedules",
    "jackdaw.debug.graph",
    "jackdaw.debug.relationships",
];

#[test]
fn debug_windows_and_workspace_are_wired() {
    let mut app = util::editor_test_app();
    // Extensions register their windows during startup; tick a few frames so the
    // registration is settled before asserting.
    for _ in 0..3 {
        app.update();
    }

    let registry = app.world().resource::<WindowRegistry>();
    for id in DEBUG_WINDOWS {
        let descriptor = registry
            .get(id)
            .unwrap_or_else(|| panic!("{id} is not registered as a dock window"));
        assert!(
            descriptor.name.starts_with("Remote "),
            "{id} should be Remote-prefixed, got {:?}",
            descriptor.name
        );
    }

    let workspace = app
        .world()
        .resource::<WorkspaceRegistry>()
        .get("debug")
        .expect("the debug workspace should be registered");
    assert_eq!(
        workspace.name, "Remote Debug",
        "the debug workspace should be named Remote Debug"
    );
}
