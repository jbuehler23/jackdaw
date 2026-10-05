//! Keeps shadow maps for the point and spot lights nearest each camera, as the scene's
//! [`Environment`] allows.

use bevy::camera::visibility::RenderLayers;
use bevy::pbr::ExtractedPointLight;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::view::ExtractedView;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use jackdaw_scene_types::{Environment, LightShadows};

/// Renders shadow maps only for the lights [`LightShadows`] keeps; the others light the scene
/// unshadowed. A scene without an [`Environment`] takes the default limit.
pub struct LightShadowsPlugin;

impl Plugin for LightShadowsPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<SceneLightShadows>()
            .add_systems(ExtractSchedule, extract_light_shadows)
            .add_systems(
                Render,
                drop_shadows_past_the_limit
                    .in_set(RenderSystems::CreateViews)
                    .before(bevy::pbr::prepare_lights),
            );
    }
}

#[derive(Resource, Default)]
struct SceneLightShadows(LightShadows);

fn extract_light_shadows(
    environments: Extract<Query<&Environment>>,
    mut scene: ResMut<SceneLightShadows>,
) {
    let held = environments
        .iter()
        .next()
        .map(|environment| environment.shadows.clone())
        .unwrap_or_default();
    if scene.0 != held {
        scene.0 = held;
    }
}

fn drop_shadows_past_the_limit(
    scene: Res<SceneLightShadows>,
    views: Query<(&ExtractedView, Option<&RenderLayers>), With<ExtractedCamera>>,
    mut lights: Query<(Entity, &mut ExtractedPointLight, Option<&RenderLayers>)>,
) {
    let eyes: Vec<Eye> = views
        .iter()
        .map(|(view, layers)| Eye {
            position: view.world_from_view.translation(),
            layers: layers.cloned().unwrap_or_default(),
        })
        .collect();
    let casters: Vec<Caster> = lights
        .iter()
        .filter(|(_, light, _)| light.shadow_maps_enabled)
        .map(|(entity, light, layers)| Caster {
            entity,
            position: light.transform.translation(),
            range: light.range,
            layers: layers.cloned().unwrap_or_default(),
        })
        .collect();
    let kept = shadowed_lights(&eyes, &casters, &scene.0);
    for (entity, mut light, _) in &mut lights {
        if light.shadow_maps_enabled && !kept.contains(&entity) {
            light.shadow_maps_enabled = false;
        }
    }
}

/// A camera the lights are ranked from.
struct Eye {
    position: Vec3,
    layers: RenderLayers,
}

/// A light that asks for shadow maps.
struct Caster {
    entity: Entity,
    position: Vec3,
    range: f32,
    layers: RenderLayers,
}

/// The lights that keep their shadows: for each camera, up to `max_lights` of the lights it
/// can see whose range reaches within `distance` of it, nearest first.
fn shadowed_lights(eyes: &[Eye], casters: &[Caster], shadows: &LightShadows) -> HashSet<Entity> {
    let mut kept = HashSet::default();
    for eye in eyes {
        let mut near: Vec<(f32, Entity)> = casters
            .iter()
            .filter(|caster| caster.layers.intersects(&eye.layers))
            .map(|caster| {
                let gap = (caster.position.distance(eye.position) - caster.range).max(0.0);
                (gap, caster.entity)
            })
            .filter(|(gap, _)| *gap <= shadows.distance)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        kept.extend(
            near.into_iter()
                .take(shadows.max_lights as usize)
                .map(|(_, entity)| entity),
        );
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eye(x: f32) -> Eye {
        Eye {
            position: Vec3::new(x, 0.0, 0.0),
            layers: RenderLayers::default(),
        }
    }

    fn casters_along_x(world: &mut World, xs: &[f32]) -> Vec<Caster> {
        xs.iter()
            .map(|&x| Caster {
                entity: world.spawn_empty().id(),
                position: Vec3::new(x, 0.0, 0.0),
                range: 5.0,
                layers: RenderLayers::default(),
            })
            .collect()
    }

    fn shadows(max_lights: u32, distance: f32) -> LightShadows {
        LightShadows {
            max_lights,
            distance,
        }
    }

    #[test]
    fn the_nearest_lights_keep_their_shadows_up_to_the_limit() {
        let mut world = World::new();
        let casters = casters_along_x(&mut world, &[40.0, 10.0, 30.0, 20.0, 15.0]);

        let kept = shadowed_lights(&[eye(0.0)], &casters, &shadows(3, 100.0));

        let expected: HashSet<Entity> = [casters[1].entity, casters[4].entity, casters[3].entity]
            .into_iter()
            .collect();
        assert_eq!(kept, expected);
    }

    #[test]
    fn lights_whose_range_ends_past_the_distance_lose_their_shadows() {
        let mut world = World::new();
        let casters = casters_along_x(&mut world, &[20.0, 54.0, 56.0]);

        let kept = shadowed_lights(&[eye(0.0)], &casters, &shadows(8, 50.0));

        assert!(kept.contains(&casters[0].entity));
        assert!(kept.contains(&casters[1].entity), "its range reaches 49 m");
        assert!(
            !kept.contains(&casters[2].entity),
            "its range stops at 51 m"
        );
    }

    #[test]
    fn a_limit_of_zero_shadows_no_light() {
        let mut world = World::new();
        let casters = casters_along_x(&mut world, &[1.0, 2.0]);

        assert!(shadowed_lights(&[eye(0.0)], &casters, &shadows(0, 50.0)).is_empty());
    }

    #[test]
    fn each_camera_keeps_its_own_nearest_lights() {
        let mut world = World::new();
        let casters = casters_along_x(&mut world, &[0.0, 10.0, 90.0, 100.0]);

        let kept = shadowed_lights(&[eye(0.0), eye(100.0)], &casters, &shadows(1, 50.0));

        let expected: HashSet<Entity> =
            [casters[0].entity, casters[3].entity].into_iter().collect();
        assert_eq!(kept, expected);
    }

    #[test]
    fn a_camera_ranks_only_the_lights_on_its_layers() {
        let mut world = World::new();
        let mut casters = casters_along_x(&mut world, &[5.0, 10.0]);
        casters[0].layers = RenderLayers::layer(2);

        let kept = shadowed_lights(&[eye(0.0)], &casters, &shadows(1, 50.0));

        assert_eq!(kept, [casters[1].entity].into_iter().collect());
    }
}
