//! The three components a prefab and its instances are spelled with.

use bevy::prelude::*;
use std::path::PathBuf;

/// Marker on the root entity of a prefab file. Loaders treat
/// `Prefab`-marked roots as reusable instance sources, not as scene roots.
///
/// The `#[type_path]` attribute pins the reflect path independently of the
/// declaring crate. Saved `.bsn` scenes spell that path, so it is part of
/// the file format.
#[derive(Component, Reflect, Default, Clone, Debug)]
#[type_path = "jackdaw::prefab::components"]
#[reflect(Component, Default)]
pub struct Prefab;

/// Stable per-prefab id assigned to every entity inside a prefab
/// file. Override entries in scenes reference inherited entities by
/// this id.
#[derive(Component, Reflect, Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[type_path = "jackdaw::prefab::components"]
#[reflect(Component)]
pub struct PrefabEntityId(
    /// The id, unique within one prefab file.
    pub u32,
);

/// Marker on the root entity of a prefab instance inside a scene.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[type_path = "jackdaw::prefab::components"]
#[reflect(Component, Default)]
pub struct IsA {
    /// The prefab file this instance inherits from, relative to the
    /// directory of the document that names it.
    pub source: PathBuf,
    /// [`PrefabEntityId`] values the instance has marked as removed from
    /// the inherited subtree.
    pub deleted: Vec<u32>,
}

/// Reflect type path of [`Prefab`], as scene documents spell it.
pub const PREFAB_TYPE: &str = "jackdaw::prefab::components::Prefab";
/// Reflect type path of [`PrefabEntityId`], as scene documents spell it.
pub const PREFAB_ENTITY_ID_TYPE: &str = "jackdaw::prefab::components::PrefabEntityId";
/// Reflect type path of [`IsA`], as scene documents spell it.
pub const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::TypePath;

    /// Saved `.bsn` scenes name these components by their full reflect path,
    /// so the `#[type_path]` attributes are part of the file format. Dropping
    /// one orphans the prefabs in every existing scene.
    #[test]
    fn the_components_keep_the_names_saved_scenes_spell() {
        assert_eq!(<Prefab as TypePath>::type_path(), PREFAB_TYPE);
        assert_eq!(
            <PrefabEntityId as TypePath>::type_path(),
            PREFAB_ENTITY_ID_TYPE
        );
        assert_eq!(<IsA as TypePath>::type_path(), ISA_TYPE);
    }
}
