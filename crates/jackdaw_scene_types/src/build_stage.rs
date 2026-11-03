use bevy::prelude::*;

/// Marks a group whose children a script owns and rebuilds as a whole. The
/// stage is named by the group's [`Name`]; running a batch for that stage
/// again replaces everything under the group and leaves the group itself, its
/// transform and its place in the hierarchy, alone.
#[derive(Component, Reflect, Default, Clone, Debug, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct BuildStage {
    /// Names of the stages this one builds on. Each must exist when the stage
    /// is built; nothing is rebuilt because of them.
    pub after: Vec<String>,
}

/// Reflect type path for [`BuildStage`].
pub const BUILD_STAGE_TYPE_PATH: &str = "jackdaw_scene_types::build_stage::BuildStage";

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::TypePath;

    #[test]
    fn the_type_path_constant_names_the_type() {
        assert_eq!(BuildStage::type_path(), BUILD_STAGE_TYPE_PATH);
    }
}
