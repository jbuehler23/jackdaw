//! The open scene as the person authored it, as opposed to what the editor
//! and plugins derive at runtime.
//!
//! An entity in the editor carries both: the components saved with the
//! scene, and companions added by `#[require]`, observers and plugins that
//! are rebuilt on load. Tools that remove what a person added read the
//! authored set here. Writes go through operators such as `component.add`,
//! `component.remove` and `component.set`, so they reach undo and the saved
//! file.
//!
//! The scene's file is [`SceneFilePath`]. [`SceneOpened`], [`SceneSaved`] and
//! [`SceneRenamed`] are triggered as it comes to the front, is written and
//! moves, for an extension that keeps files of its own beside it, and a
//! [`SceneResource`] keeps per-scene state with its tab.

use bevy::ecs::component::Mutable;
use bevy::prelude::*;

/// Reads the open scene's document. The editor installs the
/// implementation.
pub trait DocumentReader: Send + Sync + 'static {
    /// The reflected type paths of the components authored on `entity`, or
    /// `None` when `entity` is not part of the open scene.
    fn authored_components(&self, world: &World, entity: Entity) -> Option<Vec<String>>;
}

/// The open scene's document.
#[derive(Resource)]
pub struct SceneDocument(pub Box<dyn DocumentReader>);

impl SceneDocument {
    /// The reflected type paths of the components authored on `entity`.
    /// Empty when `entity` is not part of the open scene or no editor
    /// document is installed.
    pub fn authored_components(world: &World, entity: Entity) -> Vec<String> {
        world
            .get_resource::<Self>()
            .and_then(|document| document.0.authored_components(world, entity))
            .unwrap_or_default()
    }
}

/// The file the scene in front was opened from or last saved to, and its
/// metadata.
#[derive(Resource, Default)]
pub struct SceneFilePath {
    /// `None` until an untitled scene is first saved.
    pub path: Option<String>,
    pub metadata: SceneMetadata,
    /// Where the last file dialog was pointed.
    pub last_directory: Option<std::path::PathBuf>,
}

/// Human-readable metadata for the scene in front, tracked live on
/// [`SceneFilePath`].
#[derive(Clone, Debug, Default)]
pub struct SceneMetadata {
    pub name: String,
    pub description: String,
    pub author: String,
    pub created: String,
    pub modified: String,
}

/// Triggered once a scene is in front: opened from a file, or brought back
/// by switching to its tab. `path` is `None` for a scene never saved.
#[derive(Event, Clone, Debug)]
pub struct SceneOpened {
    pub path: Option<std::path::PathBuf>,
}

/// Triggered after the scene in front is written to `path`. An extension
/// that keeps files beside the scene writes them here.
#[derive(Event, Clone, Debug)]
pub struct SceneSaved {
    pub path: std::path::PathBuf,
}

/// Triggered when the scene in front takes a new file, through Save As or a
/// rename, keeping its contents.
#[derive(Event, Clone, Debug)]
pub struct SceneRenamed {
    pub path: std::path::PathBuf,
}

/// Write `contents` to `path` through a temporary file renamed over it, so a
/// process that dies or a full disk leaves the old file rather than half of the
/// new one. The bytes are not forced to the disk first, so a power cut can still
/// take them: see [`write_durable`].
///
/// The temporary file sits in `path`'s own directory, named by
/// [`temporary_path`]: a rename across filesystems is not atomic, and fails
/// outright on most platforms.
pub fn write_atomic(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    write_through_temporary(path, contents, false, || Ok(()))
}

/// [`write_atomic`] with the file and its folder forced to the disk, off the main
/// thread since that takes a while. `before_rename` runs last of all ahead of the
/// rename, and its error leaves the old file standing.
pub fn write_durable(
    path: &std::path::Path,
    contents: &[u8],
    before_rename: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    write_through_temporary(path, contents, true, before_rename)?;
    #[cfg(unix)]
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        && let Ok(folder) = std::fs::File::open(parent)
    {
        // The file is in place by now, so a folder that will not sync fails nothing.
        let _ = folder.sync_all();
    }
    Ok(())
}

fn write_through_temporary(
    path: &std::path::Path,
    contents: &[u8],
    sync: bool,
    before_rename: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    let temp_path = temporary_path(path)?;
    let written = write_temporary(&temp_path, contents, sync)
        .and_then(|()| before_rename())
        .and_then(|()| std::fs::rename(&temp_path, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    written
}

fn write_temporary(path: &std::path::Path, contents: &[u8], sync: bool) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::File::create(path)?;
    file.write_all(contents)?;
    if sync {
        file.sync_all()?;
    }
    Ok(())
}

/// Sets a temporary file apart from anything a person or another tool would name,
/// since a sweep deletes what carries it.
const TEMPORARY_MARKER: &str = ".jackdaw-tmp-";

/// Where [`write_atomic`] puts the bytes for `path` until they are complete. One
/// left by a process that died is found by [`is_temporary_name`].
pub fn temporary_path(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other(format!("{} has no file name", path.display())))?;
    Ok(parent.join(format!(
        ".{}{TEMPORARY_MARKER}{}",
        file_name.to_string_lossy(),
        std::process::id()
    )))
}

/// Whether a file name is one [`temporary_path`] gives.
pub fn is_temporary_name(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|rest| rest.rsplit_once(TEMPORARY_MARKER))
        .is_some_and(|(file, pid)| {
            !file.is_empty() && !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// A stashed [`SceneResource`] value, held by an inactive scene tab.
pub type SceneResourceValue = Box<dyn std::any::Any + Send + Sync>;

/// A resource that belongs to the scene in front rather than to the editor.
///
/// Spawned by [`crate::ExtensionContext::register_scene_resource`]. When
/// another tab comes to the front, the editor keeps this one's value with
/// its tab and brings back the other tab's. A scene opened or created in a
/// tab starts from the resource's default.
#[derive(Component, Clone, Copy, Debug)]
pub struct SceneResource {
    type_id: std::any::TypeId,
    take: fn(&mut World) -> Option<SceneResourceValue>,
    restore: fn(&mut World, Option<SceneResourceValue>),
}

impl SceneResource {
    pub fn of<R: Resource<Mutability = Mutable> + Default>() -> Self {
        Self {
            type_id: std::any::TypeId::of::<R>(),
            take: |world| {
                let mut resource = world.get_resource_mut::<R>()?;
                Some(Box::new(core::mem::take(&mut *resource)))
            },
            restore: |world, value| match value.map(SceneResourceValue::downcast::<R>) {
                Some(Ok(value)) => world.insert_resource(*value),
                _ => {
                    if let Some(mut resource) = world.get_resource_mut::<R>() {
                        *resource = R::default();
                    }
                }
            },
        }
    }

    /// The resource type, which a tab keys its stashed values by.
    pub fn type_id(&self) -> std::any::TypeId {
        self.type_id
    }

    /// Take the value out of the world, leaving the default in its place.
    pub fn take(&self, world: &mut World) -> Option<SceneResourceValue> {
        (self.take)(world)
    }

    /// Put back a value [`Self::take`] gave, or the default for `None`.
    pub fn restore(&self, world: &mut World, value: Option<SceneResourceValue>) {
        (self.restore)(world, value);
    }
}
