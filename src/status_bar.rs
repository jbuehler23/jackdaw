use bevy::prelude::*;
use jackdaw_feathers::status_bar::{StatusBarCenter, StatusBarLeft, StatusBarRight};

use crate::{
    EditorEntity,
    active_tool::ActiveTool,
    brush::{BrushEditMode, EditMode},
    build_status::{BuildState, BuildStatus},
    draw_brush::DrawBrushState,
    gizmos::{GizmoAxis, GizmoSpace},
    modal_transform::{ModalOp, ModalTransformState},
    numeric_transform::NumericTransformState,
    scene_io::{SceneDirtyState, SceneFilePath},
};

/// Git branch + short commit hash, read once at startup.
#[derive(Resource, Default)]
pub struct GitInfo {
    pub display: String,
}

pub struct StatusBarPlugin;

impl Plugin for StatusBarPlugin {
    fn build(&self, app: &mut App) {
        // Read git info once at startup
        let git_display = read_git_info();
        app.insert_resource(GitInfo {
            display: git_display,
        });
        app.add_systems(
            Update,
            (
                update_status_left,
                update_status_center,
                update_status_right,
                update_scene_stats,
                update_build_bar,
            )
                .run_if(in_state(crate::AppState::Editor)),
        );
        // Click observer on `StatusBarRight` so a Ready / Failed
        // build indicator becomes interactive (Reload / open log).
        // Attached on every entry into the editor; the launcher's
        // status bar is rebuilt across project re-opens, so this
        // catches each fresh entity. The build-progress bar is
        // spawned alongside it, hidden until a build runs.
        app.add_systems(OnEnter(crate::AppState::Editor), spawn_build_bar);
    }
}

fn read_git_info() -> String {
    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if branch.is_empty() {
        String::new()
    } else {
        format!("{branch} ({hash})")
    }
}

fn update_status_left(
    git_info: Res<GitInfo>,
    mut text_query: Query<&mut Text, With<StatusBarLeft>>,
) {
    // Git info is static, only set once
    let Ok(mut text) = text_query.single_mut() else {
        return;
    };
    if text.0.is_empty() && !git_info.display.is_empty() {
        text.0 = git_info.display.clone();
    }
}

fn update_status_center(
    scene_path: Res<SceneFilePath>,
    scene_dirty: Res<SceneDirtyState>,
    history: Res<jackdaw_commands::CommandHistory>,
    mut text_query: Query<&mut Text, With<StatusBarCenter>>,
) {
    if !scene_path.is_changed() && !scene_dirty.is_changed() && !history.is_changed() {
        return;
    }
    let Ok(mut text) = text_query.single_mut() else {
        return;
    };

    let version = env!("CARGO_PKG_VERSION");
    let dirty = history.undo_stack.len() != scene_dirty.undo_len_at_save;
    let dirty_marker = if dirty { "*" } else { "" };
    let path_str = scene_path
        .path
        .as_deref()
        .map(|p| format!(" | {dirty_marker}{p}"))
        .unwrap_or_else(|| {
            if dirty {
                " | *Unsaved".to_string()
            } else {
                String::new()
            }
        });

    text.0 = format!("Jackdaw v{version}{path_str}");
}

/// Marker for the scene stats text in the hierarchy panel footer.
#[derive(Component)]
pub struct SceneStatsText;

