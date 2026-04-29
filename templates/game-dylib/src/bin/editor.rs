//! Editor runner for `{{project-name}}`.
//!
//! `cargo editor` opens Jackdaw and the dylib loader dlopens this
//! crate's `cdylib` artefact, installing the plugin against the
//! editor's `GameSubApp`. For first-build sanity-checking and
//! CI roundtrips, the plugin is also linked statically here via
//! `with_game::<...>()` — both paths register the same plugin
//! against the same SubApp; the loader is idempotent.

use bevy::prelude::*;
use jackdaw::prelude::EditorPlugins;
use {{crate_name}}::{{crate_name | upper_camel_case}}Plugin;

fn main() -> AppExit {
    App::new()
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins(EditorPlugins::default().with_game::<{{crate_name | upper_camel_case}}Plugin>())
        .run()
}
