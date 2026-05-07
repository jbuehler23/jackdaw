//! Editor display metadata as Bevy reflect custom attributes.
//! The picker reads them via
//! `type_info.custom_attributes().get::<T>()` and falls back to
//! the type's reflected doc comment (workspace bevy
//! `reflect_documentation` feature).
//!
//! ```ignore
//! use bevy::prelude::*;
//! use jackdaw_jsn::EditorCategory;
//!
//! /// Spawns the player entity.
//! #[derive(Component, Reflect, Default)]
//! #[reflect(Component, Default, @EditorCategory("Actor"))]
//! pub struct PlayerSpawn;
//! ```
//!
//! `jackdaw_runtime` and `jackdaw` re-export both newtypes
//! through their preludes.

use bevy::prelude::*;
use std::borrow::Cow;

/// Picker grouping for a component. Attach via
/// `#[reflect(@EditorCategory("Your Group"))]`.
#[derive(Reflect, Clone, Debug, PartialEq, Eq)]
pub struct EditorCategory(pub Cow<'static, str>);

impl EditorCategory {
    pub const fn new(name: &'static str) -> Self {
        EditorCategory(Cow::Borrowed(name))
    }
}

impl From<&'static str> for EditorCategory {
    fn from(value: &'static str) -> Self {
        EditorCategory(Cow::Borrowed(value))
    }
}

impl From<String> for EditorCategory {
    fn from(value: String) -> Self {
        EditorCategory(Cow::Owned(value))
    }
}

/// Picker tooltip override. Falls back to the reflected doc
/// comment when absent.
#[derive(Reflect, Clone, Debug, PartialEq, Eq)]
pub struct EditorDescription(pub Cow<'static, str>);

impl EditorDescription {
    pub const fn new(text: &'static str) -> Self {
        EditorDescription(Cow::Borrowed(text))
    }
}

impl From<&'static str> for EditorDescription {
    fn from(value: &'static str) -> Self {
        EditorDescription(Cow::Borrowed(value))
    }
}

impl From<String> for EditorDescription {
    fn from(value: String) -> Self {
        EditorDescription(Cow::Owned(value))
    }
}

/// Hides things from editor-facing surfaces. Used in two ways:
///
/// - As a Bevy `Component` on an entity: hides that entity from
///   the hierarchy panel.
/// - As a `#[reflect(@EditorHidden)]` attribute on a Component
///   type: hides the type from the Add Component picker. Used by
///   jackdaw's own scene types (brushes, navmesh, terrain, node
///   graph, animation graph) and available to extension and game
///   crates with helper Components.
///
/// ```ignore
/// #[derive(Component, Reflect, Default)]
/// #[reflect(Component, Default, @EditorHidden)]
/// pub struct InternalRig;
/// ```
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct EditorHidden;

/// Marker for entities that exist as editor-time visual
/// indicators. The save filter skips this entity (and its
/// subtree) so the helper never lands in `.jsn`; the editor
/// viewport still renders it.
///
/// Pattern: under your scene-authored marker (e.g. `PlayerSpawn`),
/// spawn a child carrying `SkipSerialization` plus a `Mesh3d` +
/// `MeshMaterial3d`. The editor renders the helper; the saved
/// scene never includes it.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct SkipSerialization;
