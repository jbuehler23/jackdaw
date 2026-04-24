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

// Re-export from jackdaw_jsn
pub use jackdaw_jsn::GltfSource;

pub struct EntityOpsPlugin;

impl Plugin for EntityOpsPlugin {
    fn build(&self, app: &mut App) {
        // Note: GltfSource type registration is handled by JsnPlugin
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
            .register_type::<SceneLight>();
    }
}

/// Marks an entity as an intentionally-empty scene entity (`Add > Empty`).
/// Used by the viewport-overlay system to decide whether to draw a
/// fallback wireframe-cube marker. Serialises through the type registry
/// so empties loaded from a `.jsn` scene keep the marker.
#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct EmptyEntity;

/// Marks a camera as scene-authored (added via `Add > Camera` or by an
/// extension), so viewport overlays draw a frustum gizmo for it.
/// Editor-internal cameras (main viewport camera, material preview
/// camera) deliberately don't carry this marker.
#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct SceneCamera;

/// Marks a light as scene-authored, so viewport overlays draw
/// light-specific gizmos for it. Editor-internal lights (e.g. the
/// material-preview rig) deliberately don't carry this marker.
#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct SceneLight;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityTemplate {
    Empty,
    Cube,
    Sphere,
    PointLight,
    DirectionalLight,
    SpotLight,
    Camera3d,
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
            .spawn((Name::new("Empty"), EmptyEntity, Transform::default()))
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
                    shadows_enabled: true,
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
                    shadows_enabled: true,
                    ..default()
                },
                Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, 0.4, 0.0)),
            ))
            .id(),
        EntityTemplate::SpotLight => commands
            .spawn((
                Name::new("Spot Light"),
                SceneLight,
                SpotLight {
                    shadows_enabled: true,
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

/// World-access version of `create_entity`. Used from menu actions and other deferred contexts.
/// Pushes a `SpawnEntity` command so the addition can be undone.
pub fn create_entity_in_world(world: &mut World, template: EntityTemplate) {
    let label = format!("Add {}", template.label());
    let spawn_fn = Box::new(move |world: &mut World| -> Entity {
        let mut system_state: SystemState<(Commands, ResMut<Selection>)> = SystemState::new(world);
        let (mut commands, mut selection) = system_state.get_mut(world);
        let entity = create_entity(&mut commands, template, &mut selection);
        system_state.apply(world);
        crate::scene_io::register_entity_in_ast(world, entity);
        entity
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
            SceneRoot(scene),
            Transform::from_translation(position),
        ))
        .id();
    selection.select_single(commands, entity);
    entity
}

pub fn spawn_gltf_in_world(world: &mut World, path: &str, position: Vec3) {
    let mut system_state: SystemState<(Commands, Res<AssetServer>, ResMut<Selection>)> =
        SystemState::new(world);
    let (mut commands, asset_server, mut selection) = system_state.get_mut(world);
    spawn_gltf(&mut commands, &asset_server, path, position, &mut selection);
    system_state.apply(world);
}

pub fn delete_selected(world: &mut World) {
    let selection = world.resource::<Selection>();
    let entities: Vec<Entity> = selection.entities.clone();

    if entities.is_empty() {
        return;
    }

    // Build commands for each entity
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

    let mut new_entities = Vec::new();

    for &entity in &entities {
        if world.get_entity(entity).is_err() {
            continue;
        }
        if world.get::<EditorEntity>(entity).is_some() {
            continue;
        }

        // Snapshot the entity (and descendants) via DynamicSceneBuilder
        let mut snapshot_entities = Vec::new();
        crate::commands::collect_entity_ids(world, entity, &mut snapshot_entities);
        let scene = DynamicSceneBuilder::from_world(world)
            .extract_entities(snapshot_entities.into_iter())
            .build();

        // Write the snapshot back to create a clone
        let mut entity_map = Default::default();
        if scene.write_to_world(world, &mut entity_map).is_err() {
            continue;
        }

        // Find the cloned root entity
        let Some(&new_root) = entity_map.get(&entity) else {
            continue;
        };

        // Rename with incremented number suffix
        if let Some(name) = world.get::<Name>(new_root) {
            // Strip trailing " (Copy)" chains and trailing " N" to find base name
            let mut base = name.as_str().to_string();
            while base.ends_with(" (Copy)") {
                base.truncate(base.len() - 7);
            }
            if let Some(pos) = base.rfind(' ')
                && base[pos + 1..].parse::<u32>().is_ok()
            {
                base.truncate(pos);
            }

            // Find highest existing number for this base name
            let mut max_num = 0u32;
            let mut query = world.query::<&Name>();
            for existing in query.iter(world) {
                let s = existing.as_str();
                if s == base {
                    max_num = max_num.max(1);
                } else if let Some(rest) = s.strip_prefix(base.as_str())
                    && let Some(num_str) = rest.strip_prefix(' ')
                    && let Ok(n) = num_str.parse::<u32>()
                {
                    max_num = max_num.max(n);
                }
            }

            let new_name = format!("{} {}", base, max_num + 1);
            world.entity_mut(new_root).insert(Name::new(new_name));
        }

        // Preserve parent relationship from original
        let parent = world.get::<ChildOf>(entity).map(|c| c.0);
        if let Some(parent) = parent {
            world.entity_mut(new_root).insert(ChildOf(parent));
        } else {
            // Original was a root entity, remove any ChildOf the scene write may have added.
            world.entity_mut(new_root).remove::<ChildOf>();
        }

        new_entities.push(new_root);
    }

    // Register duplicates in AST
    crate::scene_io::register_entities_in_ast(world, &new_entities);

    // Select the new entities
    let mut selection = world.resource_mut::<Selection>();
    selection.entities = new_entities;
    for &entity in &selection.entities.clone() {
        world.entity_mut(entity).insert(Selected);
    }
}

/// Snap a vector to the nearest cardinal world axis (±X, ±Y, ±Z).
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

/// Copy all reflected components from the primary selected entity to the clipboard.
/// Copy selected entities to the system clipboard as JSN text.
fn copy_components(world: &mut World) {
    let selection = world.resource::<Selection>();
    if selection.entities.is_empty() {
        return;
    }

    let ast = world.resource::<jackdaw_jsn::SceneJsnAst>();
    let jsn_entities: Vec<jackdaw_jsn::format::JsnEntity> = selection
        .entities
        .iter()
        .filter_map(|&e| {
            ast.node_for_entity(e)
                .map(|node| jackdaw_jsn::format::JsnEntity {
                    parent: None,
                    components: node.components.clone(),
                })
        })
        .collect();

    if jsn_entities.is_empty() {
        warn!("Copy: no selected entities have AST nodes");
        return;
    }

    let jsn_text = match serde_json::to_string_pretty(&jsn_entities) {
        Ok(t) => t,
        Err(e) => {
            warn!("Failed to serialize entities for clipboard: {e}");
            return;
        }
    };

    let Some(mut cb) = world.get_resource_mut::<SystemClipboard>() else {
        return;
    };
    info!(
        "Display: WAYLAND_DISPLAY={:?} DISPLAY={:?}",
        std::env::var("WAYLAND_DISPLAY").ok(),
        std::env::var("DISPLAY").ok(),
    );
    cb.last_jsn = jsn_text.clone();
    match cb.clipboard.set_text(&jsn_text) {
        Ok(()) => {
            // Verify by reading back, like BSN branch does
            match cb.clipboard.get_text() {
                Ok(readback) => info!(
                    "Clipboard set+readback OK ({} bytes written, {} read back)",
                    jsn_text.len(),
                    readback.len(),
                ),
                Err(e) => warn!("Clipboard set OK but readback failed: {e}"),
            }
        }
        Err(e) => warn!("Copy: system clipboard failed ({e}), using internal fallback"),
    }
}

/// Paste entities from system clipboard JSN text.
fn paste_components(world: &mut World) {
    let jsn_text = {
        let Some(mut cb) = world.get_resource_mut::<SystemClipboard>() else {
            return;
        };
        cb.clipboard
            .get_text()
            .unwrap_or_else(|_| cb.last_jsn.clone())
    };

    if jsn_text.trim().is_empty() {
        return;
    }

    let parsed: Vec<jackdaw_jsn::format::JsnEntity> = match serde_json::from_str(&jsn_text) {
        Ok(entities) => entities,
        Err(e) => {
            warn!("Clipboard text is not valid JSN: {e}");
            return;
        }
    };

    if parsed.is_empty() {
        return;
    }

    let local_assets = std::collections::HashMap::new();
    let parent_path = std::path::Path::new(".");
    let spawned = crate::scene_io::load_scene_from_jsn(world, &parsed, parent_path, &local_assets);

    crate::scene_io::register_entities_in_ast(world, &spawned);

    // Deselect current, select pasted
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

    info!("Pasted {} entities from JSN clipboard", spawned.len());
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

        let mut cmd = crate::commands::SetJsnField {
            entity,
            type_path: "bevy_camera::visibility::Visibility".to_string(),
            field_path: String::new(),
            old_value: serde_json::Value::String(format!("{current:?}")),
            new_value: serde_json::Value::String(format!("{new_visibility:?}")),
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
        scene_entities
            .get(world)
            .iter()
            .filter(|(_, vis)| **vis == Visibility::Hidden)
            .map(|(e, _)| e)
            .collect()
    };

    for entity in hidden {
        let mut cmd = crate::commands::SetJsnField {
            entity,
            type_path: "bevy_camera::visibility::Visibility".to_string(),
            field_path: String::new(),
            old_value: serde_json::Value::String("Hidden".to_string()),
            new_value: serde_json::Value::String("Inherited".to_string()),
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
        scene_entities
            .get(world)
            .iter()
            .filter(|(_, vis)| **vis != Visibility::Hidden)
            .map(|(e, vis)| (e, *vis))
            .collect()
    };

    for (entity, current) in to_hide {
        let mut cmd = crate::commands::SetJsnField {
            entity,
            type_path: "bevy_camera::visibility::Visibility".to_string(),
            field_path: String::new(),
            old_value: serde_json::Value::String(format!("{current:?}")),
            new_value: serde_json::Value::String("Hidden".to_string()),
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
fn to_asset_path(path: &str) -> String {
    let path = Path::new(path);
    if let Some(assets_dir) = get_assets_base_dir()
        && let Ok(relative) = path.strip_prefix(&assets_dir)
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
        let assets = project_dir.join("assets");
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
    Some(base.join("assets"))
}

// ─────────────────────── Operators ────────────────────────────
//
// Entity-level operators (`entity.*`) and the `Add` menu
// (`entity.add.*`). Keybind and menu dispatch both arrive here.
// Operators are gated with `is_available = can_act_on_entities` so
// they refuse to fire while a brush sub-element drag or modal
// operator has the scene locked, matching the guards the legacy
// `handle_entity_keys` applied.

use bevy_enhanced_input::prelude::{Chord, Press, *};
use jackdaw_api::prelude::*;

use crate::core_extension::{CoreExtensionInputContext, Modifiers};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext, modifiers: &Modifiers) {
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
        .register_operator::<EntityAddCameraOp>()
        .register_operator::<EntityAddEmptyOp>()
        .register_operator::<EntityAddNavmeshOp>()
        .register_operator::<EntityAddTerrainOp>()
        .register_operator::<EntityAddPrefabOp>();

    let ext = ctx.id();
    let ctrl = modifiers.ctrl;
    let alt = modifiers.alt;
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<EntityDeleteOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::Delete, Press::default())],
        ));
        world.spawn((
            Action::<EntityDuplicateOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(ctrl),
            bindings![(KeyCode::KeyD, Press::default())],
        ));
        world.spawn((
            Action::<EntityCopyComponentsOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(ctrl),
            bindings![(KeyCode::KeyC, Press::default())],
        ));
        world.spawn((
            Action::<EntityPasteComponentsOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(ctrl),
            bindings![(KeyCode::KeyV, Press::default())],
        ));
        world.spawn((
            Action::<EntityToggleVisibilityOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::KeyH, Press::default())],
        ));
        world.spawn((
            Action::<EntityUnhideAllOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(ctrl),
            bindings![(KeyCode::KeyH, Press::default())],
        ));
        world.spawn((
            Action::<EntityHideUnselectedOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(alt),
            bindings![(KeyCode::KeyH, Press::default())],
        ));
    });
}

