//! Data a feature keeps beside a scene or prefab file rather than in it,
//! written and read through hooks the feature registers, so a save and a
//! load reach it without the core knowing the feature.

use std::any::Any;
use std::path::Path;
use std::sync::Arc;

use bevy::prelude::*;
use jackdaw_bsn::SceneBsnAst;

use super::SidecarImport;

/// One feature's part in saving, loading and closing documents.
pub trait DocumentIoHook: Send + Sync + 'static {
    /// Runs ahead of writing the document to `path`, returning what
    /// [`Self::export_sidecars`] and [`Self::finish_save`] need of it.
    fn begin_save(&self, world: &mut World, path: &str) -> Box<dyn Any + Send>;

    /// Writes the feature's files beside the document at `path`, before the
    /// document itself. An error fails the save.
    fn export_sidecars(
        &self,
        world: &mut World,
        path: &str,
        state: &(dyn Any + Send),
    ) -> Result<(), BevyError>;

    /// Runs once the save is over; `written` says whether every file landed.
    fn finish_save(&self, world: &mut World, state: Box<dyn Any + Send>, written: bool);

    /// Reads the feature's files beside the document at `path`.
    fn import_sidecars(&self, world: &mut World, path: &str, mode: SidecarImport);

    /// Drops what the feature holds for the project being closed.
    fn forget_project(&self, _world: &mut World) {}

    /// Copies the data of `document` that differs from what its files hold into
    /// [`PlayDocument::overlay_dir`] for a running game, and removes the copy of
    /// data that no longer differs. Returns whether any copy is there. The
    /// document's dirty state, data paths and files stay as they are.
    fn export_play_sidecars(
        &self,
        _world: &World,
        _document: &PlayDocument,
    ) -> Result<bool, BevyError> {
        Ok(false)
    }
}

/// A document a running game is about to spawn.
pub struct PlayDocument<'a> {
    /// The tab holding it, whose store holds its data. `None` for a prefab no tab
    /// holds, which has no data of its own beyond its files.
    pub tab: Option<usize>,
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
        let mut paths: Vec<String> = self
            .ast
            .entities_with_component(type_path)
            .into_iter()
            .filter_map(|node| {
                match jackdaw_bsn::get_bsn_field(self.ast, node, type_path, "data_path")? {
                    jackdaw_bsn::BsnValue::String(path) if !path.is_empty() => Some(path),
                    _ => None,
                }
            })
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    /// The folder of the document's file.
    pub fn file_dir(&self) -> Option<&Path> {
        self.file.and_then(Path::parent)
    }
}

/// Write `bytes` as the copy at `at`, or remove the copy for `None`. Returns
/// whether a copy is there now.
pub fn put_play_copy(at: &Path, bytes: Option<&[u8]>) -> Result<bool, BevyError> {
    let Some(bytes) = bytes else {
        return match std::fs::remove_file(at) {
            Ok(()) => Ok(false),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(format!("failed to remove {}: {err}", at.display()).into()),
        };
    };
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to make {}: {err}", parent.display()))?;
    }
    jackdaw_api_internal::document::write_atomic(at, bytes)
        .map_err(|err| format!("failed to write {}: {err}", at.display()))?;
    Ok(true)
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

pub fn export_sidecars(
    world: &mut World,
    path: &str,
    pending: &PendingSave,
) -> Result<(), BevyError> {
    for (hook, state) in &pending.0 {
        hook.export_sidecars(world, path, state.as_ref())?;
    }
    Ok(())
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

/// Run every hook's [`DocumentIoHook::export_play_sidecars`], returning whether
/// any copy is there. Every hook runs, and the first error is the one returned.
pub fn export_play_sidecars(world: &World, document: &PlayDocument) -> Result<bool, BevyError> {
    let mut copied = false;
    let mut first_error = None;
    for hook in hooks(world) {
        match hook.export_play_sidecars(world, document) {
            Ok(wrote) => copied |= wrote,
            Err(err) => first_error = first_error.or(Some(err)),
        }
    }
    first_error.map_or(Ok(copied), Err)
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
