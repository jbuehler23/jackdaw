use bevy::prelude::*;

use crate::checkbox::{CheckboxProps, checkbox};
use crate::combobox::{ComboBoxOptionData, combobox};
use crate::icons::EditorFont;
use crate::text_edit::{TextEditProps, text_edit};
use crate::tokens::TEXT_MUTED_COLOR;
use crate::vector_edit::{VectorEditProps, VectorSuffixes, vector_edit};

pub fn plugin(app: &mut App) {
    app.add_systems(Update, setup_combobox_fields);
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    F32,
    F32Percent,
    I32,
    U32,
    Bool,
    String,
    Vector(VectorSuffixes),
    Color,
    ComboBox { options: Vec<ComboBoxOptionDef> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComboBoxOptionDef {
    pub label: String,
    pub value: String,
}

impl ComboBoxOptionDef {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

pub fn path_to_label(path: &str) -> String {
    let name = path.rsplit('.').next().unwrap_or(path);
    let mut label = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch == '_' {
            label.push(' ');
        } else if ch.is_uppercase() && i > 0 {
            label.push(' ');
            label.extend(ch.to_lowercase());
        } else if i == 0 {
            label.extend(ch.to_uppercase());
        } else {
            label.push(ch);
        }
    }
    label
}

pub struct InspectorFieldProps {
    path: String,
    kind: FieldKind,
    label: Option<String>,
    suffix: Option<String>,
    placeholder: Option<String>,
    min: Option<f32>,
    max: Option<f32>,
    combobox_options: Option<Vec<ComboBoxOptionData>>,
}

impl InspectorFieldProps {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind: FieldKind::F32,
            label: None,
            suffix: None,
            placeholder: None,
            min: None,
            max: None,
            combobox_options: None,
        }
    }

    pub fn percent(mut self) -> Self {
        self.kind = FieldKind::F32Percent;
        self
    }

    pub fn i32(mut self) -> Self {
        self.kind = FieldKind::I32;
        self
    }

    pub fn u32(mut self) -> Self {
        self.kind = FieldKind::U32;
        self
    }

    pub fn bool(mut self) -> Self {
        self.kind = FieldKind::Bool;
        self
    }

    pub fn string(mut self) -> Self {
        self.kind = FieldKind::String;
        self
    }

    pub fn vector(mut self, suffixes: VectorSuffixes) -> Self {
        self.kind = FieldKind::Vector(suffixes);
        self
    }

    pub fn color(mut self) -> Self {
        self.kind = FieldKind::Color;
        self
    }

    pub fn combobox(mut self, options: Vec<ComboBoxOptionData>) -> Self {
        self.kind = FieldKind::ComboBox {
            options: options
                .iter()
                .map(|o| {
                    let value = o.value.clone().unwrap_or_else(|| o.label.clone());
                    ComboBoxOptionDef::new(&o.label, value)
                })
                .collect(),
        };
        self.combobox_options = Some(options);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn with_min(mut self, min: f32) -> Self {
        self.min = Some(min);
        self
    }

    pub fn with_max(mut self, max: f32) -> Self {
        self.max = Some(max);
        self
    }

    fn inferred_label(&self) -> String {
        self.label
            .clone()
            .unwrap_or_else(|| path_to_label(&self.path))
    }

    fn inferred_suffix(&self) -> Option<&str> {
        if self.suffix.is_some() {
            return self.suffix.as_deref();
        }
        match self.kind {
            FieldKind::F32Percent => Some("%"),
            _ => None,
        }
    }

    fn inferred_min(&self) -> Option<f32> {
        if self.min.is_some() {
            return self.min;
        }
        match self.kind {
            FieldKind::F32Percent | FieldKind::U32 => Some(0.0),
            _ => None,
        }
    }

    fn inferred_max(&self) -> Option<f32> {
        if self.max.is_some() {
            return self.max;
        }
        match self.kind {
            FieldKind::F32Percent => Some(100.0),
            _ => None,
        }
    }

    fn is_integer(&self) -> bool {
        matches!(self.kind, FieldKind::U32 | FieldKind::I32)
    }
}

pub fn spawn_inspector_field(
    commands: &mut Commands,
    props: InspectorFieldProps,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) -> Entity {
    let label = props.inferred_label();

    if props.kind == FieldKind::Bool {
        return commands
            .spawn(checkbox(CheckboxProps::new(label), editor_font, icon_font))
            .id();
    }

    if let FieldKind::Vector(suffixes) = props.kind {
        let mut vec_props = VectorEditProps::default()
            .with_label(label)
            .with_size(suffixes.vector_size())
            .with_suffixes(suffixes);

        if let Some(suffix) = props.inferred_suffix() {
            vec_props = vec_props.with_suffix(suffix);
        }
        if let Some(min) = props.inferred_min() {
            vec_props = vec_props.with_min(min as f64);
        }
        if let Some(max) = props.inferred_max() {
            vec_props = vec_props.with_max(max as f64);
        }

        return commands.spawn(vector_edit(vec_props)).id();
    }

    if let Some(options) = props.combobox_options {
        return commands.spawn(combobox_field(label, options)).id();
    }

    // Default: text edit (numeric or string)
    let mut text_props = TextEditProps::default().with_label(label);

    if props.kind == FieldKind::String {
        // Plain text, no numeric mode
    } else if props.is_integer() {
        text_props = text_props.numeric_i32();
    } else {
        text_props = text_props.numeric_f32();
    }

    if let Some(suffix) = props.inferred_suffix() {
        text_props = text_props.with_suffix(suffix);
    }

    if let Some(ref placeholder) = props.placeholder {
        text_props = text_props.with_placeholder(placeholder);
    }

    if let Some(min) = props.inferred_min() {
        text_props = text_props.with_min(min as f64);
    }

    if let Some(max) = props.inferred_max() {
        text_props = text_props.with_max(max as f64);
    }

    commands.spawn(text_edit(text_props)).id()
}

#[derive(Component)]
pub(crate) struct ComboBoxFieldConfig {
    label: String,
    options: Vec<ComboBoxOptionData>,
    initialized: bool,
}

fn combobox_field(label: String, options: Vec<ComboBoxOptionData>) -> impl Bundle {
    (
        ComboBoxFieldConfig {
            label,
            options,
            initialized: false,
        },
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(3.0),
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: Val::Px(0.0),
            ..default()
        },
    )
}

