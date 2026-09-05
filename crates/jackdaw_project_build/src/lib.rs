//! Editor-independent project build pipeline: from an open Bevy project
//! to a runnable artifact plus its extracted type schema.
//!
//! Depended on by the Jackdaw editor (for its in-process builds) and the
//! `jackdaw` CLI (`jackdaw build`). Kept bevy-light so the CLI stays
//! small and the pipeline is reusable outside the editor (for example a
//! Bevy CLI subcommand): only the `reflect` feature pulls bevy, and only
//! for the side that owns the reflected types.
//!
//! # Two build paths
//!
//! [`build_project_binary`] is what games use: the project is a normal
//! Bevy app, and `cargo build` in its root is the whole build. The game
//! keeps its own bevy, its own features, and its own toolchain. Schema
//! extraction runs the built binary with [`jackdaw_schema::SCHEMA_FLAG`].
//!
//! [`build_project_dylib`] is the SDK path for extensions, which must
//! share the editor's Bevy types and therefore compile against jackdaw's
//! prebuilt SDK so the artifact can be dlopened in-process.

pub mod bootstrap;
pub mod cargo_meta;
pub mod detect;
pub mod linkage;
pub mod plan;
pub mod project_manifest;
pub mod sdk_paths;
pub mod shim;

mod binary;
mod build;

pub use binary::{
    ProjectBinaryBuild, build_project_binary, detach_from_host_build, prepare_game_command,
};
pub use build::{
    BuildEvent, ProjectBuild, ProjectBuildError, build_project_dylib, last_built_dylib, sdk_remedy,
    shim_spec_for_project,
};

// The embedded SDK-builder recipe (relative path + bytes), assembled by
// `build.rs`. Empty when this crate was compiled outside the workspace.
include!(concat!(env!("OUT_DIR"), "/recipe_data.rs"));

/// A stable content hash of the embedded recipe. Part of the cache stamp
/// so a jackdaw upgrade that changes the recipe rebuilds the SDK.
pub const RECIPE_HASH: &str = env!("RECIPE_HASH");

/// This jackdaw build's version (the workspace version), e.g. `0.19.0`.
/// The single source for `--version` output and the scene version stamp.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The Bevy minor this build targets. The workspace version is anchored
/// to Bevy's minor (`0.19.x` targets Bevy `0.19`), so it derives from the
/// crate version rather than a separate source. Shown in `--version` and
/// stamped into saved scenes: a future jackdaw reads it to migrate type
/// paths across a Bevy rename (renames land on minor bumps).
pub const BEVY_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR")
);
