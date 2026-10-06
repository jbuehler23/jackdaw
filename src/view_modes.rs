//! How the viewport shades the scene: lit, unlit, lighting only, wireframe,
//! lit with a wireframe over it, or tinted by level of detail.
//!
//! Unlit, lighting only and wireframe swap each mesh's [`StandardMaterial`]
//! for a derived one while they last and put the mesh's own back afterwards;
//! the derived materials are made once per source material. A mesh wearing a
//! [`StandardMaterial`] extension (layered surfaces, foliage, water) is drawn
//! with a plain one derived from the extension's base. Terrain keeps its own
//! material. A view setting: the saved scene keeps its own materials.

use bevy::pbr::wireframe::{WireframeConfig, WireframePlugin};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

pub struct ViewModesPlugin;

impl Plugin for ViewModesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewModeSettings>()
            .init_resource::<ShadingMaterials>();
        if !app.is_plugin_added::<WireframePlugin>() {
            app.add_plugins(WireframePlugin::default());
        }
        app.add_systems(
            Update,
            (
                sync_global_wireframe.run_if(resource_changed::<ViewModeSettings>),
                shade_meshes,
                shade_extended_meshes::<jackdaw_surface::LayeredSurface>,
                shade_extended_meshes::<jackdaw_surface::Foliage>,
                shade_extended_meshes::<jackdaw_surface::Water>,
            ),
        );
    }
}

/// Mirror the editor's wireframe into Bevy's `WireframeConfig`.
fn sync_global_wireframe(settings: Res<ViewModeSettings>, mut config: ResMut<WireframeConfig>) {
    let global = settings.wireframe || settings.shading == Shading::Wireframe;
    if config.global != global {
        config.global = global;
    }
}

/// What the viewport draws mesh surfaces with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shading {
    /// The scene's own materials and lights.
    #[default]
    Lit,
    /// Each material's colour and textures with no lighting.
    Unlit,
    /// The light falling on a white surface, with the materials' colour taken
    /// away.
    LightingOnly,
    /// Edges only, over dark surfaces.
    Wireframe,
}

/// How the viewport shades the scene, one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Lit,
    Unlit,
    Wireframe,
    /// Lit, with every mesh's edges drawn over it.
    LitWireframe,
    LightingOnly,
    /// Each level of detail tinted in its LOD bar colour.
    LodColors,
}

impl ViewMode {
    pub const ALL: [Self; 6] = [
        Self::Lit,
        Self::Unlit,
        Self::Wireframe,
        Self::LitWireframe,
        Self::LightingOnly,
        Self::LodColors,
    ];

    /// The name the `view.mode` operator takes.
    pub fn id(self) -> &'static str {
        match self {
            Self::Lit => "lit",
            Self::Unlit => "unlit",
            Self::Wireframe => "wireframe",
            Self::LitWireframe => "lit_wireframe",
            Self::LightingOnly => "lighting_only",
            Self::LodColors => "lod_colors",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lit => "Lit",
            Self::Unlit => "Unlit",
            Self::Wireframe => "Wireframe",
            Self::LitWireframe => "Lit + Wireframe",
            Self::LightingOnly => "Lighting Only",
            Self::LodColors => "LOD Colors",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.id() == id)
    }

    /// The mode the view settings show, the LOD tint winning over the rest.
    pub fn of(settings: &ViewModeSettings, lod_colors: bool) -> Self {
        match (lod_colors, settings.shading, settings.wireframe) {
            (true, _, _) => Self::LodColors,
            (false, Shading::Lit, false) => Self::Lit,
            (false, Shading::Lit, true) => Self::LitWireframe,
            (false, Shading::Unlit, _) => Self::Unlit,
            (false, Shading::LightingOnly, _) => Self::LightingOnly,
            (false, Shading::Wireframe, _) => Self::Wireframe,
        }
    }

    /// The shading this mode draws surfaces with, and whether edges are drawn
    /// over them.
    pub fn shading(self) -> (Shading, bool) {
        match self {
            Self::Lit | Self::LodColors => (Shading::Lit, false),
            Self::Unlit => (Shading::Unlit, false),
            Self::Wireframe => (Shading::Wireframe, false),
            Self::LitWireframe => (Shading::Lit, true),
            Self::LightingOnly => (Shading::LightingOnly, false),
        }
    }
}

#[derive(Resource, Default, Clone, PartialEq)]
pub struct ViewModeSettings {
    /// Edges drawn over every mesh.
    pub wireframe: bool,
    pub shading: Shading,
    /// Render every brush chunk with a translucent unlit material so
    /// occluded geometry and reference images show through.
    pub x_ray: bool,
}

/// A mesh's own material while a shading view draws it with another.
#[derive(Component)]
pub struct ShadedBy(Handle<StandardMaterial>);

/// The materials each shading made from each source material.
#[derive(Resource, Default)]
struct ShadingMaterials(HashMap<(AssetId<StandardMaterial>, Shading), Handle<StandardMaterial>>);

/// The colour wireframe surfaces are filled with.
const WIREFRAME_FILL: Color = Color::srgb(0.06, 0.06, 0.07);

