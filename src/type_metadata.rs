//! Per-type editor chrome: category, hidden, preview, description.
//!
//! `jackdaw_metadata.toml` next to `jackdaw.toml` is a sparse overlay.
//! Unspecified fields fall through to `@EditorCategory` / `@EditorHidden`
//! / `@EditorPreview` / `@EditorDescription` (or rustdoc).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::reflect::TypeInfo;
use jackdaw_runtime::{EditorCategory, EditorDescription, EditorHidden, EditorPreview};
use serde::{Deserialize, Serialize};

use crate::project::ProjectRoot;
use crate::project_types::ProjectTypes;

/// On-disk chrome for one reflected type. `None` means the file does not
/// mention that field.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

impl TypeMeta {
    fn is_empty(&self) -> bool {
        self.category.is_none()
            && self.description.is_none()
            && self.preview.is_none()
            && self.hidden.is_none()
    }
}

/// Resolved chrome for one type. Overlay fields replace crate defaults;
/// missing overlay fields keep the crate / schema value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeChrome {
    pub category: String,
    pub description: String,
    pub preview: String,
    pub hidden: bool,
}

impl TypeChrome {
    /// Grouping key for the picker and inspector card list: authored
    /// category, else the Bevy crate name / Avian / Jackdaw, else empty (Game).
    pub fn group(&self, type_path: &str) -> String {
        if !self.category.is_empty() {
            return self.category.clone();
        }
        if type_path.starts_with("avian3d::")
            || type_path.starts_with("jackdaw_avian_integration::")
        {
            "Avian3d".to_string()
        } else if type_path.starts_with("bevy") {
            type_path.split("::").next().unwrap_or("bevy").to_string()
        } else if type_path.starts_with("jackdaw") {
            "Jackdaw".to_string()
        } else {
            String::new()
        }
    }
}

fn is_game_type(type_path: &str) -> bool {
    !type_path.starts_with("avian3d::")
        && !type_path.starts_with("bevy")
        && !type_path.starts_with("jackdaw")
}

/// Greater rank appears first. Game `@EditorCategory` groups, then Game,
/// then engine groups (including engine types that have a category).
pub fn group_order(type_path: &str, authored_category: &str) -> i32 {
    if is_game_type(type_path) {
        if !authored_category.is_empty() { 3 } else { 2 }
    } else {
        1
    }
}

/// File contents of `<project>/jackdaw_metadata.toml`, keyed by reflect type path.
#[derive(Resource, Clone, Debug, Default)]
pub struct TypeMetadata {
    pub entries: BTreeMap<String, TypeMeta>,
}

impl TypeMetadata {
    pub fn get(&self, type_path: &str) -> Option<&TypeMeta> {
        self.entries.get(type_path)
    }

    /// Overlay, then registry attributes or project schema.
    pub fn resolve(
        &self,
        type_path: &str,
        registry: &bevy::reflect::TypeRegistry,
        project_types: &ProjectTypes,
    ) -> TypeChrome {
        let defaults = crate_defaults(type_path, registry, project_types);
        let Some(overlay) = self.get(type_path) else {
            return defaults;
        };
        TypeChrome {
            category: overlay.category.clone().unwrap_or(defaults.category),
            description: overlay.description.clone().unwrap_or(defaults.description),
            preview: overlay.preview.clone().unwrap_or(defaults.preview),
            hidden: overlay.hidden.unwrap_or(defaults.hidden),
        }
    }
}

pub struct TypeMetadataPlugin;

impl Plugin for TypeMetadataPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TypeMetadata>()
            .add_systems(OnEnter(crate::AppState::Editor), load_type_metadata);
    }
}

pub fn metadata_path(project_root: &Path) -> PathBuf {
    project_root.join("jackdaw_metadata.toml")
}

fn load_type_metadata(world: &mut World) {
    let Some(root) = world.get_resource::<ProjectRoot>().map(|p| p.root.clone()) else {
        return;
    };
    let entries = match load_metadata_file(&root) {
        LoadResult::Loaded(entries) => entries,
        LoadResult::Missing => BTreeMap::new(),
        LoadResult::Invalid => return,
    };
    world.resource_mut::<TypeMetadata>().entries = entries;
}

