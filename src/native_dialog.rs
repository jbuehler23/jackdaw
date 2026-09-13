//! Every native file and folder dialog in the editor is built here, so
//! they all open parented to the main window and pointed at the folder
//! the user is already browsing rather than at the home directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, RawHandleWrapper};
use rfd::AsyncFileDialog;

use crate::project::ProjectRoot;

/// What a dialog is being opened for. Each purpose remembers the folder
/// it was last used in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogPurpose {
    Scene,
    Prefab,
    Image,
    Texture,
    Model,
    Bundle,
}

impl DialogPurpose {
    fn key(self) -> &'static str {
        match self {
            Self::Scene => "scene",
            Self::Prefab => "prefab",
            Self::Image => "image",
            Self::Texture => "texture",
            Self::Model => "model",
            Self::Bundle => "bundle",
        }
    }

    /// Whether this purpose picks files from outside the project, where the
    /// folder the editor is browsing is no help and the last one used is.
    fn looks_outside_the_project(self) -> bool {
        matches!(self, Self::Bundle)
    }
}

/// The folder each dialog purpose was last used in, kept for the session
/// and written back to `.jackdaw/project.json` while a project is open.
#[derive(Resource, Default)]
pub struct DialogMemory {
    folders: BTreeMap<String, PathBuf>,
}

impl DialogMemory {
    pub fn folder(&self, purpose: DialogPurpose) -> Option<&Path> {
        self.folders.get(purpose.key()).map(PathBuf::as_path)
    }

    pub fn remember(&mut self, purpose: DialogPurpose, directory: &Path) {
        self.folders
            .insert(purpose.key().to_string(), directory.to_path_buf());
    }

    pub fn folders(&self) -> &BTreeMap<String, PathBuf> {
        &self.folders
    }

    pub fn restore(&mut self, folders: BTreeMap<String, PathBuf>) {
        self.folders = folders;
    }
}

pub struct NativeDialogPlugin;

impl Plugin for NativeDialogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DialogMemory>()
            .add_systems(Update, restore_dialog_memory);
    }
}

fn restore_dialog_memory(project: Option<Res<ProjectRoot>>, mut memory: ResMut<DialogMemory>) {
    let Some(project) = project else {
        return;
    };
    if !project.is_added() {
        return;
    }
    memory.restore(project.config.dialog_directories.clone());
}

