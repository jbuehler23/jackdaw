//! Viewport instances for project components tagged with `@EditorPreview`.
//!
//! Project types are schema data in the editor, not live ECS types, so this
//! walks each scene entity's document type paths, looks up `ProjectTypes`,
//! and instances a glTF scene. Preview children are never registered in
//! the scene document.

use bevy::gltf::GltfAssetLabel;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;
use jackdaw_bsn::{AstNodeRef, SceneBsnAst};
use jackdaw_scene_types::{Brush, GltfSource};
use jackdaw_schema::PreviewSchema;

use crate::project_types::ProjectTypes;
use crate::{AppState, EditorEntity, EditorHidden, NonSerializable, SkipSerialization};

/// Child spawned under a marker so viewport clicks hit the preview visual.
#[derive(Component)]
pub(crate) struct SchemaPreview {
    spec: PreviewSchema,
}

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
    project_types: Res<ProjectTypes>,
    asset_server: Res<AssetServer>,
    hosts: Query<
        (Entity, &AstNodeRef),
        (
            With<Transform>,
            Without<EditorEntity>,
            Without<SchemaPreview>,
        ),
    >,
    existing: Query<(Entity, &ChildOf, &SchemaPreview)>,
    brushes: Query<(), With<Brush>>,
    gltf_sources: Query<(), With<GltfSource>>,
    mesh3d: Query<(), With<Mesh3d>>,
) {
    let Some(ast) = ast else {
        return;
    };

    let mut desired: HashMap<Entity, PreviewSchema> = HashMap::default();
    for (entity, ast_ref) in &hosts {
        if has_authored_visual(entity, &brushes, &gltf_sources, &mesh3d) {
            continue;
        }
        let Some(preview) = preview_for_node(&ast, ast_ref.patches_entity, &project_types) else {
            continue;
        };
        desired.insert(entity, preview);
    }

    let mut satisfied: HashMap<Entity, Entity> = HashMap::default();
    for (preview_entity, child_of, preview) in &existing {
        let host = child_of.0;
        match desired.get(&host) {
            Some(spec) if spec == &preview.spec => {
                satisfied.insert(host, preview_entity);
            }
            _ => {
                commands.entity(preview_entity).despawn();
            }
        }
    }

    for (host, spec) in desired {
        if satisfied.contains_key(&host) {
            continue;
        }
        spawn_preview(&mut commands, &asset_server, host, spec);
    }
}

fn preview_for_node(
    ast: &SceneBsnAst,
    node: Entity,
    project_types: &ProjectTypes,
) -> Option<PreviewSchema> {
    for type_path in ast.component_type_paths(node) {
        if let Some(preview) = project_types
            .component(&type_path)
            .and_then(|schema| schema.preview.clone())
        {
            return Some(preview);
        }
    }
    None
}

fn has_authored_visual(
    entity: Entity,
    brushes: &Query<(), With<Brush>>,
    gltf_sources: &Query<(), With<GltfSource>>,
    mesh3d: &Query<(), With<Mesh3d>>,
) -> bool {
    brushes.contains(entity) || gltf_sources.contains(entity) || mesh3d.contains(entity)
}

fn spawn_preview(
    commands: &mut Commands,
    asset_server: &AssetServer,
    host: Entity,
    spec: PreviewSchema,
) {
    let PreviewSchema::Gltf { path, scene } = spec.clone();
    let handle = asset_server.load(GltfAssetLabel::Scene(scene).from_asset(path));
    commands.spawn((
        SchemaPreview { spec },
        EditorHidden,
        NonSerializable,
        SkipSerialization,
        ChildOf(host),
        Transform::default(),
        Visibility::Inherited,
        WorldAssetRoot(handle),
    ));
}