pub(crate) fn set_category(
    world: &mut World,
    project_root: &Path,
    type_path: &str,
    category: &str,
) -> std::io::Result<()> {
    patch_type_meta(world, project_root, type_path, |meta| {
        meta.category = overlay_string(category);
    })
}

pub(crate) fn set_description(
    world: &mut World,
    project_root: &Path,
    type_path: &str,
    description: &str,
) -> std::io::Result<()> {
    patch_type_meta(world, project_root, type_path, |meta| {
        meta.description = overlay_string(description);
    })
}

pub(crate) fn set_preview(
    world: &mut World,
    project_root: &Path,
    type_path: &str,
    preview: &str,
) -> std::io::Result<()> {
    patch_type_meta(world, project_root, type_path, |meta| {
        meta.preview = overlay_string(preview);
    })
}

fn overlay_string(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn patch_type_meta(
    world: &mut World,
    project_root: &Path,
    type_path: &str,
    patch: impl FnOnce(&mut TypeMeta),
) -> std::io::Result<()> {
    let mut entries = match load_metadata_file(project_root) {
        LoadResult::Loaded(entries) => entries,
        LoadResult::Missing => BTreeMap::new(),
        LoadResult::Invalid => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "jackdaw_metadata.toml could not be parsed",
            ));
        }
    };
    let meta = entries.entry(type_path.to_string()).or_default();
    patch(meta);
    if meta.is_empty() {
        entries.remove(type_path);
    }
    write_metadata_file(project_root, &entries)?;
    world.resource_mut::<TypeMetadata>().entries = entries;
    Ok(())
}

