use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Top-level `.jsn` file structure.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JsnScene {
    /// Format header with version info.
    pub jsn: JsnHeader,
    /// Scene metadata (name, author, timestamps).
    pub metadata: JsnMetadata,
    /// Asset manifest, lists referenced asset paths.
    pub assets: JsnAssets,
    /// Reserved for future editor state (camera bookmarks, snap settings, etc.).
    pub editor: Option<JsnEditorState>,
    /// Per-entity scene data with reflection-based components.
    pub scene: Vec<JsnEntity>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JsnTransform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl From<Transform> for JsnTransform {
    fn from(t: Transform) -> Self {
        Self {
            translation: t.translation,
            rotation: t.rotation,
            scale: t.scale,
        }
    }
}

impl From<JsnTransform> for Transform {
    fn from(t: JsnTransform) -> Self {
        Self {
            translation: t.translation,
            rotation: t.rotation,
            scale: t.scale,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JsnVisibility {
    #[default]
    Inherited,
    Visible,
    Hidden,
}

impl JsnVisibility {
    pub fn is_default(&self) -> bool {
        *self == Self::Inherited
    }
}

impl From<Visibility> for JsnVisibility {
    fn from(v: Visibility) -> Self {
        match v {
            Visibility::Inherited => Self::Inherited,
            Visibility::Visible => Self::Visible,
            Visibility::Hidden => Self::Hidden,
        }
    }
}

impl From<JsnVisibility> for Visibility {
    fn from(v: JsnVisibility) -> Self {
        match v {
            JsnVisibility::Inherited => Self::Inherited,
            JsnVisibility::Visible => Self::Visible,
            JsnVisibility::Hidden => Self::Hidden,
        }
    }
}

/// JSN v3 entity. All data is in components (Name, Transform, Visibility included).
/// Only `parent` remains structural (serialization ordering).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JsnEntity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub components: HashMap<String, serde_json::Value>,
}

/// Clipboard payload: entities plus any inline assets they reference.
/// Serialized to JSON text on the OS clipboard via `arboard`. Wrapping
/// the entity list lets cross-scene paste carry material defs from the
/// source scene's inline assets.
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct ClipboardPayload {
    pub entities: Vec<JsnEntity>,
    #[serde(default)]
    pub assets: JsnAssets,
}

/// Legacy v2 entity format, only used for migration.
#[derive(Deserialize, Clone, Debug)]
pub struct JsnEntityV2 {
    pub name: Option<String>,
    pub transform: Option<JsnTransform>,
    #[serde(default)]
    pub visibility: JsnVisibility,
    pub parent: Option<usize>,
    #[serde(default)]
    pub components: HashMap<String, serde_json::Value>,
}

/// Legacy v2 scene format, only used for migration.
#[derive(Deserialize, Clone, Debug)]
pub struct JsnSceneV2 {
    pub jsn: JsnHeader,
    pub metadata: JsnMetadata,
    #[serde(default)]
    pub assets: JsnAssets,
    pub editor: Option<JsnEditorState>,
    pub scene: Vec<JsnEntityV2>,
}

impl JsnSceneV2 {
    pub fn migrate_to_v3(self) -> JsnScene {
        let scene = self
            .scene
            .into_iter()
            .map(|e| {
                let mut components = e.components;
                if let Some(t) = e.transform {
                    components.insert(
                        "bevy_transform::components::transform::Transform".to_string(),
                        serde_json::json!({
                            "translation": [t.translation.x, t.translation.y, t.translation.z],
                            "rotation": [t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w],
                            "scale": [t.scale.x, t.scale.y, t.scale.z],
                        }),
                    );
                }
                match e.visibility {
                    JsnVisibility::Visible => {
                        components.insert(
                            "bevy_camera::visibility::Visibility".to_string(),
                            serde_json::Value::String("Visible".to_string()),
                        );
                    }
                    JsnVisibility::Hidden => {
                        components.insert(
                            "bevy_camera::visibility::Visibility".to_string(),
                            serde_json::Value::String("Hidden".to_string()),
                        );
                    }
                    JsnVisibility::Inherited => {}
                }
                if let Some(name) = e.name {
                    components.insert(
                        "bevy_ecs::name::Name".to_string(),
                        serde_json::Value::String(name),
                    );
                }
                JsnEntity {
                    parent: e.parent,
                    components,
                }
            })
            .collect();
        JsnScene {
            jsn: JsnHeader {
                format_version: [3, 0, 0],
                ..self.jsn
            },
            metadata: self.metadata,
            assets: self.assets,
            editor: self.editor,
            scene,
        }
    }
}

/// Format version and tool info.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JsnHeader {
    /// Semantic version triple `[major, minor, patch]`.
    pub format_version: [u32; 3],
    /// Version of the editor that wrote this file.
    pub editor_version: String,
    /// Bevy version used.
    pub bevy_version: String,
}

impl Default for JsnHeader {
    fn default() -> Self {
        Self {
            format_version: [3, 0, 0],
            editor_version: env!("CARGO_PKG_VERSION").to_string(),
            bevy_version: "0.18".to_string(),
        }
    }
}

/// Human-readable scene metadata.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct JsnMetadata {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub modified: String,
}

/// Generic asset table, keyed by type path, then by asset name.
///
/// JSON example:
/// ```json
/// "assets": {
///   "bevy_pbr::StandardMaterial": {
///     "BrickWall": { "base_color": ... },
///     "Metal": { "metallic": 1.0 }
///   }
/// }
/// ```
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct JsnAssets(pub HashMap<String, HashMap<String, serde_json::Value>>);

impl<'de> serde::Deserialize<'de> for JsnAssets {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        match serde_json::from_value(value) {
            Ok(map) => Ok(JsnAssets(map)),
            Err(_) => Ok(JsnAssets(HashMap::new())),
        }
    }
}

/// Editor-specific state persisted alongside the scene. Restored on
/// open so the user lands back where they left off.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct JsnEditorState {
    /// Last-known viewport camera transform. Restored on open so the
    /// user picks up the same framing they last had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<JsnTransform>,
}

/// Top-level `catalog.jsn` file structure for project-wide asset deduplication.
///
/// Uses the same `JsnAssets` format as scenes. Assets are referenced with `@Name`
/// prefix (vs `#Name` for scene-local inline assets).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JsnCatalog {
    /// Format header (same as scene files).
    pub jsn: JsnHeader,
    /// Project-wide named assets.
    pub assets: JsnAssets,
}

/// Top-level `project.jsn` file structure.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JsnProject {
    /// Format header (same as scene files).
    pub jsn: JsnHeader,
    /// Project configuration.
    pub project: JsnProjectConfig,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct JsnProjectConfig {
    /// Human-readable project name.
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Default scene to open (relative to project root, e.g. "assets/scenes/level1.jsn").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_scene: Option<String>,
    /// Persisted editor layout state (which windows in which areas, active tabs, area sizes).
    /// Format is opaque to the JSN crate; consumers parse it as `jackdaw_panels::LayoutState`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<serde_json::Value>,
    /// Scene paths (relative to project root) that were open in the
    /// editor's tab strip when the project was last closed. Restored
    /// in order on the next launch. Skipped if empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub last_open_tabs: Vec<String>,
    /// Index into `last_open_tabs` of the tab that was active.
    /// Clamped to range on load. Defaults to 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub last_active_tab: usize,
}
