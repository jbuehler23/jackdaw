//! Despawns the shadow views of point and spot lights that are no longer drawn.

use bevy::pbr::{ExtractedPointLight, PointAndSpotLightViewEntities};
use bevy::prelude::*;
use bevy::render::RenderApp;

/// Despawns a point or spot light's shadow views when the light stops being extracted.
///
/// Bevy keeps the cube faces of a point light, and the view of a spot light, after the light
/// leaves every camera's view, and each of those views still runs the 3D render schedule every
/// frame. The views are made again if the light comes back into view.
pub struct ShadowViewCleanupPlugin;

impl Plugin for ShadowViewCleanupPlugin {
    fn build(&self, app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_observer(despawn_shadow_views);
        }
    }
}

fn despawn_shadow_views(
    remove: On<Remove, ExtractedPointLight>,
    views: Query<&PointAndSpotLightViewEntities>,
    mut commands: Commands,
) {
    let Ok(views) = views.get(remove.entity) else {
        return;
    };
    for &view in views.iter() {
        if let Ok(mut view) = commands.get_entity(view) {
            view.try_despawn();
        }
    }
    if let Ok(mut light) = commands.get_entity(remove.entity) {
        light.try_remove::<PointAndSpotLightViewEntities>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extracted_point_light() -> ExtractedPointLight {
        ExtractedPointLight {
            color: LinearRgba::WHITE,
            intensity: 1000.0,
            range: 10.0,
            radius: 0.0,
            transform: GlobalTransform::IDENTITY,
            shadow_maps_enabled: true,
            contact_shadows_enabled: false,
            shadow_depth_bias: 0.0,
            shadow_normal_bias: 0.0,
            shadow_map_near_z: 0.1,
            spot_light_angles: None,
            volumetric: false,
            soft_shadows_enabled: false,
            affects_lightmapped_mesh_diffuse: true,
        }
    }

    fn light_with_faces(world: &mut World) -> (Entity, Vec<Entity>) {
        let faces: Vec<Entity> = (0..6).map(|_| world.spawn_empty().id()).collect();
        let light = world.spawn(extracted_point_light()).id();
        world
            .get_mut::<PointAndSpotLightViewEntities>(light)
            .expect("an extracted light tracks its views")
            .extend(faces.iter().copied());
        (light, faces)
    }

    #[test]
    fn shadow_views_go_when_the_light_is_no_longer_extracted() {
        let mut world = World::new();
        world.add_observer(despawn_shadow_views);
        let (light, faces) = light_with_faces(&mut world);

        world.entity_mut(light).remove::<ExtractedPointLight>();
        world.flush();

        assert!(faces.iter().all(|&face| world.get_entity(face).is_err()));
        assert!(
            !world
                .entity(light)
                .contains::<PointAndSpotLightViewEntities>()
        );
    }

    #[test]
    fn shadow_views_stay_while_the_light_is_extracted_again() {
        let mut world = World::new();
        world.add_observer(despawn_shadow_views);
        let (light, faces) = light_with_faces(&mut world);

        world.entity_mut(light).insert(extracted_point_light());
        world.flush();

        assert!(faces.iter().all(|&face| world.get_entity(face).is_ok()));
        assert_eq!(
            world
                .get::<PointAndSpotLightViewEntities>(light)
                .map(|views| views.len()),
            Some(6)
        );
    }

    #[test]
    fn shadow_views_go_with_a_despawned_light() {
        let mut world = World::new();
        world.add_observer(despawn_shadow_views);
        let (light, faces) = light_with_faces(&mut world);

        world.despawn(light);
        world.flush();

        assert!(faces.iter().all(|&face| world.get_entity(face).is_err()));
    }
}
