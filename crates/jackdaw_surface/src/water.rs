//! A standard material that draws a lake, a river or a sea.
//!
//! [`Water`] extends [`StandardMaterial`] with what a flat mesh needs to read
//! as water: a surface that moves, a colour that darkens with how deep the bed
//! behind it lies, foam along the shore, caustics in the shallows and a wave
//! that lifts the vertices. It draws alpha-blended and casts no shadow, so the
//! bed under it stays lit.
//!
//! How deep the water is at a fragment comes from the scene depth behind the
//! surface, which is read from the camera's depth prepass. [`WaterPlugin`] puts
//! a [`DepthPrepass`] on the 3D cameras once a scene holds any water, since
//! without it there is nothing to compare the surface against: the shader then
//! falls back to full depth, which is the deep colour with no shore fade and no
//! foam.
//!
//! The scene's colour behind the surface is not reachable from a material
//! extension - the view's transmission texture is only filled for materials in
//! the transmissive phase, which an alpha-blended material never enters - so
//! [`Water::refraction_strength`] bends where the depth behind is read rather
//! than the image behind. The shallows and the foam wobble with the ripples;
//! the bed itself is not displaced.
//!
//! The wave, the depth blend and the foam are computed here as well as in the
//! shader, which is what the tests check: the two have to agree or a dial reads
//! one way in the inspector and another on screen.

use bevy::asset::embedded_asset;
use bevy::color::ColorToComponents;
use bevy::core_pipeline::prepass::DepthPrepass;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{AsBindGroup, AsBindGroupShaderType, ShaderType};
use bevy::render::texture::GpuImage;
use bevy::shader::ShaderRef;
use jackdaw_scene_types::{SceneWind, Wind};

const SHADER_PATH: &str = "embedded://jackdaw_surface/shaders/water.wgsl";

/// The material an entity wears to draw as water.
pub type WaterMaterial = ExtendedMaterial<StandardMaterial, Water>;

const TAU: f32 = std::f32::consts::TAU;
/// How much of the wave the long octave carries, the short one taking the rest.
const LONG_WAVE_SHARE: f32 = 0.65;
/// How many times shorter the second octave is than the first.
const SHORT_WAVE_RATE: f32 = 2.3;

/// A standard material that draws as water: a moving surface, a colour that
/// deepens with the bed behind it, foam at the shore and a wave in its
/// vertices.
#[derive(Asset, AsBindGroup, Reflect, Clone, Debug)]
#[reflect(Default, Clone)]
#[uniform(100, WaterUniform)]
pub struct Water {
    /// The colour where the bed is right behind the surface. Its alpha is how
    /// much of the bed the shallows hide.
    pub shallow_color: Color,
    /// The colour once the bed is [`Self::depth_distance`] behind the surface.
    pub deep_color: Color,
    /// How far behind the surface the bed has to lie for the water to reach
    /// [`Self::deep_color`], in world units.
    pub depth_distance: f32,
    /// How far behind the surface the bed has to lie for the water to reach
    /// full opacity, in world units. Small is a hard waterline, large a shore
    /// the water fades into.
    pub edge_fade: f32,
    /// World units one tile of the ripple pattern spans.
    pub normal_scale: f32,
    /// How far the ripples turn the shaded normal, `0..1`. At 0 the surface is
    /// a mirror-flat sheet.
    pub normal_strength: f32,
    /// How fast the ripples drift, in tiles per second.
    pub normal_speed: f32,
    /// Which way they drift, as a yaw in degrees about +Y from +X. The second
    /// of the two scrolled samples crosses this one, so the pattern never
    /// reads as a single sheet sliding.
    pub normal_direction: f32,
    /// How far the ripples bend where the depth behind the surface is read,
    /// `0..1`, which wobbles the shallows and the foam.
    pub refraction_strength: f32,
    /// World units one tile of the foam pattern spans.
    pub foam_scale: f32,
    /// How fast the foam drifts, in tiles per second.
    pub foam_speed: f32,
    /// How far from the bed foam gathers, in world units. Beyond this there is
    /// none.
    pub foam_distance: f32,
    /// How much foam the shoreline takes at its strongest, `0..1`.
    pub foam_strength: f32,
    /// Adds the light the surface focuses onto the bed. Black adds nothing.
    pub caustics_color: Color,
    /// World units one tile of the caustics pattern spans.
    pub caustics_scale: f32,
    /// How fast the caustics travel, in tiles per second.
    pub caustics_speed: f32,
    /// How far the wave lifts a vertex from its rest height, in world units.
    pub wave_height: f32,
    /// World units the long octave of the wave spans.
    pub wave_scale: f32,
    /// How fast the wave travels, in wavelengths per second.
    pub wave_speed: f32,
    /// How glassy the surface is, `0..1`. The material's roughness is what is
    /// left of this.
    pub smoothness: f32,
    /// How far the scene's wind speeds the ripples and the wave up, as a
    /// multiple of its strength. At 0 the water moves the same in any wind.
    pub wind_response: f32,
    /// A tangent-space normal map, sampled twice and averaged. It is scrolled
    /// past its own edges, so its image wants a repeating address mode.
    /// Without one the ripples come from a sine pattern.
    #[texture(101)]
    #[sampler(102)]
    pub normal_map_texture: Option<Handle<Image>>,
    /// Where the foam is, read from the red channel. Without one the foam is a
    /// banded pattern.
    #[texture(103)]
    pub foam_mask: Option<Handle<Image>>,
    /// The wind the scene is blowing by, written by [`WaterPlugin`] from
    /// [`SceneWind`] rather than authored, so it never reaches the file.
    #[reflect(ignore)]
    pub wind: Wind,
}

