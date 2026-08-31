use std::path::Path;

use bevy::{
    ecs::system::{SystemParam, SystemState},
    gltf::GltfAssetLabel,
    prelude::*,
};

use crate::{
    EditorEntity,
    commands::{CommandHistory, DespawnEntity, EditorCommand, HierarchyLocation, MoveEntity},
    selection::{Selected, Selection},
};

/// System clipboard for copy/paste of entities as scene text.
/// On Linux/X11 the clipboard is ownership-based: data is only available while
/// the Clipboard instance is alive. Storing as a Bevy Resource keeps it alive.
///
/// There is no `Default`: a machine with no clipboard has none of this, and
/// the plugin leaves the resource out rather than building one that cannot
/// work.
#[derive(Resource)]
pub struct SystemClipboard {
    clipboard: arboard::Clipboard,
    /// The text this editor last put on the OS clipboard.
    ///
    /// What makes a clipboard read recognisable as our own emission: text
    /// equal to this is the copy [`EntityClipboard`] mirrors, so a paste uses
    /// the mirror rather than re-reading text that a compositor may have
    /// re-encoded on the way through.
    last_emitted: String,
}

impl SystemClipboard {
    /// The OS clipboard image as owned RGBA8, or an error when the clipboard
    /// holds no image. Mirrors how the paste path reaches the inner clipboard.
    pub(crate) fn get_image(&mut self) -> Result<arboard::ImageData<'static>, arboard::Error> {
        self.clipboard.get_image()
    }
}

/// The last entity subtree copied in this editor, as BSN text.
///
/// The OS clipboard is the one users reach for, and it is where a copy is
/// written, so a subtree can be pasted into a text editor or into a second
/// Jackdaw window. It is also the one that can be absent (no compositor, a
/// headless run) or hold something that is not a scene, so this holds the
/// same text as the answer for those cases.
#[derive(Resource, Default)]
pub struct EntityClipboard {
    /// BSN text, empty until the first copy.
    pub text: String,
}

pub use jackdaw_scene_types::GltfSource;

pub struct EntityOpsPlugin;

impl Plugin for EntityOpsPlugin {
    fn build(&self, app: &mut App) {
        // Note: GltfSource type registration is handled by SceneTypesPlugin
        match arboard::Clipboard::new() {
            Ok(clipboard) => {
                app.insert_resource(SystemClipboard {
                    clipboard,
                    last_emitted: String::new(),
                });
            }
            Err(e) => {
                warn!("Failed to initialize system clipboard: {e}");
            }
        }
        app.init_resource::<EntityClipboard>()
            .add_observer(derive_world_asset_root)
            .register_type::<EmptyEntity>()
            .register_type::<SceneCamera>()
            .register_type::<SceneLight>()
            .register_type::<SceneFogVolume>()
            .register_type::<SceneReflectionProbe>()
            .register_type::<SceneAnimationPlayer>()
            .register_type::<SceneAudioSource>();
    }
}

/// Derive the render-side `WorldAssetRoot` from the authored `GltfSource`,
/// the way reference images and terrains derive their render state.
///
/// Undo, redo, tab swaps and file loads all re-insert `GltfSource` from the
/// document, so deriving it here is what brings the model back on each of
/// those paths without the handle ever being written to the document.
fn derive_world_asset_root(
    insert: On<Insert, GltfSource>,
    sources: Query<&GltfSource>,
    existing: Query<&WorldAssetRoot>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    let entity = insert.entity;
    let Ok(source) = sources.get(entity) else {
        return;
    };
    // Scenes authored before paths were normalised still hold an absolute
    // path; `to_asset_path` reduces those and passes a relative one through.
    let asset_path = to_asset_path(&source.path);
    let scene: Handle<bevy::world_serialization::WorldAsset> =
        asset_server.load(GltfAssetLabel::Scene(source.scene_index).from_asset(asset_path));
    // Re-inserting an equal handle still trips `Changed`, and the world-asset
    // spawner despawns and respawns the whole instance on every change.
    // Applying the document re-inserts `GltfSource` wholesale, so without this
    // the model is rebuilt on every undo.
    if existing.get(entity).is_ok_and(|root| root.0 == scene) {
        return;
    }
    commands.entity(entity).insert(WorldAssetRoot(scene));
}

/// Marks an entity as an intentionally-empty scene entity (`Add > Empty`).
/// Used by the viewport-overlay system to decide whether to draw a
/// fallback wireframe-cube marker. Serialises through the type registry
/// so empties loaded from a `.jsn` scene keep the marker.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct EmptyEntity;

/// Marks a camera as scene-authored (added via `Add > Camera` or by an
/// extension), so viewport overlays draw a frustum gizmo for it.
/// Editor-internal cameras (main viewport camera, material preview
/// camera) deliberately don't carry this marker.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneCamera;

/// Marks a light as scene-authored, so viewport overlays draw
/// light-specific gizmos for it. Editor-internal lights (e.g. the
/// material-preview rig) deliberately don't carry this marker.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneLight;

/// Marks a fog-volume entity (`Add > Fog Volume`), so viewport
/// overlays draw a box gizmo at the volume's extent. The box is the
/// unit cube scaled by the entity's `Transform.scale`.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneFogVolume;

/// Marks a reflection-probe entity (`Add > Reflection Probe`), so
/// viewport overlays draw a box gizmo at the probe's influence region.
/// The box is the unit cube scaled by the entity's `Transform.scale`.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneReflectionProbe;

/// Marks an animation-player entity (`Add > Animation Player`) so
/// viewport overlays draw a marker gizmo for it. The entity has no
/// spatial extent, so the marker is the only on-screen cue.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneAnimationPlayer;

/// Marks an audio-source entity (`Add > Audio Source`) so viewport
/// overlays draw a marker gizmo for it. The entity has no spatial
/// extent, so the marker is the only on-screen cue.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneAudioSource;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityTemplate {
    Empty,
    Cube,
    Sphere,
    PointLight,
    DirectionalLight,
    SpotLight,
    Camera3d,
    #[cfg(feature = "camera_rig")]
    CameraRig,
    Plane,
    Cylinder,
    Wedge,
    Cone,
    Pyramid,
    FogVolume,
    AnimationPlayer,
    AudioSource,
}

impl EntityTemplate {
    pub fn label(self) -> &'static str {
        match self {
            Self::Empty => "Empty Entity",
            Self::Cube => "Cube",
            Self::Sphere => "Sphere",
            Self::PointLight => "Point Light",
            Self::DirectionalLight => "Directional Light",
            Self::SpotLight => "Spot Light",
            Self::Camera3d => "Camera",
            #[cfg(feature = "camera_rig")]
            Self::CameraRig => "Camera Rig",
            Self::Plane => "Plane",
            Self::Cylinder => "Cylinder",
            Self::Wedge => "Wedge",
            Self::Cone => "Cone",
            Self::Pyramid => "Pyramid",
            Self::FogVolume => "Fog Volume",
            Self::AnimationPlayer => "Animation Player",
            Self::AudioSource => "Audio Source",
        }
    }
}