fn setup_combobox_fields(
    mut commands: Commands,
    editor_font: Res<EditorFont>,
    mut configs: Query<(Entity, &mut ComboBoxFieldConfig)>,
) {
    let font = editor_font.0.clone();

    for (entity, mut config) in &mut configs {
        if config.initialized {
            continue;
        }
        config.initialized = true;

        // Spawn the label + combobox inside a world-exclusive closure
        // that first checks the row entity is still alive. The
        // previous `commands.spawn(...).id()` + `commands.entity(entity)
        // .add_children(&[...])` flow only checked liveness at queue
        // time; if `entity` was cascade-despawned before the queued
        // spawns flushed, the children ended up orphaned with
        // `ChildOf(dead entity)` references, producing the
        // `ChildOf(...) relates to an entity that does not exist` warns
        // on every inspector rebuild.
        let label = config.label.clone();
        let options = config.options.clone();
        let font = font.clone();
        commands.queue(move |world: &mut World| {
            let Ok(mut ec) = world.get_entity_mut(entity) else {
                return;
            };
            ec.with_children(|parent| {
                parent.spawn((
                    Text::new(label),
                    TextFont {
                        font: font.clone(),
                        font_size: 11.0,
                        weight: FontWeight::MEDIUM,
                        ..default()
                    },
                    TextColor(TEXT_MUTED_COLOR.into()),
                ));
                parent.spawn(combobox(options));
            });
        });
    }
}

pub fn fields_row() -> impl Bundle {
    Node {
        width: Val::Percent(100.0),
        column_gap: Val::Px(12.0),
        ..default()
    }
}