fn resolved(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn existing_directory(path: &Path) -> Option<PathBuf> {
    path.is_dir().then(|| path.to_path_buf())
}

fn project_root_path(world: &World) -> Option<PathBuf> {
    world.get_resource::<ProjectRoot>().map(|p| p.root.clone())
}

/// Is `candidate` the project root or a folder under it? Both sides are
/// resolved first so a symlinked `assets/` or a root reached by a
/// different path still counts as inside.
fn within_project(root: &Path, candidate: &Path) -> bool {
    resolved(candidate).starts_with(resolved(root))
}

fn browsed_directory(world: &World) -> Option<PathBuf> {
    let state = world.get_resource::<crate::project_window::ProjectWindowState>()?;
    let directory = existing_directory(&state.current_directory)?;
    let root = project_root_path(world)?;
    within_project(&root, &directory).then_some(directory)
}

/// The folder of the file the editor currently has open: the active
/// scene tab first, then whatever the last scene load or save touched.
fn open_file_directory(world: &World) -> Option<PathBuf> {
    let active_tab_path = world
        .get_resource::<crate::scenes::Scenes>()
        .and_then(|scenes| scenes.tabs.get(scenes.active))
        .and_then(|tab| tab.path.clone());
    let scene_file = world.get_resource::<crate::scene_io::SceneFilePath>();
    let recorded_path = scene_file
        .and_then(|scene| scene.path.as_ref())
        .map(PathBuf::from);
    let last_directory = scene_file.and_then(|scene| scene.last_directory.clone());

    active_tab_path
        .or(recorded_path)
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .or(last_directory)
        .as_deref()
        .and_then(existing_directory)
}

fn project_directory(world: &World) -> Option<PathBuf> {
    let project = world.get_resource::<ProjectRoot>()?;
    existing_directory(&project.assets_dir()).or_else(|| existing_directory(&project.root))
}

/// The folder the user is looking at: the Project window's folder while
/// it points inside the project, else the open file's folder, else the
/// project's `assets/`, else the project root.
pub fn browsing_directory(world: &World) -> Option<PathBuf> {
    browsed_directory(world)
        .or_else(|| open_file_directory(world))
        .or_else(|| project_directory(world))
}

/// Where a dialog opened for `purpose` should start. The Project window
/// wins for the purposes that read the project's files; the folder the purpose
/// was last used in comes first for the ones that read files from outside
/// the project, and otherwise fills in when the browser has nothing.
pub fn start_directory(world: &World, purpose: DialogPurpose) -> Option<PathBuf> {
    let remembered = remembered_directory(world, purpose);
    if purpose.looks_outside_the_project() {
        return remembered.or_else(|| browsing_directory(world));
    }
    browsed_directory(world)
        .or(remembered)
        .or_else(|| browsing_directory(world))
}

fn remembered_directory(world: &World, purpose: DialogPurpose) -> Option<PathBuf> {
    world
        .get_resource::<DialogMemory>()
        .and_then(|memory| memory.folder(purpose))
        .and_then(existing_directory)
}

fn primary_window_handle(world: &mut World) -> Option<RawHandleWrapper> {
    world
        .query_filtered::<&RawHandleWrapper, With<PrimaryWindow>>()
        .single(world)
        .ok()
        .cloned()
}

/// Parent `dialog` to the editor window and start it in `directory`.
pub fn dialog_at(directory: Option<PathBuf>, parent: Option<&RawHandleWrapper>) -> AsyncFileDialog {
    let mut dialog = AsyncFileDialog::new();
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    if let Some(parent) = parent {
        // SAFETY: called on the main thread, where the primary window's
        // handle is live; it is only read to parent the dialog.
        let handle = unsafe { parent.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    dialog
}

/// An open dialog for `purpose`, started where the user is browsing.
pub fn file_dialog(world: &mut World, purpose: DialogPurpose) -> AsyncFileDialog {
    let directory = start_directory(world, purpose);
    let parent = primary_window_handle(world);
    dialog_at(directory, parent.as_ref())
}

/// A save dialog for `purpose`, started where the user is browsing and
/// prefilled with `suggested_name`.
pub fn save_dialog(
    world: &mut World,
    purpose: DialogPurpose,
    suggested_name: &str,
) -> AsyncFileDialog {
    file_dialog(world, purpose).set_file_name(suggested_name)
}

/// A dialog started at `directory`, for the pickers that already hold
/// the folder they are re-choosing.
pub fn dialog_starting_at(world: &mut World, directory: Option<PathBuf>) -> AsyncFileDialog {
    let directory = directory.as_deref().and_then(existing_directory);
    let parent = primary_window_handle(world);
    dialog_at(directory, parent.as_ref())
}

/// The folder to record for a dialog that returned `picked`: the pick
/// itself when it is a folder, its parent when it is a file.
fn picked_directory(picked: &Path) -> Option<PathBuf> {
    if picked.is_dir() {
        return Some(picked.to_path_buf());
    }
    picked.parent().map(Path::to_path_buf)
}

/// Record where a dialog for `purpose` ended up, so the next one starts
/// there. Persisted with the project when one is open.
pub fn remember_pick(world: &mut World, purpose: DialogPurpose, picked: &Path) {
    let Some(directory) = picked_directory(picked) else {
        return;
    };
    if let Some(mut memory) = world.get_resource_mut::<DialogMemory>() {
        memory.remember(purpose, &directory);
    }
    let Some(folders) = world
        .get_resource::<DialogMemory>()
        .map(|memory| memory.folders().clone())
    else {
        return;
    };
    let Some(mut project) = world.get_resource_mut::<ProjectRoot>() else {
        return;
    };
    if project.config.dialog_directories == folders {
        return;
    }
    let root = project.root.clone();
    let mut config = project.config.clone();
    config.dialog_directories = folders;
    match crate::project::save_project_config(&root, &config) {
        Ok(()) => project.config = config,
        Err(err) => warn!(
            "Failed to persist the dialog folder for {}: {err}",
            purpose.key()
        ),
    }
}

/// Where the launcher's project browser starts: beside the most recent
/// project, else the default projects folder, else home.
pub fn launcher_project_directory() -> Option<PathBuf> {
    let recent = crate::project::read_recent_projects();
    let beside_recent = recent
        .projects
        .first()
        .and_then(|entry| entry.path.parent().map(Path::to_path_buf))
        .and_then(|parent| existing_directory(&parent));
    beside_recent
        .or_else(|| {
            dirs::home_dir()
                .map(|home| home.join("Projects"))
                .as_deref()
                .and_then(existing_directory)
        })
        .or_else(|| dirs::home_dir().as_deref().and_then(existing_directory))
}
