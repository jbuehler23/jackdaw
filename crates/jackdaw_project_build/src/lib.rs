//! Editor-independent project build pipeline: from an open Bevy project
//! to a loadable dylib plus its extracted type schema.
//!
//! Depended on by the Jackdaw editor (for its in-process builds) and the
//! `jackdaw` CLI (`jackdaw build`). Kept bevy-light so the CLI stays
//! small and the pipeline is reusable outside the editor (for example a
//! Bevy CLI subcommand): only the `reflect` feature, used by the
//! throwaway extractor process, pulls bevy.

pub mod cargo_meta;
pub mod detect;
pub mod linkage;
pub mod plan;
pub mod schema;
pub mod sdk_paths;
pub mod shim;

mod build;

pub use build::{ProjectBuild, ProjectBuildError, build_project_dylib, shim_spec_for_project};
