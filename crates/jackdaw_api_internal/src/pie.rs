//! [`PlayState`]: the lifecycle of the game process the editor launches
//! for Play.
//!
//! Play runs the game as a separate process started by the editor. The two
//! talk over an IPC channel (control messages from the editor, entity and
//! component state back from the game), and the editor's `PlayState`
//! reflects that process: `Stopped` when none is running, `Playing` once
//! the game is up, `Paused` while the editor has paused it.
//!
//! Game code that runs inside the editor can gate its systems on it:
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

use bevy::prelude::*;

/// Lifecycle of the game process launched by the editor's Play button.
///
/// `Stopped` means no game process is running, which is also the editor's
/// authoring state. `Playing` means a game process is running. `Paused`
/// means the editor has paused that process without discarding its world.
#[derive(States, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub enum PlayState {
    #[default]
    Stopped,
    Playing,
    Paused,
}
