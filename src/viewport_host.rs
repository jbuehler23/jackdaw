//! The viewport panel's mode: whether it shows the 3D world or the 2D canvas.
//!
//! One panel authors both, so the mode is the panel's own state rather than a
//! second panel. This module owns the mode itself, what a scene kind asks for,
//! and the two resources that carry a mode request across a frame boundary.

use bevy::prelude::*;

use crate::scenes::operators::SceneKind;

/// What a viewport panel is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewportMode {
    /// The 3D world: perspective camera, grid, world-space tools.
    #[default]
    ThreeD,
    /// The 2D canvas: an orthographic stage at the authored reference size.
    TwoD,
}

impl ViewportMode {
    /// The mode a scene of this kind is authored in. A 2D world scene and a
    /// UI screen are both drawn flat, so both open on the canvas.
    pub fn for_scene_kind(kind: SceneKind) -> Self {
        match kind {
            SceneKind::ThreeD => Self::ThreeD,
            SceneKind::TwoD | SceneKind::Ui => Self::TwoD,
        }
    }

    /// The mode a `mode` operator parameter names, or `None` when it names
    /// neither. Case and surrounding space are ignored, as they are for the
    /// other viewport parameters a scripted run writes by hand.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "3d" => Some(Self::ThreeD),
            "2d" => Some(Self::TwoD),
            _ => None,
        }
    }
}

/// The mode the active scene tab wants its viewport panels in.
///
/// Panel-independent on purpose: two panels open at once share one mode, so a
/// tab restored from its view state puts every panel in the mode the user last
/// chose rather than trying to remember which panel the choice was made in.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportModeIntent {
    pub mode: ViewportMode,
    /// Whether the user picked this mode, as opposed to it following from the
    /// scene's kind. A chosen mode is stored in the tab's view state and
    /// restored on the next activation; an unchosen one is recomputed.
    pub chosen: bool,
}

/// A mode request made while the dock had no viewport leaf to honour it on.
///
/// A scene created before the reconciler has built the layout asks for a mode
/// on an empty tree. The request is held here rather than dropped, and honoured
/// on the first frame a leaf exists.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingViewportFocus(pub ViewportMode);

pub struct ViewportHostPlugin;

impl Plugin for ViewportHostPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewportModeIntent>();
    }
}

#[cfg(test)]
mod viewport_mode_tests {
    use super::*;

    #[test]
    fn a_ui_scene_is_authored_on_the_canvas() {
        assert_eq!(
            ViewportMode::for_scene_kind(SceneKind::Ui),
            ViewportMode::TwoD
        );
    }

    #[test]
    fn a_2d_world_scene_is_authored_on_the_canvas() {
        assert_eq!(
            ViewportMode::for_scene_kind(SceneKind::TwoD),
            ViewportMode::TwoD
        );
    }

    #[test]
    fn a_3d_scene_is_authored_in_the_world_viewport() {
        assert_eq!(
            ViewportMode::for_scene_kind(SceneKind::ThreeD),
            ViewportMode::ThreeD
        );
    }

    #[test]
    fn a_mode_parameter_names_either_mode() {
        assert_eq!(ViewportMode::parse("3d"), Some(ViewportMode::ThreeD));
        assert_eq!(ViewportMode::parse(" 2D "), Some(ViewportMode::TwoD));
        assert_eq!(ViewportMode::parse("canvas"), None);
    }
}
