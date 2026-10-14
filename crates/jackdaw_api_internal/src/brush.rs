//! Brushes: when the editor moves and rebuilds them, and the components a new
//! brush starts with.
//!
//! A brush's shape is its [`Brush`](jackdaw_scene_types::Brush) component,
//! with any `ModifierStack` applied by
//! [`evaluate_brush_geometry`](jackdaw_scene_types::evaluate_brush_geometry),
//! which is what a running game sees too.

use bevy::prelude::*;

/// The systems that keep brush geometry current, in the order they run in
/// `Update`. Order an extension's own systems against them to see a brush
/// after it moved or was rebuilt.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrushSystems {
    /// Moves each changed brush's origin to the centre of its geometry, which
    /// rewrites its `Transform`.
    Recenter,
    /// Rebuilds the brush's meshes from its faces.
    Rebuild,
}

/// A component every brush the editor creates starts with, in the scene
/// document as if the person had added it.
///
/// Spawned by [`crate::ExtensionContext::register_brush_default`] as a child
/// of the extension, so it stops applying when the extension unloads.
#[derive(Component)]
pub struct BrushDefault(pub Box<dyn Reflect>);
