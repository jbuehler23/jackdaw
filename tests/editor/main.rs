//! The editor-app integration tests. Each theme directory under `tests/` is
//! a module of this one binary, so the editor links once for all of them.
//! The keymap tests stay a binary of their own: they point the config
//! directory variable somewhere else for the whole process.

#[path = "../util/mod.rs"]
mod util;

#[path = "../controls/mod.rs"]
mod controls;

#[path = "../guards/mod.rs"]
mod guards;

#[path = "../inspector/mod.rs"]
mod inspector;

#[path = "../operators/mod.rs"]
mod operators;

#[path = "../outliner/mod.rs"]
mod outliner;

#[path = "../prefabs/mod.rs"]
mod prefabs;

#[path = "../scenes/mod.rs"]
mod scenes;

#[path = "../stage/mod.rs"]
mod stage;

#[path = "../viewport/mod.rs"]
mod viewport;
