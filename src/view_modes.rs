use bevy::pbr::wireframe::{WireframeConfig, WireframePlugin};
use bevy::prelude::*;

pub struct ViewModesPlugin;

impl Plugin for ViewModesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewModeSettings>();
        // Bevy's wireframe pass draws the global wireframe overlay. The
        // `view.toggle_wireframe` operator flips `ViewModeSettings.wireframe`,
        // which `sync_global_wireframe` mirrors into `WireframeConfig`.
        if !app.is_plugin_added::<WireframePlugin>() {
            app.add_plugins(WireframePlugin::default());
        }
        app.add_systems(
            Update,
            sync_global_wireframe.run_if(resource_changed::<ViewModeSettings>),
        );
    }
}

/// Mirror the editor's wireframe view mode into Bevy's `WireframeConfig`.
fn sync_global_wireframe(settings: Res<ViewModeSettings>, mut config: ResMut<WireframeConfig>) {
    if config.global != settings.wireframe {
        config.global = settings.wireframe;
    }
}

#[derive(Resource, Default, Clone, PartialEq)]
pub struct ViewModeSettings {
    pub wireframe: bool,
    /// Render every brush chunk with a translucent unlit material so
    /// occluded geometry and reference images show through.
    pub x_ray: bool,
}
