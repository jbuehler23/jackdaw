//! The open scene as the person authored it, as opposed to what the editor
//! and plugins derive at runtime.
//!
//! An entity in the editor carries both: the components saved with the
//! scene, and companions added by `#[require]`, observers and plugins that
//! are rebuilt on load. Tools that remove what a person added read the
//! authored set here. Writes go through operators such as `component.add`,
//! `component.remove` and `component.set`, so they reach undo and the saved
//! file.

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
