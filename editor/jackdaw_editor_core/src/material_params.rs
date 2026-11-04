//! Materials from values: the fields `material.create` and `material.set`
//! take, the presets a new material starts from, and the command that edits a
//! material as one undo step.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_commands::EditorCommand;
use jackdaw_scene_types::PropertyValue;

use crate::brush::topology_ops::select_elements::refuse;
use crate::commands::CommandHistory;
use crate::material_assets::{MaterialRegistry, sanitize_material_name};

/// The fields a material takes by value, in the order the operators list them.
pub const MATERIAL_FIELDS: [&str; 8] = [
    "base_color",
    "roughness",
    "metallic",
    "reflectance",
    "emissive",
    "alpha",
    "texture",
    "uv_scale",
];

/// The fields `material.create` takes beside its preset.
pub const CREATE_FIELDS: [&str; 7] = [
    "base_color",
    "roughness",
    "metallic",
    "emissive",
    "alpha",
    "texture",
    "uv_scale",
];

/// The values a preset gives a new material.
pub struct MaterialPreset {
    pub name: &'static str,
    /// sRGB, as `#rrggbb`.
    pub base_color: &'static str,
    pub roughness: f32,
    pub metallic: f32,
    pub reflectance: f32,
    pub alpha: f32,
}

const fn preset(
    name: &'static str,
    base_color: &'static str,
    roughness: f32,
    metallic: f32,
) -> MaterialPreset {
    MaterialPreset {
        name,
        base_color,
        roughness,
        metallic,
        reflectance: 0.5,
        alpha: 1.0,
    }
}

pub const PRESETS: [MaterialPreset; 10] = [
    preset("stone", "#8c857b", 0.9, 0.0),
    preset("timber", "#7a5434", 0.8, 0.0),
    preset("plaster", "#e6dcc8", 0.95, 0.0),
    preset("thatch", "#c4a25c", 1.0, 0.0),
    preset("shingle", "#5e4f47", 0.85, 0.0),
    preset("cobble", "#7a756c", 0.95, 0.0),
    preset("foliage", "#5f8a3c", 0.9, 0.0),
    MaterialPreset {
        reflectance: 0.6,
        alpha: 0.8,
        ..preset("water", "#3f7fa3", 0.08, 0.0)
    },
    preset("metal", "#8d8f96", 0.35, 1.0),
    MaterialPreset {
        reflectance: 0.6,
        alpha: 0.3,
        ..preset("glass", "#cfe6ee", 0.05, 0.0)
    },
];

pub fn preset_named(name: &str) -> Option<&'static MaterialPreset> {
    PRESETS.iter().find(|preset| preset.name == name)
}

impl MaterialPreset {
    fn fields(&self) -> Vec<MaterialField> {
        let mut fields = vec![
            MaterialField::BaseColor(Srgba::hex(self.base_color).unwrap_or(Srgba::WHITE).into()),
            MaterialField::Roughness(self.roughness),
            MaterialField::Metallic(self.metallic),
            MaterialField::Reflectance(self.reflectance),
        ];
        if self.alpha < 1.0 {
            fields.push(MaterialField::Alpha(self.alpha));
        }
        fields
    }
}

/// One checked field value, ready to write.
#[derive(Clone, Debug, PartialEq)]
pub enum MaterialField {
    BaseColor(Color),
    Roughness(f32),
    Metallic(f32),
    Reflectance(f32),
    Emissive(LinearRgba),
    Alpha(f32),
    /// An image path under the assets folder; empty clears the slot.
    Texture(String),
    UvScale(Vec2),
}

impl MaterialField {
    /// Where the field is written in a call: the colour before the alpha, so
    /// an alpha always wins over the colour's own.
    fn rank(&self) -> usize {
        match self {
            Self::BaseColor(_) => 0,
            Self::Roughness(_) => 1,
            Self::Metallic(_) => 2,
            Self::Reflectance(_) => 3,
            Self::Emissive(_) => 4,
            Self::Alpha(_) => 5,
            Self::Texture(_) => 6,
            Self::UvScale(_) => 7,
        }
    }
}

