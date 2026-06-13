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
    /// Render every brush chunk with a translucent unlit material so
    /// occluded geometry and reference images show through.
    pub x_ray: bool,
}
