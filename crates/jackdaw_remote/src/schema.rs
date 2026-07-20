use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level `.jackdaw/components.json` file.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ComponentsFile {
    pub header: ComponentsHeader,
    /// All component definitions, keyed by full type path.
    pub components: HashMap<String, ComponentDef>,
}

impl Default for ComponentsFile {
    fn default() -> Self {
        Self {
            header: ComponentsHeader {
                format_version: [1, 0, 0],
            },
            components: HashMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ComponentsHeader {
    pub format_version: [u32; 3],
}

/// A single component's editor definition.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ComponentDef {
    /// Editor category ("Combat", "Physics", "Audio").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Icon identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Field definitions, keyed by field name.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub fields: HashMap<String, FieldDef>,
}

/// A single field's editor definition.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct FieldDef {
    /// Reflect type path (e.g., "f32", "`bevy_math::Vec3`").
    #[serde(rename = "type")]
    pub type_path: String,
    /// Widget override: "slider", "`color_picker`", "`file_picker`",
    /// "dropdown", "`text_area`", "toggle", "angle", or auto-detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<String>,
    /// Numeric range [min, max].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    /// Numeric step size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// Override display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Tooltip description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Read-only in editor.
    #[serde(default, skip_serializing_if = "is_false")]
    pub read_only: bool,
    /// Hidden from inspector.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// Conditional visibility: field path of a bool field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !b
}

/// Extended registry response combining Bevy's type schema with Jackdaw's
/// component definitions. Returned by the `jackdaw/registry` BRP method.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ComponentRegistry {
    pub header: RegistryHeader,
    /// ISO timestamp of when this registry was extracted.
    pub extracted_at: String,
    /// Source connection info.
    pub source: RegistrySource,
    /// Raw `registry.schema` types from Bevy, keyed by type path.
    #[serde(default)]
    pub types: HashMap<String, serde_json::Value>,
    /// Component definitions (from `.jackdaw/components.json`), keyed by type path.
    #[serde(default)]
    pub components: HashMap<String, ComponentDef>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RegistryHeader {
    pub format_version: [u32; 3],
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RegistrySource {
    pub app_name: Option<String>,
    pub endpoint: String,
    pub bevy_version: String,
}
