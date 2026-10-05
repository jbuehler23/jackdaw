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

/// How the viewport shades the scene, one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    /// The scene's own materials and lights.
    Lit,
    /// Lit, with every mesh's edges drawn over it.
    Wireframe,
    /// Each level of detail tinted in its LOD bar colour.
    LodColors,
}

impl ViewMode {
    pub const ALL: [Self; 3] = [Self::Lit, Self::Wireframe, Self::LodColors];

    /// The name the `view.mode` operator takes.
    pub fn id(self) -> &'static str {
        match self {
            Self::Lit => "lit",
            Self::Wireframe => "wireframe",
            Self::LodColors => "lod_colors",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lit => "Lit",
            Self::Wireframe => "Wireframe",
            Self::LodColors => "LOD Colors",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.id() == id)
    }

    /// The mode the view settings show, the LOD tint winning over a wireframe
    /// drawn beneath it.
    pub fn of(settings: &ViewModeSettings, lod_colors: bool) -> Self {
        if lod_colors {
            Self::LodColors
        } else if settings.wireframe {
            Self::Wireframe
        } else {
            Self::Lit
        }
    }
}

#[derive(Resource, Default, Clone, PartialEq)]
pub struct ViewModeSettings {
    pub wireframe: bool,
    /// Render every brush chunk with a translucent unlit material so
    /// occluded geometry and reference images show through.
    pub x_ray: bool,
}
