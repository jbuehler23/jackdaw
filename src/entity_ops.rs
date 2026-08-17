use std::path::Path;

use bevy::{
    ecs::system::{SystemParam, SystemState},
    gltf::GltfAssetLabel,
    prelude::*,
};

use crate::{
    EditorEntity,
    commands::{CommandHistory, DespawnEntity, EditorCommand},
    selection::{Selected, Selection},
};
use bevy::input_focus::InputFocus;

/// System clipboard for copy/paste of entities as JSN text.
/// On Linux/X11 the clipboard is ownership-based: data is only available while
/// the Clipboard instance is alive. Storing as a Bevy Resource keeps it alive.
#[derive(Resource)]
pub struct SystemClipboard {
    clipboard: arboard::Clipboard,
    /// Fallback: last copied JSN text, in case system clipboard read fails.
    last_jsn: String,
}

impl Default for SystemClipboard {
    fn default() -> Self {
        Self {
            clipboard: arboard::Clipboard::new().expect("Failed to init system clipboard"),
            last_jsn: String::new(),
        }
    }
}

impl SystemClipboard {
    /// The OS clipboard image as owned RGBA8, or an error when the clipboard
    /// holds no image. Mirrors how the paste path reaches the inner clipboard.
    pub(crate) fn get_image(&mut self) -> Result<arboard::ImageData<'static>, arboard::Error> {
        self.clipboard.get_image()
    }
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
                    last_jsn: String::new(),
                });
            }
            Err(e) => {
                warn!("Failed to initialize system clipboard: {e}");
            }
        }
        app.register_type::<EmptyEntity>()
            .register_type::<SceneCamera>()
            .register_type::<SceneLight>()
            .register_type::<SceneFogVolume>()
            .register_type::<SceneReflectionProbe>()
            .register_type::<SceneAnimationPlayer>()
            .register_type::<SceneAudioSource>();
    }
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

/// Seed an empty live document with a directional light.
pub(crate) fn seed_new_scene_defaults(world: &mut World) {
    if !world.contains_resource::<jackdaw_bsn::SceneBsnAst>() {
        world.insert_resource(jackdaw_bsn::SceneBsnAst::default());
    }
    spawn_template_in_document(world, EntityTemplate::DirectionalLight);
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
    asset_server: &AssetServer,
    path: &str,
    position: Vec3,
    selection: &mut Selection,
) -> Entity {
    let file_name = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "GLTF Model".to_string());
    let scene_index = 0;
    let asset_path = to_asset_path(path);
    let scene = asset_server.load(GltfAssetLabel::Scene(scene_index).from_asset(asset_path));
    let entity = commands
        .spawn((
            Name::new(file_name),
            GltfSource {
                path: path.to_string(),
                scene_index,
            },
            WorldAssetRoot(scene),
            Transform::from_translation(position),
        ))
        .id();
    selection.select_single(commands, entity);
    entity
}