/// Shared availability check for entity manipulation operators.
/// Refuses to fire while a text input has focus, while a modal
/// operator is in flight, while the draw brush modal is active, or
/// while brush sub-element edit mode is active — matches the guards
/// the legacy `handle_entity_keys` applied.
fn can_act_on_entities(
    input_focus: Res<InputFocus>,
    active: ActiveModalQuery,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<crate::draw_brush::DrawBrushState>,
    edit_mode: Res<crate::brush::EditMode>,
) -> bool {
    if input_focus.0.is_some() || active.is_modal_running() || modal.active.is_some() {
        return false;
    }
    if draw_state.active.is_some() {
        return false;
    }
    matches!(*edit_mode, crate::brush::EditMode::Object)
}

// ── Entity lifecycle ────────────────────────────────────────────

#[operator(
    id = "entity.delete",
    label = "Delete",
    is_available = can_act_on_entities
)]
fn entity_delete(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(delete_selected);
    OperatorResult::Finished
}

#[operator(
    id = "entity.duplicate",
    label = "Duplicate",
    is_available = can_act_on_entities
)]
fn entity_duplicate(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(duplicate_selected);
    OperatorResult::Finished
}

#[operator(
    id = "entity.copy_components",
    label = "Copy Components",
    allows_undo = false,
    is_available = can_act_on_entities
)]
fn entity_copy_components(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(copy_components);
    OperatorResult::Finished
}

