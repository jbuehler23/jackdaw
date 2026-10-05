//! Puts the [`ViewportSettings`] onto what the editor draws, and nowhere else.
//!
//! The viewport cameras take the effects and edge smoothing; the shadow maps,
//! terrain detail and level-of-detail distances take the quality choices; and
//! the render world draws point, spot and sun lights without their shadows
//! while those are hidden. No scene component is written, so the saved scene
//! and Play keep the scene's own lights and effects.

use bevy::anti_alias::fxaa::{Fxaa, Sensitivity};
use bevy::anti_alias::smaa::Smaa;
use bevy::anti_alias::taa::TemporalAntiAliasing;
use bevy::core_pipeline::prepass::{MotionVectorPrepass, NormalPrepass};
use bevy::image::ToExtents;
use bevy::light::{DirectionalLightShadowMap, PointLightShadowMap};
use bevy::pbr::{ExtractedDirectionalLight, ExtractedPointLight, ScreenSpaceAmbientOcclusion};
use bevy::prelude::*;
use bevy::render::camera::{MipBias, TemporalJitter};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::ui::widget::ViewportNode;
use jackdaw_surface::EnvironmentOptOut;
use jackdaw_terrain::render::{DetailSettings, DetailTile};

use crate::viewport::MainViewportCamera;
use crate::viewport_settings::{
    AntiAliasing, DetailDistance, ShadowQuality, ViewportQuality, ViewportSettings,
};

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ShadowCasters>()
        .add_plugins(ExtractResourcePlugin::<ShadowCasters>::default())
        .add_systems(
            Update,
            (
                dress_viewport_cameras,
                (
                    follow_shadow_settings,
                    follow_detail_settings,
                    follow_lod_distance,
                )
                    .run_if(resource_changed::<ViewportSettings>),
                hide_terrain_detail,
            ),
        )
        .add_systems(
            PostUpdate,
            scale_viewport_targets.after(bevy::ui::widget::update_viewport_render_target_size),
        );
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.add_systems(
            Render,
            withhold_shadows
                .after(RenderSystems::ExtractCommands)
                .before(bevy::pbr::prepare_lights),
        );
    }
}

/// Which lights cast shadows in the editor's views.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, ExtractResource)]
pub struct ShadowCasters {
    pub sun: bool,
    /// Point and spot lights.
    pub point: bool,
}

impl Default for ShadowCasters {
    fn default() -> Self {
        Self {
            sun: true,
            point: true,
        }
    }
}

/// The edge smoothing the viewport draws with at FXAA, sensitive enough to
/// leave thin edit lines sharp.
fn viewport_fxaa() -> Fxaa {
    Fxaa {
        edge_threshold: Sensitivity::Medium,
        edge_threshold_min: Sensitivity::Medium,
        ..default()
    }
}

/// What the viewport leaves out of the scene's environment.
pub fn environment_opt_out(settings: &ViewportSettings) -> EnvironmentOptOut {
    let quality = settings.quality;
    EnvironmentOptOut {
        fog: !quality.fog,
        ambient: !settings.sky_reflection,
        post_processing: !quality.post_processing,
        bloom: !quality.bloom,
        antialiasing: true,
    }
}

/// Prepasses a viewport camera was given for an effect, taken off with it.
#[derive(Component, Default)]
struct EffectPrepasses {
    normal: bool,
    motion_vectors: bool,
}

type ViewportCameraParts = (
    Entity,
    Option<&'static EnvironmentOptOut>,
    Has<Fxaa>,
    Has<Smaa>,
    Has<TemporalAntiAliasing>,
    Has<ScreenSpaceAmbientOcclusion>,
    Has<NormalPrepass>,
    Has<MotionVectorPrepass>,
    Option<&'static EffectPrepasses>,
);

