//! Data a feature keeps beside a scene or prefab file rather than in it,
//! written and read through hooks the feature registers, so a save and a
//! load reach it without the core knowing the feature.

use std::any::Any;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::prelude::*;
use jackdaw_bsn::SceneBsnAst;

use super::SidecarImport;

/// One feature's part in saving, loading and closing documents.
///
/// A save runs [`Self::begin_save`] and [`Self::encode_sidecars`] on the main thread,
/// writes what they encoded on another thread, and runs [`Self::finish_save`] on the
/// main thread once the document is on disk. Frames run and the user goes on editing
/// between the second and the third, so `finish_save` must not take the world to be
/// what `encode_sidecars` saw. What switches or closes a tab waits for a save first,
/// so the saved tab is normally still in front.
pub trait DocumentIoHook: Send + Sync + 'static {
    /// Whether a save to `path`, asked for with the world as it is, must run from
    /// [`Self::begin_save`] to [`Self::finish_save`] with no frame in between.
    fn holds_the_frame(&self, _world: &mut World, _path: &str) -> bool {
        false
    }

    /// Runs ahead of writing the document to `path`, returning what
    /// [`Self::encode_sidecars`] and [`Self::finish_save`] need of it.
    fn begin_save(&self, world: &mut World, path: &str) -> Box<dyn Any + Send>;

    /// The feature's files beside the document at `path` and their bytes, encoded
    /// from the moment the document's snapshot was taken. Writes nothing: they
    /// reach the disk once the document has been read back, just ahead of it. An
    /// error fails the save.
    fn encode_sidecars(
        &self,
        world: &mut World,
        path: &str,
        state: &mut (dyn Any + Send),
    ) -> Result<Vec<SidecarWrite>, BevyError>;

    /// Runs once the save is over; `written` says whether every file landed. A
    /// save replaced by a newer one before it began writing ends here with
    /// `written` false.
    fn finish_save(&self, world: &mut World, state: Box<dyn Any + Send>, written: bool);

    /// Reads the feature's files beside the document at `path`.
    fn import_sidecars(&self, world: &mut World, path: &str, mode: SidecarImport);

    /// Drops what the feature holds for the project being closed.
    fn forget_project(&self, _world: &mut World) {}

    /// Settles in `copies`, under [`PlayDocument::overlay_dir`], the data of
    /// `document` with unsaved changes, for a running game. Data with none is left
    /// out, and [`PlayCopies`] removes any copy of it. The document's dirty state,
    /// data paths and files stay as they are.
    fn export_play_sidecars(
        &self,
        _world: &World,
        _document: &PlayDocument,
        _copies: &mut PlayCopies,
    ) -> Result<(), BevyError> {
        Ok(())
    }

    /// The prefabs `document`'s data names beyond its `IsA` references, as paths
    /// under the asset folder, read from the data the editor holds for it.
    fn play_prefabs(&self, _world: &World, _document: &PlayDocument) -> Vec<String> {
        Vec::new()
    }

    /// Drops what the feature holds as unsaved for the document in tab `tab`, whose
    /// edits the author just chose to throw away. The tab is still open.
    fn discard_document(&self, _world: &mut World, _tab: usize) {}

    /// Whether the feature holds anything for the document in tab `tab` that its
    /// files do not.
    fn has_unsaved_edits(&self, _world: &World, _tab: usize) -> bool {
        false
    }

    /// The feature's files beside the document in tab `tab` that something else
    /// wrote after the feature last read or wrote them, while it holds unsaved
    /// edits to them: a save would write over them.
    fn changed_on_disk(&self, _world: &World, _tab: usize) -> Vec<PathBuf> {
        Vec::new()
    }

    /// The author chose to keep what the feature holds for tab `tab` over
    /// `files`, from [`Self::changed_on_disk`], as each was modified when they
    /// were asked. Those versions stop reading as changed and the next save
    /// writes over them; a later write to the same file still reads as changed.
    fn keep_over_changed(&self, _world: &mut World, _tab: usize, _files: &[ChangedFile]) {}
}

/// A file beside a document that changed on disk, as it was modified when
/// the author was asked about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub modified: Option<std::time::SystemTime>,
}

/// The `data_path` of each node of `ast` holding the component `type_path`,
/// each once, in order.
pub fn data_paths_in(ast: &SceneBsnAst, type_path: &str) -> Vec<String> {
    let mut paths: Vec<String> = ast
        .entities_with_component(type_path)
        .into_iter()
        .filter_map(
            |node| match jackdaw_bsn::get_bsn_field(ast, node, type_path, "data_path")? {
                jackdaw_bsn::BsnValue::String(path) if !path.is_empty() => Some(path),
                _ => None,
            },
        )
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// A file beside a document and the bytes it is to hold.
pub type SidecarWrite = (PathBuf, Vec<u8>);

/// Put `writes` on disk, in order, each complete or not at all. `durable` forces
/// each to the disk, which is for off the main thread.
pub fn write_sidecars(writes: &[SidecarWrite], durable: bool) -> Result<(), String> {
    for (path, bytes) in writes {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("cannot make the folder of {}: {err}", path.display()))?;
        }
        let written = if durable {
            jackdaw_api_internal::document::write_durable(path, bytes, || Ok(()))
        } else {
            jackdaw_api_internal::document::write_atomic(path, bytes)
        };
        written.map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    }
    Ok(())
}

