//! The materials the editor ships beside Bevy's standard one.
//!
//! [`LayeredSurface`] carries a second surface on a mesh's upward faces;
//! [`foliage::Foliage`] blows with the scene's wind and passes light through
//! its leaves; [`water::Water`] draws a lake, a river or a sea.
//! [`environment::EnvironmentPlugin`] dresses the cameras in the scene's sky,
//! fog, ambient light and grading.
//!
//! # A second surface on the upward faces
//!
//! A cliff is one mesh with one rock texture set, and the grass, moss or snow
//! that gathers on its ledges is a second set blended over it. [`LayeredSurface`]
//! extends [`StandardMaterial`] with that second set plus a detail set that
//! multiplies over everything, and decides how much of the layer reaches a
//! fragment from the mesh's vertex colour and from how far the surface faces
//! up.
//!
//! The layer and detail sets are sampled triplanar in world space, so they hold
//! their scale across a mesh whose own UVs were laid out for the rock. Each set
//! takes a base colour, a normal map and an occlusion-roughness-metallic map in
//! the channel order glTF writes: occlusion in red, roughness in green,
//! metallic in blue.
//!
//! [`LayerBlend::weight`] is the whole of the blend decision and is shared with
//! the shader, which computes the same expression.

pub mod environment;
pub mod foliage;
pub mod water;
pub mod worn;

pub use environment::{EnvironmentPlugin, SkyMaterial};
pub use foliage::{Foliage, FoliageMaterial, FoliagePlugin};
pub use water::{Water, WaterMaterial, WaterPlugin};
pub use worn::WornMaterial;

use bevy::asset::embedded_asset;
use bevy::color::ColorToComponents;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{AsBindGroup, AsBindGroupShaderType, ShaderType};
use bevy::render::texture::GpuImage;
use bevy::shader::ShaderRef;

const SHADER_PATH: &str = "embedded://jackdaw_surface/shaders/layered_surface.wgsl";

/// The material an entity wears to carry a layer over its base surface.
pub type LayeredSurfaceMaterial = ExtendedMaterial<StandardMaterial, LayeredSurface>;

/// Which vertex colour channel the layer mask is read from.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Clone, PartialEq)]
pub enum VertexColorChannel {
    Red,
    Green,
    #[default]
    Blue,
    Alpha,
}

impl VertexColorChannel {
    /// The channel's index in an RGBA vertex colour.
    pub fn index(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Green => 1,
            Self::Blue => 2,
            Self::Alpha => 3,
        }
    }

    /// The channel's value in an RGBA vertex colour.
    pub fn read(self, color: LinearRgba) -> f32 {
        color.to_f32_array()[self.index()]
    }
}

/// How much of the layer reaches a fragment.
#[derive(Reflect, Clone, Copy, Debug)]
#[reflect(Default, Clone)]
pub struct LayerBlend {
    /// How much of the layer reaches the surface at its strongest, `0..1`. At
    /// `0` the material draws exactly as its base does.
    pub amount: f32,
    /// Whether the mask comes from the mesh's vertex colour. With this off the
    /// mask is how far the surface faces up.
    pub use_vertex_color: bool,
    pub vertex_color_channel: VertexColorChannel,
    /// Raises the mask everywhere, `0..1`. A surface facing halfway up takes
    /// the layer once this lifts it past the threshold.
    pub power: f32,
    /// The exponent the lifted mask is raised to, `0..50`. Low is a wide
    /// gradient down the face, high a narrow band along the ledges.
    pub threshold: f32,
    /// The exponent the vertex-colour mask is raised to before anything else,
    /// which slides a painted gradient up or down the mesh.
    pub position: f32,
    /// Steepens the vertex-colour mask around its middle.
    pub contrast: f32,
}

impl Default for LayerBlend {
    fn default() -> Self {
        Self {
            amount: 0.0,
            use_vertex_color: false,
            vertex_color_channel: VertexColorChannel::default(),
            power: 0.0,
            threshold: 50.0,
            position: 0.0,
            contrast: 0.0,
        }
    }
}

