//! Sample extension.
//!
//! Demonstrates three pieces of the extension API:
//! - A plain dock window (`Hello Extension`).
//! - A simple operator (`HelloOp`) bound to F9 that logs a message.
//! - A second operator (`HelloTimeOp`) bound to F10 that uses an
//!   `is_available` check: it only runs while time is advancing
//!   (paused simulations return `Cancelled`).
//!
//! Disabling the extension in File > Extensions removes the window,
//! kills both keybinds, and drops any registered menu entries.

use std::sync::Arc;

use bevy::prelude::*;
use bevy_enhanced_input::prelude::*;
use jackdaw_api::prelude::*;

#[derive(Default)]
pub struct SampleExtension;

impl JackdawExtension for SampleExtension {
    fn id(&self) -> String {
        "sample".to_string()
    }

    fn label(&self) -> String {
        "Example Extension".to_string()
    }

    fn description(&self) -> String {
        "Just a tiny example extension :)".to_string()
    }

    fn register_input_context(&self, app: &mut App) {
        app.add_input_context::<SampleContext>();
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(WindowDescriptor {
            id: "sample.hello".into(),
            name: "Hello Extension".into(),
            icon: None,
            default_area: None,
            priority: None,
            build: Arc::new(build_hello_panel),
        });

        ctx.register_operator::<HelloOp>();
        ctx.register_operator::<HelloTimeOp>();

        ctx.spawn((
            SampleContext,
            actions!(SampleContext[
                // the `hello` operator function generates a struct called `HelloOp`
                (Action::<HelloOp>::new(), bindings![KeyCode::F9]),
                (Action::<HelloTimeOp>::new(), bindings![KeyCode::F10]),
            ]),
        ));
    }
}

fn build_hello_panel(world: &mut World, parent: Entity) {
    world.spawn((ChildOf(parent), Text::new("Hello from an extension!")));
}

#[derive(Component, Default)]
pub struct SampleContext;

#[operator(
    id = "sample.hello",
    label = "Hello",
    description = "Logs a hello message"
)]
fn hello(_: In<OperatorParameters>) -> OperatorResult {
    info!("Hello from the sample extension operator!");
    OperatorResult::Finished
}

/// Availability check for [`HelloTimeOp`]. Bevy systems returning
/// `bool` can inject any `SystemParam`; here we read `Time` and only
/// allow the operator to run while the clock is advancing.
fn time_is_running(time: Res<Time>) -> bool {
    time.delta_secs() > 0.0
}

#[operator(
    id = "sample.hello_time",
    label = "Hello (Time)",
    description = "Logs a hello message, but only while time is advancing",
    is_available = time_is_running,
)]
fn hello_time(_: In<OperatorParameters>, time: Res<Time>) -> OperatorResult {
    info!(
        "Hello at frame delta {:.3}s from the sample extension",
        time.delta_secs()
    );
    OperatorResult::Finished
}

// Exposes `jackdaw_extension_entry_v1` so the editor's dylib loader
// can discover this extension from disk. Always emitted: the
// cdylib output needs it; the rlib output that's statically
// linked into the prebuilt binary just carries a dead symbol,
// which is harmless.
jackdaw_api::export_extension!(SampleExtension);