/// A document a running game is about to spawn.
pub struct PlayDocument<'a> {
    /// The tab holding it, whose store holds its data. `None` for a prefab no tab
    /// holds, which has no data of its own beyond its files.
    pub tab: Option<usize>,
    /// Whether another document instances it, which then edits its data.
    pub instanced: bool,
    /// Its file, or `None` for a tab that has none.
    pub file: Option<&'a Path>,
    pub ast: &'a SceneBsnAst,
    /// The private folder its copies go in, resolved against as the document's own
    /// folder is.
    pub overlay_dir: &'a Path,
}

impl PlayDocument<'_> {
    /// The `data_path` of each node holding the component `type_path`.
    pub fn data_paths(&self, type_path: &str) -> Vec<String> {
        data_paths_in(self.ast, type_path)
    }

    /// The folder of the document's file.
    pub fn file_dir(&self) -> Option<&Path> {
        self.file.and_then(Path::parent)
    }
}

/// The sidecar copies of the Play session, and what each was settled from, so
/// data that has not changed costs a game's next request nothing.
///
/// Each answer to a game is one walk over a folder of its own: a [`Self::settle`]
/// per copy the scene reaches, after which the copies the walk did not ask for go.
#[derive(Resource, Default)]
pub struct PlayCopies {
    /// The stamp each copy path was last settled from, and whether that left a
    /// copy: data that matches its file leaves none.
    settled: HashMap<PathBuf, (u64, bool)>,
    /// The copies the walk in progress wants.
    wanted: HashSet<PathBuf>,
    /// What each cached prefab last compared to its file as: the cache epoch and
    /// file time it was compared at, and whether it differed.
    pub(crate) prefab_checks: HashMap<PathBuf, (u64, Option<std::time::SystemTime>, bool)>,
}

impl PlayCopies {
    pub(crate) fn begin_walk(&mut self) {
        self.wanted.clear();
    }

    /// Make the copy at `at` current for the data `stamp` stands for. `bytes` runs
    /// only when `stamp` is new, and `None` from it means the data matches its file
    /// and wants no copy. The first document of a walk to name a copy keeps it.
    pub fn settle(
        &mut self,
        at: &Path,
        stamp: u64,
        bytes: impl FnOnce() -> Result<Option<Vec<u8>>, BevyError>,
    ) -> Result<(), BevyError> {
        if self.wanted.contains(at) {
            return Ok(());
        }
        match self.settled.get(at) {
            Some(&(held, false)) if held == stamp => return Ok(()),
            Some(&(held, true)) if held == stamp && at.is_file() => {
                self.wanted.insert(at.to_path_buf());
                return Ok(());
            }
            _ => {}
        }
        let wrote = match bytes()? {
            Some(bytes) => {
                write_play_copy(at, &bytes)?;
                self.wanted.insert(at.to_path_buf());
                true
            }
            None => {
                remove_play_copy(at);
                false
            }
        };
        self.settled.insert(at.to_path_buf(), (stamp, wrote));
        Ok(())
    }

    /// End a walk over `folder`, removing every copy under it the walk did not
    /// want. Returns whether any copy is left.
    pub(crate) fn finish_walk(&mut self, folder: &Path) -> bool {
        let gone: Vec<PathBuf> = self
            .settled
            .iter()
            .filter(|(at, (_, copied))| {
                *copied && at.starts_with(folder) && !self.wanted.contains(*at)
            })
            .map(|(at, _)| at.clone())
            .collect();
        for at in gone {
            self.settled.remove(&at);
            remove_play_copy(&at);
        }
        !self.wanted.is_empty()
    }
}

/// What stands for data at `revision` beside the file at `file`: either moving
/// asks for the comparison with the file again.
pub fn play_copy_stamp(revision: u64, file: Option<&Path>) -> u64 {
    let modified = file
        .and_then(|file| std::fs::metadata(file).ok())
        .and_then(|meta| meta.modified().ok());
    let mut hasher = DefaultHasher::new();
    (revision, modified).hash(&mut hasher);
    hasher.finish()
}

fn remove_play_copy(at: &Path) {
    match std::fs::remove_file(at) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => warn!("PIE: could not remove {}: {err}", at.display()),
    }
}

