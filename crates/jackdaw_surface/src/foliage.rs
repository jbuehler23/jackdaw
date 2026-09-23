//! A standard material that blows with the scene's wind and lets light
//! through its leaves.
//!
//! [`Foliage`] extends [`StandardMaterial`] with the four things an imported
//! tree or bush needs to stop looking like cardboard: a cutout, a tint that
//! rises up the mesh, a colour variation read from where the plant stands, and
//! light coming through a leaf from behind it. Its vertices lean with
//! [`Wind`], the whole scene's, so a trunk bends at the top while a leaf card
//! flutters.
//!
//! Every dial defaults to the value that does nothing, so an unedited
//! [`Foliage`] draws exactly what its [`StandardMaterial`] half draws, cut out
//! at [`Foliage::alpha_cutoff`].
//!
//! The vertex offset, the gradient and the variation are computed here as well
//! as in the shader, which is what the tests check: the two have to agree or a
//! dial reads one way in the inspector and another on screen.

use bevy::asset::embedded_asset;
use bevy::color::ColorToComponents;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{AsBindGroup, AsBindGroupShaderType, ShaderType};
use bevy::render::texture::GpuImage;
use bevy::shader::{Shader, ShaderRef};
use jackdaw_scene_types::{SceneWind, Wind};

const SHADER_PATH: &str = "embedded://jackdaw_surface/shaders/foliage.wgsl";
const PREPASS_SHADER_PATH: &str = "embedded://jackdaw_surface/shaders/foliage_prepass.wgsl";

/// The material an entity wears to blow with the wind and pass light.
pub type FoliageMaterial = ExtendedMaterial<StandardMaterial, Foliage>;

/// How far a wind of strength 1 leans a part responding at 1, in world units.
const FOLIAGE_LEAN: f32 = 0.35;
/// How far the high-frequency flutter reaches beside the main lean.
const FLUTTER_LEAN: f32 = 0.06;
/// How many times faster than the wind itself the flutter travels.
const FLUTTER_RATE: f32 = 7.0;

/// A standard material that leans in the scene's wind, tints up its own
/// height, varies by where it stands and passes light through from behind.
#[derive(Asset, AsBindGroup, Reflect, Clone, Debug)]
#[reflect(Default, Clone)]
#[uniform(100, FoliageUniform)]
pub struct Foliage {
    /// Alpha a fragment has to clear to draw, `0..1`.
    pub alpha_cutoff: f32,
    /// Tints the mesh above [`Self::gradient_position`], white by default,
    /// which tints nothing.
    pub gradient_color: Color,
    /// The height in the mesh's own units where the tint begins.
    pub gradient_position: f32,
    /// How many of the mesh's own units the tint takes to reach full.
    pub gradient_falloff: f32,
    /// Whether the tint gathers at the base rather than at the top.
    pub gradient_invert: bool,
    /// Tints a plant by where it stands, white by default.
    pub variation_color: Color,
    /// How much of [`Self::variation_color`] the strongest point takes, `0..1`.
    pub variation_strength: f32,
    /// How many world units the variation pattern spans, so one plant takes
    /// one tint and the one beside it another.
    pub variation_scale: f32,
    /// How much light comes through a leaf lit from behind. 0 is opaque.
    pub translucency_strength: f32,
    /// How far the surface normal bends the direction light is gathered from,
    /// which is what makes a leaf's edges glow rather than its whole face.
    pub translucency_normal_distortion: f32,
    /// How tightly the glow gathers around the light behind the leaf. High is
    /// a small bright spot, low a broad wash.
    pub translucency_scattering: f32,
    /// How much of the glow comes from the light itself.
    pub translucency_direct: f32,
    /// How much of it is there whichever way the leaf faces.
    pub translucency_ambient: f32,
    /// How much of the glow survives in shadow, `0..1`.
    pub translucency_shadow: f32,
    /// How far the lit normal leans toward world up, `0..1`, so a leaf facing away from the sky is not lit as the ground.
    pub shading_normal_up: f32,
    /// How far this material goes with the scene's wind, as a multiple of a
    /// blade of grass. 0 stands still in any wind.
    pub wind_response: f32,
    /// How far it flutters on top of that lean, at a much shorter wavelength.
    pub micro_wind_response: f32,
    /// The height in the mesh's own units at which the lean reaches full. A
    /// trunk sets this to its own height so only its crown moves; a leaf card
    /// leaves it small so the whole card flutters.
    pub bend_position: f32,
    /// The exponent the height takes before it leans, which is how sharply the
    /// bend gathers toward the top.
    pub bend_contrast: f32,
    /// The wind the scene is blowing by, written by [`FoliagePlugin`] from
    /// [`SceneWind`] rather than authored, so it never reaches the file.
    #[reflect(ignore)]
    pub wind: Wind,
}

