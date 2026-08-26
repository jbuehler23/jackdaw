//! Patches `bevy_dev_tools::infinite_grid`'s shader in place, without
//! forking the crate.
//!
//! The stock grid recomputes depth analytically per pixel; at a low,
//! oblique, zoomed-out view that z-fights opaque geometry sharing the
//! grid's plane. No opaque-material `depth_bias` fixes it, since the
//! amplification has no fixed bound. The correction lives in the grid's
//! own depth output: see `editor_grid_depth_patch.wgsl`'s
//! `GRID_DEPTH_YIELD_WORLD` and the reprojection in `fragment`, otherwise
//! byte-identical to upstream.
//!
//! `embedded_asset!` (upstream, in `infinite_grid.rs`) registers its WGSL
//! source into [`EmbeddedAssetRegistry`] under a computed path; a later
//! insert at the same path replaces the earlier one. This plugin computes
//! that same path and overwrites it with the patched source. Must run
//! after [`bevy::dev_tools::infinite_grid::InfiniteGridPlugin`]'s `build()`.

use std::path::{Path, PathBuf};

use bevy::app::prelude::*;
use bevy::asset::io::embedded::{_embedded_asset_path, EmbeddedAssetRegistry};
use bevy::log::error;

pub(crate) fn plugin(app: &mut App) {
    let registry = app.world().resource::<EmbeddedAssetRegistry>();
    // Reproduces the path `embedded_asset!(app, "infinite_grid.wgsl")`
    // computes inside `bevy_dev_tools::infinite_grid`; depends only on
    // the crate/file/asset names below, not the crate's checkout location.
    let asset_path = _embedded_asset_path(
        "bevy_dev_tools",
        Path::new("src"),
        Path::new("src/infinite_grid.rs"),
        Path::new("infinite_grid.wgsl"),
    );

    // If bevy_dev_tools renamed or moved infinite_grid.wgsl, or
    // InfiniteGridPlugin::build() has not run yet, nothing is registered at
    // `asset_path` and the insert below becomes a fresh registration rather
    // than an overwrite, leaving the z-fight correction unapplied with no
    // other signal. `remove_asset` doubles as the presence check: it pops
    // whatever was there, so the insert below is an overwrite either way.
    let had_prior_entry = registry.remove_asset(&asset_path).is_some();
    if !had_prior_entry {
        error!(
            "editor_grid_depth_patch: no embedded asset registered at {asset_path:?} \
             before patching; the grid depth-yield fix is not applied -- check that \
             InfiniteGridPlugin runs before this plugin and that bevy_dev_tools still \
             registers infinite_grid.wgsl at this path"
        );
    }
    debug_assert!(
        had_prior_entry,
        "editor_grid_depth_patch: expected an embedded asset at {asset_path:?} before overwriting it"
    );

    registry.insert_asset(
        PathBuf::from(file!())
            .parent()
            .unwrap()
            .join("editor_grid_depth_patch.wgsl"),
        &asset_path,
        include_bytes!("editor_grid_depth_patch.wgsl").as_slice(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overwrites_the_registered_asset_with_the_patched_wgsl() {
        let mut app = App::new();
        app.init_resource::<EmbeddedAssetRegistry>();

        let asset_path = _embedded_asset_path(
            "bevy_dev_tools",
            Path::new("src"),
            Path::new("src/infinite_grid.rs"),
            Path::new("infinite_grid.wgsl"),
        );
        {
            let registry = app.world().resource::<EmbeddedAssetRegistry>();
            registry.insert_asset(
                PathBuf::from("dummy/infinite_grid.rs"),
                &asset_path,
                b"dummy upstream bytes".as_slice(),
            );
        }

        plugin(&mut app);

        let registry = app.world().resource::<EmbeddedAssetRegistry>();
        let data = registry
            .remove_asset(&asset_path)
            .expect("the patch should have left an asset at the collision path");
        assert_eq!(data.value(), include_bytes!("editor_grid_depth_patch.wgsl"));
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        should_panic(expected = "expected an embedded asset")
    )]
    fn missing_prior_entry_trips_the_guard() {
        let mut app = App::new();
        app.init_resource::<EmbeddedAssetRegistry>();

        // No seed insert: simulates InfiniteGridPlugin having moved,
        // renamed, or not yet run. Debug builds panic via debug_assert!;
        // release builds fall through, so assert the patch still applies.
        plugin(&mut app);

        if !cfg!(debug_assertions) {
            let registry = app.world().resource::<EmbeddedAssetRegistry>();
            let asset_path = _embedded_asset_path(
                "bevy_dev_tools",
                Path::new("src"),
                Path::new("src/infinite_grid.rs"),
                Path::new("infinite_grid.wgsl"),
            );
            let data = registry
                .remove_asset(&asset_path)
                .expect("the patch inserts even when nothing was there before");
            assert_eq!(data.value(), include_bytes!("editor_grid_depth_patch.wgsl"));
        }
    }
}