/// The values of a material that a set of fields writes, so an edit can be
/// taken back without touching the fields it left alone.
#[derive(Clone, Debug, PartialEq, Default)]
struct FieldValues {
    color: Option<Color>,
    alpha: Option<f32>,
    alpha_mode: Option<AlphaMode>,
    roughness: Option<f32>,
    metallic: Option<f32>,
    reflectance: Option<f32>,
    emissive: Option<LinearRgba>,
    texture: Option<Option<Handle<Image>>>,
    uv_transform: Option<bevy::math::Affine2>,
}

impl FieldValues {
    fn read(fields: &[MaterialField], material: &StandardMaterial) -> Self {
        let mut values = Self::default();
        for field in fields {
            match field {
                MaterialField::BaseColor(_) => {
                    values.color = Some(material.base_color);
                    values.alpha_mode = Some(material.alpha_mode);
                }
                MaterialField::Alpha(_) => {
                    values.alpha = Some(material.base_color.alpha());
                    values.alpha_mode = Some(material.alpha_mode);
                }
                MaterialField::Roughness(_) => {
                    values.roughness = Some(material.perceptual_roughness);
                }
                MaterialField::Metallic(_) => values.metallic = Some(material.metallic),
                MaterialField::Reflectance(_) => values.reflectance = Some(material.reflectance),
                MaterialField::Emissive(_) => values.emissive = Some(material.emissive),
                MaterialField::Texture(_) => {
                    values.texture = Some(material.base_color_texture.clone());
                }
                MaterialField::UvScale(_) => values.uv_transform = Some(material.uv_transform),
            }
        }
        values
    }

    fn write(&self, material: &mut StandardMaterial) {
        if let Some(color) = self.color {
            material.base_color = color;
        }
        if let Some(alpha) = self.alpha {
            material.base_color.set_alpha(alpha);
        }
        if let Some(mode) = self.alpha_mode {
            material.alpha_mode = mode;
        }
        if let Some(value) = self.roughness {
            material.perceptual_roughness = value;
        }
        if let Some(value) = self.metallic {
            material.metallic = value;
        }
        if let Some(value) = self.reflectance {
            material.reflectance = value;
        }
        if let Some(value) = self.emissive {
            material.emissive = value;
        }
        if let Some(texture) = &self.texture {
            material.base_color_texture = texture.clone();
        }
        if let Some(transform) = self.uv_transform {
            material.uv_transform = transform;
        }
    }
}

/// A field value as a caller spelled it, before it is checked.
enum Raw {
    Number(f64),
    Text(String),
    Color(Color),
    Numbers(Vec<f32>),
}

impl Raw {
    fn of_property(value: &PropertyValue) -> Option<Self> {
        Some(match value {
            PropertyValue::Float(number) => Self::Number(*number),
            PropertyValue::Int(number) => Self::Number(*number as f64),
            PropertyValue::String(text) => Self::Text(text.to_string()),
            PropertyValue::Color(color) => Self::Color(*color),
            PropertyValue::Vec2(v) => Self::Numbers(v.to_array().to_vec()),
            PropertyValue::Vec3(v) => Self::Numbers(v.to_array().to_vec()),
            PropertyValue::Bool(_) | PropertyValue::Entity(_) => return None,
        })
    }

    fn of_json(value: &serde_json::Value) -> Option<Self> {
        Some(match value {
            serde_json::Value::Number(number) => Self::Number(number.as_f64()?),
            serde_json::Value::String(text) => Self::Text(text.clone()),
            serde_json::Value::Array(items) => Self::Numbers(
                items
                    .iter()
                    .map(|item| item.as_f64().map(|n| n as f32))
                    .collect::<Option<Vec<_>>>()?,
            ),
            _ => return None,
        })
    }

