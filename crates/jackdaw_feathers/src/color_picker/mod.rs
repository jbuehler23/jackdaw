mod color_math;
mod controls;
mod input_fields;
pub mod materials;
mod setup;
mod visuals;

use bevy_app::prelude::*;
use bevy_asset::embedded_asset;
use bevy_color::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ui::prelude::*;
use bevy_ui_render::prelude::*;
use bevy_utils::prelude::*;

use color_math::{hsv_to_rgb, rgb_to_hsv};
pub use materials::{
    AlphaSliderMaterial, CheckerboardMaterial, HsvRectMaterial, HueSliderMaterial,
};

use crate::popover::PopoverTracker;

const SLIDER_HEIGHT: f32 = 12.0;
const HSV_RECT_HEIGHT: f32 = 192.0;
const PREVIEW_SWATCH_SIZE: f32 = 36.0;
const HANDLE_SIZE: f32 = 14.0;
const HANDLE_BORDER: f32 = 1.0;
const SWATCH_SIZE: f32 = 16.0;
const CHECKERBOARD_SIZE: f32 = 8.0;
const PREVIEW_CHECKERBOARD_SIZE: f32 = 12.0;
const BORDER_RADIUS: f32 = 4.0;
const POPOVER_WIDTH: f32 = 256.0;

pub fn plugin(app: &mut App) {
    embedded_asset!(app, "shaders/common.wgsl");
    embedded_asset!(app, "shaders/color_picker_hsv_rect.wgsl");
    embedded_asset!(app, "shaders/color_picker_hue.wgsl");
    embedded_asset!(app, "shaders/color_picker_alpha.wgsl");
    embedded_asset!(app, "shaders/color_picker_checkerboard.wgsl");

    app.add_plugins(UiMaterialPlugin::<HsvRectMaterial>::default())
        .add_plugins(UiMaterialPlugin::<HueSliderMaterial>::default())
        .add_plugins(UiMaterialPlugin::<AlphaSliderMaterial>::default())
        .add_plugins(UiMaterialPlugin::<CheckerboardMaterial>::default())
        .add_observer(setup::handle_trigger_click)
        .add_observer(input_fields::handle_input_mode_change)
        .add_systems(
            Update,
            (
                setup::setup_color_picker,
                setup::setup_trigger_swatch,
                setup::setup_color_picker_content,
                visuals::update_color_picker_visuals,
                input_fields::handle_input_field_blur,
                visuals::update_trigger_display,
                input_fields::sync_text_inputs_to_state,
            ),
        );
}

#[derive(Component)]
pub struct EditorColorPicker;

#[derive(Component, Clone)]
pub struct ColorPickerState {
    pub hue: f32,
    pub saturation: f32,
    pub brightness: f32,
    pub alpha: f32,
    pub input_mode: ColorInputMode,
}

impl Default for ColorPickerState {
    fn default() -> Self {
        Self {
            hue: 0.0,
            saturation: 0.0,
            brightness: 1.0,
            alpha: 1.0,
            input_mode: ColorInputMode::Rgb,
        }
    }
}

impl ColorPickerState {
    pub fn from_rgba(rgba: [f32; 4]) -> Self {
        let (h, s, v) = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
        Self {
            hue: h,
            saturation: s,
            brightness: v,
            alpha: rgba[3],
            input_mode: ColorInputMode::Rgb,
        }
    }

    pub fn to_rgba(&self) -> [f32; 4] {
        let (r, g, b) = hsv_to_rgb(self.hue, self.saturation, self.brightness);
        [r, g, b, self.alpha]
    }

    pub fn set_from_rgba(&mut self, rgba: [f32; 4]) {
        let (h, s, v) = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
        self.hue = h;
        self.saturation = s;
        self.brightness = v;
        self.alpha = rgba[3];
    }

    pub fn to_srgba(&self) -> Srgba {
        let rgba = self.to_rgba();
        Srgba::new(
            rgba[0].clamp(0.0, 1.0),
            rgba[1].clamp(0.0, 1.0),
            rgba[2].clamp(0.0, 1.0),
            rgba[3].clamp(0.0, 1.0),
        )
    }

    pub fn to_hex(&self) -> String {
        let rgba = self.to_rgba();
        let r = (rgba[0].clamp(0.0, 1.0) * 255.0).round() as u8;
        let g = (rgba[1].clamp(0.0, 1.0) * 255.0).round() as u8;
        let b = (rgba[2].clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("{:02X}{:02X}{:02X}", r, g, b)
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
pub enum ColorInputMode {
    Hex,
    #[default]
    Rgb,
    Hsb,
    Raw,
}

impl ColorInputMode {
    fn index(&self) -> usize {
        match self {
            Self::Hex => 0,
            Self::Rgb => 1,
            Self::Hsb => 2,
            Self::Raw => 3,
        }
    }

    fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Hex,
            2 => Self::Hsb,
            3 => Self::Raw,
            _ => Self::Rgb,
        }
    }
}

#[derive(EntityEvent)]
pub struct ColorPickerChangeEvent {
    pub entity: Entity,
    pub color: [f32; 4],
}

#[derive(EntityEvent)]
pub struct ColorPickerCommitEvent {
    pub entity: Entity,
    pub color: [f32; 4],
}

#[derive(Default)]
pub struct ColorPickerProps {
    pub color: [f32; 4],
    pub inline: bool,
}

impl ColorPickerProps {
    pub fn new() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0],
            inline: false,
        }
    }

    pub fn with_color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    pub fn inline(mut self) -> Self {
        self.inline = true;
        self
    }
}

pub fn color_picker(props: ColorPickerProps) -> impl Bundle {
    let ColorPickerProps { color, inline } = props;

    (
        EditorColorPicker,
        ColorPickerState::from_rgba(color),
        ColorPickerConfig { inline },
        PopoverTracker::default(),
        Node {
            flex_direction: FlexDirection::Column,
            ..default()
        },
    )
}

// --- Internal marker components ---

#[derive(Component)]
struct ColorPickerConfig {
    inline: bool,
}

#[derive(Component)]
struct ColorPickerTrigger(Entity);

#[derive(Component)]
struct ColorPickerPopover(Entity);

#[derive(Component)]
struct ColorPickerContent(Entity);

#[derive(Component)]
struct HsvRectangle(Entity);

#[derive(Component)]
struct HsvRectMaterialNode(Entity);

#[derive(Component)]
struct HsvRectHandle(Entity);

#[derive(Component)]
struct HueSlider(Entity);

#[derive(Component)]
struct HueHandle(Entity);

#[derive(Component)]
struct AlphaSlider(Entity);

#[derive(Component)]
struct AlphaMaterialNode(Entity);

#[derive(Component)]
struct AlphaHandle(Entity);

#[derive(Component)]
struct AlphaHandleMaterial(Entity);

#[derive(Component)]
struct ColorInputRow(Entity);

#[derive(Component)]
struct TriggerSwatchConfig {
    picker: Entity,
    color: Srgba,
}

#[derive(Component)]
struct TriggerSwatch;

#[derive(Component)]
pub struct TriggerSwatchMaterial(pub Entity);

#[derive(Component)]
struct TriggerLabel(Entity);

#[derive(Component)]
struct PreviewSwatchMaterial(Entity);
