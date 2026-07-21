//! Native remote-game debugger: read-only introspection panels that inspect a
//! running Bevy game over BRP, built on the existing `super` connection client.
//!
//! Phase 1 foundation. Later phases add the remaining views onto the pieces
//! established here (the sample buffer, the poll helper, the panel conventions).

pub mod queries;
pub mod sparkline;
