//! Creates a new window chock-full of all our widgets.

use bevy::prelude::*;
use jackdaw::prelude::*;
use jackdaw_feathers::{
    button::{ButtonProps, button},
    tokens::FONT_MD,
};

fn main() -> AppExit {
    App::new()
        // log errors instead of panicking
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins((
            DefaultPlugins,
            EditorPlugins::default()
                .set(ExtensionPlugin::default().with_extension::<WindowExampleExtension>()),
        ))
        .run()
}

#[derive(Default)]
pub struct WindowExampleExtension;

impl JackdawExtension for WindowExampleExtension {
    fn id(&self) -> String {
        "widget_showcase".to_string()
    }

    fn label(&self) -> String {
        "Widget Showcase".to_string()
    }

    fn description(&self) -> String {
        "Adds a new window chock-full of all our widgets.".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_operator::<OnButtonPressOp>().register_window(
            WindowDescriptor::new("widget_showcase.window")
                // After starting the editor, add this window from Window -> Widget showcase to see it in action
                .with_name("Widget Showcase")
                .with_default_area(DefaultArea::RightSidebar)
                .with_build(|window| {
                    window.spawn((
                        Text::new("Here's a label"),
                        TextFont::from_font_size(FONT_MD),
                    ));
                    window.spawn(button(ButtonProps::from_operator::<OnButtonPressOp>()));
                    // TODO: help me out and put widgets here <3
                }),
        );
    }
}

#[operator(
    id = "widget_showcase.button_operator",
    label = "Button connected to an operator",
    description = "Does nothing, but is called when the button is pressed."
)]
fn on_button_press(_: In<OperatorParameters>) -> OperatorResult {
    OperatorResult::Finished
}
