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
/// crash or a full disk leaves the old file rather than half of the new one.
/// The editor writes scenes this way.
///
/// The temporary file sits in `path`'s own directory: a rename across
/// filesystems is not atomic, and fails outright on most platforms.
pub fn write_atomic(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other(format!("{} has no file name", path.display())))?;
    let temp_path = parent.join(format!(
        ".{}.tmp-{}",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    if let Err(err) = std::fs::write(&temp_path, contents) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }
    Ok(())
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