fn write_play_copy(at: &Path, bytes: &[u8]) -> Result<(), BevyError> {
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to make {}: {err}", parent.display()))?;
    }
    jackdaw_api_internal::document::write_atomic(at, bytes)
        .map_err(|err| format!("failed to write {}: {err}", at.display()))?;
    Ok(())
}

/// The registered [`DocumentIoHook`]s, run in registration order.
#[derive(Resource, Default, Clone)]
pub struct DocumentIoHooks(Vec<Arc<dyn DocumentIoHook>>);

impl DocumentIoHooks {
    pub fn register(&mut self, hook: impl DocumentIoHook) {
        self.0.push(Arc::new(hook));
    }
}

/// What the hooks returned from [`begin_save`], for the rest of that save.
pub struct PendingSave(Vec<(Arc<dyn DocumentIoHook>, Box<dyn Any + Send>)>);

fn hooks(world: &World) -> Vec<Arc<dyn DocumentIoHook>> {
    world
        .get_resource::<DocumentIoHooks>()
        .map(|hooks| hooks.0.clone())
        .unwrap_or_default()
}

pub fn begin_save(world: &mut World, path: &str) -> PendingSave {
    PendingSave(
        hooks(world)
            .into_iter()
            .map(|hook| {
                let state = hook.begin_save(world, path);
                (hook, state)
            })
            .collect(),
    )
}

/// Whether a hook needs a save to `path` to run with no frame in between.
pub fn holds_the_frame(world: &mut World, path: &str) -> bool {
    hooks(world)
        .iter()
        .any(|hook| hook.holds_the_frame(world, path))
}

pub fn encode_sidecars(
    world: &mut World,
    path: &str,
    pending: &mut PendingSave,
) -> Result<Vec<SidecarWrite>, BevyError> {
    let mut writes = Vec::new();
    for (hook, state) in &mut pending.0 {
        writes.extend(hook.encode_sidecars(world, path, state.as_mut())?);
    }
    Ok(writes)
}

pub fn finish_save(world: &mut World, pending: PendingSave, written: bool) {
    for (hook, state) in pending.0 {
        hook.finish_save(world, state, written);
    }
}

pub fn import_sidecars(world: &mut World, path: &str, mode: SidecarImport) {
    for hook in hooks(world) {
        hook.import_sidecars(world, path, mode);
    }
}

/// Run every hook's [`DocumentIoHook::export_play_sidecars`]. Every hook runs, and
/// the first error is the one returned.
pub fn export_play_sidecars(
    world: &World,
    document: &PlayDocument,
    copies: &mut PlayCopies,
) -> Result<(), BevyError> {
    let mut first_error = None;
    for hook in hooks(world) {
        if let Err(err) = hook.export_play_sidecars(world, document, copies) {
            first_error = first_error.or(Some(err));
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Every hook's [`DocumentIoHook::play_prefabs`] for `document`.
pub fn play_prefabs(world: &World, document: &PlayDocument) -> Vec<String> {
    hooks(world)
        .iter()
        .flat_map(|hook| hook.play_prefabs(world, document))
        .collect()
}

/// Run every hook's [`DocumentIoHook::discard_document`].
pub fn discard_document(world: &mut World, tab: usize) {
    for hook in hooks(world) {
        hook.discard_document(world, tab);
    }
}

/// Whether any hook holds unsaved edits for the document in tab `tab`.
pub fn has_unsaved_edits(world: &World, tab: usize) -> bool {
    hooks(world)
        .iter()
        .any(|hook| hook.has_unsaved_edits(world, tab))
}

/// Every hook's [`DocumentIoHook::changed_on_disk`] for tab `tab`.
pub fn changed_on_disk(world: &World, tab: usize) -> Vec<PathBuf> {
    hooks(world)
        .iter()
        .flat_map(|hook| hook.changed_on_disk(world, tab))
        .collect()
}

/// Run every hook's [`DocumentIoHook::keep_over_changed`].
pub fn keep_over_changed(world: &mut World, tab: usize, files: &[ChangedFile]) {
    for hook in hooks(world) {
        hook.keep_over_changed(world, tab, files);
    }
}

pub fn forget_project(world: &mut World) {
    for hook in hooks(world) {
        hook.forget_project(world);
    }
}

/// Drag-state resources a feature parks scene entity ids in, put back to rest
/// when the scene those ids name goes away.
#[derive(Resource, Default, Clone)]
pub struct DragResets(Vec<fn(&mut World)>);

impl DragResets {
    pub fn register<R: Resource<Mutability = bevy::ecs::component::Mutable> + Default>(&mut self) {
        self.0.push(|world| {
            if let Some(mut drag) = world.get_resource_mut::<R>() {
                *drag = R::default();
            }
        });
    }
}

pub fn forget_drags(world: &mut World) {
    let resets = world
        .get_resource::<DragResets>()
        .cloned()
        .unwrap_or_default();
    for reset in resets.0 {
        reset(world);
    }
}
