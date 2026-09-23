//! Viewport camera preferences a project remembers.
//!
//! Stored in `.jackdaw/settings.json` (see [`crate::project_settings`])
//! under `camera`, beside the canvas settings, and pushed onto every
//! viewport camera's [`JackdawCameraSettings`] and perspective projection.
//! Kept out of the undo snapshot: a preference is not part of the document.

use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_camera::JackdawCameraSettings;
use serde::{Deserialize, Serialize};

use crate::project::ProjectRoot;
use crate::project_settings::{Section, load_section, store_section};
use crate::selection::Selection;
use crate::view_ops::ViewportFocus;
use crate::viewport::{ActiveViewport, MainViewportCamera};

/// The settings-file key the camera preferences live under.
const CAMERA_SECTION: &str = "camera";

/// How far ahead of a camera the viewport orbits after looking through it.
const LOOK_THROUGH_FOCUS_DISTANCE: f32 = 10.0;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<CameraPreferences>().add_systems(
        Update,
        (sync_project_camera_preferences, apply_camera_preferences).chain(),
    );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ViewportCameraOp>()
        .register_operator::<ViewportLookThroughOp>();
}

/// How the fly camera reads a look drag, and the lens the viewport sees through.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraPreferences {
    /// Pitch the view down when the pointer moves away, rather than up.
    pub invert_y: bool,
    /// Vertical field of view, in degrees.
    pub fov_degrees: f32,
    /// Near clip plane, in metres.
    pub near: f32,
    /// Far clip plane, in metres.
    pub far: f32,
}

impl Default for CameraPreferences {
    fn default() -> Self {
        let lens = PerspectiveProjection::default();
        Self {
            invert_y: false,
            fov_degrees: lens.fov.to_degrees(),
            near: lens.near,
            far: lens.far,
        }
    }
}

impl CameraPreferences {
    /// The perspective projection the viewport sees through.
    pub fn perspective(&self) -> PerspectiveProjection {
        PerspectiveProjection {
            fov: self.fov_degrees.to_radians(),
            near: self.near,
            far: self.far,
            ..default()
        }
    }

    fn lens_problem(&self) -> Option<String> {
        if !(1.0..=179.0).contains(&self.fov_degrees) {
            return Some(format!(
                "a field of view of {} degrees is outside 1 to 179",
                self.fov_degrees
            ));
        }
        if self.near <= 0.0 || self.far <= self.near {
            return Some(format!(
                "the clip planes need 0 < near < far, got near {} and far {}",
                self.near, self.far
            ));
        }
        None
    }
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
    mut viewports: Query<&mut Projection, With<MainViewportCamera>>,
) {
    for mut settings in &mut cameras {
        if settings.invert_y != preferences.invert_y {
            settings.invert_y = preferences.invert_y;
        }
    }
    let lens = preferences.perspective();
    for mut projection in &mut viewports {
        let Projection::Perspective(current) = projection.as_ref() else {
            continue;
        };
        if (current.fov, current.near, current.far) == (lens.fov, lens.near, lens.far) {
            continue;
        }
        if let Projection::Perspective(current) = projection.as_mut() {
            current.fov = lens.fov;
            current.near = lens.near;
            current.far = lens.far;
        }
    }
}

/// Take new preferences and keep them with the open project.
fn set_preferences(world: &mut World, preferences: CameraPreferences) {
    *world.resource_mut::<CameraPreferences>() = preferences;
    if let Some(mut dirty) = world.get_resource_mut::<crate::MenuBarDirty>() {
        dirty.0 = true;
    }
    if let Some(project) = world.get_resource::<ProjectRoot>() {
        store_section(&project.root, Section::Key(CAMERA_SECTION), &preferences);
    }
}

/// Set the viewport camera's lens. Every field is optional and the ones left out keep what they were.
#[operator(
    id = "viewport.camera",
    label = "Viewport Camera",
    description = "Set the viewport camera's field of view and clip planes, kept with the project.",
    allows_undo = false,
    params(
        fov(f64, doc = "Vertical field of view, in degrees."),
        near(f64, doc = "Near clip plane, in metres."),
        far(f64, doc = "Far clip plane, in metres."),
    )
)]
pub(crate) fn viewport_camera(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let before = *world.resource::<CameraPreferences>();
    let read = |name: &str| params.as_float(name).map(|value| value as f32);
    let after = CameraPreferences {
        fov_degrees: read("fov").unwrap_or(before.fov_degrees),
        near: read("near").unwrap_or(before.near),
        far: read("far").unwrap_or(before.far),
        ..before
    };
    if let Some(problem) = after.lens_problem() {
        warn_caller(world, format!("viewport.camera: {problem}"));
        return OperatorResult::Cancelled;
    }
    if after != before {
        set_preferences(world, after);
    }
    OperatorResult::Finished
}

/// Put the viewport where a scene camera stands and see through its lens.
#[operator(
    id = "viewport.look_through",
    label = "Look Through Camera",
    description = "Put the viewport where a scene camera stands and see through its lens.",
    allows_undo = false,
    params(entity(
        Entity,
        doc = "The camera to look through. Defaults to the selected one."
    ))
)]
pub(crate) fn viewport_look_through(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let id = "viewport.look_through";
    let Some(source) = params
        .as_entity("entity")
        .or_else(|| world.resource::<Selection>().primary())
    else {
        warn_caller(world, format!("{id}: name a camera or select one"));
        return OperatorResult::Cancelled;
    };
    let seen = world.get_entity(source).ok().and_then(|node| {
        let is_scene_camera =
            node.contains::<Camera3d>() && !node.contains::<crate::EditorEntity>();
        is_scene_camera.then(|| {
            (
                node.get::<GlobalTransform>().copied().unwrap_or_default(),
                node.get::<Projection>().cloned().unwrap_or_default(),
            )
        })
    });
    let Some((placed, projection)) = seen else {
        warn_caller(world, format!("{id}: {source} is not a scene camera"));
        return OperatorResult::Cancelled;
    };
    let viewport = world.resource::<ActiveViewport>().camera.or_else(|| {
        let mut cameras = world.query_filtered::<Entity, With<MainViewportCamera>>();
        let mut found = cameras.iter(world);
        let first = found.next();
        first.filter(|_| found.next().is_none())
    });
    let Some(viewport) = viewport else {
        warn_caller(world, format!("{id}: there is no viewport to look through"));
        return OperatorResult::Cancelled;
    };

    let transform = placed.compute_transform().with_scale(Vec3::ONE);
    let focus = transform.translation + transform.forward() * LOOK_THROUGH_FOCUS_DISTANCE;
    if let Projection::Perspective(lens) = &projection {
        let before = *world.resource::<CameraPreferences>();
        let after = CameraPreferences {
            fov_degrees: lens.fov.to_degrees(),
            near: lens.near,
            far: lens.far,
            ..before
        };
        if after.lens_problem().is_none() && after != before {
            set_preferences(world, after);
        }
    }
    let lens = world.resource::<CameraPreferences>().perspective();
    let seen_through = match projection {
        Projection::Orthographic(ortho) => Projection::Orthographic(ortho),
        _ => Projection::Perspective(lens),
    };
    world
        .entity_mut(viewport)
        .insert((transform, seen_through, ViewportFocus(focus)));
    OperatorResult::Finished
}
