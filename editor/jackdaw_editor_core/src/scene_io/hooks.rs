//! Data a feature keeps beside a scene or prefab file rather than in it,
//! written and read through hooks the feature registers, so a save and a
//! load reach it without the core knowing the feature.

use std::any::Any;
use std::sync::Arc;

use bevy::prelude::*;

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

pub fn forget_project(world: &mut World) {
    for hook in hooks(world) {
        hook.forget_project(world);
    }
}
