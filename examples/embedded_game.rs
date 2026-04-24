//! Static-embedded game template.
//!
//! Demonstrates the Fyrox-style / embedded-editor shape: Jackdaw
//! linked into the binary as a library, the user's game plugin
//! registered statically through
//! [`ExtensionPlugin::with_extension`]. No dylib loading, no
//! rustc-wrapper, no `.cargo/config.toml` stitching — one
//! `cargo run --example embedded_game` and you're in the editor with
//! `MyGamePlugin` already active.
//!
//! Run:
//! ```text
//! cargo run --example embedded_game
//! ```
//!
//! This is the pattern the forthcoming `jackdaw_template_game_static`
//! scaffold will follow; keeping a working copy in-tree so the
//! embedded path stays compile-checked as the extension API evolves.

use bevy::prelude::*;
use jackdaw::prelude::*;

fn main() -> AppExit {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(
            EditorPlugins::default()
                .set(ExtensionPlugin::new().with_extension::<MyGameExtension>()),
        )
        .run()
}

/// Replace with your game's types/operators/windows. This stub
/// registers nothing so the example stays minimal; the extension
/// still shows up under File → Extensions as proof the
/// registration round-trip works.
#[derive(Default)]
struct MyGameExtension;

impl JackdawExtension for MyGameExtension {
    fn id(&self) -> String {
        "my_game".into()
    }

    fn register(&self, _ctx: &mut ExtensionContext) {}
}
