//! Viewport instances for project components tagged with `@EditorPreview`.

use bevy::ecs::system::SystemParam;
use bevy::gltf::GltfAssetLabel;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::world_serialization::{WorldAssetRoot, WorldInstanceReady, WorldInstanceSpawner};
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
        app.add_observer(unpin_schema_preview_instance).add_systems(
            Update,
            sync_schema_previews.run_if(in_state(AppState::Editor)),
        );
    }
}

/// Bevy's world-asset spawner treats `AssetEvent::Modified` as a hot reload
/// and rebuilds every instance of that scene. A glTF's meshes and textures
/// each fire that during (and after) the first load, which unparents some
/// preview meshes and leaves them at the origin. Forgetting the instance
/// once it is spawned keeps the entities and stops the rebuild.
fn unpin_schema_preview_instance(
    ready: On<WorldInstanceReady>,
    previews: Query<(), With<SchemaPreview>>,
    mut spawner: ResMut<WorldInstanceSpawner>,
) {
    if !previews.contains(ready.event_target()) {
        return;
    }
    spawner.unregister_instance(ready.event().instance_id);
}

/// What a preview depends on that can change on a host entity: its document
/// node, its transform, an authored visual that stands in for the preview, and
/// the preview itself.
#[derive(SystemParam)]
struct HostChanges<'w, 's> {
    changed: Query<
        'w,
        's,
        (),
        (
            With<AstNodeRef>,
            Or<(
                Changed<AstNodeRef>,
                Added<Transform>,
                Added<Brush>,
                Added<GltfSource>,
                Added<Mesh3d>,
            )>,
        ),
    >,
    removed_nodes: RemovedComponents<'w, 's, AstNodeRef>,
    removed_transforms: RemovedComponents<'w, 's, Transform>,
    removed_brushes: RemovedComponents<'w, 's, Brush>,
    removed_sources: RemovedComponents<'w, 's, GltfSource>,
    removed_meshes: RemovedComponents<'w, 's, Mesh3d>,
    removed_previews: RemovedComponents<'w, 's, SchemaPreview>,
}

impl HostChanges<'_, '_> {
    /// Whether any host changed since the last pass. Reads every removal
    /// list, so each removal is answered once.
    fn any(&mut self) -> bool {
        let removed = [
            self.removed_nodes.read().count(),
            self.removed_transforms.read().count(),
            self.removed_brushes.read().count(),
            self.removed_sources.read().count(),
            self.removed_meshes.read().count(),
            self.removed_previews.read().count(),
        ];
        !self.changed.is_empty() || removed.iter().any(|count| *count > 0)
    }
}

/// Spawn and despawn previews to match the document. Walks every host, so it
/// runs only when the document, the type chrome or a host has changed.
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
    mut host_changes: HostChanges,
) {
    let hosts_changed = host_changes.any();
    let Some(ast) = ast else {
        return;
    };
    if !hosts_changed
        && !ast.is_changed()
        && !type_metadata.is_changed()
        && !project_types.is_changed()
    {
        return;
    }

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
    let handle = asset_server
        .load_builder()
        .with_settings(jackdaw_scene_types::render_assets::model_settings)
        .load(GltfAssetLabel::Scene(0).from_asset(path.clone()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::type_metadata::TypeMeta;
    use bevy::world_serialization::WorldAsset;
    use jackdaw_bsn::BsnPatch;

    const FLAG: &str = "my_game::Flag";

    fn preview_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<WorldAsset>()
            .init_resource::<ProjectTypes>()
            .init_resource::<SceneBsnAst>()
            .add_systems(Update, sync_schema_previews);
        set_flag_preview(&mut app, "models/flag.glb");
        app.update();
        app
    }

    fn set_flag_preview(app: &mut App, path: &str) {
        let mut metadata = TypeMetadata::default();
        metadata.entries.insert(
            FLAG.into(),
            TypeMeta {
                preview: Some(path.into()),
                ..default()
            },
        );
        app.insert_resource(metadata);
    }

    fn spawn_host(app: &mut App, patches: Vec<BsnPatch>) -> Entity {
        let world = app.world_mut();
        let node = {
            let mut ast = world.resource_mut::<SceneBsnAst>();
            let node = ast.create_entity_node(patches);
            ast.add_to_roots(node);
            node
        };
        let host = world
            .spawn((
                Transform::default(),
                AstNodeRef {
                    patches_entity: node,
                },
            ))
            .id();
        world.resource_mut::<SceneBsnAst>().link(host, node);
        host
    }

    fn add_flag_to_node(app: &mut App, host: Entity) {
        let node = app.world().get::<AstNodeRef>(host).unwrap().patches_entity;
        let mut ast = app.world_mut().resource_mut::<SceneBsnAst>();
        let patch = ast.world.spawn(BsnPatch::Type(FLAG.into())).id();
        ast.get_patches_mut(node).unwrap().0.push(patch);
    }

    fn previews_of(app: &mut App, host: Entity) -> Vec<String> {
        let world = app.world_mut();
        world
            .query::<(&ChildOf, &SchemaPreview)>()
            .iter(world)
            .filter(|(child_of, _)| child_of.parent() == host)
            .map(|(_, preview)| preview.0.clone())
            .collect()
    }

    #[test]
    fn a_host_spawned_after_the_first_pass_gets_its_preview() {
        let mut app = preview_app();
        let host = spawn_host(&mut app, vec![BsnPatch::Type(FLAG.into())]);
        app.update();
        assert_eq!(previews_of(&mut app, host), vec!["models/flag.glb"]);
    }

    #[test]
    fn a_component_added_to_a_hosts_node_brings_its_preview() {
        let mut app = preview_app();
        let host = spawn_host(&mut app, Vec::new());
        app.update();
        assert!(previews_of(&mut app, host).is_empty());

        add_flag_to_node(&mut app, host);
        app.update();
        assert_eq!(previews_of(&mut app, host), vec!["models/flag.glb"]);
    }

    #[test]
    fn an_authored_visual_stands_in_for_the_preview_while_it_is_there() {
        let mut app = preview_app();
        let host = spawn_host(&mut app, vec![BsnPatch::Type(FLAG.into())]);
        app.update();
        assert_eq!(previews_of(&mut app, host).len(), 1);

        app.world_mut().entity_mut(host).insert(Mesh3d::default());
        app.update();
        assert!(previews_of(&mut app, host).is_empty());

        app.world_mut().entity_mut(host).remove::<Mesh3d>();
        app.update();
        assert_eq!(previews_of(&mut app, host), vec!["models/flag.glb"]);
    }

    #[test]
    fn a_preview_taken_down_elsewhere_comes_back() {
        let mut app = preview_app();
        let host = spawn_host(&mut app, vec![BsnPatch::Type(FLAG.into())]);
        app.update();
        let preview = app
            .world_mut()
            .query_filtered::<Entity, With<SchemaPreview>>()
            .single(app.world())
            .unwrap();
        app.world_mut().entity_mut(preview).despawn();
        app.update();
        assert_eq!(previews_of(&mut app, host), vec!["models/flag.glb"]);
    }

    #[test]
    fn a_new_preview_path_in_the_metadata_replaces_the_preview() {
        let mut app = preview_app();
        let host = spawn_host(&mut app, vec![BsnPatch::Type(FLAG.into())]);
        app.update();

        set_flag_preview(&mut app, "models/banner.glb");
        app.update();
        assert_eq!(previews_of(&mut app, host), vec!["models/banner.glb"]);
    }
}