impl Default for Water {
    fn default() -> Self {
        Self {
            shallow_color: Color::linear_rgba(0.24, 0.62, 0.58, 0.45),
            deep_color: Color::linear_rgba(0.01, 0.10, 0.17, 0.95),
            depth_distance: 4.0,
            edge_fade: 0.4,
            normal_scale: 6.0,
            normal_strength: 0.35,
            normal_speed: 0.05,
            normal_direction: 35.0,
            refraction_strength: 0.3,
            foam_scale: 2.5,
            foam_speed: 0.08,
            foam_distance: 0.6,
            foam_strength: 0.8,
            caustics_color: Color::linear_rgb(0.05, 0.08, 0.06),
            caustics_scale: 2.0,
            caustics_speed: 0.04,
            wave_height: 0.05,
            wave_scale: 6.0,
            wave_speed: 0.15,
            smoothness: 0.92,
            wind_response: 0.0,
            normal_map_texture: None,
            foam_mask: None,
            wind: Wind::STILL,
        }
    }
}

impl Water {
    /// How much faster the surface moves in the wind the scene is blowing by.
    pub fn wind_gain(&self) -> f32 {
        1.0 + self.wind_response * self.wind.strength
    }

    /// How far the wave lifts a vertex standing at `world`, in world units.
    ///
    /// Two sine octaves crossing each other, so a wide sheet never reads as one
    /// ridge marching across it. The shader computes this same expression.
    pub fn wave_offset(&self, world: Vec3, time: f32) -> f32 {
        if self.wave_height == 0.0 {
            return 0.0;
        }
        let travelling = time * self.wave_speed * self.wind_gain();
        let on_the_plane = Vec2::new(world.x, world.z) / self.wave_scale.max(1e-4);
        let long = (on_the_plane.dot(Vec2::new(0.94, 0.34)) * TAU + travelling * TAU).sin();
        let short = (on_the_plane.dot(Vec2::new(-0.37, 0.93)) * TAU * SHORT_WAVE_RATE
            - travelling * TAU)
            .sin();
        self.wave_height * (long * LONG_WAVE_SHARE + short * (1.0 - LONG_WAVE_SHARE))
    }