#[operator(
    id = "entity.paste_components",
    label = "Paste Components",
    is_available = can_act_on_entities
)]
fn entity_paste_components(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(paste_components);
    OperatorResult::Finished
}

#[operator(
    id = "entity.toggle_visibility",
    label = "Toggle Visibility",
    allows_undo = false,
    is_available = can_act_on_entities
)]
fn entity_toggle_visibility(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(hide_selected);
    OperatorResult::Finished
}

#[operator(
    id = "entity.hide_unselected",
    label = "Hide Unselected",
    allows_undo = false,
    is_available = can_act_on_entities
)]
fn entity_hide_unselected(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
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
fn entity_unhide_all(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        if let Err(err) = world.run_system_cached(unhide_all_entities) {
            warn!("unhide_all_entities: {err:?}");
        }
    });
    OperatorResult::Finished
}

// ── Add menu ────────────────────────────────────────────────────

#[operator(id = "entity.add.cube", label = "Cube")]
fn entity_add_cube(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Cube);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.sphere", label = "Sphere")]
fn entity_add_sphere(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Sphere);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.point_light", label = "Point Light")]
fn entity_add_point_light(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::PointLight);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.directional_light", label = "Directional Light")]
fn entity_add_directional_light(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::DirectionalLight);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.spot_light", label = "Spot Light")]
fn entity_add_spot_light(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::SpotLight);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.camera", label = "Camera")]
fn entity_add_camera(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Camera3d);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.empty", label = "Empty")]
fn entity_add_empty(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Empty);
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.navmesh", label = "Navmesh")]
fn entity_add_navmesh(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::spawn_undoable(world, "Add Navmesh Region", |world| {
            let mut system_state: SystemState<(Commands, ResMut<Selection>)> =
                SystemState::new(world);
            let (mut commands, mut selection) = system_state.get_mut(world);
            let entity = crate::navmesh::spawn_navmesh_entity(&mut commands);
            selection.select_single(&mut commands, entity);
            system_state.apply(world);
            crate::scene_io::register_entity_in_ast(world, entity);
            entity
        });
    });
    OperatorResult::Finished
}

#[operator(id = "entity.add.terrain", label = "Terrain")]
fn entity_add_terrain(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::spawn_undoable(world, "Add Terrain", |world| {
            let mut system_state: SystemState<(Commands, ResMut<Selection>)> =
                SystemState::new(world);
            let (mut commands, mut selection) = system_state.get_mut(world);
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
fn entity_add_prefab(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::prefab_picker::open_prefab_picker(world);
    });
    OperatorResult::Finished
}