/// The narrowest exponent the threshold dial reaches, so the mask is never
/// raised to zero and flattened to one.
const MIN_THRESHOLD_EXPONENT: f32 = 0.001;

impl LayerBlend {
    /// How much of the layer a fragment takes, `0..1`.
    ///
    /// `vertex_color` is the mesh's colour at the fragment and `up` is the
    /// world-space normal's Y. The shader computes this same expression.
    pub fn weight(&self, vertex_color: LinearRgba, up: f32) -> f32 {
        let selector = if self.use_vertex_color {
            let mask = self
                .vertex_color_channel
                .read(vertex_color)
                .abs()
                .powf(self.position);
            let contrasted = (self.contrast * mask + 0.5 * (1.0 - self.contrast)).clamp(0.0, 1.0);
            contrasted.powf(1.0 - self.power) * contrasted
        } else {
            up
        };
        let lifted = (selector + self.power).clamp(0.0, 1.0);
        let exponent = MIN_THRESHOLD_EXPONENT + self.threshold * (1.0 - MIN_THRESHOLD_EXPONENT);
        self.amount * lifted.abs().powf(exponent)
    }
}

/// A second surface blended over a standard material's upward faces, and a
/// detail surface multiplied over the result.
#[derive(Asset, AsBindGroup, Reflect, Clone, Debug)]
#[reflect(Default, Clone)]
#[uniform(100, LayeredSurfaceUniform)]
pub struct LayeredSurface {
    /// Tints the layer's base colour.
    pub layer_color: Color,
    /// World units the layer's textures repeat over, as a multiplier on the
    /// world position they are sampled at.
    pub layer_uv_scale: f32,
    /// How far the layer's normal map turns the shaded normal, `0..1`.
    pub layer_normal_strength: f32,
    /// Scales the metallic the layer's map reports, so a layer with no map
    /// stays dielectric.
    pub layer_metallic: f32,
    /// Scales the roughness the layer's map reports.
    pub layer_perceptual_roughness: f32,
    #[texture(101)]
    #[sampler(102)]
    pub layer_base_color_texture: Option<Handle<Image>>,
    #[texture(103)]
    pub layer_normal_map_texture: Option<Handle<Image>>,
    /// Occlusion in red, roughness in green, metallic in blue.
    #[texture(104)]
    pub layer_orm_texture: Option<Handle<Image>>,
    /// Tints the detail surface, which multiplies over base and layer alike.
    pub detail_color: Color,
    pub detail_uv_scale: f32,
    pub detail_normal_strength: f32,
    #[texture(105)]
    #[sampler(106)]
    pub detail_base_color_texture: Option<Handle<Image>>,
    #[texture(107)]
    pub detail_normal_map_texture: Option<Handle<Image>>,
    /// Occlusion in red, roughness in green, metallic in blue.
    #[texture(108)]
    pub detail_orm_texture: Option<Handle<Image>>,
    pub blend: LayerBlend,
}

impl Default for LayeredSurface {
    fn default() -> Self {
        Self {
            layer_color: Color::WHITE,
            layer_uv_scale: 1.0,
            layer_normal_strength: 1.0,
            layer_metallic: 0.0,
            layer_perceptual_roughness: 1.0,
            layer_base_color_texture: None,
            layer_normal_map_texture: None,
            layer_orm_texture: None,
            detail_color: Color::WHITE,
            detail_uv_scale: 1.0,
            detail_normal_strength: 1.0,
            detail_base_color_texture: None,
            detail_normal_map_texture: None,
            detail_orm_texture: None,
            blend: LayerBlend::default(),
        }
    }
}

impl LayeredSurface {
    fn bound_maps(&self) -> u32 {
        let mut bound = 0;
        if self.layer_normal_map_texture.is_some() {
            bound |= flags::LAYER_NORMAL_MAP;
        }
        if self.detail_normal_map_texture.is_some() {
            bound |= flags::DETAIL_NORMAL_MAP;
        }
        bound
    }
}