    fn number(&self) -> Option<f32> {
        let number = match self {
            Self::Number(number) => *number as f32,
            Self::Text(text) => text.trim().parse().ok()?,
            Self::Numbers(numbers) if numbers.len() == 1 => numbers[0],
            _ => return None,
        };
        number.is_finite().then_some(number)
    }

    fn numbers(&self) -> Option<Vec<f32>> {
        let numbers = match self {
            Self::Number(number) => vec![*number as f32],
            Self::Numbers(numbers) => numbers.clone(),
            Self::Text(text) => text
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|part| part.trim().parse::<f32>().ok())
                .collect::<Option<Vec<_>>>()?,
            Self::Color(_) => return None,
        };
        numbers.iter().all(|n| n.is_finite()).then_some(numbers)
    }

    /// `#rrggbb[aa]`, `r,g,b[,a]` (sRGB) or a colour value.
    fn color(&self) -> Option<Color> {
        match self {
            Self::Color(color) => Some(*color),
            Self::Text(text) if text.trim().starts_with('#') => {
                Srgba::hex(text.trim()).ok().map(Color::from)
            }
            _ => match self.numbers()?[..] {
                [r, g, b] => Some(Color::srgb(r, g, b)),
                [r, g, b, a] => Some(Color::srgba(r, g, b, a)),
                _ => None,
            },
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text.trim()),
            _ => None,
        }
    }
}

fn unit(field: &str, raw: &Raw) -> Result<f32, String> {
    raw.number()
        .filter(|n| (0.0..=1.0).contains(n))
        .ok_or_else(|| format!("`{field}` must be a number from 0 to 1"))
}

/// Check one field value. `assets` is the folder a texture path must name a
/// file under, when a project is open.
fn read_field(field: &str, raw: &Raw, assets: Option<&Path>) -> Result<MaterialField, String> {
    Ok(match field {
        "base_color" => {
            let color = raw
                .color()
                .map(Srgba::from)
                .filter(|c| {
                    [c.red, c.green, c.blue, c.alpha]
                        .iter()
                        .all(|n| (0.0..=1.0).contains(n))
                })
                .ok_or("`base_color` must be #rrggbb, #rrggbbaa or r,g,b[,a] from 0 to 1")?;
            MaterialField::BaseColor(color.into())
        }
        "roughness" => MaterialField::Roughness(unit(field, raw)?),
        "metallic" => MaterialField::Metallic(unit(field, raw)?),
        "reflectance" => MaterialField::Reflectance(unit(field, raw)?),
        "alpha" => MaterialField::Alpha(unit(field, raw)?),
        "emissive" => {
            let color = match raw {
                Raw::Text(text) if text.trim().starts_with('#') => {
                    raw.color().map(LinearRgba::from)
                }
                _ => raw.numbers().and_then(|n| match n[..] {
                    [r, g, b] => Some(LinearRgba::rgb(r, g, b)),
                    [r, g, b, a] => Some(LinearRgba::new(r, g, b, a)),
                    _ => None,
                }),
            }
            .filter(|c| [c.red, c.green, c.blue].iter().all(|n| *n >= 0.0))
            .ok_or("`emissive` must be #rrggbb or linear r,g,b, none below 0")?;
            MaterialField::Emissive(color)
        }
        "texture" => {
            let path = raw
                .text()
                .ok_or("`texture` must be an image path under the assets folder")?;
            if !path.is_empty()
                && let Some(assets) = assets
                && !assets.join(path).is_file()
            {
                return Err(format!("`texture` {path:?} names no file in this project"));
            }
            MaterialField::Texture(path.to_string())
        }
        "uv_scale" => {
            let scale = match raw.numbers().as_deref() {
                Some([s]) => Vec2::splat(*s),
                Some([u, v]) => Vec2::new(*u, *v),
                _ => return Err("`uv_scale` must be one number or u,v".into()),
            };
            if scale.x <= 0.0 || scale.y <= 0.0 {
                return Err("`uv_scale` must be above zero".into());
            }
            MaterialField::UvScale(scale)
        }
        other => {
            return Err(format!(
                "no material field `{other}`; the fields are {}",
                MATERIAL_FIELDS.join(", ")
            ));
        }
    })
}