/// The material `shading` draws a surface of `source` with.
pub fn shaded_material(source: &StandardMaterial, shading: Shading) -> StandardMaterial {
    match shading {
        Shading::Lit => source.clone(),
        Shading::Unlit => StandardMaterial {
            unlit: true,
            ..source.clone()
        },
        Shading::LightingOnly => StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: None,
            emissive: LinearRgba::BLACK,
            emissive_texture: None,
            ..source.clone()
        },
        Shading::Wireframe => StandardMaterial {
            base_color: WIREFRAME_FILL,
            unlit: true,
            ..default()
        },
    }
}

/// A mesh's own extended material while a shading view draws it with a plain
/// one made from the extension's base.
#[derive(Component)]
pub struct ShadedExtended<E: MaterialExtension>(Handle<ExtendedMaterial<StandardMaterial, E>>);

/// On a mesh a shading view moved off an extended material, so the plain one
/// it now wears is not shaded a second time.
#[derive(Component)]
pub struct ShadedFromExtension;

type ShadableMesh = (
    Without<ShadedBy>,
    Without<ShadedFromExtension>,
    Without<crate::EditorEntity>,
    Without<crate::lod_bar::LodTint>,
);

/// Swap the meshes' materials for the current shading's, or put their own
/// back once the viewport is lit again.
fn shade_meshes(
    settings: Res<ViewModeSettings>,
    mut commands: Commands,
    own: Query<(Entity, &MeshMaterial3d<StandardMaterial>), ShadableMesh>,
    shaded: Query<(Entity, &ShadedBy)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut made: ResMut<ShadingMaterials>,
) {
    if settings.is_changed() {
        for (mesh, kept) in &shaded {
            commands
                .entity(mesh)
                .try_insert(MeshMaterial3d(kept.0.clone()))
                .try_remove::<ShadedBy>();
        }
        return;
    }
    let shading = settings.shading;
    if shading == Shading::Lit {
        return;
    }
    for (mesh, material) in &own {
        let key = (material.id(), shading);
        let derived = match made.0.get(&key) {
            Some(derived) => derived.clone(),
            None => {
                let Some(made_from) = materials
                    .get(material.id())
                    .map(|source| shaded_material(source, shading))
                else {
                    continue;
                };
                let derived = materials.add(made_from);
                made.0.insert(key, derived.clone());
                derived
            }
        };
        commands
            .entity(mesh)
            .try_insert((ShadedBy(material.0.clone()), MeshMaterial3d(derived)));
    }
}

/// As [`shade_meshes`], for the meshes wearing an extension `E` of
/// [`StandardMaterial`].
fn shade_extended_meshes<E: MaterialExtension>(
    settings: Res<ViewModeSettings>,
    mut commands: Commands,
    own: Query<
        (
            Entity,
            &MeshMaterial3d<ExtendedMaterial<StandardMaterial, E>>,
        ),
        Without<crate::EditorEntity>,
    >,
    shaded: Query<(Entity, &ShadedExtended<E>)>,
    extended: Option<Res<Assets<ExtendedMaterial<StandardMaterial, E>>>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut made: Local<
        HashMap<
            (AssetId<ExtendedMaterial<StandardMaterial, E>>, Shading),
            Handle<StandardMaterial>,
        >,
    >,
) {
    if settings.is_changed() {
        for (mesh, kept) in &shaded {
            commands
                .entity(mesh)
                .try_remove::<(
                    MeshMaterial3d<StandardMaterial>,
                    ShadedExtended<E>,
                    ShadedFromExtension,
                )>()
                .try_insert(MeshMaterial3d(kept.0.clone()));
        }
        return;
    }
    let shading = settings.shading;
    let Some(extended) = extended.filter(|_| shading != Shading::Lit) else {
        return;
    };
    for (mesh, material) in &own {
        let key = (material.id(), shading);
        let derived = match made.get(&key) {
            Some(derived) => derived.clone(),
            None => {
                let Some(made_from) = extended
                    .get(material.id())
                    .map(|source| shaded_material(&source.base, shading))
                else {
                    continue;
                };
                let derived = materials.add(made_from);
                made.insert(key, derived.clone());
                derived
            }
        };
        commands
            .entity(mesh)
            .try_remove::<MeshMaterial3d<ExtendedMaterial<StandardMaterial, E>>>()
            .try_insert((
                ShadedExtended(material.0.clone()),
                ShadedFromExtension,
                MeshMaterial3d(derived),
            ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_reads_back_from_the_settings_it_sets() {
        for mode in ViewMode::ALL {
            let (shading, wireframe) = mode.shading();
            let settings = ViewModeSettings {
                shading,
                wireframe,
                x_ray: false,
            };
            assert_eq!(ViewMode::of(&settings, mode == ViewMode::LodColors), mode);
        }
    }

    #[test]
    fn unlit_keeps_the_colour_and_lighting_only_takes_it_away() {
        let source = StandardMaterial {
            base_color: Color::srgb(0.8, 0.2, 0.1),
            perceptual_roughness: 0.3,
            ..default()
        };
        let unlit = shaded_material(&source, Shading::Unlit);
        assert!(unlit.unlit);
        assert_eq!(unlit.base_color, source.base_color);
        let lighting = shaded_material(&source, Shading::LightingOnly);
        assert!(!lighting.unlit);
        assert_eq!(lighting.base_color, Color::WHITE);
        assert_eq!(lighting.perceptual_roughness, 0.3);
    }
}