pub fn create_entity(
    commands: &mut Commands,
    template: EntityTemplate,
    selection: &mut Selection,
) -> Entity {
    let entity = match template {
        EntityTemplate::Empty => commands
            .spawn((
                Name::new("Empty"),
                EmptyEntity,
                Transform::default(),
                // Required so `InheritedVisibility` exists on the
                // entity. Without it, viewport-overlay systems that
                // gate on `InheritedVisibility` (e.g. the empty
                // wireframe gizmo) silently skip the entity.
                Visibility::default(),
            ))
            .id(),
        EntityTemplate::Cube => {
            let id = commands
                .spawn((
                    Name::new("Cube"),
                    crate::brush::Brush::cuboid(0.5, 0.5, 0.5),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::Sphere => {
            let id = commands
                .spawn((
                    Name::new("Sphere"),
                    crate::brush::Brush::sphere(0.5),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::PointLight => commands
            .spawn((
                Name::new("Point Light"),
                SceneLight,
                PointLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                Transform::from_xyz(0.0, 3.0, 0.0),
            ))
            .id(),
        EntityTemplate::DirectionalLight => commands
            .spawn((
                Name::new("Directional Light"),
                SceneLight,
                DirectionalLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, 0.4, 0.0))
                    .with_translation(Vec3 {
                        x: 0.0,
                        y: 10.0,
                        z: 0.0,
                    }),
            ))
            .id(),
        EntityTemplate::SpotLight => commands
            .spawn((
                Name::new("Spot Light"),
                SceneLight,
                SpotLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                Transform::from_xyz(0.0, 3.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
            ))
            .id(),
        EntityTemplate::Camera3d => commands
            .spawn((
                Name::new("Camera"),
                SceneCamera,
                Camera3d::default(),
                Camera {
                    // Scene cameras are authored inactive so they don't
                    // render over the editor viewport. They become active
                    // at play time (or via a future "preview through this
                    // camera" operator).
                    is_active: false,
                    ..default()
                },
                bevy::camera::RenderTarget::None {
                    size: UVec2::splat(1),
                },
                Transform::from_xyz(0.0, 2.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            ))
            .id(),
        #[cfg(feature = "camera_rig")]
        EntityTemplate::CameraRig => commands
            .spawn((
                Name::new("Camera Rig"),
                jackdaw_camera_rig::CameraRig::default(),
                Transform::default(),
                Visibility::default(),
            ))
            .id(),
        EntityTemplate::Plane => {
            let id = commands
                .spawn((
                    Name::new("Plane"),
                    crate::brush::Brush::plane(0.5, 0.5),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::Cylinder => {
            let id = commands
                .spawn((
                    Name::new("Cylinder"),
                    crate::brush::Brush::cylinder(0.5, 0.5, 16),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::Wedge => {
            let id = commands
                .spawn((
                    Name::new("Wedge"),
                    crate::brush::Brush::wedge(0.5, 0.5, 0.5),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::Cone => {
            let id = commands
                .spawn((
                    Name::new("Cone"),
                    crate::brush::Brush::cone(0.5, 0.5, 16),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::Pyramid => {
            let id = commands
                .spawn((
                    Name::new("Pyramid"),
                    crate::brush::Brush::pyramid(0.5, 0.5, 0.5),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::FogVolume => commands
            .spawn((
                Name::new("Fog Volume"),
                bevy::light::FogVolume::default(),
                SceneFogVolume,
                Transform::default(),
                Visibility::default(),
            ))
            .id(),
        EntityTemplate::AnimationPlayer => commands
            .spawn((
                Name::new("Animation Player"),
                SceneAnimationPlayer,
                Transform::default(),
                Visibility::default(),
            ))
            .id(),
        EntityTemplate::AudioSource => commands
            .spawn((
                Name::new("Audio Source"),
                SceneAudioSource,
                Transform::default(),
                Visibility::default(),
            ))
            .id(),
    };

    selection.select_single(commands, entity);
    entity
}

/// Returns a command that applies the last-used material to all faces of a brush entity.
fn apply_last_material(entity: Entity) -> impl FnOnce(&mut World) {
    move |world: &mut World| {
        let last_mat = world
            .resource::<crate::brush::LastUsedMaterial>()
            .material
            .clone();
        if let Some(mat) = last_mat
            && let Some(mut brush) = world.get_mut::<crate::brush::Brush>(entity)
        {
            for face in &mut brush.faces {
                face.material = mat.clone();
            }
        }
    }
}

/// Spawn a template into the live world and register it in the scene document.
pub fn spawn_template_in_document(world: &mut World, template: EntityTemplate) -> Entity {
    let mut system_state: SystemState<(Commands, ResMut<Selection>)> = SystemState::new(world);
    let Ok((mut commands, mut selection)) = system_state.get_mut(world) else {
        return Entity::PLACEHOLDER;
    };
    let entity = create_entity(&mut commands, template, &mut selection);
    system_state.apply(world);
    crate::scene_io::register_entity_in_ast(world, entity);
    entity
}

/// Give the world a scene document to register entities in, if it has none.
///
/// `register_entity_in_ast` returns silently when the resource is missing, so
/// anything that seeds a new scene has to call this first or its seed reaches
/// the live world and never the file.
pub(crate) fn ensure_scene_document(world: &mut World) {
    if !world.contains_resource::<jackdaw_bsn::SceneBsnAst>() {
        world.insert_resource(jackdaw_bsn::SceneBsnAst::default());
    }
}

/// Seed an empty live document with a directional light.
///
/// A 3D scene only: a UI document has nothing to light, and a light seeded
/// into one is written to disk as furniture the author never asked for. See
/// [`crate::scenes::operators::scene_new_configured`].
pub(crate) fn seed_new_scene_defaults(world: &mut World) {
    ensure_scene_document(world);
    spawn_template_in_document(world, EntityTemplate::DirectionalLight);
}

/// What [`seed_2d_scene_root`] names the root it makes. Space-free, so an
/// operator clause can address it as `name=Scene2d`.
pub const SCENE_2D_ROOT_NAME: &str = "Scene2d";

/// Seed the root a new 2D scene starts from: one marked, transformed node
/// sprites are parented to, and nothing else.
///
/// A 2D scene has no furniture to seed -- a directional light lights nothing
/// a sprite draws -- so the root exists to say which kind the document is.
/// The marker is a reflected component, so a save writes it into the BSN and
/// a reopened document is recognised as 2D again, the way `UiSceneRoot`
/// carries a UI scene's identity.
pub fn seed_2d_scene_root(world: &mut World) -> Entity {
    ensure_scene_document(world);
    let root = world
        .spawn((
            Name::new(SCENE_2D_ROOT_NAME),
            jackdaw_scene_types::Scene2dRoot,
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    crate::scene_io::register_entity_in_ast(world, root);
    crate::selection::select_only(world, root);
    root
}

/// World-access version of `create_entity`. Used from menu actions and other deferred contexts.
/// Pushes a `SpawnEntity` command so the addition can be undone.
pub fn create_entity_in_world(world: &mut World, template: EntityTemplate) {
    let label = format!("Add {}", template.label());
    let spawn_fn = Box::new(move |world: &mut World| -> Entity {
        spawn_template_in_document(world, template)
    });

    let mut cmd: Box<dyn EditorCommand> = Box::new(crate::commands::SpawnEntity {
        spawned: None,
        spawn_fn,
        label,
    });
    cmd.execute(world);
    world.resource_mut::<CommandHistory>().push_executed(cmd);
}

pub fn spawn_gltf(
    commands: &mut Commands,
    path: &str,
    position: Vec3,
    selection: &mut Selection,
) -> Entity {
    let file_name = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "GLTF Model".to_string());
    let scene_index = 0;
    // Store the asset-relative path, not the browser's absolute one: the
    // runtime and other machines cannot strip this project's assets prefix,
    // and an unapproved absolute path is refused outright by the asset server.
    let asset_path = to_asset_path(path);
    let entity = commands
        .spawn((
            Name::new(file_name),
            GltfSource {
                path: asset_path,
                scene_index,
            },
            Transform::from_translation(position),
        ))
        .id();
    selection.select_single(commands, entity);
    entity
}

fn spawn_gltf_in_world(world: &mut World, path: &str, position: Vec3) {
    let mut system_state: SystemState<(Commands, ResMut<Selection>)> = SystemState::new(world);
    let Ok((mut commands, mut selection)) = system_state.get_mut(world) else {
        return;
    };
    let entity = spawn_gltf(&mut commands, path, position, &mut selection);
    system_state.apply(world);
    crate::scene_io::register_entity_in_ast(world, entity);
}

pub fn delete_selected(world: &mut World) {
    let selection = world.resource::<Selection>();
    let entities: Vec<Entity> = selection.entities.clone();

    if entities.is_empty() {
        return;
    }

    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();
    for &entity in &entities {
        if world.get_entity(entity).is_err() {
            continue;
        }
        if world.get::<EditorEntity>(entity).is_some() {
            continue;
        }
        cmds.push(Box::new(DespawnEntity::from_world(world, entity)));
    }

    // Deselect entities before despawning so that `On<Remove, Selected>`
    // observers can clean up tree-row UI while the entities still exist.
    for &entity in &entities {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.remove::<Selected>();
        }
    }
    let mut selection = world.resource_mut::<Selection>();
    selection.entities.clear();

    // Execute all despawn commands
    for cmd in &mut cmds {
        cmd.execute(world);
    }

    // Push as a single group command
    if !cmds.is_empty() {
        let group = crate::commands::CommandGroup {
            commands: cmds,
            label: "Delete entities".to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(group));
    }
}

/// How many siblings the list `parent` names holds. `None` is the scene's
/// own root list.
fn sibling_count(world: &World, parent: Option<Entity>) -> usize {
    match parent {
        Some(parent) => world.get::<Children>(parent).map_or(0, Children::len),
        None => world
            .get_resource::<jackdaw_bsn::SceneBsnAst>()
            .map_or(0, |ast| ast.roots.len()),
    }
}

/// Move every selected entity one slot along its own sibling list, earlier
/// for `delta` of -1 and later for 1.
///
/// The order is written to the document, not only to the ECS, so a reordered
/// row survives a save and reload and a flowed child changes place on the
/// canvas. An entity already at the end it is moving towards stays where it
/// is while the rest of the selection moves. The whole move is one entry.
pub(crate) fn move_selected_siblings(world: &mut World, delta: isize) {
    let selected: Vec<Entity> = world.resource::<Selection>().entities.clone();
    let mut located: Vec<(Entity, HierarchyLocation)> = selected
        .into_iter()
        .filter(|&entity| {
            world.get_entity(entity).is_ok() && world.get::<EditorEntity>(entity).is_none()
        })
        .map(|entity| {
            let location = HierarchyLocation::from_world(world, entity);
            (entity, location)
        })
        .collect();
    // Nearest the destination first, so two entities moving the same way
    // cannot swap past each other on the way.
    located.sort_by_key(|(_, location)| location.index);
    if delta > 0 {
        located.reverse();
    }

    let mut moves: Vec<Box<dyn EditorCommand>> = Vec::new();
    let mut lists: Vec<Option<Entity>> = Vec::new();
    for (entity, _) in located {
        let old = HierarchyLocation::from_world(world, entity);
        let Some(index) = old.index.checked_add_signed(delta) else {
            continue;
        };
        if index >= sibling_count(world, old.parent) {
            continue;
        }
        let mut command = MoveEntity::new(
            world,
            entity,
            HierarchyLocation {
                parent: old.parent,
                index,
            },
        );
        command.execute(world);
        moves.push(Box::new(command));
        if !lists.contains(&old.parent) {
            lists.push(old.parent);
        }
    }

    let entry: Box<dyn EditorCommand> = match moves.len() {
        0 => return,
        1 => moves.pop().expect("one move"),
        _ => Box::new(crate::commands::CommandGroup {
            commands: moves,
            label: "Reorder entities".to_string(),
        }),
    };
    world.resource_mut::<CommandHistory>().push_executed(entry);
    for list in lists {
        crate::hierarchy::sync_outliner_row_order(world, list);
    }
}

/// Duplicate selected entities by grafting authored AST subtrees into the live
/// document and spawning from those patches.
pub fn duplicate_selected(world: &mut World) {
    let selection = world.resource::<Selection>();
    let entities: Vec<Entity> = selection.entities.clone();

    if entities.is_empty() {
        return;
    }

    // Deselect current entities first
    for &entity in &entities {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.remove::<Selected>();
        }
    }

    // Snapshot authored subtrees (and their live parents) before mutating the document.
    let to_duplicate: Vec<Entity> = entities
        .iter()
        .copied()
        .filter(|&entity| {
            world.get_entity(entity).is_ok() && world.get::<EditorEntity>(entity).is_none()
        })
        .collect();
    let plans: Vec<(jackdaw_bsn::SceneBsnAst, Option<Entity>)> = {
        let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
        let mut plans = Vec::new();
        for entity in to_duplicate {
            let Some(src_node) = ast.ast_for(entity) else {
                warn!("Duplicate: entity {entity:?} has no document node");
                continue;
            };
            let parent_ast = ast.find_ast_parent_of(src_node);
            let mut temp = jackdaw_bsn::SceneBsnAst::default();
            jackdaw_bsn::clone_subtree_into(&mut temp, ast, src_node, None);
            plans.push((temp, parent_ast));
        }
        plans
    };

    let mut new_entities = Vec::new();
    for (mut temp, parent_ast) in plans {
        prepare_authored_subtree_for_spawn(world, &mut temp);
        let spawned = graft_and_spawn(world, &temp, parent_ast);
        if let Some(&root) = spawned.first() {
            new_entities.push(root);
        }
    }

    let mut selection = world.resource_mut::<Selection>();
    selection.entities = new_entities;
    for &entity in &selection.entities.clone() {
        world.entity_mut(entity).insert(Selected);
    }
}

/// Mint fresh ids and unique root names on an authored subtree before it is
/// grafted/spawned.
fn prepare_authored_subtree_for_spawn(world: &mut World, ast: &mut jackdaw_bsn::SceneBsnAst) {
    mint_scene_node_ids(world, ast);
    assign_unique_entity_names(world, ast);
}

/// `SceneNodeIds` on entity roots of `ast`.
fn entity_root_scene_node_ids(
    world: &World,
    ast: &jackdaw_bsn::SceneBsnAst,
) -> Vec<jackdaw_scene_types::SceneNodeId> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    jackdaw_bsn::entity_roots(ast, &reg)
        .into_iter()
        .filter_map(|root| ast.stable_id_of(root).map(jackdaw_scene_types::SceneNodeId))
        .collect()
}

/// Give every named node in `ast` a `#Name` that does not collide with a live
/// scene-entity name, with a name already claimed elsewhere in `ast`, or with
/// an earlier node in the same batch.
///
/// The whole subtree, not only its roots: a widget is a subtree, so a copied
/// Button carries its caption, and a paste that renamed only the root would
/// leave two entities called `Caption` in the scene. A name is what an
/// operator clause addresses an entity by, so a duplicate makes one of the
/// two unreachable. Editor chrome (`EditorEntity`) is ignored, so a UI label
/// does not force a rename. Collisions get the next `BaseN` suffix.
fn assign_unique_entity_names(world: &mut World, ast: &mut jackdaw_bsn::SceneBsnAst) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let asset_roots: std::collections::HashSet<Entity> = {
        let reg = registry.read();
        jackdaw_bsn::asset_roots(ast, &reg).into_iter().collect()
    };

    let mut taken = scene_entity_names(world);

    for node in walk_entity_nodes(ast) {
        if asset_roots.contains(&node) {
            // An asset entry's name is its reference: renaming it would break
            // the `#Name` the components pointing at it carry.
            continue;
        }
        let Some(name) = ast.get_name(node).map(str::to_owned) else {
            continue;
        };
        if let Some(free) = claim_free_name(&mut taken, &name) {
            crate::commands::set_name_patch(ast, node, Some(&free));
        }
    }
}

/// Every node in `ast`, parents before children, each visited once.
///
/// The visited set is what a document off the clipboard needs: one whose
/// `Children` lists form a cycle would otherwise be walked forever.
fn walk_entity_nodes(ast: &jackdaw_bsn::SceneBsnAst) -> Vec<Entity> {
    let mut queue: std::collections::VecDeque<Entity> = ast.roots.iter().copied().collect();
    let mut seen: std::collections::HashSet<Entity> = queue.iter().copied().collect();
    let mut nodes = Vec::new();
    while let Some(node) = queue.pop_front() {
        nodes.push(node);
        for child in ast.get_children_ast(node) {
            if seen.insert(child) {
                queue.push_back(child);
            }
        }
    }
    nodes
}

/// Every `Name` on a scene entity. Editor chrome (`EditorEntity`) is left out
/// so a UI label does not force a rename.
pub(crate) fn scene_entity_names(world: &mut World) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    let mut query = world.query_filtered::<&Name, Without<EditorEntity>>();
    for existing in query.iter(world) {
        names.insert(existing.as_str().to_owned());
    }
    names
}

/// Reserve `name` in `taken`, and say what it had to become.
///
/// `None` means the name was free and is now claimed. `Some(other)` is the
/// next `BaseN` suffix past every `Base` and `BaseN` already in `taken`, also
/// claimed. A name already ending in a number is renumbered from the same base
/// rather than growing a second suffix.
///
/// The suffix carries no space. A name is what an operator clause addresses an
/// entity by (`name=Button2`), and a clause has no quoting, so a name with a
/// space in it cannot be reached from `JACKDAW_RUN_OP` or the command palette;
/// it also wraps an outliner row that would otherwise fit. Only the suffix is
/// constrained: a name a person typed keeps whatever spaces it has.
pub(crate) fn claim_free_name(
    taken: &mut std::collections::HashSet<String>,
    name: &str,
) -> Option<String> {
    if taken.insert(name.to_owned()) {
        return None;
    }

    // The base is the name with any trailing number taken off, and with the
    // space an older suffix left behind: `Button`, `Button2` and `Button 2`
    // all renumber from `Button` rather than growing a second suffix.
    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let base = if trimmed.is_empty() {
        name.to_owned()
    } else {
        trimmed.trim_end().to_owned()
    };

    let mut max_num = 0u32;
    for existing in taken.iter() {
        if existing == &base {
            max_num = max_num.max(1);
        } else if let Some(rest) = existing.strip_prefix(base.as_str())
            && let Ok(n) = rest.trim_start().parse::<u32>()
        {
            max_num = max_num.max(n);
        }
    }
    let free = format!("{base}{}", max_num + 1);
    taken.insert(free.clone());
    Some(free)
}

/// Snap a vector to the nearest cardinal world axis (+/-X, +/-Y, +/-Z).
/// Returns a signed unit vector along the axis with the largest absolute component.
fn snap_to_nearest_axis(v: Vec3) -> Vec3 {
    let abs = v.abs();
    if abs.x >= abs.y && abs.x >= abs.z {
        Vec3::new(v.x.signum(), 0.0, 0.0)
    } else if abs.y >= abs.x && abs.y >= abs.z {
        Vec3::new(0.0, v.y.signum(), 0.0)
    } else {
        Vec3::new(0.0, 0.0, v.z.signum())
    }
}

/// Derive TrenchBroom-style rotation axes from the camera transform.
///
/// - **Yaw** (left/right arrows): always world Y. Vertical rotation is always intuitive.
/// - **Roll** (up/down arrows): camera forward projected to horizontal, snapped to nearest
///   world axis, then negated. This is the axis you're "looking along".
/// - **Pitch** (PageUp/PageDown): camera right snapped to nearest world axis. If it
///   collides with the roll axis, use the cross product with Y instead.
pub(crate) fn camera_snapped_rotation_axes(gt: &GlobalTransform) -> (Vec3, Vec3, Vec3) {
    let yaw_axis = Vec3::Y;

    // Forward projected onto the horizontal plane, snapped to nearest axis
    let fwd = gt.forward().as_vec3();
    let fwd_horiz = Vec3::new(fwd.x, 0.0, fwd.z);
    let roll_axis = if fwd_horiz.length_squared() > 1e-6 {
        -snap_to_nearest_axis(fwd_horiz)
    } else {
        // Looking straight down/up, use camera up projected horizontally instead.
        let up = gt.up().as_vec3();
        let up_horiz = Vec3::new(up.x, 0.0, up.z);
        if up_horiz.length_squared() > 1e-6 {
            snap_to_nearest_axis(up_horiz)
        } else {
            Vec3::NEG_Z
        }
    };

    // Right snapped to nearest axis, with deduplication against roll
    let right = gt.right().as_vec3();
    let mut pitch_axis = snap_to_nearest_axis(right);
    if pitch_axis.abs() == roll_axis.abs() {
        // Collision, derive perpendicular horizontal axis.
        pitch_axis = snap_to_nearest_axis(yaw_axis.cross(roll_axis));
    }

    (yaw_axis, roll_axis, pitch_axis)
}

pub(crate) enum TransformReset {
    Position,
    Rotation,
    Scale,
}

pub(crate) fn reset_transform_selected(world: &mut World, reset: TransformReset) {
    let selection = world.resource::<Selection>();
    let entities: Vec<Entity> = selection.entities.clone();

    if entities.is_empty() {
        return;
    }

    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

    for &entity in &entities {
        if world.get_entity(entity).is_err() {
            continue;
        }
        let Some(&old_transform) = world.get::<Transform>(entity) else {
            continue;
        };

        let new_transform = match reset {
            TransformReset::Position => Transform {
                translation: Vec3::ZERO,
                ..old_transform
            },
            TransformReset::Rotation => Transform {
                rotation: Quat::IDENTITY,
                ..old_transform
            },
            TransformReset::Scale => Transform {
                scale: Vec3::ONE,
                ..old_transform
            },
        };

        if old_transform == new_transform {
            continue;
        }

        let mut cmd = crate::commands::SetTransform {
            entity,
            old_transform,
            new_transform,
        };
        cmd.execute(world);
        cmds.push(Box::new(cmd));
    }

    if !cmds.is_empty() {
        let label = match reset {
            TransformReset::Position => "Reset position",
            TransformReset::Rotation => "Reset rotation",
            TransformReset::Scale => "Reset scale",
        };
        let group = crate::commands::CommandGroup {
            commands: cmds,
            label: label.to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(group));
    }
}

pub(crate) fn nudge_selected(world: &mut World, offset: Vec3) {
    let selection = world.resource::<Selection>();
    let entities: Vec<Entity> = selection.entities.clone();

    if entities.is_empty() {
        return;
    }

    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

    for &entity in &entities {
        if world.get_entity(entity).is_err() {
            continue;
        }
        let Some(&old_transform) = world.get::<Transform>(entity) else {
            continue;
        };

        let new_transform = Transform {
            translation: old_transform.translation + offset,
            ..old_transform
        };

        let mut cmd = crate::commands::SetTransform {
            entity,
            old_transform,
            new_transform,
        };
        cmd.execute(world);
        cmds.push(Box::new(cmd));
    }

    if !cmds.is_empty() {
        let group = crate::commands::CommandGroup {
            commands: cmds,
            label: "Nudge".to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(group));
    }
}

pub(crate) fn rotate_selected(world: &mut World, rotation: Quat) {
    let selection = world.resource::<Selection>();
    let entities: Vec<Entity> = selection.entities.clone();

    if entities.is_empty() {
        return;
    }

    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

    for &entity in &entities {
        if world.get_entity(entity).is_err() {
            continue;
        }
        let Some(&old_transform) = world.get::<Transform>(entity) else {
            continue;
        };

        let new_transform = Transform {
            rotation: rotation * old_transform.rotation,
            ..old_transform
        };

        let mut cmd = crate::commands::SetTransform {
            entity,
            old_transform,
            new_transform,
        };
        cmd.execute(world);
        cmds.push(Box::new(cmd));
    }

    if !cmds.is_empty() {
        let group = crate::commands::CommandGroup {
            commands: cmds,
            label: "Rotate 90\u{00b0}".to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(group));
    }
}

/// The selection as BSN text: the selected subtrees plus embedded asset
/// entries, the same shape a saved scene uses, so a copy pastes into a code
/// editor readably and back into any scene.
///
/// `None` when nothing is selected or nothing selected is in the document.
fn selection_as_bsn(world: &mut World) -> Option<String> {
    let selected: Vec<Entity> = world.resource::<Selection>().entities.clone();
    if selected.is_empty() {
        return None;
    }
    let nodes: Vec<Entity> = {
        let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
        selected.iter().filter_map(|&e| ast.ast_for(e)).collect()
    };
    if nodes.is_empty() {
        warn!("Copy: no selected entities have document nodes");
        return None;
    }
    let parent_path = world
        .resource::<crate::scene_io::SceneFilePath>()
        .path
        .as_ref()
        .and_then(|p| Path::new(p).parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let text = crate::scene_io::emit_bsn_entities_with_inline_assets(world, &parent_path, &nodes);
    if text.trim().is_empty() {
        warn!("Copy: selected entities emitted no BSN text");
        return None;
    }
    Some(text)
}

/// Put `text` on the OS clipboard, and on the editor's own either way.
///
/// The OS clipboard is what carries a subtree to another window or another
/// application, so it is written first; a run with no clipboard at all still
/// copies, and pastes, through [`EntityClipboard`].
fn write_clipboard(world: &mut World, text: String) {
    if let Some(mut clipboard) = world.get_resource_mut::<SystemClipboard>() {
        clipboard.last_emitted = text.clone();
        if let Err(error) = clipboard.clipboard.set_text(&text) {
            warn!("Copy: system clipboard failed ({error}), keeping the editor's own copy");
        }
    }
    world.resource_mut::<EntityClipboard>().text = text;
}

/// The largest clipboard payload a paste will read.
///
/// The OS clipboard is written by every process on the machine, so what comes
/// back is not this editor's to trust: a payload past this size is refused
/// before it is parsed, rather than parsed into a document that then has to be
/// walked, spawned and emitted.
const MAX_CLIPBOARD_BYTES: usize = 2 * 1024 * 1024;

/// What a refused paste says.
const NOT_ENTITIES: &str = "the clipboard does not hold entities";

/// Whether `text` is an entity document this editor can paste.
///
/// Strict, because the alternative is spawning whatever a user happened to
/// copy. Prose parses: a bare identifier is a valid `BsnPatch::Type`, so the
/// parser saying yes is not on its own an answer. A payload has to be within
/// [`MAX_CLIPBOARD_BYTES`], parse, and yield at least one entity root carrying
/// a component this build has in the type registry. A root whose only patch is
/// a name, or one naming a type nothing registers, is text that happens to
/// parse rather than a scene.
fn is_entity_document(world: &World, text: &str) -> bool {
    if text.len() > MAX_CLIPBOARD_BYTES {
        warn!(
            "Paste: the clipboard holds {} bytes, past the {MAX_CLIPBOARD_BYTES} a paste reads",
            text.len()
        );
        return false;
    }
    let Ok(ast) = jackdaw_bsn::parse_bsn_text(text) else {
        return false;
    };
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    jackdaw_bsn::entity_roots(&ast, &registry)
        .into_iter()
        .any(|root| {
            let Some(patches) = ast.get_patches(root) else {
                return false;
            };
            patches.0.iter().any(|&pe| {
                ast.get_patch(pe)
                    .and_then(jackdaw_bsn::patch_type_path)
                    .is_some_and(|type_path| registry.get_with_type_path(type_path).is_some())
            })
        })
}

/// The scene text a paste should spawn from, or `None` when the clipboard
/// holds no entities.
///
/// The editor's own [`EntityClipboard`] mirrors every copy, and the OS
/// clipboard is what carries one between windows and applications. Which of
/// the two answers is decided by comparing them: OS text equal to what this
/// editor last emitted is that same copy, so the mirror answers; different
/// text belongs to somebody else, and is accepted only when it reads as an
/// entity document. Text that is neither is a refusal, never a quiet fall back
/// to the mirror -- a paste of a paragraph must not produce the subtree copied
/// an hour ago.
fn clipboard_entities(world: &mut World) -> Option<String> {
    let own = world
        .get_resource::<EntityClipboard>()
        .map(|clipboard| clipboard.text.clone())
        .filter(|text| !text.trim().is_empty());

    let os_text = world
        .get_resource_mut::<SystemClipboard>()
        .and_then(|mut clipboard| {
            let text = clipboard.clipboard.get_text().ok()?;
            Some((text, clipboard.last_emitted.clone()))
        })
        .filter(|(text, _)| !text.trim().is_empty());

    choose_clipboard_text(world, own, os_text)
}

/// [`clipboard_entities`] with the two clipboards already read, so the choice
/// between them can be exercised without an OS clipboard to write to.
///
/// `os` is the OS clipboard's text paired with what this editor last put
/// there.
fn choose_clipboard_text(
    world: &World,
    own: Option<String>,
    os: Option<(String, String)>,
) -> Option<String> {
    let Some((text, last_emitted)) = os else {
        // No readable OS clipboard: a headless run, or a session with no
        // compositor. The mirror is the whole answer there.
        return own;
    };
    if text == last_emitted {
        return own.or(Some(text));
    }
    is_entity_document(world, &text).then_some(text)
}

/// Copy selected entities to the clipboard as BSN text.
fn copy_components(world: &mut World) {
    let Some(text) = selection_as_bsn(world) else {
        return;
    };
    write_clipboard(world, text);
}

/// Undo command for a paste operation. On undo, finds each pasted root by its
/// `SceneNodeId` and despawns it (children go with the hierarchy). On redo,
/// re-spawns from the remapped clipboard text that already carries those ids.
struct PasteEntitiesCommand {
    /// `SceneNodeIds` assigned to pasted entity roots at first paste.
    spawned_node_ids: Vec<jackdaw_scene_types::SceneNodeId>,
    /// Clipboard BSN with fresh ids and unique names already written in,
    /// preserved so redo re-spawns the same authored patches.
    remapped_text: String,
    /// Where the paste landed, so a redo puts it back in the same place
    /// rather than at the end of the scene's root list.
    target: PasteTarget,
    label: String,
}

impl crate::commands::EditorCommand for PasteEntitiesCommand {
    /// Only a redo reaches this: the paste itself already happened, and the
    /// entry went onto the stack through `push_executed`, which does not
    /// execute what it is given.
    fn execute(&mut self, world: &mut World) {
        let spawned = spawn_bsn_clipboard(world, &self.remapped_text, self.target);
        select_entities(world, &spawned);
        info!("Redo: re-pasted {} entities", spawned.len());
    }

    fn undo(&mut self, world: &mut World) {
        let id_to_entity: std::collections::HashMap<_, _> = world
            .query::<(Entity, &jackdaw_scene_types::SceneNodeId)>()
            .iter(world)
            .map(|(entity, node_id)| (*node_id, entity))
            .collect();

        let mut to_despawn: Vec<Entity> = Vec::new();
        for node_id in &self.spawned_node_ids {
            if let Some(&entity) = id_to_entity.get(node_id) {
                to_despawn.push(entity);
            }
        }

        crate::commands::deselect_entities(world, &to_despawn);
        for e in to_despawn {
            crate::commands::despawn_scene_entity(world, e);
        }
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// Graft entity roots from `source` into the live document and spawn
/// them under `parent_ast` (`None` = scene roots).
fn graft_and_spawn(
    world: &mut World,
    source: &jackdaw_bsn::SceneBsnAst,
    parent_ast: Option<Entity>,
) -> Vec<Entity> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let entity_roots = {
        let reg = registry.read();
        jackdaw_bsn::entity_roots(source, &reg)
    };

    let ecs_parent = parent_ast.and_then(|parent| {
        world
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .ecs_for_ast(parent)
    });

    let mut grafted_roots = Vec::new();
    {
        let mut live = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
        for src_root in entity_roots {
            let new_root = jackdaw_bsn::clone_subtree_into(&mut live, source, src_root, parent_ast);
            grafted_roots.push(new_root);
        }
    }

    let mut spawned_roots = Vec::new();
    let mut all_spawned = Vec::new();
    for &ast_root in &grafted_roots {
        let before = all_spawned.len();
        jackdaw_bsn::spawn_ast_node(world, ast_root, ecs_parent, &mut all_spawned);
        if let Some(&ecs_root) = all_spawned.get(before) {
            spawned_roots.push(ecs_root);
        }
    }
    jackdaw_bsn::apply_dirty_ast_patches(world);
    spawned_roots
}

/// Parse clipboard BSN text and graft it into the live scene document.
fn spawn_bsn_clipboard(world: &mut World, text: &str, target: PasteTarget) -> Vec<Entity> {
    let parsed = match jackdaw_bsn::parse_bsn_text(text) {
        Ok(ast) => ast,
        Err(e) => {
            warn!("Paste: failed to parse clipboard BSN: {e}");
            return Vec::new();
        }
    };
    graft_and_spawn_at(world, &parsed, target)
}

/// Where a paste lands, in terms a redo can still resolve.
///
/// The parent is named by its `SceneNodeId` rather than by its `Entity`: an
/// undo that despawned the parent and a redo that put it back give it a new
/// entity id, and the stable id is what survives that.
#[derive(Clone, Copy, Default)]
struct PasteTarget {
    /// `None` is the scene's own root list.
    parent: Option<jackdaw_scene_types::SceneNodeId>,
    /// Sibling index the first pasted root takes; the rest follow it.
    index: usize,
}

impl PasteTarget {
    /// The live location, or the end of the scene root list when the parent
    /// this names is no longer in the scene.
    fn resolve(&self, world: &mut World) -> HierarchyLocation {
        let Some(wanted) = self.parent else {
            return HierarchyLocation {
                parent: None,
                index: self.index,
            };
        };
        let parent = world
            .query::<(Entity, &jackdaw_scene_types::SceneNodeId)>()
            .iter(world)
            .find(|(_, id)| **id == wanted)
            .map(|(entity, _)| entity);
        HierarchyLocation {
            parent,
            index: if parent.is_some() {
                self.index
            } else {
                usize::MAX
            },
        }
    }
}

/// Sibling straight after the primary selection, which is where a paste goes.
///
/// Nothing selected, or a selection the document does not hold, lands the
/// paste at the end of the open UI scene's root, and at the end of the scene's
/// own root list when there is no UI scene.
fn paste_target(world: &mut World) -> PasteTarget {
    let primary = world
        .get_resource::<Selection>()
        .and_then(Selection::primary);
    if let Some(primary) = primary
        && world
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .ast_for(primary)
            .is_some()
    {
        let location = HierarchyLocation::from_world(world, primary);
        return PasteTarget {
            parent: location.parent.and_then(|parent| {
                world
                    .get::<jackdaw_scene_types::SceneNodeId>(parent)
                    .copied()
            }),
            index: location.index + 1,
        };
    }
    let root = crate::ui_palette::ui_scene_root(world);
    PasteTarget {
        parent: root.and_then(|root| world.get::<jackdaw_scene_types::SceneNodeId>(root).copied()),
        index: usize::MAX,
    }
}

/// [`graft_and_spawn`] landing the roots at `target` rather than at the end
/// of the scene's root list.
fn graft_and_spawn_at(
    world: &mut World,
    source: &jackdaw_bsn::SceneBsnAst,
    target: PasteTarget,
) -> Vec<Entity> {
    let location = target.resolve(world);
    let parent_ast = location
        .parent
        .and_then(|parent| world.resource::<jackdaw_bsn::SceneBsnAst>().ast_for(parent));
    let spawned = graft_and_spawn(world, source, parent_ast);
    if location.index != usize::MAX {
        for (offset, &root) in spawned.iter().enumerate() {
            // The roots were spawned a few lines ago and nothing has
            // propagated a transform for them yet, so there is no world
            // position to keep: reading one would write an identity
            // `Transform` into the document for a node that authored none.
            crate::commands::place_entity(
                world,
                root,
                HierarchyLocation {
                    parent: location.parent,
                    index: location.index + offset,
                },
                crate::commands::WorldTransform::Unplaced,
            );
        }
        crate::hierarchy::sync_outliner_row_order(world, location.parent);
    }
    spawned
}

/// Ensure every entity node in `ast` carries a fresh `SceneNodeId` patch so
/// spawn/apply installs unique ids.
fn mint_scene_node_ids(world: &World, ast: &mut jackdaw_bsn::SceneBsnAst) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let asset_roots: std::collections::HashSet<Entity> = {
        let reg = registry.read();
        jackdaw_bsn::asset_roots(ast, &reg).into_iter().collect()
    };

    // A visited set, not merely a stack: the document being walked came off
    // the clipboard, and one whose `Children` lists form a cycle would
    // otherwise be pushed onto the stack for as long as memory lasted.
    let mut stack: Vec<Entity> = ast.roots.clone();
    let mut seen: std::collections::HashSet<Entity> = stack.iter().copied().collect();
    let mut nodes = Vec::new();
    while let Some(node) = stack.pop() {
        nodes.push(node);
        for child in ast.get_children_ast(node) {
            if seen.insert(child) {
                stack.push(child);
            }
        }
    }
    for node in nodes {
        if asset_roots.contains(&node) {
            continue;
        }
        let existing = ast.get_patches(node).and_then(|patches| {
            patches.0.iter().copied().find(|&pe| {
                matches!(
                    ast.get_patch(pe),
                    Some(jackdaw_bsn::BsnPatch::TupleStruct(data))
                        if data.type_path.ends_with("SceneNodeId")
                )
            })
        });
        let fresh = jackdaw_scene_types::SceneNodeId::next();
        let patch = jackdaw_bsn::BsnPatch::TupleStruct(jackdaw_bsn::BsnTupleStructData {
            type_path: jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH.to_string(),
            values: vec![jackdaw_bsn::BsnValue::Int(fresh.0 as i128)],
        });
        if let Some(pe) = existing {
            ast.set_patch(pe, patch);
        } else {
            let pe = ast.world.spawn(patch).id();
            if let Some(patches) = ast.get_patches_mut(node) {
                patches.0.push(pe);
            }
        }
    }
}

/// Whether the open document is a UI scene, which decides what may be pasted
/// into it.
fn open_scene_is_ui(world: &mut World) -> bool {
    crate::ui_palette::ui_scene_root(world).is_some()
}

/// Whether `ast`'s entity roots are UI nodes.
///
/// A `Node` patch is what makes a root a UI node: it is laid out by a parent
/// node and drawn on the canvas. Anything else -- a mesh, a light, a camera --
/// is placed by a `Transform` in a world.
fn roots_are_ui_nodes(world: &World, ast: &jackdaw_bsn::SceneBsnAst) -> bool {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let node_type_path = crate::inspector::node_card::node_type_path();
    jackdaw_bsn::entity_roots(ast, &registry)
        .into_iter()
        .any(|root| ast.find_patch_by_type_path(root, node_type_path).is_some())
}

/// Paste entities from clipboard scene text at `target`, as one history entry.
///
/// Returns the pasted roots, which become the selection.
fn paste_clipboard_entities(world: &mut World, text: &str, target: PasteTarget) -> Vec<Entity> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let mut parsed = match jackdaw_bsn::parse_bsn_text(text) {
        Ok(ast) => ast,
        Err(e) => {
            warn!("Clipboard text is not valid BSN: {e}");
            crate::status_bar::notify_error(world, NOT_ENTITIES);
            return Vec::new();
        }
    };
    if parsed.roots.is_empty() {
        crate::status_bar::notify_error(world, NOT_ENTITIES);
        return Vec::new();
    }

    // A UI node laid out in a world, or a mesh parented into a screen, is a
    // scene that neither draws nor saves as its author meant. Both directions
    // refuse rather than pasting something the document cannot hold.
    let payload_is_ui = roots_are_ui_nodes(world, &parsed);
    let scene_is_ui = open_scene_is_ui(world);
    if payload_is_ui != scene_is_ui {
        let message = if payload_is_ui {
            "the clipboard holds UI nodes, and this is not a UI scene"
        } else {
            "the clipboard holds world entities, and this is a UI scene"
        };
        crate::status_bar::notify_error(world, message);
        return Vec::new();
    }

    // The assets the copied components name travel with the subtree, so a
    // pasted image node finds its image instead of a reference nothing backs.
    let dropped = jackdaw_bsn::adopt_asset_roots(world, &parsed);
    if !dropped.is_empty() {
        crate::status_bar::notify_warn(world, format!("pasted without {}", dropped.join(", ")));
    }

    // Fresh ids and names before the graft, so the pasted subtree is a second
    // thing in the scene rather than a second reference to the first.
    prepare_authored_subtree_for_spawn(world, &mut parsed);
    let spawned_node_ids = entity_root_scene_node_ids(world, &parsed);
    let remapped_text = jackdaw_bsn::emit_scene(&parsed);

    let spawned = graft_and_spawn_at(world, &parsed, target);
    if spawned.is_empty() {
        return Vec::new();
    }

    select_entities(world, &spawned);
    info!("Pasted {} entities from BSN clipboard", spawned.len());

    let cmd = PasteEntitiesCommand {
        spawned_node_ids,
        remapped_text,
        target,
        label: "Paste entities".to_string(),
    };
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(cmd));
    spawned
}

/// Make `entities` the whole selection.
fn select_entities(world: &mut World, entities: &[Entity]) {
    for &entity in &world.resource::<Selection>().entities.clone() {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.remove::<Selected>();
        }
    }
    world.resource_mut::<Selection>().entities = entities.to_vec();
    for &entity in entities {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.insert(Selected);
        }
    }
}

/// Paste clipboard entities at the end of the scene's root list.
fn paste_components(world: &mut World) {
    paste_clipboard(
        world,
        PasteTarget {
            parent: None,
            index: usize::MAX,
        },
    );
}

/// Paste clipboard entities as the sibling straight after the selection.
fn paste_entities_after_selection(world: &mut World) {
    let target = paste_target(world);
    paste_clipboard(world, target);
}

/// Put whatever the clipboard holds into the scene at `target`.
///
/// Entities first: an image is only what a paste means when there is no entity
/// payload to be had. The other order lets a screenshot taken between a copy
/// and its paste swallow the subtree the user was carrying, since a clipboard
/// commonly holds an image and text at once. With neither, the paste refuses
/// out loud rather than doing nothing.
fn paste_clipboard(world: &mut World, target: PasteTarget) {
    if let Some(text) = clipboard_entities(world) {
        paste_clipboard_entities(world, &text, target);
        return;
    }
    if crate::asset_ingest::paste_clipboard_image(world) {
        return;
    }
    crate::status_bar::notify_error(world, NOT_ENTITIES);
}

/// Copy the selection to the clipboard as BSN text.
fn copy_selected_entities(world: &mut World) {
    let Some(text) = selection_as_bsn(world) else {
        return;
    };
    write_clipboard(world, text);
}

/// Copy the selection and then delete it.
///
/// The copy records nothing, so the delete's own entry is the whole cut and
/// one undo puts the subtree back where it was.
fn cut_selected_entities(world: &mut World) {
    let Some(text) = selection_as_bsn(world) else {
        return;
    };
    write_clipboard(world, text);
    delete_selected(world);
}

/// Flip `Visibility` between hidden and inherited on every selected
/// entity, writing through the document so the change is saved.
///
/// It pushes no history entry of its own: `entity.toggle_visibility` is
/// undoable, so the dispatcher's before/after snapshot pair is the entry,
/// and a second one here would leave a single undo doing half the job.
fn hide_selected(world: &mut World) {
    let selection = world.resource::<Selection>();
    let entities: Vec<Entity> = selection.entities.clone();

    if entities.is_empty() {
        return;
    }

    for &entity in &entities {
        let current = world
            .get::<Visibility>(entity)
            .copied()
            .unwrap_or(Visibility::Inherited);

        let new_visibility = match current {
            Visibility::Hidden => Visibility::Inherited,
            _ => Visibility::Hidden,
        };

        let mut cmd = crate::commands::SetBsnField {
            entity,
            type_path: "bevy_camera::visibility::Visibility".to_string(),
            field_path: String::new(),
            old_value: Some(jackdaw_bsn::BsnValue::Type(format!(
                "bevy_camera::visibility::Visibility::{current:?}"
            ))),
            new_value: jackdaw_bsn::BsnValue::Type(format!(
                "bevy_camera::visibility::Visibility::{new_visibility:?}"
            )),
            was_derived: false,
        };
        cmd.execute(world);
    }
}

// FIXME: this breaks down whenever an extension uses `Name`
#[derive(SystemParam, Deref, DerefMut)]
struct SceneEntities<'w, 's> {
    query: Query<
        'w,
        's,
        (Entity, &'static Visibility),
        (With<Name>, Without<EditorEntity>, Without<Node>),
    >,
}

fn unhide_all_entities(world: &mut World, scene_entities: &mut SystemState<SceneEntities>) {
    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

    // Only unhide top-level scene entities (with Name), matching hide_unselected logic.
    let hidden: Vec<Entity> = {
        let Ok(entities) = scene_entities.get(world) else {
            return;
        };
        entities
            .iter()
            .filter(|(_, vis)| **vis == Visibility::Hidden)
            .map(|(e, _)| e)
            .collect()
    };

    for entity in hidden {
        let mut cmd = crate::commands::SetBsnField {
            entity,
            type_path: "bevy_camera::visibility::Visibility".to_string(),
            field_path: String::new(),
            old_value: Some(jackdaw_bsn::BsnValue::Type(
                "bevy_camera::visibility::Visibility::Hidden".to_string(),
            )),
            new_value: jackdaw_bsn::BsnValue::Type(
                "bevy_camera::visibility::Visibility::Inherited".to_string(),
            ),
            was_derived: false,
        };
        cmd.execute(world);
        cmds.push(Box::new(cmd));
    }

    if !cmds.is_empty() {
        let group = crate::commands::CommandGroup {
            commands: cmds,
            label: "Unhide all".to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(group));
    }
}

fn hide_all_entities(world: &mut World, scene_entities: &mut SystemState<SceneEntities>) {
    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

    // Hide all top-level scene entities (same filter as H, applied to everything).
    let to_hide: Vec<(Entity, Visibility)> = {
        let Ok(entities) = scene_entities.get(world) else {
            return;
        };
        entities
            .iter()
            .filter(|(_, vis)| **vis != Visibility::Hidden)
            .map(|(e, vis)| (e, *vis))
            .collect()
    };

    for (entity, current) in to_hide {
        let mut cmd = crate::commands::SetBsnField {
            entity,
            type_path: "bevy_camera::visibility::Visibility".to_string(),
            field_path: String::new(),
            old_value: Some(jackdaw_bsn::BsnValue::Type(format!(
                "bevy_camera::visibility::Visibility::{current:?}"
            ))),
            new_value: jackdaw_bsn::BsnValue::Type(
                "bevy_camera::visibility::Visibility::Hidden".to_string(),
            ),
            was_derived: false,
        };
        cmd.execute(world);
        cmds.push(Box::new(cmd));
    }

    if !cmds.is_empty() {
        let group = crate::commands::CommandGroup {
            commands: cmds,
            label: "Hide all".to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(group));
    }
}

/// Convert a filesystem path to a Bevy asset path (relative to the assets directory).
///
/// Bevy's default asset source reads from `<base>/assets/` where `<base>` is
/// `BEVY_ASSET_ROOT`, `CARGO_MANIFEST_DIR`, or the executable's parent directory.
///
/// Strips the assets-dir prefix when the input is absolute so the load goes
/// through Bevy's approved-path machinery (no `UnapprovedPathMode::Allow`
/// needed). Returns the original path on a miss and warns; callers should not
/// rely on the fallback ever loading successfully under `Forbid`.
pub fn to_asset_path(path: &str) -> String {
    let path = dunce::simplified(Path::new(path));
    if let Some(assets_dir) = get_assets_base_dir()
        && let Ok(relative) = path.strip_prefix(dunce::simplified(&assets_dir))
    {
        return relative.to_string_lossy().to_string();
    }
    // Fallback: if already a simple relative path, use as-is
    if !path.is_absolute() {
        return path.to_string_lossy().to_string();
    }
    warn!(
        "Cannot load '{}': file is outside the assets directory. \
         Move it into your project's assets/ folder.",
        path.display()
    );
    path.to_string_lossy().to_string()
}

/// Get the absolute path of Bevy's assets directory.
/// Uses the last-opened `ProjectRoot` if available, then falls back to
/// the standard `FileAssetReader` lookup (`BEVY_ASSET_ROOT` / `CARGO_MANIFEST_DIR` / exe dir).
fn get_assets_base_dir() -> Option<std::path::PathBuf> {
    // Try ProjectRoot via recent projects config
    if let Some(project_dir) = crate::project::read_last_project() {
        let assets = dunce::simplified(project_dir.as_path()).join("assets");
        if assets.is_dir() {
            return Some(assets);
        }
    }

    let base = if let Ok(dir) = std::env::var("BEVY_ASSET_ROOT") {
        std::path::PathBuf::from(dir)
    } else if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
        std::path::PathBuf::from(dir)
    } else {
        std::env::current_exe().ok()?.parent()?.to_path_buf()
    };
    Some(dunce::simplified(base.as_path()).join("assets"))
}

// ----------------------- Operators ----------------------------
//
// Entity-level operators (`entity.*`) and the `Add` menu
// (`entity.add.*`). Keybind and menu dispatch both arrive here.
// Operators are gated with `is_available = can_act_on_entities` so
// they refuse to fire while a brush sub-element drag or modal
// operator has the scene locked, matching the guards the legacy
// `handle_entity_keys` applied.

use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;

use crate::core_extension::CoreExtensionInputContext;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<EntityDeleteOp>()
        .register_operator::<EntityDuplicateOp>()
        .register_operator::<EntityCopyOp>()
        .register_operator::<EntityCutOp>()
        .register_operator::<EntityPasteOp>()
        .register_operator::<EntityMoveUpOp>()
        .register_operator::<EntityMoveDownOp>()
        .register_operator::<EntityPlaceGltfOp>()
        .register_operator::<EntityCopyComponentsOp>()
        .register_operator::<EntityPasteComponentsOp>()
        .register_operator::<EntityToggleVisibilityOp>()
        .register_operator::<EntityHideUnselectedOp>()
        .register_operator::<EntityUnhideAllOp>()
        .register_operator::<EntityAddCubeOp>()
        .register_operator::<EntityAddSphereOp>()
        .register_operator::<EntityAddPointLightOp>()
        .register_operator::<EntityAddDirectionalLightOp>()
        .register_operator::<EntityAddSpotLightOp>()
        .register_operator::<EntityAddCameraOp>();
    #[cfg(feature = "camera_rig")]
    ctx.register_operator::<EntityAddCameraRigOp>();
    ctx.register_operator::<EntityAddEmptyOp>()
        .register_operator::<EntityAddImageOp>()
        .register_operator::<EntityAddTerrainOp>()
        .register_operator::<EntityAddPrefabOp>()
        .register_operator::<EntityAddPlaneOp>()
        .register_operator::<EntityAddCylinderOp>()
        .register_operator::<EntityAddWedgeOp>()
        .register_operator::<EntityAddConeOp>()
        .register_operator::<EntityAddPyramidOp>()
        .register_operator::<EntityAddAnimationPlayerOp>()
        .register_operator::<EntityAddAudioSourceOp>()
        .register_operator::<EntityAddFogVolumeOp>()
        .register_operator::<EntityAddReflectionProbeOp>()
        // Registered on core rather than on the UI Widgets extension that
        // supplies the definitions, so the operator stays available and
        // reports an unknown widget name rather than disappearing with it.
        .register_operator::<crate::ui_palette::WidgetAddOp>()
        .register_operator::<crate::add_entity_picker::EntityAddPickerOp>();

    #[cfg(feature = "multiplayer")]
    ctx.register_operator::<EntityAddSpawnPointOp>()
        .register_operator::<EntityAddZoneTransitionOp>()
        .register_operator::<EntityAddNetworkRoomOp>();

    ctx.bind_operator::<CoreExtensionInputContext, EntityDeleteOp>([PresetInput::key("Delete")]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityDuplicateOp>([
        PresetInput::key("KeyD").ctrl()
    ]);
    // Ctrl+C / Ctrl+X / Ctrl+V act on entities. The timeline claims the same
    // chords for keyframes; the two availability checks are disjoint on the
    // timeline being the focused window, so one press answers once.
    ctx.bind_operator::<CoreExtensionInputContext, EntityCopyOp>([PresetInput::key("KeyC").ctrl()]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityCutOp>([PresetInput::key("KeyX").ctrl()]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityPasteOp>(
        [PresetInput::key("KeyV").ctrl()],
    );
    // Ctrl+Shift for the whole-component clipboard, which pastes at the
    // scene root rather than beside the selection.
    ctx.bind_operator::<CoreExtensionInputContext, EntityCopyComponentsOp>([PresetInput::key(
        "KeyC",
    )
    .ctrl()
    .shift()]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityPasteComponentsOp>([PresetInput::key(
        "KeyV",
    )
    .ctrl()
    .shift()]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityMoveUpOp>([
        PresetInput::key("ArrowUp").ctrl()
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityMoveDownOp>([PresetInput::key(
        "ArrowDown",
    )
    .ctrl()]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityToggleVisibilityOp>([PresetInput::key(
        "KeyH",
    )]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityUnhideAllOp>([
        PresetInput::key("KeyH").ctrl()
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityHideUnselectedOp>([PresetInput::key(
        "KeyH",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, crate::add_entity_picker::EntityAddPickerOp>([
        PresetInput::key("KeyA").ctrl(),
    ]);
}

/// Shared availability check for entity manipulation operators.
/// Refuses to fire while a text input has focus, while a modal
/// operator is in flight, while the draw brush modal is active, or
/// while brush sub-element edit mode is active; matches the guards
/// the legacy `handle_entity_keys` applied.
///
/// Also refuses while the timeline dock window is active, so the
/// keyframe operators sharing those chords answer alone. Delete and the
/// clipboard chords are bound on both sides; the timeline is the
/// narrower claim, so it wins wherever it is the focused window.
///
/// Typing is asked through [`crate::keybind_focus::KeybindFocus`] and
/// not through `InputFocus` directly: bevy parks focus on the primary
/// window when nothing has claimed it, so the bare resource reports
/// "the user is typing" for a scene nobody has clicked in yet.
pub(crate) fn can_act_on_entities(
    keybind_focus: crate::keybind_focus::KeybindFocus,
    active: ActiveModalQuery,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<crate::draw_brush::DrawBrushState>,
    edit_mode: Res<crate::brush::EditMode>,
    tree: Res<jackdaw_panels::tree::DockTree>,
) -> bool {
    if keybind_focus.is_typing() || active.is_modal_running() || modal.active.is_some() {
        return false;
    }
    if draw_state.active.is_some() {
        return false;
    }
    if crate::transform_ops::active_tab_kind_present(&tree, "jackdaw.timeline") {
        return false;
    }

    matches!(*edit_mode, crate::brush::EditMode::Object)
}

// -- Entity lifecycle --------------------------------------------

#[operator(
    id = "entity.place_gltf",
    label = "Place GLTF",
    description = "Place a GLTF asset into the active scene at a world position.",
    allows_undo = true,
    params(
        path(String, doc = "Path to the GLTF asset."),
        pos_x(f64, doc = "World-space X position."),
        pos_y(f64, doc = "World-space Y position."),
        pos_z(f64, doc = "World-space Z position."),
    )
)]
pub(crate) fn entity_place_gltf(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(path) = params.as_str("path").map(str::to_owned) else {
        warn!("entity.place_gltf: missing `path` param");
        return OperatorResult::Cancelled;
    };
    let Some(x) = params.as_float("pos_x") else {
        warn!("entity.place_gltf: missing `pos_x` param");
        return OperatorResult::Cancelled;
    };
    let Some(y) = params.as_float("pos_y") else {
        warn!("entity.place_gltf: missing `pos_y` param");
        return OperatorResult::Cancelled;
    };
    let Some(z) = params.as_float("pos_z") else {
        warn!("entity.place_gltf: missing `pos_z` param");
        return OperatorResult::Cancelled;
    };
    let position = Vec3::new(x as f32, y as f32, z as f32);
    commands.queue(move |world: &mut World| {
        spawn_gltf_in_world(world, &path, position);
    });
    OperatorResult::Finished
}

#[operator(
    id = "entity.delete",
    label = "Delete",
    is_available = can_act_on_entities
)]
pub(crate) fn entity_delete(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(delete_selected);
    OperatorResult::Finished
}

#[operator(
    id = "entity.duplicate",
    label = "Duplicate",
    is_available = can_act_on_entities
)]
pub(crate) fn entity_duplicate(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(duplicate_selected);
    OperatorResult::Finished
}

#[operator(
    id = "entity.move_up",
    label = "Move Up",
    description = "Move the selection one slot earlier among its siblings.",
    is_available = can_act_on_entities
)]
pub(crate) fn entity_move_up(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| move_selected_siblings(world, -1));
    OperatorResult::Finished
}

#[operator(
    id = "entity.move_down",
    label = "Move Down",
    description = "Move the selection one slot later among its siblings.",
    is_available = can_act_on_entities
)]
pub(crate) fn entity_move_down(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| move_selected_siblings(world, 1));
    OperatorResult::Finished
}

#[operator(
    id = "entity.copy",
    label = "Copy",
    description = "Copy the selected subtrees to the clipboard as scene text.",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_copy(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(copy_selected_entities);
    OperatorResult::Finished
}

#[operator(
    id = "entity.cut",
    label = "Cut",
    description = "Copy the selected subtrees to the clipboard and delete them.",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_cut(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(cut_selected_entities);
    OperatorResult::Finished
}

#[operator(
    id = "entity.paste",
    label = "Paste",
    description = "Paste the clipboard's subtrees as siblings after the selection.",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_paste(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(paste_entities_after_selection);
    OperatorResult::Finished
}

#[operator(
    id = "entity.copy_components",
    label = "Copy Components",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_copy_components(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(copy_components);
    OperatorResult::Finished
}

#[operator(
    id = "entity.paste_components",
    label = "Paste Components",
    is_available = can_act_on_entities
)]
pub(crate) fn entity_paste_components(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(paste_components);
    OperatorResult::Finished
}

#[operator(
    id = "entity.toggle_visibility",
    label = "Toggle Visibility",
    is_available = can_act_on_entities
)]
pub(crate) fn entity_toggle_visibility(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(hide_selected);
    OperatorResult::Finished
}

#[operator(
    id = "entity.hide_unselected",
    label = "Hide Unselected",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_hide_unselected(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        if let Err(err) = world.run_system_cached(hide_all_entities) {
            warn!("hide_all_entities: {err:?}");
        }
    });
    OperatorResult::Finished
}

#[operator(
    id = "entity.unhide_all",
    label = "Unhide All",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_unhide_all(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        if let Err(err) = world.run_system_cached(unhide_all_entities) {
            warn!("unhide_all_entities: {err:?}");
        }
    });
    OperatorResult::Finished
}

// -- Add menu ----------------------------------------------------

#[operator(id = "entity.add.cube", label = "Cube")]
pub(crate) fn entity_add_cube(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Cube);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.sphere", label = "Sphere")]
pub(crate) fn entity_add_sphere(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Sphere);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.point_light", label = "Point Light")]
pub(crate) fn entity_add_point_light(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::PointLight);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.directional_light", label = "Directional Light")]
pub(crate) fn entity_add_directional_light(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::DirectionalLight);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.spot_light", label = "Spot Light")]
pub(crate) fn entity_add_spot_light(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::SpotLight);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.camera", label = "Camera")]
pub(crate) fn entity_add_camera(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Camera3d);
    });
    OperatorResult::Finished
}

#[cfg(feature = "camera_rig")]
#[operator(id = "entity.add.camera_rig", label = "Camera Rig")]
pub(crate) fn entity_add_camera_rig(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::CameraRig);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.image", label = "Reference Image")]
pub fn entity_add_image(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(crate::reference_image::open_reference_image_picker);
    OperatorResult::Finished
}

#[operator(id = "entity.add.empty", label = "Empty")]
pub(crate) fn entity_add_empty(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Empty);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.plane", label = "Plane")]
pub(crate) fn entity_add_plane(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Plane);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.cylinder", label = "Cylinder")]
pub(crate) fn entity_add_cylinder(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Cylinder);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.wedge", label = "Wedge")]
pub(crate) fn entity_add_wedge(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Wedge);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.cone", label = "Cone")]
pub(crate) fn entity_add_cone(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Cone);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.pyramid", label = "Pyramid")]
pub(crate) fn entity_add_pyramid(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Pyramid);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.animation_player", label = "Animation Player")]
pub(crate) fn entity_add_animation_player(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::AnimationPlayer);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.audio_source", label = "Audio Source")]
pub(crate) fn entity_add_audio_source(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::AudioSource);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.fog_volume", label = "Fog Volume")]
pub(crate) fn entity_add_fog_volume(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::FogVolume);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.reflection_probe", label = "Reflection Probe")]
pub(crate) fn entity_add_reflection_probe(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::spawn_undoable(world, "Add Reflection Probe", |world| {
            let mut system_state: SystemState<(Commands, Res<AssetServer>, ResMut<Selection>)> =
                SystemState::new(world);
            let Ok((mut commands, asset_server, mut selection)) = system_state.get_mut(world)
            else {
                return Entity::PLACEHOLDER;
            };
            // Reuse the editor's shipped environment-map cubemaps as the
            // probe's reflection source, the same embedded asset the
            // viewport and material preview load.
            let diffuse_map = bevy::asset::load_embedded_asset!(
                &*asset_server,
                "../assets/environment_maps/voortrekker_interior_1k_diffuse.ktx2"
            );
            let specular_map = bevy::asset::load_embedded_asset!(
                &*asset_server,
                "../assets/environment_maps/voortrekker_interior_1k_specular.ktx2"
            );
            let entity = commands
                .spawn((
                    Name::new("Reflection Probe"),
                    LightProbe::default(),
                    EnvironmentMapLight {
                        diffuse_map,
                        specular_map,
                        intensity: 1000.0,
                        ..default()
                    },
                    SceneReflectionProbe,
                    Transform::from_scale(Vec3::splat(2.0)),
                    Visibility::default(),
                ))
                .id();
            selection.select_single(&mut commands, entity);
            system_state.apply(world);
            crate::scene_io::register_entity_in_ast(world, entity);
            entity
        });
    });
    OperatorResult::Finished
}

#[cfg(feature = "multiplayer")]
#[operator(id = "entity.add.spawn_point", label = "Spawn Point")]
pub(crate) fn entity_add_spawn_point(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::spawn_undoable(world, "Add Spawn Point", |world| {
            let mut system_state: SystemState<(Commands, ResMut<Selection>)> =
                SystemState::new(world);
            let Ok((mut commands, mut selection)) = system_state.get_mut(world) else {
                return Entity::PLACEHOLDER;
            };
            let entity = commands
                .spawn((
                    Name::new("Spawn Point"),
                    jackdaw_multiplayer::SpawnPoint::default(),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            selection.select_single(&mut commands, entity);
            system_state.apply(world);
            crate::scene_io::register_entity_in_ast(world, entity);
            entity
        });
    });
    OperatorResult::Finished
}

#[cfg(feature = "multiplayer")]
#[operator(id = "entity.add.zone_transition", label = "Zone Transition")]
pub(crate) fn entity_add_zone_transition(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::spawn_undoable(world, "Add Zone Transition", |world| {
            let mut system_state: SystemState<(Commands, ResMut<Selection>)> =
                SystemState::new(world);
            let Ok((mut commands, mut selection)) = system_state.get_mut(world) else {
                return Entity::PLACEHOLDER;
            };
            let entity = commands
                .spawn((
                    Name::new("Zone Transition"),
                    jackdaw_multiplayer::ZoneTransition {
                        half_extents: Vec3::splat(1.0),
                        ..default()
                    },
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            selection.select_single(&mut commands, entity);
            system_state.apply(world);
            crate::scene_io::register_entity_in_ast(world, entity);
            entity
        });
    });
    OperatorResult::Finished
}

#[cfg(feature = "multiplayer")]
#[operator(id = "entity.add.network_room", label = "Network Room")]
pub(crate) fn entity_add_network_room(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::spawn_undoable(world, "Add Network Room", |world| {
            let mut system_state: SystemState<(Commands, ResMut<Selection>)> =
                SystemState::new(world);
            let Ok((mut commands, mut selection)) = system_state.get_mut(world) else {
                return Entity::PLACEHOLDER;
            };
            let entity = commands
                .spawn((
                    Name::new("Network Room"),
                    jackdaw_multiplayer::NetworkRoom::default(),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            selection.select_single(&mut commands, entity);
            system_state.apply(world);
            crate::scene_io::register_entity_in_ast(world, entity);
            entity
        });
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.terrain", label = "Terrain")]
pub(crate) fn entity_add_terrain(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let is_first_terrain = world
            .query_filtered::<Entity, With<jackdaw_scene_types::Terrain>>()
            .iter(world)
            .next()
            .is_none();
        crate::spawn_undoable(world, "Add Terrain", move |world| {
            let mut system_state: SystemState<(Commands, ResMut<Selection>)> =
                SystemState::new(world);
            let Ok((mut commands, mut selection)) = system_state.get_mut(world) else {
                return Entity::PLACEHOLDER;
            };
            let entity = crate::terrain::spawn_terrain_entity(&mut commands);
            selection.select_single(&mut commands, entity);
            system_state.apply(world);
            crate::scene_io::register_entity_in_ast(world, entity);
            // Terrain settings live in the Terrain panel, not the Components
            // inspector, so the panel is opened for the document's first
            // terrain. Only the first: a later add must not take focus from
            // the tab the user is working in.
            if is_first_terrain {
                crate::open_window_in_default_area_if_absent(world, "jackdaw.inspector.terrain");
            }
            entity
        });
    });
    OperatorResult::Finished
}

/// Pick a prefab file and drop an instance of it at the origin.
///
/// The Add menu's other rows make what they name; this one has to be told
/// which prefab, so it asks through an async picker, polled, which keeps the
/// editor drawing while the dialog is up.
#[operator(id = "entity.add.prefab", label = "Prefab")]
pub fn entity_add_prefab(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(crate::prefab::operators::open_prefab_picker);
    OperatorResult::Finished
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;

    /// A world whose registry holds `Node`, so a document naming it reads as
    /// a scene and one naming nothing registered does not.
    fn world_with_node_registered() -> World {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        registry.write().register::<Node>();
        world.insert_resource(registry);
        world
    }

    /// One entity root carrying a registered component: the shape a copy
    /// emits and the only shape a paste accepts from a stranger.
    fn a_real_subtree() -> String {
        format!(
            "#Pasted\n{}\n",
            crate::inspector::node_card::node_type_path()
        )
    }

    #[test]
    fn prose_is_not_an_entity_document() {
        let world = world_with_node_registered();
        // A bare identifier parses as a component patch, so the parser
        // accepting the text is not on its own an answer.
        assert!(!is_entity_document(&world, "Remember to buy milk"));
        assert!(!is_entity_document(&world, "hello"));
    }

    #[test]
    fn json_is_not_an_entity_document() {
        let world = world_with_node_registered();
        assert!(!is_entity_document(
            &world,
            r#"{"name": "thing", "value": 3}"#
        ));
    }

    #[test]
    fn a_root_naming_no_registered_component_is_not_an_entity_document() {
        let world = world_with_node_registered();
        assert!(
            !is_entity_document(&world, "#Lonely\nsome::type::NobodyRegisters\n"),
            "a root whose every patch names an unknown type is text that parses"
        );
    }

    #[test]
    fn a_copied_subtree_is_an_entity_document() {
        let world = world_with_node_registered();
        assert!(is_entity_document(&world, &a_real_subtree()));
    }

    #[test]
    fn a_payload_past_the_size_cap_is_refused() {
        let world = world_with_node_registered();
        let mut oversized = a_real_subtree();
        oversized.push_str(&" ".repeat(MAX_CLIPBOARD_BYTES));
        assert!(
            !is_entity_document(&world, &oversized),
            "a payload past the cap is refused before it is parsed"
        );
    }

    #[test]
    fn os_text_this_editor_emitted_pastes_the_mirror() {
        let world = world_with_node_registered();
        let emitted = a_real_subtree();
        assert_eq!(
            choose_clipboard_text(
                &world,
                Some(emitted.clone()),
                Some((emitted.clone(), emitted.clone()))
            ),
            Some(emitted),
            "text equal to the last emission is our own copy"
        );
    }

    #[test]
    fn os_text_that_is_not_entities_refuses_rather_than_pasting_the_mirror() {
        let world = world_with_node_registered();
        let mirror = a_real_subtree();
        for foreign in ["Remember to buy milk", r#"{"a": 1}"#] {
            assert_eq!(
                choose_clipboard_text(
                    &world,
                    Some(mirror.clone()),
                    Some((foreign.to_string(), mirror.clone()))
                ),
                None,
                "{foreign} must refuse, not paste what was copied an hour ago"
            );
        }
    }

    #[test]
    fn a_subtree_from_another_window_is_accepted_over_the_mirror() {
        let world = world_with_node_registered();
        let foreign = format!(
            "#FromElsewhere\n{}\n",
            crate::inspector::node_card::node_type_path()
        );
        assert_eq!(
            choose_clipboard_text(
                &world,
                Some(a_real_subtree()),
                Some((foreign.clone(), a_real_subtree()))
            ),
            Some(foreign),
            "a real subtree written by another instance wins over the mirror"
        );
    }

    #[test]
    fn with_no_os_clipboard_the_mirror_answers() {
        let world = world_with_node_registered();
        let mirror = a_real_subtree();
        assert_eq!(
            choose_clipboard_text(&world, Some(mirror.clone()), None),
            Some(mirror)
        );
        assert_eq!(choose_clipboard_text(&world, None, None), None);
    }

    /// A document whose `Children` lists point back at an ancestor is not
    /// something a parser can produce, but it is something a hostile or
    /// corrupt payload could be built into. The walks that touch a pasted
    /// document have to end.
    fn ast_with_a_cycle() -> jackdaw_bsn::SceneBsnAst {
        let mut ast = jackdaw_bsn::SceneBsnAst::default();
        let child = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Child".to_string())]);
        let root = ast.create_entity_node(vec![
            jackdaw_bsn::BsnPatch::Name("Root".to_string()),
            jackdaw_bsn::BsnPatch::Children(vec![child]),
        ]);
        ast.add_to_roots(root);
        ast.add_child_to_ast(child, root);
        ast
    }

    #[test]
    fn minting_ids_over_a_cyclic_document_ends() {
        let world = world_with_node_registered();
        let mut ast = ast_with_a_cycle();
        mint_scene_node_ids(&world, &mut ast);
        assert_eq!(
            walk_entity_nodes(&ast).len(),
            2,
            "each node is visited once however the children point"
        );
    }

    #[test]
    fn naming_a_cyclic_document_ends() {
        let mut world = world_with_node_registered();
        let mut ast = ast_with_a_cycle();
        assign_unique_entity_names(&mut world, &mut ast);
        let names: Vec<Option<&str>> = walk_entity_nodes(&ast)
            .into_iter()
            .map(|node| ast.get_name(node))
            .collect();
        assert_eq!(names, vec![Some("Root"), Some("Child")]);
    }

    /// A widget is a subtree, so a copied Button carries its caption. A paste
    /// that renamed only the root would leave two entities called `Caption`.
    #[test]
    fn a_pasted_subtree_uniquifies_its_descendants_too() {
        let mut world = world_with_node_registered();
        world.spawn(Name::new("Button"));
        world.spawn(Name::new("Caption"));

        let mut ast = jackdaw_bsn::SceneBsnAst::default();
        let caption =
            ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Caption".to_string())]);
        let root = ast.create_entity_node(vec![
            jackdaw_bsn::BsnPatch::Name("Button".to_string()),
            jackdaw_bsn::BsnPatch::Children(vec![caption]),
        ]);
        ast.add_to_roots(root);

        assign_unique_entity_names(&mut world, &mut ast);

        assert_eq!(ast.get_name(root), Some("Button2"));
        assert_eq!(
            ast.get_name(caption),
            Some("Caption2"),
            "a descendant colliding with a live name is renamed too"
        );
    }

    /// The suffix has no space in it: a clause value cannot carry one, so a
    /// name with a space is unreachable from `JACKDAW_RUN_OP`.
    #[test]
    fn free_names_are_addressable_from_a_clause() {
        let mut taken = std::collections::HashSet::new();
        assert_eq!(claim_free_name(&mut taken, "Button"), None);
        assert_eq!(
            claim_free_name(&mut taken, "Button"),
            Some("Button2".to_string())
        );
        assert_eq!(
            claim_free_name(&mut taken, "Button"),
            Some("Button3".to_string())
        );
        for name in taken {
            assert!(!name.contains(' '), "{name} cannot be written in a clause");
        }
    }

    /// A name written with the older spacing renumbers from the same base
    /// rather than growing a second suffix.
    #[test]
    fn an_older_spaced_suffix_renumbers_from_its_base() {
        let mut taken: std::collections::HashSet<String> =
            ["Button".to_string(), "Button 2".to_string()]
                .into_iter()
                .collect();
        assert_eq!(
            claim_free_name(&mut taken, "Button 2"),
            Some("Button3".to_string())
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `mint_scene_node_ids` replaces any existing `SceneNodeId` with a fresh
    /// sparse id and leaves other patches alone.
    #[test]
    fn mint_scene_node_ids_replaces_existing_ids() {
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();

        let mut ast = jackdaw_bsn::SceneBsnAst::default();
        let node = ast.create_entity_node(vec![
            jackdaw_bsn::BsnPatch::TupleStruct(jackdaw_bsn::BsnTupleStructData {
                type_path: jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH.to_string(),
                values: vec![jackdaw_bsn::BsnValue::Int(42)],
            }),
            jackdaw_bsn::BsnPatch::Name("Kept".to_string()),
        ]);
        ast.add_to_roots(node);

        mint_scene_node_ids(&world, &mut ast);

        let id = ast.stable_id_of(node).expect("id patch present");
        assert_ne!(id, 42, "the stale id must be replaced");
        assert!(
            id >= jackdaw_scene_types::SPARSE_MIN,
            "minted id must be in the sparse range"
        );
        assert_eq!(ast.get_name(node), Some("Kept"), "other patches survive");
    }

    #[test]
    fn assign_unique_entity_names_keeps_free_names_and_numbers_collisions() {
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        world.spawn(Name::new("Brush"));
        world.spawn(Name::new("Brush2"));
        world.spawn((Name::new("Camera"), EditorEntity));

        let mut ast = jackdaw_bsn::SceneBsnAst::default();
        let free = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Camera".to_string())]);
        let taken = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Brush".to_string())]);
        let also_taken =
            ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Brush".to_string())]);
        ast.add_to_roots(free);
        ast.add_to_roots(taken);
        ast.add_to_roots(also_taken);

        assign_unique_entity_names(&mut world, &mut ast);

        assert_eq!(
            ast.get_name(free),
            Some("Camera"),
            "editor chrome names do not force renames"
        );
        assert_eq!(
            ast.get_name(taken),
            Some("Brush3"),
            "colliding name takes the next free number"
        );
        assert_eq!(
            ast.get_name(also_taken),
            Some("Brush4"),
            "batch collisions advance past names assigned earlier in the same pass"
        );
    }

    /// The Terrain panel auto-focuses only when a document gains its first
    /// terrain. A later add must not take focus from the tab the user is
    /// working in.
    mod terrain_focus_guard {
        use jackdaw_panels::DockAreaStyle;
        use jackdaw_panels::tree::{DockLeaf, DockNode, DockTree};

        use super::*;

        /// Mirrors `build_default_tree`'s `right_sidebar` leaf: seeded
        /// at boot with every registered window as a tab, Components
        /// first and therefore active.
        fn world_with_right_sidebar_seeded() -> World {
            let mut world = World::new();
            world.insert_resource(CommandHistory::default());
            world.insert_resource(Selection::default());
            let mut tree = DockTree::new();
            tree.set_root_leaf(
                DockLeaf::new("right_sidebar", DockAreaStyle::TabBar).with_windows(vec![
                    "jackdaw.inspector".to_string(),
                    "jackdaw.inspector.terrain".to_string(),
                    "jackdaw.inspector.materials".to_string(),
                ]),
            );
            world.insert_resource(tree);
            world
        }

        fn active_window(world: &World) -> Option<String> {
            let tree = world.resource::<DockTree>();
            let leaf = tree.get(tree.root?).and_then(DockNode::as_leaf)?;
            leaf.windows
                .iter()
                .find(|t| Some(t.id) == leaf.active)
                .map(|t| t.window_id.clone())
        }

        fn focus_window(world: &mut World, window_id: &str) {
            let mut tree = world.resource_mut::<DockTree>();
            let leaf_id = tree.root.expect("seeded tree has a root");
            let tab_id = tree
                .get(leaf_id)
                .and_then(DockNode::as_leaf)
                .and_then(|l| l.tabs().find(|(id, _)| *id == window_id))
                .map(|(_, tab)| tab)
                .expect("window is present as a seeded tab");
            tree.set_active(leaf_id, tab_id);
        }

        #[test]
        fn first_terrain_add_focuses_the_seeded_unfocused_tab() {
            let mut world = world_with_right_sidebar_seeded();

            let result = world
                .run_system_cached_with(entity_add_terrain, OperatorParameters::default())
                .expect("system runs");
            assert_eq!(result, OperatorResult::Finished);

            assert_eq!(
                active_window(&world).as_deref(),
                Some("jackdaw.inspector.terrain"),
                "the document's first terrain should bring the panel to front"
            );
        }

        #[test]
        fn second_terrain_add_leaves_components_active() {
            let mut world = world_with_right_sidebar_seeded();

            let result = world
                .run_system_cached_with(entity_add_terrain, OperatorParameters::default())
                .expect("first add runs");
            assert_eq!(result, OperatorResult::Finished);
            assert_eq!(
                active_window(&world).as_deref(),
                Some("jackdaw.inspector.terrain"),
                "sanity check: first add still focuses Terrain"
            );

            // The user switches back to Components to keep working there.
            focus_window(&mut world, "jackdaw.inspector");

            let result = world
                .run_system_cached_with(entity_add_terrain, OperatorParameters::default())
                .expect("second add runs");
            assert_eq!(result, OperatorResult::Finished);

            assert_eq!(
                active_window(&world).as_deref(),
                Some("jackdaw.inspector"),
                "a second terrain must not steal focus from Components"
            );
        }
    }

    /// A terrain's extent lives in its sidecar, so a saved scene states no rectangle beside
    /// it. The two extent fields are read only by the load-time migration, where a stated
    /// rectangle means a sidecar that cannot place its own grid.
    #[test]
    fn a_saved_terrain_declares_no_extent() {
        use bevy::ecs::reflect::AppTypeRegistry;

        let mut world = World::new();
        world.insert_resource(CommandHistory::default());
        world.insert_resource(Selection::default());
        world.init_resource::<AppTypeRegistry>();
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let mut writer = registry.write();
            writer.register::<Name>();
            writer.register::<Transform>();
            writer.register::<Visibility>();
            writer.register::<jackdaw_scene_types::Terrain>();
            writer.register::<jackdaw_scene_types::SceneNodeId>();
        }
        world.init_resource::<jackdaw_bsn::SceneBsnAst>();
        world.init_resource::<jackdaw_panels::tree::DockTree>();
        world.init_resource::<jackdaw_panels::registry::WindowRegistry>();

        let result = world
            .run_system_cached_with(entity_add_terrain, OperatorParameters::default())
            .expect("the operator runs");
        assert_eq!(result, OperatorResult::Finished);

        let text = crate::scene_io::emit_bsn_scene_with_inline_assets(
            &mut world,
            std::path::Path::new("."),
        );

        assert!(
            !text.contains("resolution:"),
            "the saved scene must not declare a resolution:\n{text}"
        );
        assert!(
            !text.contains("size:"),
            "the saved scene must not declare a size:\n{text}"
        );
    }
}
