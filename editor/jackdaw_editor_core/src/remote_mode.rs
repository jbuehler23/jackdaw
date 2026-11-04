//! Remote debugger state the core reads without the remote editor crate: the
//! marker on entities that mirror a running game, the status-bar spot the
//! connection indicator fills, and the name the remote extension goes by.

use bevy::prelude::*;

/// Catalog name of the remote editor extension.
pub const REMOTE_EDITOR_EXTENSION_ID: &str = "jackdaw.remote_editor";

/// Marks a local entity that mirrors an entity of a connected game, so local
/// editing leaves it alone.
#[derive(Component)]
pub struct RemoteEntityProxy {
    /// The mirrored entity's id in the game.
    pub remote_bits: u64,
}

/// The status-bar node the remote editor fills with its connection indicator.
#[derive(Component, Default)]
pub struct ConnectionIndicatorSlot;