impl Default for Foliage {
    fn default() -> Self {
        Self {
            alpha_cutoff: 0.5,
            gradient_color: Color::WHITE,
            gradient_position: 0.0,
            gradient_falloff: 1.0,
            gradient_invert: false,
            variation_color: Color::WHITE,
            variation_strength: 0.0,
            variation_scale: 8.0,
            translucency_strength: 0.0,
            translucency_normal_distortion: 0.5,
            translucency_scattering: 4.0,
            translucency_direct: 1.0,
            translucency_ambient: 0.2,
            translucency_shadow: 0.5,
            shading_normal_up: 0.0,
            wind_response: 0.0,
            micro_wind_response: 0.0,
            bend_position: 1.0,
            bend_contrast: 2.0,
            wind: Wind::STILL,
        }
    }
}

/// The smallest exponent a dial reaches, so a value raised to zero is never
/// flattened to one.
const MIN_EXPONENT: f32 = 0.001;
const TAU: f32 = std::f32::consts::TAU;

impl Foliage {
    /// How much of [`Self::gradient_color`] a point sitting `up_the_mesh`
    /// above its own root takes, `0..1`. The shader computes this same
    /// expression.
    pub fn gradient_weight(&self, up_the_mesh: f32) -> f32 {
        let risen = ((up_the_mesh - self.gradient_position) / self.gradient_falloff.max(1e-4))
            .clamp(0.0, 1.0);
        if self.gradient_invert {
            1.0 - risen
        } else {
            risen
        }
    }

    /// How much of [`Self::variation_color`] a plant standing at `at` takes,
    /// `0..1`. A smooth field over world space, so two plants a pattern apart
    /// differ and one plant is tinted whole.
    pub fn variation_weight(&self, at: Vec3) -> f32 {
        let scaled = at / self.variation_scale.max(1e-4);
        let field = (scaled.x * TAU * 0.13 + 1.7).sin() * (scaled.z * TAU * 0.11 - 0.4).cos()
            + (scaled.z * TAU * 0.07 + 2.3).sin() * 0.5;
        (0.5 + 0.25 * field).clamp(0.0, 1.0) * self.variation_strength.clamp(0.0, 1.0)
    }

    /// The normal a leaf is lit by: its own, leaned toward world up by [`Self::shading_normal_up`]. The shader computes this same expression.
    pub fn shaded_normal(&self, normal: Vec3) -> Vec3 {
        normal
            .lerp(Vec3::Y, self.shading_normal_up.clamp(0.0, 1.0))
            .normalize_or(Vec3::Y)
    }

    /// How far the wind carries a vertex, in world units.
    ///
    /// `up_the_mesh` is how far above its own root the vertex sits and `world`
    /// where it stands, which is what the pattern is read at so two plants
    /// side by side move apart. The shader computes this same expression.
    pub fn wind_offset(&self, up_the_mesh: f32, world: Vec3, time: f32) -> Vec3 {
        let wind = &self.wind;
        if wind.strength == 0.0 {
            return Vec3::ZERO;
        }
        let heading = wind.heading();
        let along = Vec3::new(heading.x, 0.0, heading.y);
        let risen = (up_the_mesh / self.bend_position.max(1e-4)).clamp(0.0, 1.0);
        let leaned = risen.powf(self.bend_contrast.max(MIN_EXPONENT));

        let travelled = Vec2::new(world.x, world.z).dot(heading) / wind.turbulence_scale.max(1e-4)
            - time * wind.gust_speed;
        let sway = (travelled * TAU).sin();
        let swell = 0.5 + 0.5 * (travelled * TAU * 0.25 + 1.3).sin();
        let gusted = sway * (1.0 - wind.gust + wind.gust * swell);

        let flutter = ((travelled * FLUTTER_RATE + world.y) * TAU).sin()
            * self.micro_wind_response
            * FLUTTER_LEAN;
        along
            * (gusted * self.wind_response * FOLIAGE_LEAN * leaned + flutter * leaned)
            * wind.strength
    }
}

