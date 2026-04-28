//! Extends the `extend_panel` example by moving its contents into an own window.

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
        "window_example".to_string()
    }

    fn label(&self) -> String {
        "Window Example".to_string()
    }

    fn description(&self) -> String {
        "Adds a new window that can be placed in the editor by the user.".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_operator::<UpdateElapsedTimeLabelOp>()
            .register_window(
                // New windows need to have a unique ID. Usually, this follows the pattern `{extension_id}.window` if it's a single window,
                // otherwise `{extension_id}.{some_name}`.
                WindowDescriptor::new("window_example.window")
                    // To see out new UI, the user first needs to open the window by clicking on Window -> Example Window
                    // Do that after starting the editor to see our UI in action.
                    .with_name("Example Window")
                    // After adding the window, it will be docked on the right side of the editor by default.
                    .with_default_area(DefaultArea::RightSidebar)
                    .with_build(|window| {
                        // This method here is used exactly like `Commands::with_children`.
                        // using `.spawn` will spawn a new entity as a child of the window.
                        // While you can style your UI however you want, jackdaw comes with a set of pre-built themed widgets
                        // that you can use to have a consistent look and feel.
                        window.spawn((
                            Node {
                                margin: UiRect::all(px(10.0)),
                                ..default()
                            },
                            // This here is a simple row layout with two columns: the label and the elapsed time.
                            children![
                                (
                                    Text::new("Time passed since application startup:"),
                                    TextFont::from_font_size(FONT_MD),
                                ),
                                (
                                    ElapsedTimeLabel,
                                    Text::new("<press the button below to update>"),
                                    TextFont::from_font_size(FONT_MD),
                                ),
                            ],
                        ));
                        // Here, we use the built-in `button` widget, which is directly linked to an operator.
                        // Things like the label, tooltip, etc. are automatically set up for us based on the operator definition.
                        window.spawn(button(
                            ButtonProps::from_operator::<UpdateElapsedTimeLabelOp>(),
                        ));
                    }),
            );
    }
}

#[derive(Component)]
struct ElapsedTimeLabel;

#[operator(
    id = "window_example.update_elapsed_time_label",
    label = "Update time label",
    description = "Updates the label with the amount of time that passed since Jackdaw started."
)]
fn update_elapsed_time_label(
    _: In<OperatorParameters>,
    time: Res<Time>,
    mut label: Single<&mut Text, With<ElapsedTimeLabel>>,
) -> OperatorResult {
    let elapsed = time.elapsed();
    ***label = format!(
        "{}m {}s {}ms",
        elapsed.as_secs() / 60,
        elapsed.as_secs() % 60,
        elapsed.as_millis()
    );
    OperatorResult::Finished
}
