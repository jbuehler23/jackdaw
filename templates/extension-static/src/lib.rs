//! `{{project-name}}` — a Jackdaw extension (static linkage).
//!
//! Add this extension to a downstream editor or game binary via:
//!
//! ```ignore
//! use jackdaw::prelude::*;
//! use {{crate_name}}::{{crate_name | upper_camel_case}}Extension;
//!
//! App::new()
//!     .add_plugins(
//!         EditorPlugins::default().set(
//!             ExtensionPlugin::new()
//!                 .with_extension::<{{crate_name | upper_camel_case}}Extension>(),
//!         ),
//!     )
//!     .run();
//! ```

use bevy::prelude::*;
use jackdaw_api::prelude::*;

#[derive(Default)]
pub struct {{crate_name | upper_camel_case}}Extension;

impl JackdawExtension for {{crate_name | upper_camel_case}}Extension {
    fn id(&self) -> String {
        "{{project-name}}".to_string()
    }

    fn label(&self) -> String {
        "{{project-name}}".to_string()
    }

    fn description(&self) -> String {
        "Description of your extension.".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_operator::<HelloOp>();
    }
}

/// Example operator the extension registers. Replace with your own.
#[operator(
    id = "{{project-name}}.hello",
    label = "Hello",
    description = "Logs a greeting.",
)]
fn hello(_: In<OperatorParameters>) -> OperatorResult {
    info!("Hello from {{project-name}}!");
    OperatorResult::Finished
}