/// If type is in the registry, get its chrome values, otherwise get values from the schema or default.
fn crate_defaults(
    type_path: &str,
    registry: &bevy::reflect::TypeRegistry,
    project_types: &ProjectTypes,
) -> TypeChrome {
    if let Some(registration) = registry.get_with_type_path(type_path) {
        let info = registration.type_info();
        let attrs = type_info_custom_attributes(info);
        let category = attrs
            .and_then(|a| a.get::<EditorCategory>())
            .map(|c| c.0.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        let description = attrs
            .and_then(|a| a.get::<EditorDescription>())
            .map(|d| d.0.to_string())
            .or_else(|| {
                info.docs()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_default();
        let preview = attrs
            .and_then(|a| a.get::<EditorPreview>())
            .map(|p| p.0.to_string())
            .unwrap_or_default();
        let hidden = attrs.is_some_and(|a| a.get::<EditorHidden>().is_some());
        return TypeChrome {
            category,
            description,
            preview,
            hidden,
        };
    }
    if let Some(schema) = project_types.component(type_path) {
        let description = nonempty(&schema.editor_description)
            .or_else(|| nonempty(&schema.description))
            .unwrap_or_default();
        return TypeChrome {
            category: nonempty(&schema.category).unwrap_or_default(),
            description,
            preview: nonempty(&schema.preview).unwrap_or_default(),
            hidden: schema.hidden,
        };
    }
    TypeChrome::default()
}

fn type_info_custom_attributes(
    info: &TypeInfo,
) -> Option<&bevy::reflect::attributes::CustomAttributes> {
    match info {
        TypeInfo::Struct(s) => Some(s.custom_attributes()),
        TypeInfo::TupleStruct(s) => Some(s.custom_attributes()),
        TypeInfo::Enum(e) => Some(e.custom_attributes()),
        _ => None,
    }
}

fn nonempty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

enum LoadResult {
    Loaded(BTreeMap<String, TypeMeta>),
    Missing,
    Invalid,
}

fn load_metadata_file(project_root: &Path) -> LoadResult {
    let path = metadata_path(project_root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return LoadResult::Missing;
        }
        Err(err) => {
            warn!("failed to read {}: {err}", path.display());
            return LoadResult::Invalid;
        }
    };
    if text.trim().is_empty() {
        return LoadResult::Loaded(BTreeMap::new());
    }
    match parse_metadata(&text) {
        Ok(entries) => LoadResult::Loaded(entries),
        Err(err) => {
            warn!("failed to parse {}: {err}", path.display());
            LoadResult::Invalid
        }
    }
}

fn write_metadata_file(
    project_root: &Path,
    entries: &BTreeMap<String, TypeMeta>,
) -> std::io::Result<()> {
    let path = metadata_path(project_root);
    let text = emit_metadata(entries)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    let _ = std::fs::remove_file(&path);
    std::fs::rename(&tmp, &path)
}

fn parse_metadata(text: &str) -> Result<BTreeMap<String, TypeMeta>, String> {
    let entries: BTreeMap<String, TypeMeta> = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .filter(|(_, meta)| !meta.is_empty())
        .collect())
}

fn emit_metadata(entries: &BTreeMap<String, TypeMeta>) -> Result<String, toml::ser::Error> {
    let entries: BTreeMap<&str, &TypeMeta> = entries
        .iter()
        .filter(|(_, meta)| !meta.is_empty())
        .map(|(type_path, meta)| (type_path.as_str(), meta))
        .collect();
    toml::to_string_pretty(&entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Reflect)]
    #[reflect(
        @EditorCategory::new("Actor"),
        @EditorDescription::new("from crate"),
        @EditorPreview::gltf("models/flag.glb")
    )]
    struct CrateChrome;

    fn sample_entries() -> BTreeMap<String, TypeMeta> {
        let mut entries = BTreeMap::new();
        entries.insert(
            "my_game::PlayerSpawn".into(),
            TypeMeta {
                category: Some("Actor".into()),
                description: Some("Where the player respawns.".into()),
                preview: Some("models/player.glb".into()),
                hidden: None,
            },
        );
        entries.insert(
            "my_game::Internal".into(),
            TypeMeta {
                hidden: Some(true),
                ..Default::default()
            },
        );
        entries
    }

    fn schema_entry(type_path: &str) -> jackdaw_schema::TypeSchema {
        jackdaw_schema::TypeSchema {
            type_path: type_path.into(),
            short_name: type_path.rsplit("::").next().unwrap_or(type_path).into(),
            module_path: "my_game".into(),
            category: "Actor".into(),
            description: "Rustdoc for the type.".into(),
            editor_description: "A checkpoint.".into(),
            hidden: false,
            preview: "models/flag.glb".into(),
            default_constructible: true,
            fields: Vec::new(),
            kind: jackdaw_schema::TypeKind::Marker,
            default: None,
        }
    }

    fn project_types_with(schema: jackdaw_schema::TypeSchema) -> ProjectTypes {
        let mut project_types = ProjectTypes::default();
        project_types.update(
            &jackdaw_schema::ProjectSchema {
                components: vec![schema],
                resources: Vec::new(),
            },
            &std::collections::HashSet::new(),
        );
        project_types
    }

    fn empty_world() -> World {
        let mut world = World::new();
        world.init_resource::<TypeMetadata>();
        world
    }

    #[test]
    fn round_trips_category_preview_hidden_and_description() {
        let text = emit_metadata(&sample_entries()).expect("emit metadata");
        assert!(text.contains("[\"my_game::PlayerSpawn\"]"));
        assert!(text.contains("category = \"Actor\""));
        assert!(text.contains("preview = \"models/player.glb\""));
        assert!(text.contains("hidden = true"));

        let parsed = parse_metadata(&text).expect("parse emitted metadata");
        let spawn = parsed.get("my_game::PlayerSpawn").expect("PlayerSpawn");
        assert_eq!(spawn.category.as_deref(), Some("Actor"));
        assert_eq!(spawn.preview.as_deref(), Some("models/player.glb"));
        assert_eq!(
            spawn.description.as_deref(),
            Some("Where the player respawns.")
        );
        assert_eq!(
            parsed.get("my_game::Internal").and_then(|m| m.hidden),
            Some(true)
        );
    }

    #[test]
    fn schema_chrome_is_used_when_file_and_registry_miss() {
        let metadata = TypeMetadata::default();
        let registry = bevy::reflect::TypeRegistry::default();
        let project_types = project_types_with(schema_entry("my_game::Checkpoint"));
        let chrome = metadata.resolve("my_game::Checkpoint", &registry, &project_types);

        assert_eq!(chrome.category, "Actor");
        assert_eq!(chrome.description, "A checkpoint.");
        assert_eq!(chrome.preview, "models/flag.glb");
        assert!(!chrome.hidden);

        let mut hidden = schema_entry("my_game::Internal");
        hidden.hidden = true;
        assert!(
            metadata
                .resolve("my_game::Internal", &registry, &project_types_with(hidden))
                .hidden
        );
    }

    #[test]
    fn schema_rustdoc_is_used_when_editor_description_is_empty() {
        let mut schema = schema_entry("my_game::Checkpoint");
        schema.editor_description.clear();
        let metadata = TypeMetadata::default();
        let registry = bevy::reflect::TypeRegistry::default();
        let project_types = project_types_with(schema);
        assert_eq!(
            metadata
                .resolve("my_game::Checkpoint", &registry, &project_types)
                .description,
            "Rustdoc for the type."
        );
    }

    #[test]
    fn file_overrides_schema_per_field_and_leaves_the_rest() {
        let mut metadata = TypeMetadata::default();
        metadata.entries.insert(
            "my_game::Checkpoint".into(),
            TypeMeta {
                category: Some("Gameplay".into()),
                ..Default::default()
            },
        );
        let registry = bevy::reflect::TypeRegistry::default();
        let project_types = project_types_with(schema_entry("my_game::Checkpoint"));
        let chrome = metadata.resolve("my_game::Checkpoint", &registry, &project_types);

        assert_eq!(chrome.category, "Gameplay");
        assert_eq!(chrome.preview, "models/flag.glb");
        assert_eq!(chrome.description, "A checkpoint.");
        assert_eq!(
            metadata.resolve("my_game::Other", &registry, &project_types),
            TypeChrome::default()
        );
    }

    #[test]
    fn registry_chrome_is_used_then_overlaid_per_field() {
        let mut registry = bevy::reflect::TypeRegistry::default();
        registry.register::<CrateChrome>();
        let path = CrateChrome::type_path();
        let project_types = ProjectTypes::default();

        let defaults = TypeMetadata::default().resolve(path, &registry, &project_types);
        assert_eq!(defaults.category, "Actor");
        assert_eq!(defaults.description, "from crate");
        assert_eq!(defaults.preview, "models/flag.glb");

        let mut metadata = TypeMetadata::default();
        metadata.entries.insert(
            path.to_string(),
            TypeMeta {
                category: Some("Gameplay".into()),
                ..Default::default()
            },
        );
        let chrome = metadata.resolve(path, &registry, &project_types);
        assert_eq!(chrome.category, "Gameplay");
        assert_eq!(chrome.description, "from crate");
        assert_eq!(chrome.preview, "models/flag.glb");
    }

    #[test]
    fn group_falls_back_from_type_path() {
        let unspecified = TypeChrome::default();
        for (path, expected) in [
            ("avian3d::dynamics::rigid_body::RigidBody", "Avian3d"),
            ("jackdaw_avian_integration::AvianCollider", "Avian3d"),
            ("bevy_pbr::pbr_material::StandardMaterial", "bevy_pbr"),
            ("bevy::ecs::Name", "bevy"),
            ("jackdaw_scene_types::Brush", "Jackdaw"),
            ("my_game::PlayerSpawn", ""),
        ] {
            assert_eq!(unspecified.group(path), expected, "{path}");
        }

        let authored = TypeChrome {
            category: "Gameplay".into(),
            ..Default::default()
        };
        assert_eq!(
            authored.group("avian3d::dynamics::rigid_body::RigidBody"),
            "Gameplay"
        );
    }

    #[test]
    fn game_editor_categories_rank_above_game_and_engine() {
        assert!(group_order("my_game::PlayerSpawn", "Actor") > group_order("my_game::Health", ""));
        assert!(group_order("my_game::Health", "") > group_order("bevy_transform::Transform", ""));
        assert_eq!(
            group_order("jackdaw_scene_types::Brush", "Mesh"),
            group_order("bevy_transform::Transform", ""),
        );
        assert_eq!(
            group_order("jackdaw_scene_types::Brush", "Mesh"),
            group_order("avian3d::dynamics::rigid_body::RigidBody", ""),
        );
    }

    #[test]
    fn set_category_persists_and_drops_empty_entries() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let mut world = empty_world();

        set_category(&mut world, tmp.path(), "my_game::PlayerSpawn", "Actor")
            .expect("write category");
        assert_eq!(
            world
                .resource::<TypeMetadata>()
                .get("my_game::PlayerSpawn")
                .and_then(|m| m.category.as_deref()),
            Some("Actor")
        );
        let text = std::fs::read_to_string(metadata_path(tmp.path())).expect("read metadata");
        assert!(text.contains("category = \"Actor\""));

        set_category(&mut world, tmp.path(), "my_game::PlayerSpawn", "").expect("clear category");
        assert!(
            world
                .resource::<TypeMetadata>()
                .get("my_game::PlayerSpawn")
                .is_none()
        );
        let parsed = parse_metadata(
            &std::fs::read_to_string(metadata_path(tmp.path())).expect("read after clear"),
        )
        .expect("parse after clear");
        assert!(parsed.is_empty());
    }

    #[test]
    fn set_category_rejects_unparseable_file() {
        let tmp = tempfile::tempdir().expect("temp dir");
        std::fs::write(metadata_path(tmp.path()), "not valid toml {{{").expect("write junk");
        let mut world = empty_world();
        let err = set_category(&mut world, tmp.path(), "my_game::PlayerSpawn", "Actor")
            .expect_err("invalid file");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(world.resource::<TypeMetadata>().entries.is_empty());
    }

    #[test]
    fn set_description_keeps_other_overlay_fields() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let mut world = empty_world();

        set_category(&mut world, tmp.path(), "my_game::PlayerSpawn", "Actor")
            .expect("write category");
        set_description(
            &mut world,
            tmp.path(),
            "my_game::PlayerSpawn",
            "Where the player respawns.",
        )
        .expect("write description");

        let meta = world
            .resource::<TypeMetadata>()
            .get("my_game::PlayerSpawn")
            .cloned()
            .expect("overlay entry");
        assert_eq!(meta.category.as_deref(), Some("Actor"));
        assert_eq!(
            meta.description.as_deref(),
            Some("Where the player respawns.")
        );

        set_description(&mut world, tmp.path(), "my_game::PlayerSpawn", "")
            .expect("clear description");
        assert_eq!(
            world
                .resource::<TypeMetadata>()
                .get("my_game::PlayerSpawn")
                .and_then(|m| m.category.as_deref()),
            Some("Actor")
        );
        assert!(
            world
                .resource::<TypeMetadata>()
                .get("my_game::PlayerSpawn")
                .and_then(|m| m.description.as_deref())
                .is_none()
        );
    }

    #[test]
    fn set_description_round_trips_newlines() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let mut world = empty_world();
        set_description(
            &mut world,
            tmp.path(),
            "my_game::PlayerSpawn",
            "Line one.\nLine two.",
        )
        .expect("write description");
        let parsed = parse_metadata(
            &std::fs::read_to_string(metadata_path(tmp.path())).expect("read metadata"),
        )
        .expect("parse metadata");
        assert_eq!(
            parsed
                .get("my_game::PlayerSpawn")
                .and_then(|m| m.description.as_deref()),
            Some("Line one.\nLine two.")
        );
    }
}
