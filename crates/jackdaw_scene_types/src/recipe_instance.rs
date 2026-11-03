use std::collections::BTreeMap;

use bevy::prelude::*;

/// Marks the root a recipe built. Its children are the recipe's to rebuild:
/// running it again with new inputs replaces everything under the root and
/// leaves the root itself, its name, transform, tags and place, alone.
#[derive(Component, Reflect, Default, Clone, Debug, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct RecipeInstance {
    /// The recipe it was built from: a path under the assets directory, or
    /// `starter:<name>` for one the editor ships.
    pub recipe: String,
    /// Every input the root was built with, each written as text.
    pub inputs: BTreeMap<String, String>,
    /// The seed its `rand` calls drew from.
    pub seed: u64,
}

pub const RECIPE_INSTANCE_TYPE_PATH: &str = "jackdaw_scene_types::recipe_instance::RecipeInstance";

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::TypePath;

    #[test]
    fn the_type_path_constant_names_the_type() {
        assert_eq!(RecipeInstance::type_path(), RECIPE_INSTANCE_TYPE_PATH);
    }
}
