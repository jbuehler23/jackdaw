//! Reflection probes: a box of the scene that reflects a baked cubemap.
//!
//! A [`ReflectionProbe`] names a Radiance HDR image of six faces stacked top to
//! bottom. [`ReflectionProbePlugin`] loads it, turns it into a cubemap, and puts
//! a hidden child under the probe carrying Bevy's [`LightProbe`], which takes
//! the probe's box from its scale, filtered by
//! [`GeneratedEnvironmentMapLight`]. Surfaces inside the box then take their
//! reflections and ambient light from the bake instead of the camera's
//! environment map.

use bevy::asset::RenderAssetUsages;
use bevy::light::{GeneratedEnvironmentMapLight, LightProbe};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use jackdaw_scene_types::{EditorHidden, NavmeshExclude, ReflectionProbe};

/// Loads each probe's bake and keeps the light probe that reflects it.
pub struct ReflectionProbePlugin;

impl Plugin for ReflectionProbePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ProbeMaps>().add_systems(
            PostUpdate,
            (build_probe_cubemaps, keep_probe_volumes)
                .chain()
                .run_if(resource_exists::<Assets<Image>>),
        );
    }
}

/// The hidden child that carries a probe's [`LightProbe`].
#[derive(Component)]
pub struct ProbeVolume {
    /// The bake the volume reflects, so a new bake or a new path rebuilds it.
    built_from: (String, u32),
}

/// A probe's bake as loaded, and the cubemap made from it.
struct ProbeMap {
    stacked: Handle<Image>,
    cubemap: Option<Handle<Image>>,
    /// Counts each new cubemap made from the image, so a reload rebuilds the volume.
    generation: u32,
}

/// Every bake a probe in the world names, by asset path.
#[derive(Resource, Default)]
struct ProbeMaps(HashMap<String, ProbeMap>);

