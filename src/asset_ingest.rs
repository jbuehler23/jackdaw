//! Bring external files into the project: drag-drop and clipboard paste feed
//! one ingest path that copies the file into the project assets dir, then
//! routes by type. This module owns the filesystem import, the OS file-drop
//! reader, and the per-type (image) handler.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use bevy::picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility};
use bevy::prelude::*;
use bevy::window::FileDragAndDrop;

use crate::project::ProjectRoot;
use crate::reference_image::ReferenceImage;
use crate::viewport::{MainViewportCamera, ViewportCursor};

/// The kind of asset a dropped / pasted file is, resolved from its extension.
/// Drives which handler runs after the file is imported into the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Image,
    /// Imported into the project but with no viewport handler yet (models,
    /// materials, prefabs, audio land here until their handlers exist).
    Other,
}

/// Classify an asset by file extension (case-insensitive). v1 recognizes
/// images; everything else is `Other` (import-only).
pub fn classify(extension: &str) -> AssetKind {
    match extension.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tga" => AssetKind::Image,
        _ => AssetKind::Other,
    }
}

/// Copy `source` into `assets_dir`, returning the project-relative file name.
/// On a name collision, append `-1`, `-2`, ... before the extension so an
/// import never overwrites an existing asset.
pub fn import_to_assets(assets_dir: &Path, source: &Path) -> std::io::Result<String> {
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset");
    let ext = source.extension().and_then(|e| e.to_str());
    let dest_name = unique_name(assets_dir, stem, ext);
    std::fs::copy(source, assets_dir.join(&dest_name))?;
    Ok(dest_name)
}

/// Write in-memory `bytes` (a pasted clipboard image) to a unique `stem.ext`
/// in `assets_dir`, returning the project-relative file name.
pub fn write_to_assets(
    assets_dir: &Path,
    stem: &str,
    ext: &str,
    bytes: &[u8],
) -> std::io::Result<String> {
    let dest_name = unique_name(assets_dir, stem, Some(ext));
    std::fs::write(assets_dir.join(&dest_name), bytes)?;
    Ok(dest_name)
}

/// A file name under `dir` that does not collide with an existing file:
/// `stem.ext`, then `stem-1.ext`, `stem-2.ext`, ...
fn unique_name(dir: &Path, stem: &str, ext: Option<&str>) -> String {
    let with = |name: &str| match ext {
        Some(e) => format!("{name}.{e}"),
        None => name.to_string(),
    };
    if !dir.join(with(stem)).exists() {
        return with(stem);
    }
    let mut n = 1;
    loop {
        let candidate = with(&format!("{stem}-{n}"));
        if !dir.join(&candidate).exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Distance in front of the camera to place an image when the cursor ray
/// misses all geometry, so a dropped picture still lands somewhere visible.
const DROP_FALLBACK_DISTANCE: f32 = 5.0;

/// Where a dropped image plane should be spawned: its world position and a
/// rotation that turns the plane to face the camera. Copied per image in a
/// multi-file drop, so the same placement applies to each.
#[derive(Clone, Copy)]
struct ImagePlacement {
    position: Vec3,
    rotation: Quat,
}

pub struct AssetIngestPlugin;

impl Plugin for AssetIngestPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            handle_file_drops.run_if(in_state(crate::AppState::Editor)),
        );
    }
}