/// The layered half of the material's bind group, as the shader reads it.
#[derive(Clone, Default, ShaderType)]
pub struct LayeredSurfaceUniform {
    pub layer_color: Vec4,
    pub detail_color: Vec4,
    pub layer_uv_scale: f32,
    pub layer_normal_strength: f32,
    pub layer_metallic: f32,
    pub layer_perceptual_roughness: f32,
    pub detail_uv_scale: f32,
    pub detail_normal_strength: f32,
    pub blend_amount: f32,
    pub blend_power: f32,
    pub blend_threshold: f32,
    pub blend_position: f32,
    pub blend_contrast: f32,
    pub vertex_color_channel: u32,
    pub use_vertex_color: u32,
    pub flags: u32,
}

/// Bits of [`LayeredSurfaceUniform::flags`], telling the shader which maps are
/// bound. Only the normal maps need it: an unbound colour or
/// occlusion-roughness-metallic map falls back to white, which is what a set
/// without one should contribute, while white read as a normal is a slant.
pub mod flags {
    pub const LAYER_NORMAL_MAP: u32 = 1;
    pub const DETAIL_NORMAL_MAP: u32 = 2;
}

impl AsBindGroupShaderType<LayeredSurfaceUniform> for LayeredSurface {
    fn as_bind_group_shader_type(&self, _images: &RenderAssets<GpuImage>) -> LayeredSurfaceUniform {
        LayeredSurfaceUniform {
            layer_color: LinearRgba::from(self.layer_color).to_vec4(),
            detail_color: LinearRgba::from(self.detail_color).to_vec4(),
            layer_uv_scale: self.layer_uv_scale,
            layer_normal_strength: self.layer_normal_strength,
            layer_metallic: self.layer_metallic,
            layer_perceptual_roughness: self.layer_perceptual_roughness,
            detail_uv_scale: self.detail_uv_scale,
            detail_normal_strength: self.detail_normal_strength,
            blend_amount: self.blend.amount,
            blend_power: self.blend.power,
            blend_threshold: self.blend.threshold,
            blend_position: self.blend.position,
            blend_contrast: self.blend.contrast,
            vertex_color_channel: self.blend.vertex_color_channel.index() as u32,
            use_vertex_color: u32::from(self.blend.use_vertex_color),
            flags: self.bound_maps(),
        }
    }
}

impl MaterialExtension for LayeredSurface {
    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }
}

/// Registers the layered material, its shader and its reflected types, so a
/// scene naming one renders it.
pub struct LayeredSurfacePlugin;

