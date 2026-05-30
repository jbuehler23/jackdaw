//! `ActiveTool` resource: which top-level tool the user has active.
//!
//! Drives gizmo rendering and the toolbar active-state highlight.
//! `Select` hides any manipulator; the other variants render their
//! respective gizmo when something is selected.

use bevy::prelude::*;

#[derive(Resource, Default, PartialEq, Eq, Clone, Copy, Debug)]
pub enum ActiveTool {
    #[default]
    Select,
    Translate,
    Rotate,
    Scale,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_select() {
        assert_eq!(ActiveTool::default(), ActiveTool::Select);
    }
}
