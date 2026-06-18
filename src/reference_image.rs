//! Reference image planes: scene entities artists model against.

use bevy::asset::LoadState;
use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use bevy::window::{PrimaryWindow, RawHandleWrapper};
use rfd::AsyncFileDialog;

use crate::selection::Selection;

/// A 2D reference picture in the scene. Serialized with the scene so
/// front/side boards survive sessions. `locked` makes viewport clicks
/// pass through to geometry behind it; the outliner still selects it.
#[derive(Component, Reflect, Clone)]
#[reflect(Component)]
pub struct ReferenceImage {
    pub path: String,
    pub opacity: f32,
    pub locked: bool,
}

impl Default for ReferenceImage {
    fn default() -> Self {
        Self {
            path: String::new(),
            opacity: 0.7,
            // Selectable and click-draggable like any object until the user
            // opts into locking (which makes the board pass-through for
            // modeling against it).
            locked: false,
        }
    }
}

/// Runtime texture tracking for a reference image. Plain (unreflected)
/// component, so it never serializes; `maintain_reference_images`
/// recreates it alongside the render components for scene-loaded and
/// undo-restored entities.
#[derive(Component)]
pub struct ReferenceImageRuntime {
    /// Path the current texture handle was loaded from. Compared
    /// against `ReferenceImage::path` to detect stale handles.
    loaded_path: String,
    image: Option<Handle<Image>>,
    /// The image decoded and its aspect ratio was applied to
    /// `Transform::scale`.
    aspect_applied: bool,
    /// A load failure for `loaded_path` was already reported.
    warned: bool,
}

/// Shared unit-quad mesh for every reference image plane. Aspect ratio
/// is applied per entity via `Transform::scale`, so one mesh serves all.
#[derive(Resource)]
pub struct ReferenceImageQuad(Handle<Mesh>);

impl FromWorld for ReferenceImageQuad {
    fn from_world(world: &mut World) -> Self {
        let mut meshes = world.resource_mut::<Assets<Mesh>>();
        Self(meshes.add(Rectangle::new(1.0, 1.0)))
    }
}

pub struct ReferenceImagePlugin;

impl Plugin for ReferenceImagePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ReferenceImage>()
            .init_resource::<ReferenceImageQuad>()
            .add_systems(
                Update,
                (maintain_reference_images, poll_reference_image_pick)
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

/// Spawn a reference image plane at `position`. Single spawn path
/// shared by the `entity.add.image` operator and the asset-browser
/// image drop. Render state (quad mesh, material, aspect scale) is
/// attached by [`maintain_reference_images`], the same way brushes and
/// terrains derive their meshes from the authored component.
pub fn spawn_reference_image(
    commands: &mut Commands,
    path: &str,
    position: Vec3,
    selection: &mut Selection,
) -> Entity {
    let entity = commands
        .spawn((
            Name::new("Image"),
            ReferenceImage {
                path: path.to_string(),
                ..default()
            },
            Transform::from_translation(position),
            Visibility::default(),
        ))
        .id();
    selection.select_single(commands, entity);
    entity
}

/// World-access wrapper: undoable spawn plus AST registration,
/// mirroring `create_entity_in_world`.
pub fn spawn_reference_image_in_world(world: &mut World, path: &str, position: Vec3) {
    let path = path.to_string();
    crate::spawn_undoable(world, "Add Image", move |world| {
        let mut system_state: SystemState<(Commands, ResMut<Selection>)> = SystemState::new(world);
        let (mut commands, mut selection) = system_state.get_mut(world);
        let entity = spawn_reference_image(&mut commands, &path, position, &mut selection);
        system_state.apply(world);
        crate::scene_io::register_entity_in_ast(world, entity);
        entity
    });
}

/// Pending image file-pick dialog for `entity.add.image`.
#[derive(Resource)]
pub struct ReferenceImagePickTask(Task<Option<rfd::FileHandle>>);

/// Open the image file picker backing `entity.add.image`. No-op while
/// a pick is already pending.
pub fn open_reference_image_picker(world: &mut World) {
    if world.contains_resource::<ReferenceImagePickTask>() {
        return;
    }
    let raw_handle = world
        .query_filtered::<&RawHandleWrapper, With<PrimaryWindow>>()
        .single(world)
        .ok()
        .cloned();
    let mut dialog = AsyncFileDialog::new()
        .set_title("Select reference image")
        .add_filter(
            "Images",
            &["png", "jpg", "jpeg", "ktx2", "bmp", "tga", "webp"],
        );
    if let Some(ref rh) = raw_handle {
        // SAFETY: called on the main thread from an exclusive context
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
    world.insert_resource(ReferenceImagePickTask(task));
}

fn poll_reference_image_pick(world: &mut World) {
    let Some(mut task_res) = world.get_resource_mut::<ReferenceImagePickTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task_res.0)) else {
        return;
    };
    world.remove_resource::<ReferenceImagePickTask>();
    let Some(file_handle) = result else {
        return;
    };
    let path = file_handle.path().to_string_lossy().replace('\\', "/");
    spawn_reference_image_in_world(world, &path, Vec3::ZERO);
}