    /// How far toward [`Self::deep_color`] a fragment is, `0..1`, with the bed
    /// `depth` behind the surface. The shader computes this same expression.
    pub fn depth_blend(&self, depth: f32) -> f32 {
        (depth / self.depth_distance.max(1e-4)).clamp(0.0, 1.0)
    }

    /// The colour of water with the bed `depth` behind its surface.
    pub fn color_at_depth(&self, depth: f32) -> LinearRgba {
        let shallow = LinearRgba::from(self.shallow_color).to_vec4();
        let deep = LinearRgba::from(self.deep_color).to_vec4();
        LinearRgba::from_vec4(shallow.lerp(deep, self.depth_blend(depth)))
    }

    /// How much foam gathers where the bed is `depth` behind the surface,
    /// `0..1`. The shader computes this same expression and multiplies the
    /// foam pattern into it.
    pub fn foam_weight(&self, depth: f32) -> f32 {
        let reached = (depth / self.foam_distance.max(1e-4)).clamp(0.0, 1.0);
        self.foam_strength * (1.0 - reached)
    }

    fn bound_maps(&self) -> u32 {
        let mut bound = 0;
        if self.normal_map_texture.is_some() {
            bound |= flags::NORMAL_MAP;
        }
        if self.foam_mask.is_some() {
            bound |= flags::FOAM_MASK;
        }
        bound
    }
}

/// The water half of the material's bind group, as the shader reads it.
///
/// The two speeds arrive already scaled by [`Water::wind_gain`], so the shader
/// has no wind of its own to read.
#[derive(Clone, Default, ShaderType)]
pub struct WaterUniform {
    pub shallow_color: Vec4,
    pub deep_color: Vec4,
    pub caustics_color: Vec4,
    pub normal_heading: Vec2,
    pub depth_distance: f32,
    pub edge_fade: f32,
    pub normal_scale: f32,
    pub normal_strength: f32,
    pub normal_speed: f32,
    pub refraction_strength: f32,
    pub foam_scale: f32,
    pub foam_speed: f32,
    pub foam_distance: f32,
    pub foam_strength: f32,
    pub caustics_scale: f32,
    pub caustics_speed: f32,
    pub wave_height: f32,
    pub wave_scale: f32,
    pub wave_speed: f32,
    pub smoothness: f32,
    pub flags: u32,
}

/// Bits of [`WaterUniform::flags`], telling the shader which maps are bound. An
/// unbound map falls back to a pattern the shader draws itself rather than to
/// the blank texture, which would read as a flat sheet with no foam.
pub mod flags {
    pub const NORMAL_MAP: u32 = 1;
    pub const FOAM_MASK: u32 = 2;
}

impl AsBindGroupShaderType<WaterUniform> for Water {
    fn as_bind_group_shader_type(&self, _images: &RenderAssets<GpuImage>) -> WaterUniform {
        let radians = self.normal_direction.to_radians();
        WaterUniform {
            shallow_color: LinearRgba::from(self.shallow_color).to_vec4(),
            deep_color: LinearRgba::from(self.deep_color).to_vec4(),
            caustics_color: LinearRgba::from(self.caustics_color).to_vec4(),
            normal_heading: Vec2::new(radians.cos(), radians.sin()),
            depth_distance: self.depth_distance,
            edge_fade: self.edge_fade,
            normal_scale: self.normal_scale,
            normal_strength: self.normal_strength,
            normal_speed: self.normal_speed * self.wind_gain(),
            refraction_strength: self.refraction_strength,
            foam_scale: self.foam_scale,
            foam_speed: self.foam_speed,
            foam_distance: self.foam_distance,
            foam_strength: self.foam_strength,
            caustics_scale: self.caustics_scale,
            caustics_speed: self.caustics_speed,
            wave_height: self.wave_height,
            wave_scale: self.wave_scale,
            wave_speed: self.wave_speed * self.wind_gain(),
            smoothness: self.smoothness,
            flags: self.bound_maps(),
        }
    }
}

impl MaterialExtension for Water {
    fn vertex_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn alpha_mode() -> Option<AlphaMode> {
        Some(AlphaMode::Blend)
    }

