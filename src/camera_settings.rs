//! Viewport camera preferences a project remembers.
//!
//! Stored in `.jackdaw/settings.json` (see [`crate::project_settings`])
//! under `camera`, beside the canvas settings, and pushed onto every
//! viewport camera's [`JackdawCameraSettings`]. Kept out of the undo
//! snapshot: a preference is not part of the document.

use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_camera::JackdawCameraSettings;
use serde::{Deserialize, Serialize};

use crate::project::ProjectRoot;
use crate::project_settings::{Section, load_section};

/// The settings-file key the camera preferences live under.
const CAMERA_SECTION: &str = "camera";

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<CameraPreferences>().add_systems(
        Update,
        (sync_project_camera_preferences, apply_camera_preferences).chain(),
    );
}

/// How the fly camera reads a look drag.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraPreferences {
    /// Pitch the view down when the pointer moves away, rather than up.
    pub invert_y: bool,
}

/// Load the open project's camera preferences, once per project opened.
/// Closing a project takes its preferences with it, so the next one opened
/// without a `camera` section starts from the defaults rather than inheriting.
fn sync_project_camera_preferences(
    project: Option<Res<ProjectRoot>>,
    mut preferences: ResMut<CameraPreferences>,
    mut loaded_root: Local<Option<PathBuf>>,
) {
    let root = project.map(|project| project.root.clone());
    if *loaded_root == root {
        return;
    }
    *preferences = match &root {
        Some(root) => load_section(root, Section::Key(CAMERA_SECTION)),
        None => CameraPreferences::default(),
    };
    *loaded_root = root;
}

/// Push the preferences onto every camera. Written every frame rather
/// than on change, because a viewport panel added later spawns its
/// camera with the component's own defaults.
fn apply_camera_preferences(
    preferences: Res<CameraPreferences>,
    mut cameras: Query<&mut JackdawCameraSettings>,
) {
    for mut settings in &mut cameras {
        if settings.invert_y != preferences.invert_y {
            settings.invert_y = preferences.invert_y;
        }
    }
}