/// Write `fields` onto `material`, the colour before the alpha whatever order
/// they came in. Touching the colour or the alpha picks the blend mode the
/// alpha needs.
pub fn apply_fields(
    fields: &[MaterialField],
    material: &mut StandardMaterial,
    server: &AssetServer,
) {
    let mut ordered: Vec<&MaterialField> = fields.iter().collect();
    ordered.sort_by_key(|field| field.rank());
    let mut blend = false;
    for field in ordered {
        match field {
            MaterialField::BaseColor(color) => {
                material.base_color = *color;
                blend = true;
            }
            MaterialField::Roughness(value) => material.perceptual_roughness = *value,
            MaterialField::Metallic(value) => material.metallic = *value,
            MaterialField::Reflectance(value) => material.reflectance = *value,
            MaterialField::Emissive(color) => material.emissive = *color,
            MaterialField::Alpha(value) => {
                material.base_color.set_alpha(*value);
                blend = true;
            }
            MaterialField::Texture(path) => {
                material.base_color_texture = (!path.is_empty()).then(|| {
                    server
                        .load_builder()
                        .with_settings(jackdaw_scene_types::render_assets::drawn_texture_settings(
                            false,
                        ))
                        .load::<Image>(path.clone())
                });
            }
            MaterialField::UvScale(scale) => {
                material.uv_transform = bevy::math::Affine2::from_scale(*scale);
            }
        }
    }
    if blend {
        if material.base_color.alpha() < 1.0 {
            material.alpha_mode = AlphaMode::Blend;
        } else if material.alpha_mode == AlphaMode::Blend {
            material.alpha_mode = AlphaMode::Opaque;
        }
    }
}

fn assets_dir(world: &World) -> Option<PathBuf> {
    world
        .get_resource::<crate::project::ProjectRoot>()
        .map(crate::project::ProjectRoot::assets_dir)
}

/// The fields `keys` name in `params`, checked, in the order of `keys`.
fn fields_of_params(
    world: &World,
    params: &OperatorParameters,
    keys: &[&str],
) -> Result<Vec<MaterialField>, String> {
    let assets = assets_dir(world);
    let mut fields = Vec::new();
    for key in keys {
        let Some(value) = params.get(*key) else {
            continue;
        };
        let raw = Raw::of_property(value).ok_or_else(|| format!("`{key}` has no value"))?;
        fields.push(read_field(key, &raw, assets.as_deref())?);
    }
    Ok(fields)
}

/// The fields a `material.set` call asks for: one `field` and its `value`, or
/// a `fields` object of many.
fn requested_fields(
    world: &World,
    params: &OperatorParameters,
) -> Result<Vec<MaterialField>, String> {
    let assets = assets_dir(world);
    match (params.get("fields"), params.as_str("field")) {
        (Some(_), Some(_)) => Err("give `field` and `value`, or `fields`, not both".into()),
        (Some(_), None) => {
            let text = params
                .as_str("fields")
                .ok_or("`fields` must be an object of field names to values")?;
            let object: serde_json::Map<String, serde_json::Value> = serde_json::from_str(text)
                .map_err(|_| "`fields` must be an object of field names to values".to_string())?;
            if object.is_empty() {
                return Err("`fields` names no field".into());
            }
            let mut fields = Vec::new();
            for (name, value) in &object {
                let raw = Raw::of_json(value).ok_or_else(|| {
                    format!("`{name}` must be a number, text or a list of numbers")
                })?;
                fields.push(read_field(name, &raw, assets.as_deref())?);
            }
            Ok(fields)
        }
        (None, Some(field)) => {
            let value = params
                .get("value")
                .and_then(Raw::of_property)
                .ok_or("`value` is missing")?;
            Ok(vec![read_field(field, &value, assets.as_deref())?])
        }
        (None, None) => Err("name a `field` and its `value`, or give `fields`".into()),
    }
}