pub fn spawn_gltf_in_world(world: &mut World, path: &str, position: Vec3) {
    let mut system_state: SystemState<(Commands, Res<AssetServer>, ResMut<Selection>)> =
        SystemState::new(world);
    let Ok((mut commands, asset_server, mut selection)) = system_state.get_mut(world) else {
        return;
    };
    spawn_gltf(&mut commands, &asset_server, path, position, &mut selection);
    system_state.apply(world);
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
    remap_brush_stable_ids(world, ast);
    mint_scene_node_ids(world, ast);
    assign_unique_entity_root_names(world, ast);
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

/// Give each entity root in `ast` a `#Name` that does not collide with live
/// scene-entity names (or with earlier roots in the same batch). Editor chrome
/// (`EditorEntity`) is ignored so UI labels do not force renames. Collisions
/// get the next `Base N` suffix.
fn assign_unique_entity_root_names(world: &mut World, ast: &mut jackdaw_bsn::SceneBsnAst) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let entity_roots = {
        let reg = registry.read();
        jackdaw_bsn::entity_roots(ast, &reg)
    };

    let mut taken: std::collections::HashSet<String> = {
        let mut names = std::collections::HashSet::new();
        let mut query = world.query_filtered::<&Name, Without<EditorEntity>>();
        for existing in query.iter(world) {
            names.insert(existing.as_str().to_owned());
        }
        names
    };

    for root in entity_roots {
        let Some(name) = ast.get_name(root).map(str::to_owned) else {
            continue;
        };
        if taken.insert(name.clone()) {
            continue;
        }

        let mut base = name;
        if let Some(pos) = base.rfind(' ')
            && base[pos + 1..].parse::<u32>().is_ok()
        {
            base.truncate(pos);
        }
        let mut max_num = 0u32;
        for existing in &taken {
            if existing == &base {
                max_num = max_num.max(1);
            } else if let Some(rest) = existing.strip_prefix(base.as_str())
                && let Some(num_str) = rest.strip_prefix(' ')
                && let Ok(n) = num_str.parse::<u32>()
            {
                max_num = max_num.max(n);
            }
        }
        let new_name = format!("{} {}", base, max_num + 1);
        taken.insert(new_name.clone());
        crate::commands::set_name_patch(ast, root, Some(&new_name));
    }
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

/// Copy selected entities to the system clipboard as BSN text.
fn copy_components(world: &mut World) {
    let selection = world.resource::<Selection>();
    if selection.entities.is_empty() {
        return;
    }
    let selected: Vec<Entity> = selection.entities.clone();

    let nodes: Vec<Entity> = {
        let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
        selected.iter().filter_map(|&e| ast.ast_for(e)).collect()
    };
    if nodes.is_empty() {
        warn!("Copy: no selected entities have document nodes");
        return;
    }

    // Clipboard text is BSN: the selected subtrees plus embedded asset
    // entries, the same shape a saved scene uses, so it pastes into code
    // editors readably. Paste mints fresh SceneNodeIds before grafting.
    let parent_path = world
        .resource::<crate::scene_io::SceneFilePath>()
        .path
        .as_ref()
        .and_then(|p| Path::new(p).parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let bsn_text =
        crate::scene_io::emit_bsn_entities_with_inline_assets(world, &parent_path, &nodes);
    if bsn_text.trim().is_empty() {
        warn!("Copy: selected entities emitted no BSN text");
        return;
    }

    let Some(mut cb) = world.get_resource_mut::<SystemClipboard>() else {
        return;
    };
    info!(
        "Display: WAYLAND_DISPLAY={:?} DISPLAY={:?}",
        std::env::var("WAYLAND_DISPLAY").ok(),
        std::env::var("DISPLAY").ok(),
    );
    cb.last_jsn = bsn_text.clone();
    match cb.clipboard.set_text(&bsn_text) {
        Ok(()) => {
            // Verify by reading back
            match cb.clipboard.get_text() {
                Ok(readback) => info!(
                    "Clipboard set+readback OK ({} bytes written, {} read back)",
                    bsn_text.len(),
                    readback.len(),
                ),
                Err(e) => warn!("Clipboard set OK but readback failed: {e}"),
            }
        }
        Err(e) => warn!("Copy: system clipboard failed ({e}), using internal fallback"),
    }
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
    label: String,
    /// False on first push (paste already happened); true on subsequent executes (redo).
    is_redo: bool,
}

impl crate::commands::EditorCommand for PasteEntitiesCommand {
    fn execute(&mut self, world: &mut World) {
        // First push: paste already happened in paste_components; nothing to do.
        // Subsequent calls (redo): re-spawn from the remapped text.
        if !self.is_redo {
            self.is_redo = true;
            return;
        }

        let spawned = spawn_bsn_clipboard(world, &self.remapped_text);

        // Re-select the redo-pasted entities.
        for &entity in &world.resource::<Selection>().entities.clone() {
            if let Ok(mut ec) = world.get_entity_mut(entity) {
                ec.remove::<Selected>();
            }
        }
        let mut selection = world.resource_mut::<Selection>();
        selection.entities = spawned.clone();
        for &entity in &spawned {
            world.entity_mut(entity).insert(Selected);
        }

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
            world
                .resource_mut::<jackdaw_bsn::SceneBsnAst>()
                .remove_entity_node(e);
            if let Ok(ec) = world.get_entity_mut(e) {
                ec.despawn();
            }
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
fn spawn_bsn_clipboard(world: &mut World, text: &str) -> Vec<Entity> {
    let parsed = match jackdaw_bsn::parse_bsn_text(text) {
        Ok(ast) => ast,
        Err(e) => {
            warn!("Paste: failed to parse clipboard BSN: {e}");
            return Vec::new();
        }
    };
    graft_and_spawn(world, &parsed, None)
}

/// Rewrite `BrushStableId` values in a parsed clipboard document, replacing
/// each with a fresh ID minted from `StableIdCounter`. No-ops on nodes that
/// do not carry one (cameras, empties, etc.).
fn remap_brush_stable_ids(world: &mut World, ast: &mut jackdaw_bsn::SceneBsnAst) {
    let mut stack: Vec<Entity> = ast.roots.clone();
    let mut nodes = Vec::new();
    while let Some(node) = stack.pop() {
        nodes.push(node);
        stack.extend(ast.get_children_ast(node));
    }
    for node in nodes {
        let found = ast.get_patches(node).and_then(|patches| {
            patches.0.iter().copied().find(|&pe| {
                matches!(
                    ast.get_patch(pe),
                    Some(jackdaw_bsn::BsnPatch::TupleStruct(data))
                        if data.type_path.ends_with("BrushStableId")
                )
            })
        });
        if let Some(pe) = found {
            let type_path = match ast.get_patch(pe) {
                Some(jackdaw_bsn::BsnPatch::TupleStruct(data)) => data.type_path.clone(),
                _ => continue,
            };
            let fresh = crate::draw_brush::mint_stable_id(world);
            ast.set_patch(
                pe,
                jackdaw_bsn::BsnPatch::TupleStruct(jackdaw_bsn::BsnTupleStructData {
                    type_path,
                    values: vec![jackdaw_bsn::BsnValue::Int(fresh.0 as i128)],
                }),
            );
        }
    }
}

/// Ensure every entity node in `ast` carries a fresh `SceneNodeId` patch so
/// spawn/apply installs unique ids.
fn mint_scene_node_ids(world: &World, ast: &mut jackdaw_bsn::SceneBsnAst) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let asset_roots: std::collections::HashSet<Entity> = {
        let reg = registry.read();
        jackdaw_bsn::asset_roots(ast, &reg).into_iter().collect()
    };

    let mut stack: Vec<Entity> = ast.roots.clone();
    let mut nodes = Vec::new();
    while let Some(node) = stack.pop() {
        nodes.push(node);
        stack.extend(ast.get_children_ast(node));
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

/// Paste entities from system clipboard BSN text.
fn paste_components(world: &mut World) {
    if crate::asset_ingest::paste_clipboard_image(world) {
        return;
    }

    let text = {
        let Some(mut cb) = world.get_resource_mut::<SystemClipboard>() else {
            return;
        };
        cb.clipboard
            .get_text()
            .unwrap_or_else(|_| cb.last_jsn.clone())
    };

    if text.trim().is_empty() {
        return;
    }

    let mut parsed = match jackdaw_bsn::parse_bsn_text(&text) {
        Ok(ast) => ast,
        Err(e) => {
            warn!("Clipboard text is not valid BSN: {e}");
            return;
        }
    };
    if parsed.roots.is_empty() {
        return;
    }

    prepare_authored_subtree_for_spawn(world, &mut parsed);
    let spawned_node_ids = entity_root_scene_node_ids(world, &parsed);
    let remapped_text = jackdaw_bsn::emit_scene(&parsed);

    let spawned = graft_and_spawn(world, &parsed, None);
    if spawned.is_empty() {
        return;
    }

    for &entity in &world.resource::<Selection>().entities.clone() {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.remove::<Selected>();
        }
    }
    let mut selection = world.resource_mut::<Selection>();
    selection.entities = spawned.clone();
    for &entity in &spawned {
        world.entity_mut(entity).insert(Selected);
    }

    info!("Pasted {} entities from BSN clipboard", spawned.len());

    let cmd = PasteEntitiesCommand {
        spawned_node_ids,
        remapped_text,
        label: "Paste entities".to_string(),
        is_redo: false,
    };
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(cmd));
}

fn hide_selected(world: &mut World) {
    let selection = world.resource::<Selection>();
    let entities: Vec<Entity> = selection.entities.clone();

    if entities.is_empty() {
        return;
    }

    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

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
        cmds.push(Box::new(cmd));
    }

    if !cmds.is_empty() {
        let group = crate::commands::CommandGroup {
            commands: cmds,
            label: "Toggle visibility".to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(group));
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
        .register_operator::<EntityAddNavmeshOp>()
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
        .register_operator::<EntityAddReflectionProbeOp>();

    #[cfg(feature = "multiplayer")]
    ctx.register_operator::<EntityAddSpawnPointOp>()
        .register_operator::<EntityAddZoneTransitionOp>()
        .register_operator::<EntityAddNetworkRoomOp>();

    ctx.bind_operator::<CoreExtensionInputContext, EntityDeleteOp>([PresetInput::key("Delete")]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityDuplicateOp>([
        PresetInput::key("KeyD").ctrl()
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityCopyComponentsOp>([PresetInput::key(
        "KeyC",
    )
    .ctrl()]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityPasteComponentsOp>([PresetInput::key(
        "KeyV",
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
}

/// Shared availability check for entity manipulation operators.
/// Refuses to fire while a text input has focus, while a modal
/// operator is in flight, while the draw brush modal is active, or
/// while brush sub-element edit mode is active; matches the guards
/// the legacy `handle_entity_keys` applied.
fn can_act_on_entities(
    input_focus: Res<InputFocus>,
    active: ActiveModalQuery,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<crate::draw_brush::DrawBrushState>,
    edit_mode: Res<crate::brush::EditMode>,
) -> bool {
    if input_focus.get().is_some() || active.is_modal_running() || modal.active.is_some() {
        return false;
    }
    if draw_state.active.is_some() {
        return false;
    }
    matches!(*edit_mode, crate::brush::EditMode::Object)
}

// -- Entity lifecycle --------------------------------------------

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
    allows_undo = false,
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

#[operator(id = "entity.add.image", label = "Image")]
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

#[operator(id = "entity.add.navmesh", label = "Navmesh")]
pub(crate) fn entity_add_navmesh(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::spawn_undoable(world, "Add Navmesh Region", |world| {
            let mut system_state: SystemState<(Commands, ResMut<Selection>)> =
                SystemState::new(world);
            let Ok((mut commands, mut selection)) = system_state.get_mut(world) else {
                return Entity::PLACEHOLDER;
            };
            let entity = crate::navmesh::spawn_navmesh_entity(&mut commands);
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
        crate::spawn_undoable(world, "Add Terrain", |world| {
            let mut system_state: SystemState<(Commands, ResMut<Selection>)> =
                SystemState::new(world);
            let Ok((mut commands, mut selection)) = system_state.get_mut(world) else {
                return Entity::PLACEHOLDER;
            };
            let entity = crate::terrain::spawn_terrain_entity(&mut commands);
            selection.select_single(&mut commands, entity);
            system_state.apply(world);
            crate::scene_io::register_entity_in_ast(world, entity);
            entity
        });
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.prefab", label = "Prefab")]
pub(crate) fn entity_add_prefab(
    _: In<OperatorParameters>,
    mut _commands: Commands,
) -> OperatorResult {
    warn!(
        "entity.add.prefab: drag a prefab from the Asset Browser onto the viewport \
         to spawn an instance"
    );
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `remap_brush_stable_ids` replaces existing `BrushStableId`
    /// values in a parsed clipboard document with fresh IDs from
    /// `StableIdCounter`, so pasted copies don't share IDs with their
    /// originals.
    #[test]
    fn paste_assigns_fresh_stable_ids() {
        let mut world = World::new();
        crate::draw_brush::init_stable_id_counter(&mut world);

        const STABLE_ID_PATH: &str = "jackdaw::draw_brush::BrushStableId";
        let original_id: u64 = 7;

        let mut ast = jackdaw_bsn::SceneBsnAst::default();
        let node = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::TupleStruct(
            jackdaw_bsn::BsnTupleStructData {
                type_path: STABLE_ID_PATH.to_string(),
                values: vec![jackdaw_bsn::BsnValue::Int(original_id as i128)],
            },
        )]);
        ast.add_to_roots(node);

        remap_brush_stable_ids(&mut world, &mut ast);

        let stored = jackdaw_bsn::get_bsn_field(&ast, node, STABLE_ID_PATH, "0");
        assert!(
            matches!(stored, Some(jackdaw_bsn::BsnValue::Int(v)) if v != i128::from(original_id)),
            "pasted entity should have a new stable ID, got {stored:?}"
        );
    }

    /// Verifies that nodes without a `BrushStableId` patch are untouched by
    /// `remap_brush_stable_ids`.
    #[test]
    fn paste_does_not_add_stable_ids_to_non_brush_entities() {
        let mut world = World::new();
        crate::draw_brush::init_stable_id_counter(&mut world);

        let mut ast = jackdaw_bsn::SceneBsnAst::default();
        let node = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Empty".to_string())]);
        ast.add_to_roots(node);

        remap_brush_stable_ids(&mut world, &mut ast);

        assert_eq!(
            ast.component_type_paths(node),
            Vec::<String>::new(),
            "no component patch was added"
        );
    }

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
    fn assign_unique_entity_root_names_keeps_free_names_and_numbers_collisions() {
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        world.spawn(Name::new("Brush"));
        world.spawn(Name::new("Brush 2"));
        world.spawn((Name::new("Camera"), EditorEntity));

        let mut ast = jackdaw_bsn::SceneBsnAst::default();
        let free = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Camera".to_string())]);
        let taken = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Brush".to_string())]);
        let also_taken =
            ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Brush".to_string())]);
        ast.add_to_roots(free);
        ast.add_to_roots(taken);
        ast.add_to_roots(also_taken);

        assign_unique_entity_root_names(&mut world, &mut ast);

        assert_eq!(
            ast.get_name(free),
            Some("Camera"),
            "editor chrome names do not force renames"
        );
        assert_eq!(
            ast.get_name(taken),
            Some("Brush 3"),
            "colliding name takes the next free number"
        );
        assert_eq!(
            ast.get_name(also_taken),
            Some("Brush 4"),
            "batch collisions advance past names assigned earlier in the same pass"
        );
    }
}