/// The foliage half of the material's bind group, as the shader reads it.
#[derive(Clone, Default, ShaderType)]
pub struct FoliageUniform {
    pub gradient_color: Vec4,
    pub variation_color: Vec4,
    pub wind_direction: Vec2,
    pub wind_strength: f32,
    pub wind_gust: f32,
    pub wind_gust_speed: f32,
    pub wind_turbulence_scale: f32,
    pub alpha_cutoff: f32,
    pub gradient_position: f32,
    pub gradient_falloff: f32,
    pub gradient_invert: u32,
    pub variation_strength: f32,
    pub variation_scale: f32,
    pub translucency_strength: f32,
    pub translucency_normal_distortion: f32,
    pub translucency_scattering: f32,
    pub translucency_direct: f32,
    pub translucency_ambient: f32,
    pub translucency_shadow: f32,
    pub shading_normal_up: f32,
    pub wind_response: f32,
    pub micro_wind_response: f32,
    pub bend_position: f32,
    pub bend_contrast: f32,
}

impl AsBindGroupShaderType<FoliageUniform> for Foliage {
    fn as_bind_group_shader_type(&self, _images: &RenderAssets<GpuImage>) -> FoliageUniform {
        FoliageUniform {
            gradient_color: LinearRgba::from(self.gradient_color).to_vec4(),
            variation_color: LinearRgba::from(self.variation_color).to_vec4(),
            wind_direction: self.wind.heading(),
            wind_strength: self.wind.strength,
            wind_gust: self.wind.gust,
            wind_gust_speed: self.wind.gust_speed,
            wind_turbulence_scale: self.wind.turbulence_scale,
            alpha_cutoff: self.alpha_cutoff,
            gradient_position: self.gradient_position,
            gradient_falloff: self.gradient_falloff,
            gradient_invert: u32::from(self.gradient_invert),
            variation_strength: self.variation_strength,
            variation_scale: self.variation_scale,
            translucency_strength: self.translucency_strength,
            translucency_normal_distortion: self.translucency_normal_distortion,
            translucency_scattering: self.translucency_scattering,
            translucency_direct: self.translucency_direct,
            translucency_ambient: self.translucency_ambient,
            translucency_shadow: self.translucency_shadow,
            shading_normal_up: self.shading_normal_up.clamp(0.0, 1.0),
            wind_response: self.wind_response,
            micro_wind_response: self.micro_wind_response,
            bend_position: self.bend_position,
            bend_contrast: self.bend_contrast,
        }
    }
}

impl MaterialExtension for Foliage {
    fn vertex_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn prepass_vertex_shader() -> ShaderRef {
        PREPASS_SHADER_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }
}

/// Keeps the shader the foliage stages import loaded for as long as the app runs.
#[derive(Resource)]
struct FoliageShaderLibrary(
    #[expect(dead_code, reason = "held only to keep the shader loaded")] Handle<Shader>,
);

/// Registers the foliage material, its shader and its reflected type, so a
/// scene naming one renders it.
pub struct FoliagePlugin;

impl Plugin for FoliagePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/foliage.wgsl");
        embedded_asset!(app, "shaders/foliage_prepass.wgsl");
        embedded_asset!(app, "shaders/foliage_wind.wgsl");
        if app.world().contains_resource::<Assets<Shader>>() {
            let library = bevy::asset::load_embedded_asset!(app, "shaders/foliage_wind.wgsl");
            app.insert_resource(FoliageShaderLibrary(library));
        }
        app.add_plugins(MaterialPlugin::<FoliageMaterial>::default())
            .init_resource::<SceneWind>()
            .register_type::<Foliage>()
            .register_asset_reflect::<FoliageMaterial>()
            .register_type_data::<FoliageMaterial, bevy::reflect::std_traits::ReflectDefault>()
            .add_systems(PostUpdate, blow_the_foliage_by_the_scenes_wind);
    }
}