/// The standard material a reference names: a palette name, a file path or a
/// bare material name.
pub fn material_named(world: &World, reference: &str) -> Option<Handle<StandardMaterial>> {
    crate::material_assets::material_of_reference(
        world.get_resource::<crate::asset_index::AssetIndex>(),
        world.get_resource::<MaterialRegistry>()?,
        reference,
    )
    .filter(|handle| *handle != Handle::default())
}

/// Write the fields one `material.set` call changed, as one undo step. Undo
/// puts back only those fields, so later edits to the others survive it.
pub struct SetMaterialValue {
    handle: Handle<StandardMaterial>,
    old: FieldValues,
    new: FieldValues,
}

impl SetMaterialValue {
    fn put(&self, world: &mut World, values: &FieldValues) {
        if let Some(mut material) = world
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(&self.handle)
        {
            values.write(&mut material);
        }
        world
            .get_resource_or_init::<crate::material_assets::EditedMaterials>()
            .edited(&self.handle);
        if let Some(mut preview) =
            world.get_resource_mut::<crate::material_preview::MaterialPreviewState>()
        {
            preview.set_changed();
        }
    }
}

impl EditorCommand for SetMaterialValue {
    fn execute(&mut self, world: &mut World) {
        self.put(world, &self.new.clone());
    }

    fn undo(&mut self, world: &mut World) {
        self.put(world, &self.old.clone());
    }

    fn description(&self) -> &str {
        "Set material"
    }
}

/// Set one field of a material, or several at once, as one undo step. Every
/// value is checked before any is written.
#[operator(
    id = "material.set",
    writes_disk = true,
    label = "Set Material",
    description = "Set fields of a material by value, such as its colour or roughness.",
    allows_undo = false,
    params(
        material(
            String,
            doc = "The material: a palette name, a file path such as materials/stone.bsn, or a \
                   material name."
        ),
        field(
            String,
            doc = "The field to set: base_color, roughness, metallic, reflectance, emissive, \
                   alpha, texture or uv_scale."
        ),
        value(
            String,
            doc = "The value for `field`: a number, #rrggbb or r,g,b for colours, an image path \
                   for texture, one number or u,v for uv_scale."
        ),
        fields(
            String,
            doc = "Several fields at once, as an object such as {\"roughness\":0.8,\
                   \"base_color\":\"#8c857b\"}, in place of `field` and `value`."
        ),
    )
)]
pub(crate) fn material_set(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    match set_material(world, &params) {
        Ok(outcome) => {
            set_outcome(world, outcome);
            OperatorResult::Finished
        }
        Err(why) => refuse(world, "material.set", why),
    }
}

fn set_material(world: &mut World, params: &OperatorParameters) -> Result<Outcome, String> {
    let reference = params
        .as_str("material")
        .ok_or("name the `material` to set")?;
    let handle = material_named(world, reference)
        .ok_or_else(|| format!("`material` {reference:?} names no material in this project"))?;
    let fields = requested_fields(world, params)?;
    let old = world
        .resource::<Assets<StandardMaterial>>()
        .get(&handle)
        .cloned()
        .ok_or_else(|| format!("`material` {reference:?} is not loaded"))?;
    let mut new = old.clone();
    apply_fields(&fields, &mut new, world.resource::<AssetServer>());
    let old = FieldValues::read(&fields, &old);
    let new = FieldValues::read(&fields, &new);
    if old == new {
        return Ok(Outcome::Unchanged);
    }
    let mut command: Box<dyn EditorCommand> = Box::new(SetMaterialValue { handle, old, new });
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(command);
    Ok(Outcome::Changed)
}

/// What a `material.create` call asked for, checked.
struct CreateRequest {
    name: String,
    fields: Vec<MaterialField>,
    path: Option<PathBuf>,
    palette: Option<String>,
}