/// Give each viewport camera the effects and edge smoothing the settings ask
/// for, writing only what differs.
fn dress_viewport_cameras(
    mut commands: Commands,
    settings: Res<ViewportSettings>,
    cameras: Query<ViewportCameraParts, With<MainViewportCamera>>,
) {
    let quality = settings.quality;
    let opt_out = environment_opt_out(&settings);
    for (entity, current, fxaa, smaa, taa, ssao, normal, motion, added) in &cameras {
        let mut camera = commands.entity(entity);
        if current != Some(&opt_out) {
            camera.insert(opt_out);
        }
        let mut added_prepasses = EffectPrepasses {
            normal: added.is_some_and(|added| added.normal),
            motion_vectors: added.is_some_and(|added| added.motion_vectors),
        };
        let wants_taa = quality.anti_aliasing == AntiAliasing::Taa;
        let wants_fxaa = quality.anti_aliasing == AntiAliasing::Fxaa;
        if wants_fxaa != fxaa {
            if wants_fxaa {
                camera.insert(viewport_fxaa());
            } else {
                camera.remove::<Fxaa>();
            }
        }
        if smaa {
            camera.remove::<Smaa>();
        }
        if wants_taa && !taa {
            added_prepasses.motion_vectors |= !motion;
            camera.insert(TemporalAntiAliasing::default());
        } else if !wants_taa && taa {
            camera.remove::<(TemporalAntiAliasing, TemporalJitter, MipBias)>();
            if added_prepasses.motion_vectors {
                camera.remove::<MotionVectorPrepass>();
                added_prepasses.motion_vectors = false;
            }
        }
        if quality.ambient_occlusion && !ssao {
            added_prepasses.normal |= !normal;
            camera.insert(ScreenSpaceAmbientOcclusion::default());
        } else if !quality.ambient_occlusion && ssao {
            camera.remove::<ScreenSpaceAmbientOcclusion>();
            if added_prepasses.normal {
                camera.remove::<NormalPrepass>();
                added_prepasses.normal = false;
            }
        }
        let was = (
            added.is_some_and(|added| added.normal),
            added.is_some_and(|added| added.motion_vectors),
        );
        if was != (added_prepasses.normal, added_prepasses.motion_vectors) {
            camera.insert(added_prepasses);
        }
    }
}

/// The sun's shadow map edge, in texels, at each shadow quality.
pub fn sun_shadow_map_size(quality: ShadowQuality) -> usize {
    match quality {
        ShadowQuality::Low => 512,
        ShadowQuality::Medium => 1024,
        ShadowQuality::High => DirectionalLightShadowMap::default().size,
    }
}

/// A point or spot light's shadow map edge, in texels, at each shadow quality.
pub fn point_shadow_map_size(quality: ShadowQuality) -> usize {
    match quality {
        ShadowQuality::Low => 256,
        ShadowQuality::Medium => 512,
        ShadowQuality::High => PointLightShadowMap::default().size,
    }
}

fn follow_shadow_settings(
    settings: Res<ViewportSettings>,
    mut casters: ResMut<ShadowCasters>,
    mut sun_map: ResMut<DirectionalLightShadowMap>,
    mut point_map: ResMut<PointLightShadowMap>,
) {
    let quality = settings.quality;
    let wanted = ShadowCasters {
        sun: quality.sun_shadows,
        point: quality.point_shadows,
    };
    if *casters != wanted {
        *casters = wanted;
    }
    let sun = sun_shadow_map_size(quality.shadow_quality);
    if sun_map.size != sun {
        sun_map.size = sun;
    }
    let point = point_shadow_map_size(quality.shadow_quality);
    if point_map.size != point {
        point_map.size = point;
    }
}

/// How far terrain detail reaches, as a share of what each layer declares.
pub fn detail_cull_scale(distance: DetailDistance) -> f32 {
    match distance {
        DetailDistance::Near => 0.4,
        DetailDistance::Medium => 0.7,
        DetailDistance::Far => 1.0,
    }
}

/// How far each finer level of detail holds, as a share of its own distance.
pub fn lod_distance_bias(distance: DetailDistance) -> f32 {
    match distance {
        DetailDistance::Near => 0.5,
        DetailDistance::Medium => 0.75,
        DetailDistance::Far => 1.0,
    }
}

fn follow_detail_settings(settings: Res<ViewportSettings>, detail: Option<ResMut<DetailSettings>>) {
    let Some(mut detail) = detail else {
        return;
    };
    let cull_scale = detail_cull_scale(settings.quality.detail_distance);
    if detail.cull_scale != cull_scale {
        detail.cull_scale = cull_scale;
    }
}

fn follow_lod_distance(
    settings: Res<ViewportSettings>,
    lod: Option<ResMut<jackdaw_runtime::LodQuality>>,
) {
    let Some(mut lod) = lod else {
        return;
    };
    let bias = lod_distance_bias(settings.quality.detail_distance);
    if lod.bias != bias {
        lod.bias = bias;
    }
}

