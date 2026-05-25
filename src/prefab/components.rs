use bevy::prelude::*;
use std::path::PathBuf;

/// Marker on the root entity of a prefab file. Loaders treat
/// `Prefab`-marked roots as reusable instance sources, not as scene roots.
#[derive(Component, Reflect, Default, Clone, Debug)]
#[reflect(Component)]
pub struct Prefab;

/// Stable per-prefab id assigned to every entity inside a prefab
/// file. Override entries in scenes reference inherited entities by
/// this id.
#[derive(Component, Reflect, Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[reflect(Component)]
pub struct PrefabEntityId(pub u32);

/// Marker on the root entity of a prefab instance inside a scene.
/// `source` is project-relative. `deleted` lists `PrefabEntityId`
/// values the instance has marked as removed from the inherited
/// subtree.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component)]
pub struct IsA {
    pub source: PathBuf,
    pub deleted: Vec<u32>,
}