impl Plugin for LayeredSurfacePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/layered_surface.wgsl");
        app.add_plugins(MaterialPlugin::<LayeredSurfaceMaterial>::default())
            .register_type::<LayeredSurface>()
            .register_type::<LayerBlend>()
            .register_type::<VertexColorChannel>()
            .register_asset_reflect::<LayeredSurfaceMaterial>()
            .register_type_data::<LayeredSurfaceMaterial, bevy::reflect::std_traits::ReflectDefault>(
            );
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
    const SHADER_SOURCE: &str = include_str!("shaders/layered_surface.wgsl");

    #[test]
    fn every_material_binding_is_declared_in_the_shader_at_its_derive_index() {
        for (binding, declaration) in [
            (100, "var<uniform> layered: LayeredSurfaceUniform"),
            (101, "var layer_base_color_texture: texture_2d<f32>"),
            (102, "var layer_sampler: sampler"),
            (103, "var layer_normal_texture: texture_2d<f32>"),
            (104, "var layer_orm_texture: texture_2d<f32>"),
            (105, "var detail_base_color_texture: texture_2d<f32>"),
            (106, "var detail_sampler: sampler"),
            (107, "var detail_normal_texture: texture_2d<f32>"),
            (108, "var detail_orm_texture: texture_2d<f32>"),
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

    /// The uniform is one buffer built from [`LayeredSurfaceUniform`]'s fields
    /// in declaration order, so the WGSL struct has to list them in the same
    /// order or every field reads its neighbour's bytes.
    #[test]
    fn the_uniform_struct_matches_the_rust_field_order() {
        let start = SHADER_SOURCE
            .find("struct LayeredSurfaceUniform {")
            .expect("LayeredSurfaceUniform is declared");
        let body = &SHADER_SOURCE[start..];
        let end = body.find('}').expect("LayeredSurfaceUniform is closed");
        let body = &body[..end];

        let mut at = 0usize;
        for field in [
            "layer_color: vec4<f32>",
            "detail_color: vec4<f32>",
            "layer_uv_scale: f32",
            "layer_normal_strength: f32",
            "layer_metallic: f32",
            "layer_perceptual_roughness: f32",
            "detail_uv_scale: f32",
            "detail_normal_strength: f32",
            "blend_amount: f32",
            "blend_power: f32",
            "blend_threshold: f32",
            "blend_position: f32",
            "blend_contrast: f32",
            "vertex_color_channel: u32",
            "use_vertex_color: u32",
            "flags: u32",
        ] {
            let found = body[at..].find(field).unwrap_or_else(|| {
                panic!("the shader declares `{field}` after the field above it")
            });
            at += found + field.len();
        }
    }

    #[test]
    fn a_material_reports_which_normal_maps_are_bound() {
        let bare = LayeredSurface::default();
        assert_eq!(bare.bound_maps(), 0);

        let mapped = LayeredSurface {
            layer_normal_map_texture: Some(Handle::default()),
            ..default()
        };
        assert_eq!(mapped.bound_maps(), flags::LAYER_NORMAL_MAP);
    }

    fn painted(channel: f32) -> LinearRgba {
        LinearRgba::new(0.0, 0.0, channel, 1.0)
    }

    #[test]
    fn an_unedited_material_takes_none_of_its_layer() {
        let blend = LayerBlend::default();
        for up in [-1.0, 0.0, 0.5, 1.0] {
            assert_eq!(blend.weight(painted(1.0), up), 0.0);
        }
    }

    #[test]
    fn a_surface_facing_up_takes_the_layer_and_a_wall_takes_none() {
        let blend = LayerBlend {
            amount: 1.0,
            threshold: 10.0,
            ..default()
        };
        assert_eq!(blend.weight(LinearRgba::BLACK, 1.0), 1.0);
        assert_eq!(blend.weight(LinearRgba::BLACK, 0.0), 0.0);
        assert!(blend.weight(LinearRgba::BLACK, -1.0) == 0.0);
    }

    #[test]
    fn lifting_the_mask_reaches_down_the_face() {
        let blend = LayerBlend {
            amount: 1.0,
            power: 0.5,
            threshold: 1.0,
            ..default()
        };
        assert_eq!(blend.weight(LinearRgba::BLACK, 0.0), 0.5);
        assert_eq!(blend.weight(LinearRgba::BLACK, 0.5), 1.0);
    }

    #[test]
    fn a_painted_mask_reads_the_channel_it_names() {
        let blend = LayerBlend {
            amount: 1.0,
            use_vertex_color: true,
            vertex_color_channel: VertexColorChannel::Blue,
            contrast: 1.0,
            position: 1.0,
            threshold: 1.0,
            power: 0.0,
        };
        assert_eq!(blend.weight(painted(0.0), 1.0), 0.0);
        assert_eq!(blend.weight(painted(1.0), 1.0), 1.0);
        assert_eq!(
            blend.weight(LinearRgba::new(1.0, 1.0, 0.0, 1.0), 1.0),
            0.0,
            "a mask painted in another channel is not read",
        );
    }

    #[test]
    fn the_narrowest_threshold_leaves_a_partly_lit_mask_partly_covered() {
        let wide = LayerBlend {
            amount: 1.0,
            threshold: 0.0,
            ..default()
        };
        let narrow = LayerBlend {
            threshold: 50.0,
            ..wide
        };
        assert!(wide.weight(LinearRgba::BLACK, 0.5) > 0.99);
        assert!(narrow.weight(LinearRgba::BLACK, 0.5) < 0.01);
    }
}