    fn enable_shadows() -> bool {
        false
    }
}

/// Registers the water material, its shader and its reflected type, so a scene
/// naming one renders it.
pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/water.wgsl");
        app.add_plugins(MaterialPlugin::<WaterMaterial>::default())
            .init_resource::<SceneWind>()
            .register_type::<Water>()
            .register_asset_reflect::<WaterMaterial>()
            .register_type_data::<WaterMaterial, bevy::reflect::std_traits::ReflectDefault>()
            .add_systems(
                PostUpdate,
                (
                    give_the_cameras_the_depth_water_reads,
                    move_the_water_by_the_scenes_wind,
                ),
            );
    }
}

/// Put a depth prepass on the 3D cameras once a scene holds water.
///
/// The depth behind the surface is what colours the water and where its foam
/// gathers, and nothing else in the editor asks for a prepass. It is left in
/// place afterwards: taking it off again would respecialize every material in
/// the scene each time the last water asset came and went.
fn give_the_cameras_the_depth_water_reads(
    mut commands: Commands,
    water: Res<Assets<WaterMaterial>>,
    cameras: Query<Entity, (With<Camera3d>, Without<DepthPrepass>)>,
) {
    if water.iter().next().is_none() {
        return;
    }
    for camera in &cameras {
        commands.entity(camera).insert(DepthPrepass);
    }
}

