use bevy_color::Color;
use bevy_ecs::prelude::*;
use bevy_ui::prelude::*;
use bevy_utils::prelude::*;

use crate::text_edit::{TextEditPrefix, TextEditProps, text_edit};
use crate::tokens::{self, TEXT_SIZE, TEXT_SIZE_SM};

#[derive(Component)]
pub struct EditorVectorEdit;

#[derive(Component)]
pub struct VectorComponentIndex(pub usize);

#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub enum VectorSuffixes {
    #[default]
    XYZ,
    XY,
    WHD,
    WD,
    Range,
}

impl VectorSuffixes {
    fn get(&self, index: usize) -> &'static str {
        match self {
            Self::XYZ => ["X", "Y", "Z"].get(index).unwrap_or(&""),
            Self::XY => ["X", "Y"].get(index).unwrap_or(&""),
            Self::WHD => ["W", "H", "D"].get(index).unwrap_or(&""),
            Self::WD => ["W", "D"].get(index).unwrap_or(&""),
            Self::Range => ["min", "max"].get(index).unwrap_or(&""),
        }
    }

    fn text_size(&self) -> f32 {
        match self {
            Self::Range => TEXT_SIZE_SM,
            _ => TEXT_SIZE,
        }
    }

    pub fn vector_size(&self) -> VectorSize {
        match self {
            Self::Range | Self::XY | Self::WD => VectorSize::Vec2,
            Self::XYZ | Self::WHD => VectorSize::Vec3,
        }
    }

    pub fn is_integer(&self) -> bool {
        matches!(self, Self::WD)
    }
}

#[derive(Default, Clone, Copy)]
pub enum VectorSize {
    Vec2,
    #[default]
    Vec3,
}

impl VectorSize {
    pub fn count(&self) -> usize {
        match self {
            Self::Vec2 => 2,
            Self::Vec3 => 3,
        }
    }
}

pub struct VectorEditProps {
    pub label: Option<String>,
    pub size: VectorSize,
    pub suffixes: VectorSuffixes,
    pub default_values: Vec<f32>,
    pub suffix: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl Default for VectorEditProps {
    fn default() -> Self {
        Self {
            label: None,
            size: VectorSize::Vec3,
            suffixes: VectorSuffixes::XYZ,
            default_values: Vec::new(),
            suffix: None,
            min: None,
            max: None,
        }
    }
}

impl VectorEditProps {
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_size(mut self, size: VectorSize) -> Self {
        self.size = size;
        self
    }

    pub fn with_suffixes(mut self, suffixes: VectorSuffixes) -> Self {
        self.suffixes = suffixes;
        self
    }

    pub fn with_default_values(mut self, values: impl Into<Vec<f32>>) -> Self {
        self.default_values = values.into();
        self
    }

    pub fn with_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    pub fn with_min(mut self, min: f64) -> Self {
        self.min = Some(min);
        self
    }

    pub fn with_max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }
}

pub fn vector_edit(props: VectorEditProps) -> impl Bundle {
    let VectorEditProps {
        label,
        size,
        suffixes,
        default_values,
        suffix,
        min,
        max,
    } = props;

    let is_integer = suffixes.is_integer();

    let children: Vec<_> = (0..size.count())
        .map(|i| {
            let mut text_edit_props = if is_integer {
                TextEditProps::default().numeric_i32()
            } else {
                TextEditProps::default().numeric_f32()
            }
            .with_prefix(TextEditPrefix::Label {
                label: suffixes.get(i).to_string(),
                size: suffixes.text_size(),
                color: axis_color(&suffixes, i),
            });

            if i == 0
                && let Some(ref label) = label
            {
                text_edit_props = text_edit_props.with_label(label.clone());
            }

            if let Some(&value) = default_values.get(i) {
                text_edit_props = text_edit_props.with_default_value(value.to_string());
            }

            if let Some(ref suffix) = suffix {
                text_edit_props = text_edit_props.with_suffix(suffix);
            }

            if let Some(min) = min {
                text_edit_props = text_edit_props.with_min(min);
            }

            if let Some(max) = max {
                text_edit_props = text_edit_props.with_max(max);
            }

            (VectorComponentIndex(i), text_edit(text_edit_props))
        })
        .collect();

    (
        EditorVectorEdit,
        Node {
            width: percent(100),
            column_gap: px(12),
            align_items: AlignItems::FlexEnd,
            ..default()
        },
        Children::spawn(SpawnIter(children.into_iter())),
    )
}

/// Returns the accent color for a vector axis label (XYZ suffixes only).
fn axis_color(suffixes: &VectorSuffixes, index: usize) -> Option<Color> {
    match suffixes {
        VectorSuffixes::XYZ | VectorSuffixes::XY => match index {
            0 => Some(tokens::AXIS_X_COLOR),
            1 => Some(tokens::AXIS_Y_COLOR),
            2 => Some(tokens::AXIS_Z_COLOR),
            _ => None,
        },
        _ => None,
    }
}