/// Read OS file-drop events and bring each dropped file into the project.
///
/// Everything that needs `SystemParam` access (the project root, the cursor,
/// the viewport camera, the geometry raycast, the filesystem import) happens
/// here; the spawn of a reference-image plane needs `&mut World`, so it is
/// deferred to a `commands.queue` closure that only does the spawn and sets
/// the camera-facing rotation.
fn handle_file_drops(
    mut drops: MessageReader<FileDragAndDrop>,
    project: Option<Res<ProjectRoot>>,
    vp: ViewportCursor,
    mut ray_cast: MeshRayCast,
    editor_entities: Query<(), With<crate::EditorEntity>>,
    mut commands: Commands,
) {
    let dropped: Vec<PathBuf> = drops
        .read()
        .filter_map(|event| match event {
            FileDragAndDrop::DroppedFile { path_buf, .. } => Some(path_buf.clone()),
            _ => None,
        })
        .collect();
    if dropped.is_empty() {
        return;
    }

    let Some(project) = project else {
        warn!("file dropped but no project is open; ignoring the drop");
        return;
    };
    let assets_dir = project.assets_dir();

    // Computed once and reused: the cursor placement is the same for every
    // image in a multi-file drop.
    let placement = image_placement_under_cursor(&vp, &mut ray_cast, &editor_entities);

    for path in dropped {
        let rel = match import_to_assets(&assets_dir, &path) {
            Ok(rel) => rel,
            Err(err) => {
                warn!("failed to import {}: {err}", path.display());
                continue;
            }
        };
        // The asset browser watches the assets dir with `notify`, so the
        // freshly-copied file shows up on the next refresh on its own; no
        // explicit rescan trigger is fired here.

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match classify(ext) {
            AssetKind::Image => {
                let Some(ImagePlacement { position, rotation }) = placement else {
                    // Cursor wasn't over the viewport: import only.
                    info!(
                        "imported {rel}; drop the image over a viewport to place it in the scene"
                    );
                    continue;
                };
                commands.queue(move |world: &mut World| {
                    let entity = spawn_reference_image_facing(world, &rel, position, rotation);
                    if entity.is_none() {
                        warn!("imported {rel} but could not locate the spawned reference image");
                    }
                });
            }
            AssetKind::Other => {
                info!("imported {rel}; viewport placement for this type is not supported yet");
            }
        }
    }
}

/// World position + camera-facing rotation for a plane dropped under the
/// cursor, or `None` when the cursor isn't over a viewport.
///
/// Casts the cursor ray and uses the first geometry hit; if the ray misses
/// everything, falls back to a point [`DROP_FALLBACK_DISTANCE`] along the ray
/// so the plane still lands in view. The rotation turns the quad's local +Z
/// normal toward the camera.
fn image_placement_under_cursor(
    vp: &ViewportCursor,
    ray_cast: &mut MeshRayCast,
    editor_entities: &Query<(), With<crate::EditorEntity>>,
) -> Option<ImagePlacement> {
    let cursor_pos = vp.cursor()?;
    let (vp_computed, vp_tf) = vp.viewport()?;
    let (camera, cam_tf) = vp.camera()?;

    let map = crate::viewport_util::ViewportRemap::new(camera, vp_computed, vp_tf);
    let local_cursor = (cursor_pos - map.top_left) * map.remap;

    let ray = camera.viewport_to_world(cam_tf, local_cursor).ok()?;

    // Editor-internal meshes (gizmos, previews, the per-viewport grid) carry
    // `EditorEntity` and sit at world origin on off-screen render layers;
    // `MeshRayCast` ignores render layers, so filter them out to avoid
    // snapping the drop onto an invisible mesh. Same guard the viewport
    // selection raycast uses.
    let editor_filter = |entity: Entity| !editor_entities.contains(entity);
    let settings = MeshRayCastSettings::default()
        .with_visibility(RayCastVisibility::Any)
        .with_filter(&editor_filter);
    let position = match ray_cast.cast_ray(ray, &settings).first() {
        Some((_, hit)) => hit.point,
        None => ray.origin + *ray.direction * DROP_FALLBACK_DISTANCE,
    };

    let rotation = facing_rotation(position, cam_tf.translation());

    Some(ImagePlacement { position, rotation })
}

/// Rotation that aims a `+Z`-forward plane at the camera from `position`.
fn facing_rotation(position: Vec3, camera: Vec3) -> Quat {
    let to_camera = (camera - position).normalize_or_zero();
    if to_camera == Vec3::ZERO {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_arc(Vec3::Z, to_camera)
    }
}

