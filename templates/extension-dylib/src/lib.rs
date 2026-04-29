//! `{{project-name}}` — a Jackdaw extension (dylib linkage).
//!
//! Building this crate produces `target/<profile>/lib{{crate_name}}.{so,dll,dylib}`,
//! which the editor's loader dlopens. The [`export_extension!`] macro
//! emits the `jackdaw_extension_entry_v1` symbol the loader looks
//! up.
//!
//! For static integration into a downstream editor or game binary,
//! add this crate as an `rlib` dep and call `with_extension::<...>()`
//! on `ExtensionPlugin`.

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

// Emits the FFI entry symbol the loader looks up.
jackdaw_api::export_extension!({{crate_name | upper_camel_case}}Extension);

#[operator(
    id = "{{project-name}}.hello",
    label = "Hello",
    description = "Logs a greeting.",
)]
fn hello(_: In<OperatorParameters>) -> OperatorResult {
    info!("Hello from {{project-name}}!");
    OperatorResult::Finished
}
