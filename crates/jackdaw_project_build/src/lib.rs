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
pub mod build_source;
pub mod cargo_meta;
pub mod detect;
pub mod linkage;
pub mod plan;
pub mod project_manifest;
pub mod runtime_wiring;
pub mod sdk_paths;
pub mod shim;

mod binary;
mod build;
#[cfg(test)]
mod registry_recipe;

pub use binary::{
    BuildLoad, ProjectBinaryBuild, background_jobs, build_project_binary,
    build_project_binary_with_load, detach_from_host_build, game_target_dir, prepare_game_command,
};
pub use build::{
    BuildEvent, ProjectBuild, ProjectBuildError, build_project_dylib, last_built_dylib, sdk_remedy,
    shim_spec_for_project,
};

// The embedded SDK-builder recipe (relative path + bytes), assembled by
// `build.rs`. Empty without the `embed-recipe` feature or in another workspace.
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

/// The version requirement a project states for this build's jackdaw crates.
pub fn jackdaw_requirement() -> String {
    jackdaw_requirement_for(VERSION)
}

/// The version requirement a project states for the jackdaw crates of release
/// `version`: the minor line for a stable release, or the exact version for a
/// prerelease, which a minor-line requirement never matches.
pub fn jackdaw_requirement_for(version: &str) -> String {
    let release = version
        .split_once('+')
        .map_or(version, |(release, _)| release);
    if release.contains('-') {
        return format!("={release}");
    }
    let mut parts = release.splitn(3, '.');
    match (parts.next(), parts.next()) {
        (Some(major), Some(minor)) => format!("{major}.{minor}"),
        _ => release.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::jackdaw_requirement_for;

    #[test]
    fn a_stable_release_requests_its_minor_line() {
        assert_eq!(jackdaw_requirement_for("0.19.0"), "0.19");
        assert_eq!(jackdaw_requirement_for("0.19.3"), "0.19");
        assert_eq!(jackdaw_requirement_for("0.19.3+build.5"), "0.19");
    }

    #[test]
    fn a_prerelease_requests_its_exact_version() {
        assert_eq!(jackdaw_requirement_for("0.19.0-rc.1"), "=0.19.0-rc.1");
        assert_eq!(
            jackdaw_requirement_for("0.20.0-alpha.2+build.5"),
            "=0.20.0-alpha.2"
        );
    }
}