fn update_status_right(
    mode: Res<ActiveTool>,
    space: Res<GizmoSpace>,
    modal: Res<ModalTransformState>,
    edit_mode: Res<EditMode>,
    draw_state: Res<DrawBrushState>,
    build_status: Res<BuildStatus>,
    numeric: Res<NumericTransformState>,
    mut text_query: Query<(&mut Text, &mut TextColor), With<StatusBarRight>>,
) {
    // The build-progress states (`Building` / `Ready` / `Failed`)
    // need to re-render every frame because the `progress` Arc is
    // mutated by the cargo reader thread (no `is_changed`
    // observation available on data outside the ECS) and because
    // `Ready` is what the click observer reacts to. The other
    // status sources are change-detected to keep this cheap when
    // no build is active. Builds typically happen pre-editor (the
    // launcher's modal renders them); this footer rendering covers
    // any build that fires while the user is already in the editor
    // (e.g., a future file-watch / user-triggered rebuild).
    let build_active = !matches!(build_status.state, BuildState::Idle);
    if !build_active
        && !mode.is_changed()
        && !space.is_changed()
        && !modal.is_changed()
        && !edit_mode.is_changed()
        && !draw_state.is_changed()
        && !numeric.is_changed()
    {
        return;
    }
    let Ok((mut text, mut color)) = text_query.single_mut() else {
        return;
    };

    match &build_status.state {
        BuildState::Building { progress, .. } => {
            let (current, done) = progress
                .lock()
                .map(|g| (g.current_crate.clone(), g.artifacts_done))
                .unwrap_or((None, 0));
            text.0 = match current {
                Some(name) => format!("Compiling {name} ({done})"),
                None => "Building project...".to_string(),
            };
            color.0 = jackdaw_feathers::tokens::TEXT_SECONDARY;
            return;
        }
        BuildState::Ready { components, .. } => {
            text.0 = if *components == 0 {
                "Project built".to_string()
            } else {
                format!("Project built - {components} components")
            };
            color.0 = jackdaw_feathers::tokens::TEXT_SUCCESS;
            return;
        }
        BuildState::Failed { .. } => {
            text.0 = "Build failed - click for log".to_string();
            color.0 = jackdaw_feathers::tokens::TEXT_ERROR;
            return;
        }
        BuildState::Idle => {
            color.0 = jackdaw_feathers::tokens::TEXT_SECONDARY;
            // Fall through to the existing gizmo / edit-mode
            // rendering below.
        }
    }

    // Numeric transform entry takes priority while active: show the tool,
    // the constrained axis, and the number typed so far.
    if let Some(axis) = numeric.axis {
        let tool_str = match *mode {
            ActiveTool::Select => "Select",
            ActiveTool::Translate => "Translate",
            ActiveTool::Rotate => "Rotate",
            ActiveTool::Scale => "Scale",
        };
        let axis_str = match axis {
            GizmoAxis::X => "X",
            GizmoAxis::Y => "Y",
            GizmoAxis::Z => "Z",
            GizmoAxis::Uniform => "",
        };
        text.0 = format!("{tool_str} {axis_str}: {}", numeric.input);
        color.0 = jackdaw_feathers::tokens::TEXT_ACCENT;
        return;
    }

    // Show draw brush mode status
    if draw_state.active.is_some() {
        text.0 = "Draw Brush".to_string();
        return;
    }

    // Show physics tool mode info; Space commits, Esc cancels.
    if *edit_mode == EditMode::Physics {
        text.0 = "Physics Tool | drag selected to release | Space commit | Esc cancel".to_string();
        return;
    }

    // Show brush edit mode info
    if let EditMode::BrushEdit(sub_mode) = *edit_mode {
        let sub_str = match sub_mode {
            BrushEditMode::Face => "Face",
            BrushEditMode::Vertex => "Vertex",
            BrushEditMode::Edge => "Edge",
            BrushEditMode::Clip => "Clip",
            BrushEditMode::Knife => "Knife",
        };
        text.0 = format!("Edit: {sub_str}");
        return;
    }

    // Show modal operation info when active
    if let Some(ref active) = modal.active {
        let op_str = match active.op {
            ModalOp::Grab => "Grab",
            ModalOp::Rotate => "Rotate",
            ModalOp::Scale => "Scale",
        };
        text.0 = format!("{op_str} | LMB confirm, RMB cancel");
        return;
    }

    let mode_str = match *mode {
        ActiveTool::Select => "Select",
        ActiveTool::Translate => "Translate",
        ActiveTool::Rotate => "Rotate",
        ActiveTool::Scale => "Scale",
    };
    let space_str = match *space {
        GizmoSpace::World => "World",
        GizmoSpace::Local => "Local",
    };

    text.0 = format!("{mode_str} ({space_str})");
}

