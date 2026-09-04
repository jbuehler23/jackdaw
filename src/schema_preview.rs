//! Viewport instances for project components tagged with `@EditorPreview`.

use bevy::gltf::GltfAssetLabel;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;
use jackdaw_bsn::{AstNodeRef, SceneBsnAst};
use jackdaw_scene_types::{Brush, GltfSource};

use crate::project_types::ProjectTypes;
use crate::type_metadata::TypeMetadata;
use crate::{AppState, EditorEntity, EditorHidden, NonSerializable, SkipSerialization};

/// Child spawned under a marker so viewport clicks hit the preview visual.
#[derive(Component)]
pub(crate) struct SchemaPreview(String);

pub struct SchemaPreviewPlugin;

impl Plugin for SchemaPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            sync_schema_previews.run_if(in_state(AppState::Editor)),
        );
    }
}

fn sync_schema_previews(
    mut commands: Commands,
    ast: Option<Res<SceneBsnAst>>,
    type_metadata: Res<TypeMetadata>,
    type_registry: Res<AppTypeRegistry>,
    project_types: Res<ProjectTypes>,
    asset_server: Res<AssetServer>,
    hosts: Query<(Entity, &AstNodeRef), (With<Transform>, Without<EditorEntity>)>,
    existing: Query<(Entity, &ChildOf, &SchemaPreview)>,
    authored_visuals: Query<(), Or<(With<Brush>, With<GltfSource>, With<Mesh3d>)>>,
) {
    let Some(ast) = ast else {
        return;
    };

    let mut desired: HashMap<Entity, String> = HashMap::default();
    let registry = type_registry.read();
    for (entity, ast_ref) in &hosts {
        if authored_visuals.contains(entity) {
            continue;
        }
        let Some(preview) = preview_for_node(
            &ast,
            ast_ref.patches_entity,
            &type_metadata,
            &registry,
            &project_types,
        ) else {
            continue;
        };
        desired.insert(entity, preview);
    }

    let mut satisfied: HashSet<Entity> = HashSet::default();
    for (preview_entity, child_of, preview) in &existing {
        let host = child_of.0;
        match desired.get(&host) {
            Some(path) if path == &preview.0 => {
                satisfied.insert(host);
            }
            _ => {
                commands.entity(preview_entity).despawn();
            }
        }
    }

    for (host, spec) in desired {
        if satisfied.contains(&host) {
            continue;
        }
        spawn_preview(&mut commands, &asset_server, host, &spec);
    }
}

fn preview_for_node(
    ast: &SceneBsnAst,
    node: Entity,
    type_metadata: &TypeMetadata,
    registry: &bevy::reflect::TypeRegistry,
    project_types: &ProjectTypes,
) -> Option<String> {
    for type_path in ast.component_type_paths(node) {
        let path = type_metadata
            .resolve(&type_path, registry, project_types)
            .preview;
        if !path.is_empty() {
            return Some(path);
        }
    }
    None
}

fn spawn_preview(commands: &mut Commands, asset_server: &AssetServer, host: Entity, path: &str) {
    let path = path.to_string();
    let handle = asset_server.load(GltfAssetLabel::Scene(0).from_asset(path.clone()));
    commands.spawn((
        SchemaPreview(path),
        EditorHidden,
        NonSerializable,
        SkipSerialization,
        ChildOf(host),
        Transform::default(),
        Visibility::Inherited,
        WorldAssetRoot(handle),
    ));
}