/// Turn a stack of six square faces, top to bottom, into a cubemap. `None`
/// when the image is not six squares tall or holds a format other than the
/// 32-bit float a Radiance HDR file loads as.
pub fn cubemap_from_stacked_faces(stacked: &Image) -> Option<Image> {
    let size = stacked.texture_descriptor.size;
    let face = size.width;
    if face == 0 || size.height != face * 6 || !face.is_power_of_two() {
        return None;
    }
    let data = stacked.data.as_ref()?;
    let halves: Vec<u8> = match stacked.texture_descriptor.format {
        TextureFormat::Rgba32Float => data
            .chunks_exact(4)
            .flat_map(|bytes| {
                let value = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                half::f16::from_f32(value).to_le_bytes()
            })
            .collect(),
        TextureFormat::Rgba16Float => data.clone(),
        _ => return None,
    };
    let mut cubemap = Image::new(
        Extent3d {
            width: face,
            height: face,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        halves,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    cubemap.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    Some(cubemap)
}

fn build_probe_cubemaps(
    mut maps: ResMut<ProbeMaps>,
    mut images: ResMut<Assets<Image>>,
    mut events: MessageReader<AssetEvent<Image>>,
) {
    let changed: Vec<AssetId<Image>> = events
        .read()
        .filter_map(|event| match event {
            AssetEvent::LoadedWithDependencies { id } | AssetEvent::Modified { id } => Some(*id),
            _ => None,
        })
        .collect();
    for map in maps.0.values_mut() {
        let stale = map.cubemap.is_none() || changed.contains(&map.stacked.id());
        if !stale {
            continue;
        }
        let Some(cubemap) = images
            .get(&map.stacked)
            .and_then(cubemap_from_stacked_faces)
        else {
            continue;
        };
        map.cubemap = Some(images.add(cubemap));
        map.generation += 1;
    }
}

fn keep_probe_volumes(
    mut commands: Commands,
    assets: Option<Res<AssetServer>>,
    mut maps: ResMut<ProbeMaps>,
    probes: Query<(
        Entity,
        Ref<ReflectionProbe>,
        Ref<GlobalTransform>,
        Option<&Children>,
    )>,
    mut volumes: Query<(&ProbeVolume, &mut LightProbe)>,
) {
    let mut named: Vec<String> = Vec::new();
    for (entity, probe, placed, children) in &probes {
        let falloff = probe.falloff(placed.to_scale_rotation_translation().0);
        let current: Vec<Entity> = children
            .into_iter()
            .flatten()
            .copied()
            .filter(|child| volumes.contains(*child))
            .collect();
        if probe.baked.is_empty() {
            for volume in current {
                commands.entity(volume).despawn();
            }
            continue;
        }
        named.push(probe.baked.clone());
        let map = match (maps.0.get(&probe.baked), assets.as_deref()) {
            (Some(map), _) => map,
            (None, Some(assets)) => {
                maps.0.insert(
                    probe.baked.clone(),
                    ProbeMap {
                        stacked: assets.load(probe.baked.clone()),
                        cubemap: None,
                        generation: 0,
                    },
                );
                continue;
            }
            (None, None) => continue,
        };
        let Some(cubemap) = map.cubemap.clone() else {
            continue;
        };
        let wanted = (probe.baked.clone(), map.generation);
        let up_to_date = current.len() == 1
            && !probe.is_changed()
            && volumes
                .get(current[0])
                .is_ok_and(|(volume, _)| volume.built_from == wanted);
        if up_to_date {
            if placed.is_changed()
                && let Ok((_, mut light_probe)) = volumes.get_mut(current[0])
            {
                light_probe.falloff = falloff;
            }
            continue;
        }
        for volume in current {
            commands.entity(volume).despawn();
        }
        commands.spawn((
            ChildOf(entity),
            ProbeVolume { built_from: wanted },
            Name::new("Probe Volume"),
            LightProbe { falloff },
            GeneratedEnvironmentMapLight {
                environment_map: cubemap,
                intensity: probe.intensity,
                ..default()
            },
            Transform::IDENTITY,
            EditorHidden,
            NavmeshExclude,
        ));
    }
    maps.0.retain(|path, _| named.contains(path));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stacked(face: u32, format: TextureFormat, texel: impl Fn(u32) -> Vec<u8>) -> Image {
        let data: Vec<u8> = (0..face * face * 6).flat_map(texel).collect();
        Image::new(
            Extent3d {
                width: face,
                height: face * 6,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            format,
            RenderAssetUsages::MAIN_WORLD,
        )
    }

    #[test]
    fn six_stacked_faces_become_a_half_float_cubemap_in_the_same_order() {
        let face = 4;
        let image = stacked(face, TextureFormat::Rgba32Float, |index| {
            let layer = (index / (face * face)) as f32;
            [layer, 0.5, 2.0, 1.0]
                .iter()
                .flat_map(|value: &f32| value.to_le_bytes())
                .collect()
        });

        let cubemap = cubemap_from_stacked_faces(&image).expect("a cubemap");
        assert_eq!(cubemap.texture_descriptor.size.depth_or_array_layers, 6);
        assert_eq!(cubemap.texture_descriptor.size.width, face);
        assert_eq!(
            cubemap.texture_descriptor.format,
            TextureFormat::Rgba16Float
        );
        let data = cubemap.data.as_ref().unwrap();
        let red_of = |layer: u32| {
            let at = (layer * face * face * 8) as usize;
            half::f16::from_le_bytes([data[at], data[at + 1]]).to_f32()
        };
        for layer in 0..6 {
            assert_eq!(red_of(layer), layer as f32);
        }
    }

    #[test]
    fn an_image_that_is_not_six_square_faces_is_refused() {
        let wrong = Image::new_fill(
            Extent3d {
                width: 4,
                height: 20,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0; 16],
            TextureFormat::Rgba32Float,
            RenderAssetUsages::MAIN_WORLD,
        );
        assert!(cubemap_from_stacked_faces(&wrong).is_none());
    }

    fn probe_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Assets<Image>>()
            .add_message::<AssetEvent<Image>>()
            .add_plugins(ReflectionProbePlugin);
        app
    }

    #[test]
    fn a_baked_probe_reflects_through_a_light_probe_the_size_of_its_box() {
        let mut app = probe_app();
        let image = stacked(4, TextureFormat::Rgba32Float, |_| {
            [1.0f32, 1.0, 1.0, 1.0]
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect()
        });
        let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        app.world_mut().resource_mut::<ProbeMaps>().0.insert(
            "scenes/lake.probe.hdr".to_string(),
            ProbeMap {
                stacked: handle,
                cubemap: None,
                generation: 0,
            },
        );
        let probe = app
            .world_mut()
            .spawn((
                ReflectionProbe {
                    blend_distance: 5.0,
                    baked: "scenes/lake.probe.hdr".to_string(),
                    ..default()
                },
                Transform::from_scale(Vec3::new(40.0, 10.0, 20.0)),
                GlobalTransform::from_scale(Vec3::new(40.0, 10.0, 20.0)),
            ))
            .id();
        app.update();
        app.update();

        let mut volumes = app.world_mut().query::<(
            &ChildOf,
            &LightProbe,
            &Transform,
            &GeneratedEnvironmentMapLight,
        )>();
        let (parent, light_probe, transform, generated) = volumes
            .single(app.world())
            .expect("one volume under the probe");
        assert_eq!(parent.parent(), probe);
        assert_eq!(
            *transform,
            Transform::IDENTITY,
            "the box is the probe's own scale"
        );
        assert_eq!(light_probe.falloff, Vec3::new(0.25, 1.0, 0.5));
        let cubemap = app
            .world()
            .resource::<Assets<Image>>()
            .get(&generated.environment_map)
            .expect("the cubemap");
        assert_eq!(cubemap.texture_descriptor.size.depth_or_array_layers, 6);

        app.world_mut()
            .get_mut::<ReflectionProbe>(probe)
            .unwrap()
            .baked
            .clear();
        app.update();
        assert_eq!(
            volumes.iter(app.world()).count(),
            0,
            "an unbaked probe reflects nothing"
        );
    }
}