/// Hide the terrain's detail tiles while terrain detail is off. The tiles are
/// streamed by the editor and never saved.
fn hide_terrain_detail(
    settings: Res<ViewportSettings>,
    mut tiles: Query<&mut Visibility, With<DetailTile>>,
) {
    let wanted = if settings.terrain_detail {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut visibility in &mut tiles {
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

/// The size a viewport's render target is drawn at: its node's size, scaled.
pub fn scaled_target_size(node_size: Vec2, quality: &ViewportQuality) -> UVec2 {
    let scale = quality.render_scale as f32 / 100.0;
    (node_size * scale).round().as_uvec2().max(UVec2::ONE)
}

/// Draw each viewport at the render scale, its image stretched over the node.
/// Runs after Bevy sizes the target to the node, every frame, because Bevy
/// sizes it again whenever the node changes.
fn scale_viewport_targets(
    settings: Res<ViewportSettings>,
    viewports: Query<(&ViewportNode, &ComputedNode)>,
    cameras: Query<&bevy::camera::RenderTarget, With<MainViewportCamera>>,
    mut images: ResMut<Assets<Image>>,
) {
    for (viewport, node) in &viewports {
        let Some(target) = viewport
            .camera
            .and_then(|camera| cameras.get(camera).ok())
            .and_then(|target| target.as_image())
        else {
            continue;
        };
        let wanted = scaled_target_size(node.size(), &settings.quality);
        let Some(current) = images.get(target).map(Image::size) else {
            continue;
        };
        if current != wanted
            && let Some(mut image) = images.get_mut(target)
        {
            image.resize(wanted.to_extents());
        }
    }
}

/// On a light in the render world whose shadow maps the editor turned off.
#[derive(Component)]
struct ShadowsWithheld;

/// Turn the shadow maps of hidden casters off on the extracted lights, and
/// back on once they show again. Lights are extracted only when they change,
/// so a light keeps a mark while its shadows are withheld; a light extracted
/// again carries the scene's own setting and loses the mark.
fn withhold_shadows(
    mut commands: Commands,
    casters: Option<Res<ShadowCasters>>,
    mut points: Query<
        (Entity, &mut ExtractedPointLight, Has<ShadowsWithheld>),
        Without<ExtractedDirectionalLight>,
    >,
    mut suns: Query<
        (Entity, &mut ExtractedDirectionalLight, Has<ShadowsWithheld>),
        Without<ExtractedPointLight>,
    >,
) {
    let casters = casters.map(|casters| *casters).unwrap_or_default();
    for (entity, mut light, withheld) in &mut points {
        let extracted_again = light.is_changed();
        let mut shadows = light.shadow_maps_enabled;
        let mark = withhold(&mut shadows, withheld && !extracted_again, casters.point);
        if shadows != light.shadow_maps_enabled {
            light.shadow_maps_enabled = shadows;
        }
        mark_withheld(&mut commands, entity, withheld, mark);
    }
    for (entity, mut light, withheld) in &mut suns {
        let extracted_again = light.is_changed();
        let mut shadows = light.shadow_maps_enabled;
        let mark = withhold(&mut shadows, withheld && !extracted_again, casters.sun);
        if shadows != light.shadow_maps_enabled {
            light.shadow_maps_enabled = shadows;
        }
        mark_withheld(&mut commands, entity, withheld, mark);
    }
}

/// Settle one light's shadow maps: off while its kind is hidden, and back on
/// when shown if they were on before. Returns whether they are withheld.
fn withhold(shadows: &mut bool, withheld: bool, show: bool) -> bool {
    match (show, withheld) {
        (false, _) if *shadows => {
            *shadows = false;
            true
        }
        (false, withheld) => withheld,
        (true, true) => {
            *shadows = true;
            false
        }
        (true, false) => false,
    }
}

fn mark_withheld(commands: &mut Commands, entity: Entity, marked: bool, withheld: bool) {
    if withheld && !marked {
        commands.entity(entity).insert(ShadowsWithheld);
    } else if !withheld && marked {
        commands.entity(entity).remove::<ShadowsWithheld>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_a_casting_light_turns_its_shadows_off_and_showing_it_restores_them() {
        let mut shadows = true;
        assert!(withhold(&mut shadows, false, false));
        assert!(!shadows);
        assert!(!withhold(&mut shadows, true, true));
        assert!(shadows);
    }

    #[test]
    fn a_light_without_shadows_stays_without_them_when_shown() {
        let mut shadows = false;
        assert!(!withhold(&mut shadows, false, false));
        assert!(!withhold(&mut shadows, false, true));
        assert!(!shadows);
    }

    #[test]
    fn a_withheld_light_stays_withheld_while_hidden() {
        let mut shadows = false;
        assert!(withhold(&mut shadows, true, false));
        assert!(!shadows);
    }

    #[test]
    fn the_render_scale_sizes_the_target_from_the_node() {
        let mut quality = ViewportQuality {
            render_scale: 50,
            ..default()
        };
        assert_eq!(
            scaled_target_size(Vec2::new(941.0, 711.0), &quality),
            UVec2::new(471, 356)
        );
        quality.render_scale = 100;
        assert_eq!(
            scaled_target_size(Vec2::new(941.0, 711.0), &quality),
            UVec2::new(941, 711)
        );
        assert_eq!(scaled_target_size(Vec2::ZERO, &quality), UVec2::ONE);
    }
}
