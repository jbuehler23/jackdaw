//! The navmesh baked beside a scene, loaded with that scene.
//!
//! A bake is saved as `<scene>.jdnav` next to `<scene>.bsn`, and arrives as a
//! [`JackdawNavmesh`] component on the scene root, so two scenes loaded at
//! once do not share one. Its queries, [`NavmeshArtifact::contains_point`] and
//! [`NavmeshArtifact::height_at`], come from `jackdaw_terrain`, so a server
//! can validate moves against the baked artifact without the `terrain`
//! feature's mesher and shader.
//!
//! A missing file is the unbaked scene and is not reported. A file that does
//! not decode is reported and left on disk.
//!
//! A scene reloads onto the root it already spawned from, so the component is
//! removed before the file is read again: a reload that finds the bake deleted
//! or broken leaves the root bare rather than answering moves from ground the
//! world no longer has.

use std::path::Path;

use bevy::prelude::*;
use jackdaw_terrain::navmesh::{self, NavmeshArtifact};

/// The navmesh baked for a scene, on that scene's root entity.
///
/// Dereferences to the artifact, so a game asks it directly:
///
/// ```ignore
/// fn can_step_to(point: Vec2, nav: Single<&JackdawNavmesh>) -> bool {
///     nav.contains_point(point)
/// }
/// ```
#[derive(Component, Debug, Deref)]
pub struct JackdawNavmesh(pub NavmeshArtifact);

/// Read the navmesh beside the scene that just spawned, if it has one.
///
/// A reload spawns onto the same root, so any existing component is dropped
/// first: every path that ends without an artifact is a scene with no
/// navmesh, and keeping the previous one would answer moves from ground that
/// is gone.
pub(crate) fn attach_navmesh(
    world: &mut World,
    root_entity: Entity,
    parent_path: &Path,
    stem: Option<&str>,
) {
    world.entity_mut(root_entity).remove::<JackdawNavmesh>();

    // A scene built from text in memory has no file name to look beside.
    let Some(stem) = stem.filter(|stem| !stem.is_empty()) else {
        return;
    };
    let Some(assets) = crate::assets_root(world) else {
        return;
    };
    let path = assets
        .join(parent_path)
        .join(format!("{stem}.{}", navmesh::EXTENSION));
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    match navmesh::decode(&bytes) {
        Ok(artifact) => {
            world
                .entity_mut(root_entity)
                .insert(JackdawNavmesh(artifact));
        }
        Err(err) => error!(
            "navmesh {} is unreadable ({err}); this scene loads without one",
            path.display()
        ),
    }
}
