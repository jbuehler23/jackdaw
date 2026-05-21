use bevy_asset::prelude::*;
use bevy_ecs::prelude::*;
use bevy_math::prelude::*;
use bevy_reflect::TypePath;
use bevy_render::render_resource::*;
use bevy_shader::ShaderRef;
use bevy_ui_render::prelude::*;

const SHADER_HSV_RECT_PATH: &str =
    "embedded://jackdaw_feathers/color_picker/shaders/color_picker_hsv_rect.wgsl";
const SHADER_HUE_PATH: &str =
    "embedded://jackdaw_feathers/color_picker/shaders/color_picker_hue.wgsl";
const SHADER_ALPHA_PATH: &str =
    "embedded://jackdaw_feathers/color_picker/shaders/color_picker_alpha.wgsl";
const SHADER_CHECKERBOARD_PATH: &str =
    "embedded://jackdaw_feathers/color_picker/shaders/color_picker_checkerboard.wgsl";

#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
pub struct HsvRectMaterial {
    #[uniform(0)]
    pub hue: f32,
    #[uniform(0)]
    pub border_radius: f32,
}

impl UiMaterial for HsvRectMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_HSV_RECT_PATH.into()
    }
}

#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
pub struct HueSliderMaterial {
    #[uniform(0)]
    pub border_radius: f32,
}

impl UiMaterial for HueSliderMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_HUE_PATH.into()
    }
}

#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
pub struct AlphaSliderMaterial {
    #[uniform(0)]
    pub color: Vec4,
    #[uniform(0)]
    pub checkerboard_size: f32,
    #[uniform(0)]
    pub border_radius: f32,
}

impl UiMaterial for AlphaSliderMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_ALPHA_PATH.into()
    }
}

#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
pub struct CheckerboardMaterial {
    #[uniform(0)]
    pub color: Vec4,
    #[uniform(0)]
    pub size: f32,
    #[uniform(0)]
    pub border_radius: f32,
}

impl UiMaterial for CheckerboardMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_CHECKERBOARD_PATH.into()
    }
}