fn read_create(world: &World, params: &OperatorParameters) -> Result<CreateRequest, String> {
    let registry = world
        .get_resource::<MaterialRegistry>()
        .ok_or("no material registry")?;
    let preset = match params.as_str("preset") {
        None => None,
        Some(name) => Some(preset_named(name).ok_or_else(|| {
            format!(
                "no preset `{name}`; the presets are {}",
                PRESETS
                    .iter()
                    .map(|p| p.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?),
    };
    let asked = fields_of_params(world, params, &CREATE_FIELDS)?;
    let colour_alpha = asked
        .iter()
        .any(|field| matches!(field, MaterialField::BaseColor(color) if color.alpha() < 1.0));
    let mut fields: Vec<MaterialField> = preset
        .map(MaterialPreset::fields)
        .unwrap_or_default()
        .into_iter()
        .filter(|field| {
            !asked.iter().any(|asked| asked.rank() == field.rank())
                && !(colour_alpha && matches!(field, MaterialField::Alpha(_)))
        })
        .collect();
    fields.extend(asked);

    let palette = match params.as_str("palette") {
        None => None,
        Some(slot) if crate::material_palette::is_palette_name(slot) => Some(slot.to_string()),
        Some(slot) => {
            return Err(format!(
                "`palette` {slot:?} must be 1 to 64 ASCII letters, digits, `_` or `-`"
            ));
        }
    };

    let taken = |name: &str| {
        registry
            .entries
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case(name))
    };
    let name = match params.as_str("name").map(str::trim) {
        Some(name) => {
            if name.is_empty() || sanitize_material_name(name) != name {
                return Err(format!(
                    "`name` {name:?} must be ASCII letters, digits, `_`, `-` or `.`"
                ));
            }
            if taken(name) {
                return Err(format!("a material named {name:?} already exists"));
            }
            name.to_string()
        }
        None => match preset {
            Some(preset) => std::iter::once(preset.name.to_string())
                .chain((2..).map(|n| format!("{}_{n}", preset.name)))
                .find(|name| !taken(name))
                .unwrap_or_default(),
            None => registry.next_created_name(),
        },
    };
    Ok(CreateRequest {
        name,
        fields,
        path: params.as_str("path").map(PathBuf::from),
        palette,
    })
}

/// Make a material from a preset and field values, save it when a project is
/// open, bind it in the palette when asked, and hand back its name.
pub fn create_material(world: &mut World, params: &OperatorParameters) -> Result<String, String> {
    let request = read_create(world, params)?;
    let mut value = StandardMaterial::default();
    apply_fields(&request.fields, &mut value, world.resource::<AssetServer>());
    let handle = world.resource_mut::<Assets<StandardMaterial>>().add(value);
    let name = request.name;
    world
        .resource_mut::<crate::asset_catalog::AssetCatalog>()
        .insert(format!("@{name}"), handle.clone().untyped());
    world
        .resource_mut::<MaterialRegistry>()
        .add(name.clone(), handle.clone());
    if let Some(mut preview) =
        world.get_resource_mut::<crate::material_preview::MaterialPreviewState>()
    {
        preview.active_material = Some(handle.clone());
        preview.orbit_yaw = 0.5;
        preview.orbit_pitch = -0.3;
        preview.zoom_distance = 3.0;
    }

    if world.contains_resource::<crate::project::ProjectRoot>() {
        crate::material_assets::write_and_promote(
            world,
            &handle,
            Some(&name),
            &name,
            request.path.as_deref(),
        );
        if !world.resource::<MaterialRegistry>().is_saved(&name) {
            warn_caller(
                world,
                format!("material.create: '{name}' was made but its file could not be written"),
            );
        }
    }

    if let Some(slot) = request.palette {
        let bound = crate::material_assets::filed_path(world, &handle)
            .ok_or_else(|| format!("'{name}' has no file to bind `{slot}` to"))
            .and_then(|path| crate::material_palette::bind_path(world, None, &slot, Some(path)));
        if let Err(why) = bound {
            warn_caller(world, format!("material.create: {why}"));
        }
    }
    Ok(name)
}
