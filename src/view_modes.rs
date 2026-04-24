use bevy::prelude::*;

pub struct ViewModesPlugin;

impl Plugin for ViewModesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewModeSettings>().add_systems(
            Update,
            toggle_wireframe_key.in_set(crate::EditorInteractionSystems),
        );
    }
}

#[derive(Resource, Default, Clone, PartialEq)]
pub struct ViewModeSettings {
    pub wireframe: bool,
}

fn toggle_wireframe_key(
    keyboard: Res<ButtonInput<KeyCode>>,
    keybinds: Res<crate::keybinds::KeybindRegistry>,
    mut settings: ResMut<ViewModeSettings>,
) {
    if keybinds.just_pressed(crate::keybinds::EditorAction::ToggleWireframe, &keyboard) {
        settings.wireframe = !settings.wireframe;
        if settings.wireframe {
            info!("Wireframe mode ON");
        } else {
            info!("Wireframe mode OFF");
        }
    }
}
