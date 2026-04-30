use bevy::prelude::*;

pub struct ViewModesPlugin;

impl Plugin for ViewModesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewModeSettings>();
    }
}

#[derive(Resource, Default, Clone, PartialEq)]
pub struct ViewModeSettings {
    pub wireframe: bool,
}
