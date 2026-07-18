//! Editor-independent project build pipeline: from an open Bevy project
//! to a loadable dylib plus its extracted type schema.
//!
//! Depended on by the Jackdaw editor (for its in-process builds) and the
//! `jackdaw` CLI (`jackdaw build`). Kept bevy-light so the CLI stays
//! small and the pipeline is reusable outside the editor (for example a
//! Bevy CLI subcommand): only the `reflect` feature, used by the
//! throwaway extractor process, pulls bevy.

pub mod bootstrap;
pub mod cargo_meta;
pub mod detect;
pub mod linkage;
pub mod plan;
pub mod schema;
pub mod sdk_paths;
pub mod shim;

mod build;

pub use build::{ProjectBuild, ProjectBuildError, build_project_dylib, shim_spec_for_project};

// The embedded SDK-builder recipe (relative path + bytes), assembled by
// `build.rs`. Empty when this crate was compiled outside the workspace.
include!(concat!(env!("OUT_DIR"), "/recipe_data.rs"));

/// A stable content hash of the embedded recipe. Part of the cache stamp
/// so a jackdaw upgrade that changes the recipe rebuilds the SDK.
pub const RECIPE_HASH: &str = env!("RECIPE_HASH");
