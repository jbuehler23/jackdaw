//! Shows how to create and register a minimal Jackdaw extension.

use bevy::prelude::*;
use jackdaw::prelude::*;

fn main() -> AppExit {
    App::new()
        // log errors instead of panicking
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins((
            DefaultPlugins,
            EditorPlugins::default()
                // Extensions can be statically registered using `with_extension`.
                .set(ExtensionPlugin::default().with_extension::<MinimalExampleExtension>()),
        ))
        .run()
}

#[derive(Default)]
pub struct MinimalExampleExtension;

impl JackdawExtension for MinimalExampleExtension {
    fn id(&self) -> String {
        // Every extension must have a unique ID that is used to identify it internally.
        "minimal_example".to_string()
    }

    fn label(&self) -> String {
        // The label is the optional human-readable name of the extension.
        "Minimal Example".to_string()
    }

    fn description(&self) -> String {
        // The description is the optional text description of the extension,
        // since it's easy to forget after a while which of your extensions does what.
        "This is a simple example extension which does nothing except log something when it gets registered"
            .to_string()
    }

    fn register(&self, _ctx: &mut ExtensionContext) {
        // This method is called when the extension is registered.
        // This happens automatically for extensions that are added to the app with `with_extension`.
        //
        // Go to File -> Extensions and toggle "Minimal Example" off and on to unload and reload it again
        // to see this message printed to the console repeatedly.
        info!("The custom extension has been registered! How cool is that?!");
    }
}