/// Spawn a reference-image plane and orient it to face the camera.
///
/// [`crate::reference_image::spawn_reference_image_in_world`] doesn't surface
/// the spawned entity, so the new `ReferenceImage` is recovered by diffing the
/// set of reference-image entities across the spawn. Returns the entity whose
/// rotation was set, or `None` if it couldn't be found.
fn spawn_reference_image_facing(
    world: &mut World,
    rel: &str,
    position: Vec3,
    rotation: Quat,
) -> Option<Entity> {
    let before: HashSet<Entity> = world
        .query_filtered::<Entity, With<ReferenceImage>>()
        .iter(world)
        .collect();

    crate::reference_image::spawn_reference_image_in_world(world, rel, position);

    let new_entity = world
        .query_filtered::<Entity, With<ReferenceImage>>()
        .iter(world)
        .find(|entity| !before.contains(entity))?;

    if let Some(mut transform) = world.get_mut::<Transform>(new_entity) {
        transform.rotation = rotation;
    }
    Some(new_entity)
}

/// `SystemParam` wrapper around [`image_placement_under_cursor`] so the paste
/// path can reach the cursor, camera, and geometry raycast via
/// `world.run_system_cached`.
fn drop_placement_system(
    vp: ViewportCursor,
    mut ray_cast: MeshRayCast,
    editor_entities: Query<(), With<crate::EditorEntity>>,
) -> Option<ImagePlacement> {
    image_placement_under_cursor(&vp, &mut ray_cast, &editor_entities)
}

/// Place an image at the viewport center: a point [`DROP_FALLBACK_DISTANCE`] in
/// front of the main viewport camera, facing it. Used when a pasted image has
/// no cursor placement (cursor not over a viewport). `None` when there is no
/// viewport camera at all.
fn viewport_center_placement(
    cameras: Query<&GlobalTransform, With<MainViewportCamera>>,
) -> Option<ImagePlacement> {
    let cam_tf = cameras.iter().next()?;
    let position = cam_tf.translation() + cam_tf.forward() * DROP_FALLBACK_DISTANCE;
    let rotation = facing_rotation(position, cam_tf.translation());
    Some(ImagePlacement { position, rotation })
}

/// If the OS clipboard holds an image, write it into the project assets as a
/// PNG and spawn a reference-image plane in the viewport. Returns true when an
/// image was handled (so the caller skips the entity paste), false otherwise
/// (no clipboard image -> fall through to the normal paste).
pub(crate) fn paste_clipboard_image(world: &mut World) -> bool {
    let image = {
        let Some(mut cb) = world.get_resource_mut::<crate::entity_ops::SystemClipboard>() else {
            return false;
        };
        match cb.get_image() {
            Ok(image) => image,
            Err(_) => return false,
        }
    };

    let mut png: Vec<u8> = Vec::new();
    let encoded = image::RgbaImage::from_raw(
        image.width as u32,
        image.height as u32,
        image.bytes.into_owned(),
    )
    .and_then(|img| {
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()
    });
    if encoded.is_none() {
        warn!("clipboard image could not be encoded to PNG; nothing pasted");
        return true;
    }

    let Some(project) = world.get_resource::<ProjectRoot>() else {
        warn!("clipboard holds an image but no project is open; nothing pasted");
        return true;
    };
    let assets_dir = project.assets_dir();

    let rel = match write_to_assets(&assets_dir, "pasted", "png", &png) {
        Ok(rel) => rel,
        Err(err) => {
            warn!("failed to write pasted image into the project: {err}");
            return true;
        }
    };

    // A paste is an explicit add, so always place: under the cursor when it is
    // over a viewport, otherwise at the viewport center.
    let placement = world
        .run_system_cached(drop_placement_system)
        .ok()
        .flatten()
        .or_else(|| {
            world
                .run_system_cached(viewport_center_placement)
                .ok()
                .flatten()
        });

    let Some(ImagePlacement { position, rotation }) = placement else {
        info!("imported {rel}; open a viewport to place the pasted image in the scene");
        return true;
    };

    if spawn_reference_image_facing(world, &rel, position, rotation).is_none() {
        warn!("imported {rel} but could not locate the spawned reference image");
    }
    true
}