/// System to update the scene stats text in the hierarchy panel footer.
pub fn update_scene_stats(
    scene_entities: Query<Entity, (With<Transform>, Without<EditorEntity>)>,
    meshes: Query<(), (With<Mesh3d>, Without<EditorEntity>)>,
    point_lights: Query<(), (With<PointLight>, Without<EditorEntity>)>,
    dir_lights: Query<(), (With<DirectionalLight>, Without<EditorEntity>)>,
    spot_lights: Query<(), (With<SpotLight>, Without<EditorEntity>)>,
    cameras: Query<(), (With<Camera3d>, Without<EditorEntity>)>,
    mut text_query: Query<&mut Text, With<SceneStatsText>>,
) {
    let Ok(mut text) = text_query.single_mut() else {
        return;
    };

    let total = scene_entities.iter().count();
    let mesh_count = meshes.iter().count();
    let light_count =
        point_lights.iter().count() + dir_lights.iter().count() + spot_lights.iter().count();
    let camera_count = cameras.iter().count();

    let new_text = format!(
        "{total} entities  {mesh_count} meshes  {light_count} lights  {camera_count} cameras"
    );
    if text.0 != new_text {
        text.0 = new_text;
    }
}

/// Height of the footer build-progress bar.
const BUILD_BAR_HEIGHT: f32 = 3.0;
/// Width of the sliding segment in the indeterminate animation, in percent.
const BUILD_BAR_SEGMENT: f32 = 22.0;
/// Seconds for one back-and-forth of the indeterminate segment.
const BUILD_BAR_CYCLE: f32 = 1.4;

/// Track (background) of the footer build-progress bar. A thin full-width
/// loading bar flush along the window's bottom edge.
#[derive(Component)]
struct BuildBarTrack;

/// Fill of the footer build-progress bar. Absolutely positioned so it can
/// either fill from the left (determinate) or slide (indeterminate).
#[derive(Component)]
struct BuildBarFill;

/// Spawn the build-progress bar once on entering the editor, hidden.
/// `update_build_bar` shows and drives it while a build runs.
fn spawn_build_bar(mut commands: Commands) {
    commands.spawn((
        BuildBarTrack,
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(0.0),
            left: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Px(BUILD_BAR_HEIGHT),
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(jackdaw_feathers::tokens::PANEL_HEADER_BG),
        Visibility::Hidden,
        ZIndex(1000),
        children![(
            BuildBarFill,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(0.0),
                width: Val::Percent(0.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(jackdaw_feathers::tokens::TEXT_ACCENT),
        )],
    ));
}

/// Show the build bar while a build runs: fill from the left when a total
/// is known, or slide a segment back and forth otherwise (the redirected
/// project build has no reliable unit total). Hide it when idle.
fn update_build_bar(
    time: Res<Time>,
    build_status: Res<BuildStatus>,
    mut track: Query<&mut Visibility, With<BuildBarTrack>>,
    mut fill: Query<&mut Node, With<BuildBarFill>>,
) {
    let Ok(mut visibility) = track.single_mut() else {
        return;
    };
    let BuildState::Building { progress, .. } = &build_status.state else {
        if *visibility != Visibility::Hidden {
            *visibility = Visibility::Hidden;
        }
        return;
    };
    *visibility = Visibility::Visible;

    let fraction = progress.lock().ok().and_then(|g| g.fraction());
    let Ok(mut node) = fill.single_mut() else {
        return;
    };
    match fraction {
        Some(f) => {
            node.left = Val::Percent(0.0);
            node.width = Val::Percent((f * 100.0).clamp(0.0, 100.0));
        }
        None => {
            // Triangle wave in [0, 1]: 0 at the ends, 1 at the midpoint, so
            // the segment eases to the right edge and back.
            let phase = (time.elapsed_secs() % BUILD_BAR_CYCLE) / BUILD_BAR_CYCLE;
            let sweep = 1.0 - (2.0 * phase - 1.0).abs();
            node.left = Val::Percent(sweep * (100.0 - BUILD_BAR_SEGMENT));
            node.width = Val::Percent(BUILD_BAR_SEGMENT);
        }
    }
}