fn reference_material(
    reference: &ReferenceImage,
    texture: Option<Handle<Image>>,
) -> StandardMaterial {
    // Flat placeholder tint when no texture is available, so an empty
    // or broken path still shows a visible, selectable plane.
    let base_color = if texture.is_some() {
        Color::srgba(1.0, 1.0, 1.0, reference.opacity)
    } else {
        Color::srgba(0.5, 0.5, 0.55, reference.opacity)
    };
    StandardMaterial {
        base_color,
        base_color_texture: texture,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        alpha_mode: AlphaMode::Blend,
        ..default()
    }
}

/// Keep every reference image's render state in sync with its
/// authored component. Rebuilds the material on `ReferenceImage`
/// changes, (re)creates missing `Mesh3d`/`MeshMaterial3d` so
/// scene-loaded and undo-restored entities self-heal, and applies the
/// image's aspect ratio to `Transform::scale` once the texture
/// decodes.
pub fn maintain_reference_images(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    images: Res<Assets<Image>>,
    quad: Res<ReferenceImageQuad>,
    mut refs: Query<(
        Entity,
        Ref<ReferenceImage>,
        Option<&mut ReferenceImageRuntime>,
        Has<Mesh3d>,
        Option<&MeshMaterial3d<StandardMaterial>>,
        &mut Transform,
    )>,
) {
    for (entity, reference, runtime, has_mesh, material, mut transform) in &mut refs {
        if !has_mesh {
            commands.entity(entity).insert(Mesh3d(quad.0.clone()));
        }

        let stale = match &runtime {
            Some(runtime) => runtime.loaded_path != reference.path,
            None => true,
        };
        if stale || material.is_none() {
            reload_reference_render(
                &mut commands,
                &asset_server,
                &mut materials,
                entity,
                &reference,
                material,
            );
            continue;
        }

        // Path is current and the material exists. Refresh just the
        // material alpha when non-path fields (opacity, locked) changed,
        // without resetting the runtime (aspect_applied and warned persist).
        if reference.is_changed() {
            if let Some(existing) = material.and_then(|handle| materials.get_mut(&handle.0)) {
                let texture = runtime.as_ref().and_then(|r| r.image.clone());
                *existing = reference_material(&reference, texture);
            }
            continue;
        }

        // Poll the pending texture: apply the aspect ratio once it
        // decodes, or fall back to the placeholder if the load failed.
        let Some(mut runtime) = runtime else {
            continue;
        };
        poll_reference_aspect(
            &asset_server,
            &images,
            &mut materials,
            entity,
            &reference,
            &mut runtime,
            material,
            &mut transform,
        );
    }
}

/// Full reload of a reference image's render: (re)load the texture from the
/// authored path (or warn for an empty path), rebuild or insert the material,
/// and reset the runtime tracking so the aspect-ratio poll re-runs.
fn reload_reference_render(
    commands: &mut Commands,
    asset_server: &AssetServer,
    materials: &mut Assets<StandardMaterial>,
    entity: Entity,
    reference: &ReferenceImage,
    material: Option<&MeshMaterial3d<StandardMaterial>>,
) {
    let texture = if reference.path.is_empty() {
        // Once per change, not per frame: the runtime insert
        // below stops this branch from re-running.
        warn!("Reference image {entity} has no image path; showing placeholder");
        None
    } else {
        let asset_path = crate::entity_ops::to_asset_path(&reference.path);
        Some(asset_server.load::<Image>(asset_path))
    };
    let new_material = reference_material(reference, texture.clone());
    match material.and_then(|handle| materials.get_mut(&handle.0)) {
        Some(existing) => *existing = new_material,
        None => {
            let handle = materials.add(new_material);
            commands.entity(entity).insert(MeshMaterial3d(handle));
        }
    }
    commands.entity(entity).insert(ReferenceImageRuntime {
        loaded_path: reference.path.clone(),
        image: texture,
        aspect_applied: false,
        warned: false,
    });
}

/// Poll the pending texture for a reference image: apply the aspect ratio to
/// `Transform::scale` once it decodes, or fall back to the placeholder material
/// if the load failed.
fn poll_reference_aspect(
    asset_server: &AssetServer,
    images: &Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    entity: Entity,
    reference: &ReferenceImage,
    runtime: &mut ReferenceImageRuntime,
    material: Option<&MeshMaterial3d<StandardMaterial>>,
    transform: &mut Transform,
) {
    let Some(image_handle) = runtime.image.clone() else {
        return;
    };
    if runtime.aspect_applied {
        return;
    }
    if let Some(image) = images.get(&image_handle) {
        let size = image.size().as_vec2();
        if size.y > 0.0 {
            let aspect = size.x / size.y;
            transform.scale.x = transform.scale.y * aspect;
        }
        runtime.aspect_applied = true;
    } else if !runtime.warned
        && matches!(
            asset_server.get_load_state(image_handle.id()),
            Some(LoadState::Failed(_))
        )
    {
        warn!(
            "Reference image {entity}: failed to load '{}'; showing placeholder",
            runtime.loaded_path
        );
        runtime.warned = true;
        runtime.image = None;
        if let Some(existing) = material.and_then(|handle| materials.get_mut(&handle.0)) {
            *existing = reference_material(reference, None);
        }
    }
}