/// Hand every water material the wind the scene is blowing by, so a wind
/// authored once carries every surface wearing one.
fn move_the_water_by_the_scenes_wind(
    blowing: Res<SceneWind>,
    mut materials: ResMut<Assets<WaterMaterial>>,
) {
    let ids: Vec<AssetId<WaterMaterial>> = materials
        .iter()
        .filter(|(_, material)| material.extension.wind != blowing.0)
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        if let Some(mut material) = materials.get_mut(id) {
            material.extension.wind = blowing.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader source, for the binding checks below.
    ///
    /// It cannot be compiled here: it is naga-oil input, not WGSL. The checks
    /// are textual, and catch binding numbers drifting between the
    /// `AsBindGroup` derive and the shader that reads them.
    const SHADER_SOURCE: &str = include_str!("shaders/water.wgsl");

    #[test]
    fn every_material_binding_is_declared_in_the_shader_at_its_derive_index() {
        for (binding, declaration) in [
            (100, "var<uniform> water: WaterUniform"),
            (101, "var normal_map_texture: texture_2d<f32>"),
            (102, "var water_sampler: sampler"),
            (103, "var foam_mask: texture_2d<f32>"),
        ] {
            let expected =
                format!("@group(#{{MATERIAL_BIND_GROUP}}) @binding({binding}) {declaration};");
            assert!(
                SHADER_SOURCE.contains(&expected),
                "the AsBindGroup derive binds {binding} as `{declaration}`, \
                 but the shader does not declare it that way"
            );
        }
    }

    /// The uniform is one buffer built from [`WaterUniform`]'s fields in
    /// declaration order, so the WGSL struct has to list them in the same order
    /// or every field reads its neighbour's bytes.
    #[test]
    fn the_uniform_struct_matches_the_rust_field_order() {
        let start = SHADER_SOURCE
            .find("struct WaterUniform {")
            .expect("WaterUniform is declared");
        let body = &SHADER_SOURCE[start..];
        let end = body.find('}').expect("WaterUniform is closed");
        let body = &body[..end];

        let mut at = 0usize;
        for field in [
            "shallow_color: vec4<f32>",
            "deep_color: vec4<f32>",
            "caustics_color: vec4<f32>",
            "normal_heading: vec2<f32>",
            "depth_distance: f32",
            "edge_fade: f32",
            "normal_scale: f32",
            "normal_strength: f32",
            "normal_speed: f32",
            "refraction_strength: f32",
            "foam_scale: f32",
            "foam_speed: f32",
            "foam_distance: f32",
            "foam_strength: f32",
            "caustics_scale: f32",
            "caustics_speed: f32",
            "wave_height: f32",
            "wave_scale: f32",
            "wave_speed: f32",
            "smoothness: f32",
            "flags: u32",
        ] {
            let found = body[at..].find(field).unwrap_or_else(|| {
                panic!("the shader declares `{field}` after the field above it")
            });
            at += found + field.len();
        }
    }

    #[test]
    fn a_material_reports_which_maps_are_bound() {
        assert_eq!(Water::default().bound_maps(), 0);

        let mapped = Water {
            normal_map_texture: Some(Handle::default()),
            foam_mask: Some(Handle::default()),
            ..default()
        };
        assert_eq!(mapped.bound_maps(), flags::NORMAL_MAP | flags::FOAM_MASK,);
    }

    #[test]
    fn a_flat_surface_rides_no_wave_and_a_raised_one_rides_a_taller_one() {
        let still = Water {
            wave_height: 0.0,
            ..default()
        };
        for at in [
            Vec3::ZERO,
            Vec3::new(3.0, 0.0, 7.0),
            Vec3::new(-11.0, 0.0, 2.0),
        ] {
            for time in [0.0, 0.7, 4.3] {
                assert_eq!(still.wave_offset(at, time), 0.0);
            }
        }

        let at = Vec3::new(3.0, 0.0, 7.0);
        let low = Water {
            wave_height: 0.2,
            ..default()
        };
        let high = Water {
            wave_height: 0.8,
            ..low.clone()
        };
        assert!(
            low.wave_offset(at, 1.0).abs() > 0.0,
            "a wave of its own height lifts the surface",
        );
        assert!(
            (high.wave_offset(at, 1.0) - low.wave_offset(at, 1.0) * 4.0).abs() < 1e-5,
            "and four times the height lifts it four times as far",
        );
    }

    #[test]
    fn the_wave_travels_over_the_surface() {
        let water = Water {
            wave_height: 0.3,
            ..default()
        };
        let at = Vec3::new(2.0, 0.0, 1.0);

        assert!(
            (water.wave_offset(at, 0.0) - water.wave_offset(at, 1.7)).abs() > 1e-3,
            "the same point rides differently a moment later",
        );
    }

    #[test]
    fn the_wind_carries_the_wave_faster_than_still_air() {
        let mut water = Water {
            wave_height: 0.3,
            wind_response: 1.0,
            ..default()
        };
        let still = water.wave_offset(Vec3::new(2.0, 0.0, 1.0), 1.0);
        water.wind = Wind {
            strength: 2.0,
            ..Wind::default()
        };

        assert_eq!(water.wind_gain(), 3.0);
        assert!(
            (water.wave_offset(Vec3::new(2.0, 0.0, 1.0), 1.0) - still).abs() > 1e-3,
            "the wind has carried the wave on",
        );
    }

    #[test]
    fn the_shallows_take_the_shallow_colour_and_the_deep_the_deep_one() {
        let water = Water {
            depth_distance: 4.0,
            ..default()
        };

        assert_eq!(water.depth_blend(0.0), 0.0);
        assert_eq!(water.depth_blend(2.0), 0.5);
        assert_eq!(water.depth_blend(4.0), 1.0);
        assert_eq!(water.depth_blend(40.0), 1.0);
        assert_eq!(
            water.color_at_depth(0.0),
            LinearRgba::from(water.shallow_color)
        );
        assert_eq!(
            water.color_at_depth(9.0),
            LinearRgba::from(water.deep_color)
        );
    }

    #[test]
    fn foam_gathers_at_the_shore_and_stops_beyond_its_reach() {
        let water = Water {
            foam_distance: 0.5,
            foam_strength: 1.0,
            ..default()
        };

        assert_eq!(water.foam_weight(0.0), 1.0);
        assert_eq!(water.foam_weight(0.25), 0.5);
        assert_eq!(water.foam_weight(0.5), 0.0);
        assert_eq!(water.foam_weight(3.0), 0.0);
    }
}
