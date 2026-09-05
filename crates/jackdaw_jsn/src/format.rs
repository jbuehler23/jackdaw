use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

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
    /// Stable node id (see `jackdaw_scene_types::SceneNodeId`). Absent in
    /// scenes authored before node ids existed; a fresh id is minted for
    /// those on first load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub components: HashMap<String, serde_json::Value>,
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
                    id: None,
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

/// Return the current type path for a component key found in a `.jsn` file,
/// or `None` when the key is already current. Scene component types moved out
/// of this crate into `jackdaw_scene_types`; files written before that move
/// still carry the old paths.
pub fn canonical_type_path(path: &str) -> Option<String> {
    if path == "jackdaw_jsn::ast::JsnNodeId" {
        return Some(jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH.to_string());
    }
    if let Some(rest) = path.strip_prefix("jackdaw_jsn::types::") {
        return Some(format!("jackdaw_scene_types::types::{rest}"));
    }
    if let Some(rest) = path.strip_prefix("jackdaw_jsn::editor_meta::") {
        return Some(format!("jackdaw_scene_types::{rest}"));
    }
    None
}

/// Rewrite every legacy component type path in `scene` to its current path,
/// including the keys of the asset manifest and the nested component maps
/// inside prefab baselines. Run once at the parse boundary so everything
/// downstream (registry lookups, AST keys) sees only current paths.
pub fn canonicalize_scene(scene: &mut JsnScene) {
    for entity in &mut scene.scene {
        canonicalize_components(&mut entity.components);
    }
    let assets = std::mem::take(&mut scene.assets.0);
    scene.assets.0 = assets
        .into_iter()
        .map(|(type_path, entries)| {
            let key = canonical_type_path(&type_path).unwrap_or(type_path);
            (key, entries)
        })
        .collect();
}

fn canonicalize_components(components: &mut HashMap<String, serde_json::Value>) {
    let entries = std::mem::take(components);
    for (type_path, mut value) in entries {
        let key = canonical_type_path(&type_path).unwrap_or(type_path);
        if key == "jackdaw_scene_types::types::JsnPrefabBaseline"
            && let Some(baseline_components) =
                value.get_mut("components").and_then(|c| c.as_object_mut())
        {
            let inner = std::mem::take(baseline_components);
            for (inner_path, inner_value) in inner {
                let inner_key = canonical_type_path(&inner_path).unwrap_or(inner_path);
                baseline_components.insert(inner_key, inner_value);
            }
        }
        components.insert(key, value);
    }
}

#[cfg(test)]
mod canonicalize_tests {
    use super::*;

    #[test]
    fn moved_type_paths_rewrite_to_scene_types() {
        assert_eq!(
            canonical_type_path("jackdaw_jsn::types::Brush").as_deref(),
            Some("jackdaw_scene_types::types::Brush")
        );
        assert_eq!(
            canonical_type_path("jackdaw_jsn::ast::JsnNodeId").as_deref(),
            Some(jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH)
        );
        assert_eq!(
            canonical_type_path("jackdaw_jsn::editor_meta::EditorHidden").as_deref(),
            Some("jackdaw_scene_types::EditorHidden")
        );
        assert_eq!(
            canonical_type_path("bevy_transform::components::transform::Transform"),
            None
        );
    }

    #[test]
    fn canonicalize_scene_rewrites_components_assets_and_baselines() {
        let json = serde_json::json!({
            "jsn": {"format_version": [3, 0, 0], "editor_version": "0.5.0", "bevy_version": "0.19"},
            "metadata": {"name": "t"},
            "assets": {
                "jackdaw_jsn::types::Terrain": {"#T": {}}
            },
            "editor": null,
            "scene": [{
                "components": {
                    "jackdaw_jsn::types::Brush": {"x": 1},
                    "jackdaw_jsn::types::JsnPrefabBaseline": {
                        "components": {"jackdaw_jsn::types::Terrain": {"y": 2}}
                    }
                }
            }]
        });
        let mut scene: JsnScene = serde_json::from_value(json).expect("fixture parses");
        canonicalize_scene(&mut scene);

        let components = &scene.scene[0].components;
        assert!(components.contains_key("jackdaw_scene_types::types::Brush"));
        let baseline = &components["jackdaw_scene_types::types::JsnPrefabBaseline"];
        assert!(
            baseline["components"]
                .as_object()
                .expect("baseline components object")
                .contains_key("jackdaw_scene_types::types::Terrain")
        );
        assert!(
            scene
                .assets
                .0
                .contains_key("jackdaw_scene_types::types::Terrain")
        );
    }
}

/// Header-only probe used to pick the right parser for a `.jsn` file.
#[derive(Deserialize)]
struct VersionProbe {
    jsn: JsnHeader,
}

/// Parse `.jsn` text into a current-format scene, dispatching on the header's
/// `format_version` (v2 files migrate to v3) and rewriting legacy component
/// type paths. Returns the scene together with the file's original version.
///
/// Version dispatch matters: v2 and v3 share their top-level field names and
/// every v3 entity field is optional, so v2 text also parses as v3, silently
/// dropping the structural name/transform/visibility fields. The header is
/// the only reliable discriminator.
pub fn parse_scene(text: &str) -> Result<(JsnScene, [u32; 3]), serde_json::Error> {
    let version = serde_json::from_str::<VersionProbe>(text)
        .map(|p| p.jsn.format_version)
        .unwrap_or([3, 0, 0]);

    let mut scene = if version[0] < 3 {
        serde_json::from_str::<JsnSceneV2>(text)?.migrate_to_v3()
    } else {
        match serde_json::from_str::<JsnScene>(text) {
            Ok(scene) => scene,
            // Belt and suspenders for files with a v3 header but a v2 body.
            Err(v3_err) => match serde_json::from_str::<JsnSceneV2>(text) {
                Ok(v2) => v2.migrate_to_v3(),
                Err(_) => return Err(v3_err),
            },
        }
    };
    canonicalize_scene(&mut scene);
    Ok((scene, version))
}
