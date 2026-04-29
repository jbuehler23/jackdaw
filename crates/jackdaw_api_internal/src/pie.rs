//! Types shared between jackdaw's PIE runtime and loaded games.
//!
//! Game dylibs import [`PlayState`] to gate their systems on the
//! user's Play/Pause/Stop transport buttons:
//!
//! ```ignore
//! use bevy::prelude::*;
//! use jackdaw_api::prelude::*;
//!
//! pub struct GamePlugin;
//! impl Plugin for GamePlugin {
//!     fn build(&self, app: &mut App) {
//!         app.add_systems(
//!             Update,
//!             spin_cube.run_if(in_state(PlayState::Playing)),
//!         );
//!     }
//! }
//! ```
//!
//! Jackdaw's editor crate owns the rest of the PIE infrastructure
//! (the `PiePlugin`, the scene-snapshot resource, the toolbar
//! buttons); those stay editor-private because they depend on the
//! editor's scene representation.

use bevy::prelude::*;

/// Three-state transport controlled by the editor's Play / Pause /
/// Stop buttons.
///
/// Games attach `.run_if(in_state(PlayState::Playing))` to their
/// update systems and `OnEnter(PlayState::Playing)` to their
/// start-of-play systems (entity spawning, initial state setup).
/// `Stopped` is authoring mode; the game is loaded but dormant.
/// `Paused` freezes game systems without discarding world state.
#[derive(States, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub enum PlayState {
    #[default]
    Stopped,
    Playing,
    Paused,
}
