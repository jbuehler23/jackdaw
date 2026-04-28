//! Adds a new entry to the "Add" menu that spawns a cone.

use bevy::prelude::*;
use jackdaw::{brush::LastUsedMaterial, prelude::*};

fn main() -> AppExit {
    App::new()
        // log errors instead of panicking
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins((
            DefaultPlugins,
            EditorPlugins::default()
                .set(ExtensionPlugin::default().with_extension::<MenuEntryExampleExtension>()),
        ))
        .run()
}

#[derive(Default)]
pub struct MenuEntryExampleExtension;

impl JackdawExtension for MenuEntryExampleExtension {
    fn id(&self) -> String {
        "menu_entry_example".to_string()
    }

    fn label(&self) -> String {
        "Menu Entry Example".to_string()
    }

    fn description(&self) -> String {
        "Adds a new entry to the \"Add\" menu to spawn a cone".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_operator::<SpawnConeOp>()
            // Add a new entry to the "Add" menu (on the top left of the screen by default) that spawns a cone.
            .register_menu_entry::<SpawnConeOp>(TopLevelMenu::Add);
    }
}

#[operator(
    id = "menu_entry_example.spawn_cone",
    label = "Cone",
    description = "Spawns a new cone into the scene."
)]
fn spawn_cone(
    _: In<OperatorParameters>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    last_mat: Res<LastUsedMaterial>,
) -> OperatorResult {
    commands.spawn((
        Mesh3d(meshes.add(Cone::new(0.5, 2.0))),
        MeshMaterial3d(last_mat.material.clone().unwrap_or_default()),
    ));
    OperatorResult::Finished
}
