//! Re-export of the editor-independent build pipeline
//! ([`jackdaw_project_build`]) at the editor's historical module path, so
//! existing `crate::project_build::*` call sites keep working after the
//! pipeline moved to its own bevy-light crate.

pub use jackdaw_project_build::{
    BuildEvent, ProjectBinaryBuild, ProjectBuild, ProjectBuildError, build_project_binary,
    build_project_dylib, cargo_meta, detach_from_host_build, linkage, plan, prepare_game_command,
    project_manifest, shim, shim_spec_for_project,
};