/// Hand every foliage material the wind the scene is blowing by, so a wind
/// authored once moves every plant wearing one.
fn blow_the_foliage_by_the_scenes_wind(
    blowing: Res<SceneWind>,
    mut materials: ResMut<Assets<FoliageMaterial>>,
) {
    let ids: Vec<AssetId<FoliageMaterial>> = materials
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
    const SHADER_SOURCE: &str = include_str!("shaders/foliage_wind.wgsl");
    const PREPASS_SOURCE: &str = include_str!("shaders/foliage_prepass.wgsl");

    #[test]
    fn the_prepass_leans_the_leaves_the_way_the_main_pass_does() {
        assert!(
            PREPASS_SOURCE.contains("wind_offset(up_the_mesh, planted.xyz, globals.time)"),
            "a prepass that left the leaves where the model put them would cut holes where they sway"
        );
    }

    fn blowing(material: &mut Foliage) {
        material.wind = Wind {
            direction: 0.0,
            strength: 1.0,
            gust: 0.0,
            gust_speed: 0.25,
            turbulence_scale: 8.0,
        };
    }

    #[test]
    fn every_material_binding_is_declared_in_the_shader_at_its_derive_index() {
        let expected =
            "@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> foliage: FoliageUniform;";
        assert!(
            SHADER_SOURCE.contains(expected),
            "the AsBindGroup derive binds 100 as the foliage uniform, \
             but the shader does not declare it that way"
        );
    }

    /// The uniform is one buffer built from [`FoliageUniform`]'s fields in
    /// declaration order, so the WGSL struct has to list them in the same
    /// order or every field reads its neighbour's bytes.
    #[test]
    fn the_uniform_struct_matches_the_rust_field_order() {
        let start = SHADER_SOURCE
            .find("struct FoliageUniform {")
            .expect("FoliageUniform is declared");
        let body = &SHADER_SOURCE[start..];
        let end = body.find('}').expect("FoliageUniform is closed");
        let body = &body[..end];

        let mut at = 0usize;
        for field in [
            "gradient_color: vec4<f32>",
            "variation_color: vec4<f32>",
            "wind_direction: vec2<f32>",
            "wind_strength: f32",
            "wind_gust: f32",
            "wind_gust_speed: f32",
            "wind_turbulence_scale: f32",
            "alpha_cutoff: f32",
            "gradient_position: f32",
            "gradient_falloff: f32",
            "gradient_invert: u32",
            "variation_strength: f32",
            "variation_scale: f32",
            "translucency_strength: f32",
            "translucency_normal_distortion: f32",
            "translucency_scattering: f32",
            "translucency_direct: f32",
            "translucency_ambient: f32",
            "translucency_shadow: f32",
            "shading_normal_up: f32",
            "wind_response: f32",
            "micro_wind_response: f32",
            "bend_position: f32",
            "bend_contrast: f32",
        ] {
            let found = body[at..].find(field).unwrap_or_else(|| {
                panic!("the shader declares `{field}` after the field above it")
            });
            at += found + field.len();
        }
    }

    /// An unedited foliage material is a standard material with a cutout:
    /// nothing leans, nothing is tinted and nothing glows.
    #[test]
    fn an_unedited_material_moves_and_tints_nothing() {
        let mut material = Foliage::default();
        blowing(&mut material);

        assert_eq!(
            material.wind_offset(2.0, Vec3::new(3.0, 2.0, 5.0), 1.0),
            Vec3::ZERO,
            "a material that responds to nothing stands still in a wind",
        );
        assert_eq!(material.variation_weight(Vec3::new(3.0, 0.0, 5.0)), 0.0);
        assert_eq!(material.translucency_strength, 0.0);
        assert_eq!(material.alpha_cutoff, 0.5);
    }

    #[test]
    fn the_gradient_reaches_the_top_and_leaves_the_base_alone() {
        let material = Foliage {
            gradient_position: 1.0,
            gradient_falloff: 2.0,
            ..Foliage::default()
        };

        assert_eq!(material.gradient_weight(0.0), 0.0);
        assert_eq!(material.gradient_weight(1.0), 0.0);
        assert_eq!(material.gradient_weight(2.0), 0.5);
        assert_eq!(material.gradient_weight(3.0), 1.0);
        assert_eq!(material.gradient_weight(9.0), 1.0);
    }

    #[test]
    fn inverting_the_gradient_gathers_it_at_the_base() {
        let material = Foliage {
            gradient_falloff: 1.0,
            gradient_invert: true,
            ..Foliage::default()
        };

        assert_eq!(material.gradient_weight(0.0), 1.0);
        assert_eq!(material.gradient_weight(1.0), 0.0);
    }

    #[test]
    fn variation_differs_between_two_places_and_holds_within_one() {
        let material = Foliage {
            variation_strength: 1.0,
            variation_scale: 8.0,
            ..Foliage::default()
        };

        let here = material.variation_weight(Vec3::new(0.0, 0.0, 0.0));
        let across_the_field = material.variation_weight(Vec3::new(31.0, 0.0, 17.0));
        let the_same_plant = material.variation_weight(Vec3::new(0.1, 1.4, 0.1));

        assert!(
            (here - across_the_field).abs() > 0.05,
            "two plants a field apart take different tints, got {here} and {across_the_field}",
        );
        assert!(
            (here - the_same_plant).abs() < 0.02,
            "and one plant is tinted whole, got {here} and {the_same_plant}",
        );
    }

    #[test]
    fn nothing_leans_in_a_scene_with_no_wind() {
        let material = Foliage {
            wind_response: 2.0,
            micro_wind_response: 1.0,
            ..Foliage::default()
        };

        assert_eq!(
            material.wind_offset(1.0, Vec3::new(2.0, 1.0, 1.0), 4.0),
            Vec3::ZERO,
        );
    }

    #[test]
    fn a_harder_wind_leans_a_crown_further() {
        let mut material = Foliage {
            wind_response: 1.0,
            bend_position: 4.0,
            ..Foliage::default()
        };
        blowing(&mut material);
        let crown = Vec3::new(2.0, 4.0, 1.0);
        let at = |strength: f32, material: &mut Foliage| {
            material.wind.strength = strength;
            material.wind_offset(4.0, crown, 0.7).length()
        };

        let breeze = at(1.0, &mut material);
        let gale = at(3.0, &mut material);

        assert!(breeze > 0.0, "a breeze leans the crown, got {breeze}");
        assert!(
            gale > breeze * 2.0,
            "and a harder wind leans it further, got {breeze} then {gale}",
        );
    }

    #[test]
    fn a_trunk_bends_at_the_crown_and_stands_still_at_the_root() {
        let mut material = Foliage {
            wind_response: 1.0,
            bend_position: 6.0,
            bend_contrast: 2.0,
            ..Foliage::default()
        };
        blowing(&mut material);
        let lean = |height: f32| {
            let at = Vec3::new(2.0, height, 1.0);
            material.wind_offset(height, at, 0.7).length()
        };

        assert_eq!(lean(0.0), 0.0, "the root stays where it is planted");
        assert!(
            lean(6.0) > lean(3.0) * 2.0,
            "and the crown carries the bend"
        );
    }

    #[test]
    fn an_unleaned_leaf_is_lit_by_its_own_normal() {
        let leaf = Vec3::new(0.3, -0.9, 0.2).normalize();
        assert!(
            Foliage::default()
                .shaded_normal(leaf)
                .abs_diff_eq(leaf, 1e-6)
        );
    }

    #[test]
    fn a_leaned_leaf_facing_the_ground_is_lit_from_above_the_horizon() {
        let foliage = Foliage {
            shading_normal_up: 0.6,
            ..Foliage::default()
        };
        let facing_down = Vec3::new(0.2, -0.95, 0.1).normalize();
        assert!(
            foliage.shaded_normal(facing_down).y > 0.0,
            "a leaf turned away from the sky still takes light from above"
        );
    }
}
