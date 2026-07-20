use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
    window::{PrimaryWindow, RawHandleWrapper},
};
use jackdaw_feathers::{
    button::{ButtonVariant, IconButtonProps, icon_button},
    icons::{EditorFont, Icon},
    text_edit::{TextEditProps, TextEditValue, text_edit},
    tokens,
};
use jackdaw_localization::LocalizedText;
use rfd::{AsyncFileDialog, FileHandle};

use crate::{
    AppState,
    new_project::scaffold_project,
    project::{self, ProjectRoot},
    scaffold::{ScaffoldError, TemplateKind},
    windowing::{JackdawIcon, title_bar_repo_link},
};
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
use bevy_window_chrome::CaptionFont;
use bevy_window_chrome::{WindowChromeTheme, spawn_window_shell};

pub struct ProjectSelectPlugin;

impl Plugin for ProjectSelectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NewProjectState>()
            .init_resource::<crate::build_status::BuildStatus>()
            .init_resource::<PreflightState>()
            .add_systems(
                OnEnter(AppState::ProjectSelect),
                (spawn_project_selector, start_preflight),
            )
            .add_systems(
                Update,
                (
                    poll_folder_dialog,
                    refresh_build_progress_ui,
                    poll_preflight,
                )
                    .run_if(in_state(AppState::ProjectSelect)),
            )
            .add_systems(
                Update,
                apply_pending_auto_open.run_if(in_state(AppState::ProjectSelect)),
            )
            .add_systems(
                Update,
                update_preflight_banner
                    .run_if(in_state(AppState::ProjectSelect))
                    .run_if(resource_changed::<PreflightState>),
            )
            // Build-progress polling and task draining run in BOTH
            // states: a background build task can outlive the
            // launcher, and the status_bar's progress region depends
            // on `refresh_build_progress_snapshot` to keep updating.
            // They're cheap no-ops when nothing is in flight.
            .add_systems(
                Update,
                (poll_new_project_tasks, refresh_build_progress_snapshot),
            )
            // The dylib-install step MUST run outside of `Update`'s
            // `schedule_scope`. The game's `GameApp::add_systems(Update, ...)`
            // inserts into `Schedules`; doing that while bevy has
            // `Update` checked out via `schedule_scope` causes the
            // modification to be overwritten when the scope re-inserts
            // at exit. `Last` has its own scope and doesn't clash.
            .add_systems(
                Last,
                apply_pending_install.run_if(in_state(AppState::ProjectSelect)),
            );
    }
}

/// Marker for the project selector root UI node.
#[derive(Component, Copy, Clone)]
struct ProjectSelectorRoot;

/// When set, the project selector will skip UI and auto-open the given project.
#[derive(Resource)]
pub struct PendingAutoOpen {
    pub path: PathBuf,
    /// `true` when we got here via a post-restart auto-open ;
    /// the parent process already built + installed the dylib,
    /// so we skip that step (preventing an infinite
    /// build->restart->auto-open->build loop).
    pub skip_build: bool,
}

/// Resource holding the async folder picker task.
#[derive(Resource)]
struct FolderDialogTask(Task<Option<rfd::FileHandle>>);

/// Root marker for the New Project modal overlay. Spawned when the
/// user clicks **+ New Extension** / **+ New Game**; despawned on
/// Cancel or on successful scaffold.
#[derive(Component)]
struct NewProjectModalRoot;

/// Wraps the Name `TextEdit` so the Create handler can read its
/// current value.
#[derive(Component)]
struct NewProjectNameInput;

#[derive(Component)]
struct NewProjectLocationText;

#[derive(Component)]
struct NewProjectStatusText;

/// Outer container for the progress-bar + log-tail UI, toggled on
/// when a build is in flight so the idle modal doesn't leave a
/// visual gap.
#[derive(Component)]
struct NewProjectProgressContainer;

/// Wraps the "currently compiling `<crate>`" label.
#[derive(Component)]
struct NewProjectProgressCrateLabel;

/// Wraps the `progress_bar` widget so the refresh system can walk
/// its fill child.
#[derive(Component)]
struct NewProjectProgressBarSlot;

/// Wraps the log-tail text; refreshed with the last 20 lines of
/// cargo output each frame.
#[derive(Component)]
struct NewProjectLogText;

#[derive(Component)]
struct NewProjectCancelButton;

#[derive(Component)]
struct NewProjectCancelButtonLabel;

#[derive(Component)]
struct NewProjectCreateButton;

#[derive(Component)]
struct NewProjectBrowseButton;

/// Tiny "reset to default" affordance shown next to the Browse
/// button when the remembered location differs from
/// [`default_projects_dir`]. Hidden otherwise.
#[derive(Component)]
struct NewProjectResetLocationButton;

/// Drives the modal's async operations. Internal to this module;
/// external systems that need to observe build progress read the
/// public `BuildStatus` resource instead.
#[derive(Resource, Default)]
struct NewProjectState {
    /// Which template the user opened the dialog with. `None` when
    /// the modal isn't open.
    kind: Option<TemplateKind>,
    /// Parent directory the new project will be placed under.
    /// Scaffolder produces `location/name/`.
    location: PathBuf,
    /// In-flight folder picker (rfd).
    folder_task: Option<Task<Option<FileHandle>>>,
    /// In-flight scaffold from the embedded templates.
    scaffold_task: Option<Task<Result<PathBuf, ScaffoldError>>>,
    /// In-flight cdylib build for a project being opened.
    build_task: Option<Task<Result<PathBuf, crate::ext_build::BuildError>>>,
    /// Cancel flag for the in-flight `build_task`. Flipped by
    /// `on_cancel_new_project` when a build is running; the worker
    /// polls it and surfaces `BuildError::Cancelled` on exit.
    build_cancel: Option<Arc<AtomicBool>>,
    /// Artifact waiting to be installed by `apply_pending_install`
    /// (runs in `Last`, not `Update`, so modifications to the
    /// `Update` schedule by the game's `GameApp::add_systems` don't
    /// collide with `Update`'s active `schedule_scope`).
    pending_install: Option<PathBuf>,
    /// Shared progress sink the build task writes to. The
    /// `refresh_build_progress_ui` system reads a snapshot from
    /// here each frame and copies it into `build_progress_snapshot`
    /// so the modal's bar/log nodes can update without locking on
    /// the hot path.
    build_progress: Option<std::sync::Arc<std::sync::Mutex<crate::ext_build::BuildProgress>>>,
    /// Latest snapshot of `build_progress`, copied each frame.
    /// Used by `refresh_build_progress_ui` to render the dylib
    /// install modal's progress bar.
    build_progress_snapshot: Option<crate::ext_build::BuildProgress>,
    /// Path to the freshly-scaffolded project, kept around so the
    /// build-completion handler can transition into the editor
    /// pointing at the right root.
    pending_project: Option<PathBuf>,
    /// Last user-visible message (used for both progress and errors).
    status: Option<String>,
}

fn default_projects_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join("Projects"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Environment preflight results for the launcher. Populated asynchronously on
/// entering the project selector so a missing toolchain / cmake / a Windows
/// linker misconfiguration is reported before the user starts a long build.
#[derive(Resource, Default)]
pub struct PreflightState {
    task: Option<Task<Vec<crate::preflight::CheckResult>>>,
    /// Latest results, available for the launcher UI to render.
    pub results: Vec<crate::preflight::CheckResult>,
    reported: bool,
}

/// Kick off the environment checks off the main thread when the launcher opens.
fn start_preflight(mut state: ResMut<PreflightState>, pending: Option<Res<PendingAutoOpen>>) {
    // No launcher shell during an auto-open handoff; skip the checks.
    if pending.is_some() {
        return;
    }
    state.results.clear();
    state.reported = false;
    state.task =
        Some(AsyncComputeTaskPool::get().spawn(async move { crate::preflight::run_all_checks() }));
}

/// Drain the preflight task when it completes, store the results for the UI, and
/// log them so a failure is visible even before the panel renders.
fn poll_preflight(mut state: ResMut<PreflightState>) {
    let Some(task) = state.task.as_mut() else {
        return;
    };
    let Some(results) = future::block_on(future::poll_once(task)) else {
        return;
    };
    state.task = None;
    if !state.reported {
        use crate::preflight::CheckStatus;
        for r in &results {
            let fix = r.fix.as_deref().unwrap_or("");
            match r.status {
                CheckStatus::Ok => info!("Preflight {}: {}", r.label, r.detail),
                CheckStatus::Warn => warn!("Preflight {}: {} -- {fix}", r.label, r.detail),
                CheckStatus::Fail => error!("Preflight {}: {} -- {fix}", r.label, r.detail),
            }
        }
        state.reported = true;
    }
    state.results = results;
}

/// Marker on the launcher's preflight banner container (top of the body).
#[derive(Component)]
pub struct PreflightBanner;

/// Rebuild the preflight banner from the latest results. Quiet while checking
/// (one line) or healthy (a single green line); expands with per-issue detail
/// and a fix when a check warns or fails. Runs only when `PreflightState`
/// changes, so it does not rebuild every frame.
fn update_preflight_banner(
    mut commands: Commands,
    state: Res<PreflightState>,
    banner_q: Query<(Entity, Option<&Children>), With<PreflightBanner>>,
    editor_font: Res<EditorFont>,
    icon_font: Res<jackdaw_feathers::icons::IconFont>,
) {
    use crate::preflight::CheckStatus;

    let Ok((banner, children)) = banner_q.single() else {
        return;
    };
    if let Some(children) = children {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }

    const GREEN: Color = Color::srgb(0.45, 0.80, 0.55);
    const AMBER: Color = Color::srgb(0.92, 0.74, 0.36);
    const RED: Color = Color::srgb(0.92, 0.45, 0.45);

    let font = editor_font.0.clone();
    let ifont = icon_font.0.clone();

    if state.task.is_some() {
        preflight_line(
            &mut commands,
            banner,
            &ifont,
            &font,
            Icon::Loader,
            tokens::TEXT_SECONDARY,
            "Checking environment...".to_string(),
            tokens::TEXT_SECONDARY,
        );
        return;
    }
    if state.results.is_empty() {
        return;
    }
    let issues: Vec<&crate::preflight::CheckResult> = state
        .results
        .iter()
        .filter(|r| r.status != CheckStatus::Ok)
        .collect();
    if issues.is_empty() {
        preflight_line(
            &mut commands,
            banner,
            &ifont,
            &font,
            Icon::CircleCheck,
            GREEN,
            "Environment ready".to_string(),
            GREEN,
        );
        return;
    }
    let n = issues.len();
    let header = format!("Environment - {n} issue{}", if n == 1 { "" } else { "s" });
    preflight_line(
        &mut commands,
        banner,
        &ifont,
        &font,
        Icon::TriangleAlert,
        AMBER,
        header,
        AMBER,
    );
    for r in issues {
        let (glyph, color) = match r.status {
            CheckStatus::Fail => (Icon::CircleAlert, RED),
            _ => (Icon::TriangleAlert, AMBER),
        };
        preflight_line(
            &mut commands,
            banner,
            &ifont,
            &font,
            glyph,
            color,
            format!("{}: {}", r.label, r.detail),
            tokens::TEXT_PRIMARY,
        );
        if let Some(fix) = &r.fix {
            preflight_fix_line(&mut commands, banner, &font, format!("fix: {fix}"));
        }
    }

    // Recheck button: re-runs the environment checks after the user fixes
    // something, without reopening the launcher.
    let recheck = commands
        .spawn((
            Node {
                align_self: AlignSelf::FlexStart,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(3.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_LG)),
                ..Default::default()
            },
            BackgroundColor(tokens::TOOLBAR_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(banner),
            children![
                (
                    Text::new(String::from(Icon::RefreshCw.unicode())),
                    TextFont {
                        font: ifont.clone().into(),
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                ),
                (
                    Text::new("Recheck"),
                    TextFont {
                        font: font.clone().into(),
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                ),
            ],
        ))
        .id();
    commands
        .entity(recheck)
        .observe(|_: On<Pointer<Click>>, mut state: ResMut<PreflightState>| {
            state.results.clear();
            state.reported = false;
            state.task = Some(
                AsyncComputeTaskPool::get()
                    .spawn(async move { crate::preflight::run_all_checks() }),
            );
        });
}

/// A banner row: an icon glyph followed by a label.
fn preflight_line(
    commands: &mut Commands,
    banner: Entity,
    icon_font: &Handle<Font>,
    font: &Handle<Font>,
    icon: Icon,
    icon_color: Color,
    text: String,
    text_color: Color,
) {
    commands.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(tokens::SPACING_SM),
            ..Default::default()
        },
        ChildOf(banner),
        children![
            (
                Text::new(String::from(icon.unicode())),
                TextFont {
                    font: icon_font.clone().into(),
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(icon_color),
            ),
            (
                Text::new(text),
                TextFont {
                    font: font.clone().into(),
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(text_color),
            ),
        ],
    ));
}

/// An indented secondary "fix:" line under an issue.
fn preflight_fix_line(commands: &mut Commands, banner: Entity, font: &Handle<Font>, text: String) {
    commands.spawn((
        Node {
            margin: UiRect::left(Val::Px(22.0)),
            ..Default::default()
        },
        ChildOf(banner),
        children![(
            Text::new(text),
            TextFont {
                font: font.clone().into(),
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
    ));
}

/// Perform a queued auto-open, one frame after entering the project selector.
/// Deferring the open to `Update` (rather than running it from the `OnEnter`
/// spawn) lets `Startup` finish loading the editor extensions first, so the
/// open path finds the resources they register, such as `UntitledCounter`.
/// Removing the resource makes this run exactly once.
fn apply_pending_auto_open(world: &mut World) {
    let Some(pending) = world.remove_resource::<PendingAutoOpen>() else {
        return;
    };
    enter_project_with(world, pending.path, pending.skip_build);
}

fn spawn_project_selector(
    mut commands: Commands,
    theme: Res<WindowChromeTheme>,
    editor_font: Res<EditorFont>,
    icon_font: Res<jackdaw_feathers::icons::IconFont>,
    jackdaw_icon: Res<JackdawIcon>,
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    caption_font: Res<CaptionFont>,
    pending: Option<Res<PendingAutoOpen>>,
) {
    if pending.is_some() {
        // Camera only, no shell: the open itself runs in `apply_pending_auto_open`
        // (an `Update` system) rather than here, so `Startup` has loaded the
        // editor extensions first. Those extensions register resources the open
        // path needs (e.g. `UntitledCounter`, populated when a project with no
        // scene falls back to creating an untitled one). The build modal still
        // draws over this camera.
        commands.spawn((Camera2d, ProjectSelectorRoot));
        return;
    }

    let recent = project::read_recent_projects();
    let font = editor_font.0.clone();
    let icon_font_handle = icon_font.0.clone();

    // Detect CWD project candidate
    let cwd = std::env::current_dir().unwrap_or_default();
    let cwd_has_project = cwd.join(".jsn/project.jsn").is_file()
        || cwd.join("project.jsn").is_file()
        || cwd.join("assets").is_dir();

    let slots = spawn_window_shell(
        &mut commands,
        &theme,
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        caption_font,
        ProjectSelectorRoot,
    );
    fill_project_selector(
        &mut commands,
        slots.title_bar,
        slots.body,
        font,
        icon_font_handle,
        jackdaw_icon.0.clone(),
        recent,
        cwd,
        cwd_has_project,
    );
}

fn fill_project_selector(
    commands: &mut Commands,
    title_bar: Entity,
    body: Entity,
    font: Handle<Font>,
    icon_font_handle: Handle<Font>,
    jackdaw_icon: Handle<Image>,
    recent: project::RecentProjects,
    cwd: PathBuf,
    cwd_has_project: bool,
) {
    commands
        .entity(title_bar)
        .with_children(|title_bar_parent| {
            title_bar_parent.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::SpaceBetween,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    padding: UiRect::horizontal(Val::Px(tokens::SPACING_MD)),
                    ..Default::default()
                },
                Pickable::IGNORE,
                children![
                    (
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(tokens::SPACING_MD),
                            ..Default::default()
                        },
                        children![
                            title_bar_repo_link(jackdaw_icon),
                            (
                                Text::new("jackdaw"),
                                TextFont {
                                    font: font.clone().into(),
                                    font_size: tokens::TEXT_SIZE,
                                    ..Default::default()
                                },
                                TextColor(tokens::TEXT_PRIMARY),
                                Pickable::IGNORE,
                            ),
                        ],
                    ),
                    (
                        Text::new(format!("v{}", env!("CARGO_PKG_VERSION"))),
                        TextFont {
                            font: font.clone().into(),
                            font_size: tokens::TEXT_SIZE_SM,
                            ..Default::default()
                        },
                        TextColor(tokens::DOC_TAB_INACTIVE_LABEL),
                        Pickable::IGNORE,
                    )
                ],
            ));
        });
    commands.entity(body).with_children(|body_parent| {
        // Preflight banner at the top of the body, filled live by
        // `update_preflight_banner` as the environment checks complete.
        body_parent.spawn((
            PreflightBanner,
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                padding: UiRect::new(Val::Px(8.0), Val::Px(8.0), Val::Px(8.0), Val::Px(0.0)),
                ..Default::default()
            },
        ));
        body_parent
            .spawn(Node {
                width: Val::Percent(100.0),
                flex_grow: 1.0,
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                padding: UiRect::new(Val::Px(8.0), Val::Px(8.0), Val::Px(0.0), Val::Px(8.0)),
                ..Default::default()
            })
            .with_children(|content| {
                // Left rail owns project-creation actions. Keeping these
                // separate from the project list makes the launcher read like
                // the rest of the editor: tools on the side, content adjacent.
                content
                    .spawn((
                        Node {
                            width: Val::Px(300.0),
                            height: Val::Percent(100.0),
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(8.0),
                            padding: UiRect::all(Val::Px(10.0)),
                            border: UiRect::all(Val::Px(1.0)),
                            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_LG)),
                            ..Default::default()
                        },
                        BackgroundColor(tokens::PANEL_BG),
                        BorderColor::all(tokens::BORDER_SUBTLE),
                    ))
                    .with_children(|sidebar| {
                        spawn_launcher_section_label(sidebar, "Start", font.clone());

                        spawn_new_project_button(
                            sidebar,
                            "New Game",
                            Icon::Gamepad2,
                            font.clone(),
                            icon_font_handle.clone(),
                            TemplateKind::Game,
                            true,
                        );
                        spawn_new_project_button(
                            sidebar,
                            "New Extension",
                            Icon::PackagePlus,
                            font.clone(),
                            icon_font_handle.clone(),
                            TemplateKind::Extension,
                            false,
                        );

                        let browse_entity = spawn_launcher_action_button(
                            sidebar,
                            "Open Folder",
                            Icon::FolderOpen,
                            font.clone(),
                            icon_font_handle.clone(),
                            tokens::TOOLBAR_BG,
                            tokens::HOVER_BG,
                        );
                        sidebar
                            .commands()
                            .entity(browse_entity)
                            .observe(spawn_browse_dialog);

                        sidebar.spawn((
                            Node {
                                flex_grow: 1.0,
                                ..Default::default()
                            },
                            Pickable::IGNORE,
                        ));

                        sidebar.spawn((
                            LocalizedText::new("source-checkout"),
                            TextFont {
                                font: font.clone().into(),
                                font_size: tokens::TEXT_SIZE_SM,
                                ..Default::default()
                            },
                            TextColor(tokens::DOC_TAB_INACTIVE_LABEL),
                        ));
                        sidebar.spawn((
                            Text::new(cwd.to_string_lossy().to_string()),
                            TextFont {
                                font: font.clone().into(),
                                font_size: tokens::TEXT_SIZE_XS,
                                ..Default::default()
                            },
                            TextColor(tokens::TEXT_SECONDARY),
                            Node {
                                max_width: Val::Px(260.0),
                                overflow: Overflow::clip(),
                                ..Default::default()
                            },
                        ));
                    });

                // Main panel lists openable projects. The current checkout is
                // promoted above recents so local development builds are one
                // click from the launcher.
                content
                    .spawn((
                        Node {
                            flex_grow: 1.0,
                            height: Val::Percent(100.0),
                            flex_direction: FlexDirection::Column,
                            border: UiRect::all(Val::Px(1.0)),
                            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_LG)),
                            overflow: Overflow::clip(),
                            ..Default::default()
                        },
                        BackgroundColor(tokens::PANEL_BG),
                        BorderColor::all(tokens::BORDER_SUBTLE),
                    ))
                    .with_children(|projects| {
                        projects.spawn((
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Px(34.0),
                                align_items: AlignItems::Center,
                                padding: UiRect::axes(Val::Px(12.0), Val::Px(0.0)),
                                border: UiRect::bottom(Val::Px(1.0)),
                                border_radius: BorderRadius::top(Val::Px(tokens::BORDER_RADIUS_LG)),
                                ..Default::default()
                            },
                            BackgroundColor(tokens::PANEL_HEADER_BG),
                            BorderColor::all(tokens::BORDER_SUBTLE),
                            children![(
                                Text::new("Projects"),
                                TextFont {
                                    font: font.clone().into(),
                                    font_size: tokens::TEXT_SIZE,
                                    ..Default::default()
                                },
                                TextColor(tokens::TEXT_PRIMARY),
                            )],
                        ));

                        projects
                            .spawn(Node {
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(6.0),
                                padding: UiRect::all(Val::Px(10.0)),
                                width: Val::Percent(100.0),
                                flex_grow: 1.0,
                                ..Default::default()
                            })
                            .with_children(|list| {
                                if cwd_has_project {
                                    let cwd_name = cwd
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_else(|| cwd.to_string_lossy().to_string());
                                    spawn_launcher_section_label(
                                        list,
                                        "Current Directory",
                                        font.clone(),
                                    );
                                    spawn_project_row(
                                        list,
                                        &cwd_name,
                                        &cwd.to_string_lossy(),
                                        font.clone(),
                                        icon_font_handle.clone(),
                                        cwd.clone(),
                                        true,
                                    );
                                }

                                spawn_launcher_section_label(list, "Recent", font.clone());
                                let mut shown_recent = 0usize;
                                for entry in &recent.projects {
                                    if cwd_has_project && entry.path == cwd {
                                        continue;
                                    }
                                    spawn_project_row(
                                        list,
                                        &entry.name,
                                        &entry.path.to_string_lossy(),
                                        font.clone(),
                                        icon_font_handle.clone(),
                                        entry.path.clone(),
                                        false,
                                    );
                                    shown_recent += 1;
                                }

                                if shown_recent == 0 {
                                    spawn_empty_recent_state(
                                        list,
                                        font.clone(),
                                        icon_font_handle.clone(),
                                    );
                                }
                            });
                    });
            });
    });
}

fn spawn_project_row(
    parent: &mut ChildSpawnerCommands,
    name: &str,
    path_display: &str,
    font: Handle<Font>,
    icon_font: Handle<Font>,
    project_path: PathBuf,
    is_cwd: bool,
) {
    // Rows use the same dense panel styling as editor lists: icon, primary
    // label, path, and an optional remove action for persisted recents.
    let row_entity = parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                width: Val::Percent(100.0),
                min_height: Val::Px(46.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_LG)),
                align_items: AlignItems::Center,
                column_gap: Val::Px(10.0),
                ..Default::default()
            },
            BackgroundColor(tokens::INPUT_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
        ))
        .id();

    let project_icon = parent
        .commands()
        .spawn((
            Node {
                width: Val::Px(26.0),
                height: Val::Px(26.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::DOC_TAB_ACTIVE_BG),
            children![(
                Text::new(String::from(Icon::Folder.unicode())),
                TextFont {
                    font: icon_font.clone().into(),
                    font_size: tokens::ICON_SM,
                    ..Default::default()
                },
                TextColor(tokens::DIR_ICON_COLOR),
            )],
            Pickable::IGNORE,
        ))
        .id();
    parent.commands().entity(row_entity).add_child(project_icon);

    let info_column = parent
        .commands()
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                row_gap: Val::Px(2.0),
                overflow: Overflow::clip(),
                ..Default::default()
            },
            children![
                (
                    Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(8.0),
                        align_items: AlignItems::Center,
                        ..Default::default()
                    },
                    children![
                        (
                            Text::new(name.to_string()),
                            TextFont {
                                font: font.clone().into(),
                                font_size: tokens::TEXT_SIZE,
                                ..Default::default()
                            },
                            TextColor(tokens::TEXT_PRIMARY),
                        ),
                        if_cwd_badge(is_cwd, font.clone()),
                    ],
                ),
                (
                    Text::new(path_display.to_string()),
                    TextFont {
                        font: font.clone().into(),
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                    Node {
                        max_width: Val::Percent(100.0),
                        overflow: Overflow::clip(),
                        ..Default::default()
                    },
                ),
            ],
            Pickable::IGNORE,
        ))
        .id();

    parent.commands().entity(row_entity).add_child(info_column);

    if !is_cwd {
        let remove_path = project_path.clone();
        let x_button = parent
            .commands()
            .spawn(icon_button(
                IconButtonProps::new(Icon::X).variant(ButtonVariant::Ghost),
                &icon_font,
            ))
            .id();

        parent.commands().entity(x_button).observe(
            move |mut click: On<Pointer<Click>>, mut commands: Commands| {
                click.propagate(false);
                let path = remove_path.clone();
                project::remove_recent(&path);
                commands.entity(row_entity).try_despawn();
            },
        );

        parent.commands().entity(row_entity).add_child(x_button);
    }

    parent.commands().entity(row_entity).observe(
        |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                bg.0 = tokens::HOVER_BG;
            }
        },
    );
    parent.commands().entity(row_entity).observe(
        |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                bg.0 = tokens::INPUT_BG;
            }
        },
    );

    parent.commands().entity(row_entity).observe(
        move |_: On<Pointer<Click>>, mut commands: Commands| {
            let path = project_path.clone();
            commands.queue(move |world: &mut World| {
                enter_project(world, path);
            });
        },
    );
}

fn if_cwd_badge(is_cwd: bool, font: Handle<Font>) -> impl Bundle {
    let text = if is_cwd { "current dir" } else { "" };
    (
        Text::new(text.to_string()),
        TextFont {
            font: font.into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_ACCENT),
    )
}

fn spawn_browse_dialog(
    _: On<Pointer<Click>>,
    mut commands: Commands,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) {
    let mut dialog = AsyncFileDialog::new().set_title("Select project folder");

    if let Ok(rh) = raw_handle.single() {
        // SAFETY: called on the main thread during an observer
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }

    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_folder().await });
    commands.insert_resource(FolderDialogTask(task));
}

fn poll_folder_dialog(world: &mut World) {
    let Some(mut task_res) = world.get_resource_mut::<FolderDialogTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task_res.0)) else {
        return;
    };
    world.remove_resource::<FolderDialogTask>();

    if let Some(handle) = result {
        let path = handle.path().to_path_buf();
        enter_project(world, path);
    }
}

/// Entry point for **every** "open a project" action from the
/// launcher (new-scaffold completion, recent-project click, manual
/// folder browse). Projects open in-process: anything without a
/// `Cargo.toml`, and any Cargo project with a `jackdaw.toml`,
/// transitions straight to the editor. A cdylib extension project
/// gets its build-and-install pass first, and an unrecognized Cargo
/// project gets the import offer.
pub fn enter_project(world: &mut World, root: PathBuf) {
    enter_project_with(world, root, false);
}

/// Same as [`enter_project`] but lets the caller bypass the build
/// step. Used by the post-restart auto-open path: the parent
/// process already produced the dylib, the loader picked it up at
/// startup, so a second build-and-install would either be a no-op
/// or (for games) trigger another restart loop.
pub fn enter_project_with(world: &mut World, root: PathBuf, skip_build: bool) {
    if skip_build || !root.join("Cargo.toml").is_file() {
        transition_to_editor(world, root);
        return;
    }
    // If the Cargo.toml is a plain (non-cdylib) crate there's no
    // dylib for the loader to pick up at open time. A project with a
    // `jackdaw.toml` is already set up and opens in-process; anything
    // else gets the import offer.
    if !crate::ext_build::manifest_declares_cdylib(&root) {
        if root.join("jackdaw.toml").is_file() {
            transition_to_editor(world, root);
            return;
        }
        info!(
            "Project at {} has a Cargo.toml but no jackdaw.toml; offering setup.",
            root.display()
        );
        show_setup_jackdaw_card(world, root, None);
        return;
    }

    let project_name = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_owned();

    // Show the "Opening project" modal so the user sees the build
    // + any auto-recovery retry rather than staring at a frozen
    // launcher. The scaffold flow already has its own modal; when
    // called from there we skip spawning a second one.
    let scaffold_modal_already_open = {
        let mut q = world.query_filtered::<Entity, With<NewProjectModalRoot>>();
        q.iter(world).next().is_some()
    };
    if !scaffold_modal_already_open {
        open_project_progress_modal(world, &project_name);
    }

    let progress = std::sync::Arc::new(std::sync::Mutex::new(
        crate::ext_build::BuildProgress::default(),
    ));
    {
        let mut state = world.resource_mut::<NewProjectState>();
        state.pending_project = Some(root.clone());
        state.status = Some(format!("Building `{project_name}`..."));
        state.build_progress = Some(std::sync::Arc::clone(&progress));
        state.build_progress_snapshot = Some(crate::ext_build::BuildProgress::default());
    }

    let root_for_task = root;
    let progress_for_task = std::sync::Arc::clone(&progress);
    let build_cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_task = Arc::clone(&build_cancel);
    let task = AsyncComputeTaskPool::get().spawn(async move {
        crate::ext_build::build_extension_project_with_progress(
            &root_for_task,
            Some(progress_for_task),
            Some(cancel_for_task),
        )
    });
    let mut state = world.resource_mut::<NewProjectState>();
    state.build_task = Some(task);
    state.build_cancel = Some(build_cancel);
}

/// Apply the project-root state change and flip `AppState` to
/// `Editor`. Called from [`enter_project`] (no build needed) and
/// from the build-complete poller (build finished, transitioning).
///
/// If the project has a file at `<root>/assets/scene.jsn`, that
/// scene is auto-loaded so the user lands in a populated editor
/// rather than an empty one. This is the convention the game
/// template ships with.
fn transition_to_editor(world: &mut World, root: PathBuf) {
    let config = project::load_project_config(&root)
        .unwrap_or_else(|| project::create_default_project(&root));

    project::touch_recent(&root, &config.name);

    world.insert_resource(ProjectRoot {
        root: root.clone(),
        config,
    });

    // Despawn the launcher UI.
    let mut to_despawn = Vec::new();
    let mut query = world.query_filtered::<Entity, With<ProjectSelectorRoot>>();
    for entity in query.iter(world) {
        to_despawn.push(entity);
    }
    for entity in to_despawn {
        if let Ok(ec) = world.get_entity_mut(entity) {
            ec.despawn();
        }
    }

    let mut next_state = world.resource_mut::<NextState<AppState>>();
    next_state.set(AppState::Editor);

    let last_open_tabs = world
        .resource::<crate::project::ProjectRoot>()
        .config
        .last_open_tabs
        .clone();
    let last_active = world
        .resource::<crate::project::ProjectRoot>()
        .config
        .last_active_tab;

    if !last_open_tabs.is_empty() {
        for rel in &last_open_tabs {
            let abs = root.join(rel);
            if !abs.is_file() {
                warn!("Persisted tab not found, skipping: {abs:?}");
                continue;
            }
            crate::scenes::operators::scene_open_system(world, &abs);
        }
        // Clamp last_active to current tab count.
        let tab_count = world.resource::<crate::scenes::Scenes>().tabs.len();
        if tab_count > 0 {
            let target = last_active.min(tab_count - 1);
            crate::scenes::swap::swap_active_tab(world, target);
        }
    }

    // If we ended up with zero tabs (no persisted list, or every
    // persisted entry was missing on disk), fall back to `assets/scene.bsn`
    // (the legacy `.jsn` sibling if that is all that exists) or an empty
    // untitled scene, so the user never lands in the editor with no scene.
    if world.resource::<crate::scenes::Scenes>().tabs.is_empty() {
        let assets = root.join("assets");
        let bsn = assets.join("scene.bsn");
        let jsn = assets.join("scene.jsn");
        let scene_path = if bsn.is_file() {
            Some(bsn)
        } else if jsn.is_file() {
            Some(jsn)
        } else {
            None
        };
        if let Some(scene_path) = scene_path {
            crate::scene_io::load_scene_from_file(world, &scene_path);
        } else {
            crate::scenes::operators::scene_new_system(world);
        }
    }
}

fn spawn_launcher_section_label(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    font: Handle<Font>,
) {
    parent.spawn((
        Text::new(label.to_string()),
        TextFont {
            font: font.into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::DOC_TAB_INACTIVE_LABEL),
        Node {
            margin: UiRect::top(Val::Px(4.0)),
            ..Default::default()
        },
        Pickable::IGNORE,
    ));
}

fn spawn_empty_recent_state(
    parent: &mut ChildSpawnerCommands,
    font: Handle<Font>,
    icon_font: Handle<Font>,
) {
    // Empty-state copy is intentionally terse; the left rail already exposes
    // the creation and browse affordances.
    parent.spawn((
        Node {
            width: Val::Percent(100.0),
            min_height: Val::Px(90.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            row_gap: Val::Px(6.0),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_LG)),
            ..Default::default()
        },
        BackgroundColor(tokens::TOOLBAR_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        children![
            (
                Text::new(String::from(Icon::FolderOpen.unicode())),
                TextFont {
                    font: icon_font.into(),
                    font_size: tokens::ICON_LG,
                    ..Default::default()
                },
                TextColor(tokens::DOC_TAB_INACTIVE_LABEL),
            ),
            (
                Text::new("No recent projects"),
                TextFont {
                    font: font.into(),
                    font_size: tokens::TEXT_SIZE,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
        ],
        Pickable::IGNORE,
    ));
}

fn spawn_launcher_action_button(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    icon: Icon,
    font: Handle<Font>,
    icon_font: Handle<Font>,
    idle_bg: Color,
    hover_bg: Color,
) -> Entity {
    // Shared launcher button primitive for actions that need an icon + label
    // but do not fit the generic icon-only button component.
    let button = parent
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(34.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(0.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_LG)),
                ..Default::default()
            },
            BackgroundColor(idle_bg),
            BorderColor::all(tokens::BORDER_SUBTLE),
            children![
                (
                    Text::new(String::from(icon.unicode())),
                    TextFont {
                        font: icon_font.into(),
                        font_size: tokens::ICON_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_PRIMARY),
                ),
                (
                    Text::new(label.to_string()),
                    TextFont {
                        font: font.into(),
                        font_size: tokens::TEXT_SIZE,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_PRIMARY),
                ),
            ],
        ))
        .id();

    parent.commands().entity(button).observe(
        move |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                bg.0 = hover_bg;
            }
        },
    );
    parent.commands().entity(button).observe(
        move |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                bg.0 = idle_bg;
            }
        },
    );

    button
}

/// Spawn a launcher action that opens the New Project modal with the
/// given template kind already selected.
fn spawn_new_project_button(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    icon: Icon,
    font: Handle<Font>,
    icon_font: Handle<Font>,
    kind: TemplateKind,
    primary: bool,
) {
    let idle_bg = if primary {
        tokens::SELECTED_BG
    } else {
        tokens::TOOLBAR_BG
    };
    let hover_bg = if primary {
        tokens::SELECTED_BORDER
    } else {
        tokens::HOVER_BG
    };
    let button =
        spawn_launcher_action_button(parent, label, icon, font, icon_font, idle_bg, hover_bg);

    parent.commands().entity(button).observe(
        move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.queue(move |world: &mut World| {
                open_new_project_modal(world, kind);
            });
        },
    );
}

/// Tear down any existing New Project modal. Idempotent.
pub fn close_new_project_modal(world: &mut World) {
    let mut q = world.query_filtered::<Entity, With<NewProjectModalRoot>>();
    let entities: Vec<Entity> = q.iter(world).collect();
    for entity in entities {
        if let Ok(ec) = world.get_entity_mut(entity) {
            ec.despawn();
        }
    }
    let mut state = world.resource_mut::<NewProjectState>();
    state.kind = None;
    state.folder_task = None;
    state.scaffold_task = None;
    state.status = None;
}

/// Card shown when opening a Cargo project that isn't set up for the
/// jackdaw editor. Offers to import it (the same code as `jackdaw
/// init`) or open it without setup. `error` re-renders the card with
/// a failure message (e.g. a Bevy version mismatch).
fn show_setup_jackdaw_card(world: &mut World, root: PathBuf, error: Option<String>) {
    close_new_project_modal(world);
    let font = world.resource::<EditorFont>().0.clone();

    let scrim = world
        .spawn((
            NewProjectModalRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(tokens::DIALOG_BACKDROP),
            GlobalZIndex(100),
        ))
        .id();
    let card = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(24.0)),
                min_width: Val::Px(480.0),
                max_width: Val::Px(640.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(scrim),
        ))
        .id();

    world.spawn((
        Text::new("Set up jackdaw for this project"),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_LG,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(card),
    ));
    world.spawn((
        Text::new(
            "This project has a Cargo.toml but isn't set up for the jackdaw editor. \
             Setting it up writes a `jackdaw.toml`, a gitignored `.jackdaw/` directory, \
             and nothing else; your code and your project's own builds are untouched. \
             If the project has no library target, a `GamePlugin` stub is created for \
             you.",
        ),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    if let Some(error) = error {
        world.spawn((
            Text::new(format!("Could not set up: {error}")),
            TextFont {
                font: font.clone().into(),
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(Color::srgb(0.92, 0.45, 0.45)),
            ChildOf(card),
        ));
    }

    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                justify_content: JustifyContent::FlexEnd,
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();

    let open_anyway = spawn_card_button(world, row, "Open without setup", &font, false);
    let setup = spawn_card_button(world, row, "Set up jackdaw", &font, true);

    let root_open = root.clone();
    world
        .entity_mut(open_anyway)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let root = root_open.clone();
            commands.queue(move |world: &mut World| {
                close_new_project_modal(world);
                transition_to_editor(world, root);
            });
        });
    world
        .entity_mut(setup)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let root = root.clone();
            commands.queue(move |world: &mut World| on_setup_jackdaw_clicked(world, root));
        });
}

/// A modal-card button. Primary uses the accent fill; secondary the toolbar fill.
fn spawn_card_button(
    world: &mut World,
    parent: Entity,
    label: &str,
    font: &Handle<Font>,
    primary: bool,
) -> Entity {
    let (bg, border) = if primary {
        (tokens::SELECTED_BG, tokens::SELECTED_BORDER)
    } else {
        (tokens::TOOLBAR_BG, tokens::BORDER_SUBTLE)
    };
    world
        .spawn((
            Node {
                height: Val::Px(32.0),
                padding: UiRect::axes(Val::Px(14.0), Val::Px(0.0)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_LG)),
                ..Default::default()
            },
            BackgroundColor(bg),
            BorderColor::all(border),
            ChildOf(parent),
            children![(
                Text::new(label.to_string()),
                TextFont {
                    font: font.clone().into(),
                    font_size: tokens::TEXT_SIZE,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
        ))
        .id()
}

/// Run the project import for the "Set up jackdaw" card. On success,
/// re-enter the project (now set up, so it opens in-process); on
/// failure (including a Bevy version mismatch), re-show the card with
/// the error instead of opening. If setup had to create a
/// `src/lib.rs` stub (bin-only project), warn the dev to move their game code
/// into the new `GamePlugin` before continuing.
fn on_setup_jackdaw_clicked(world: &mut World, root: PathBuf) {
    match crate::scaffold::import_project(&root, None) {
        Ok(report) => {
            info!(
                "Set up jackdaw at {}: {}",
                root.display(),
                report.actions.join(", ")
            );
            if report.created_lib_stub {
                show_lib_stub_warning_card(world, root);
            } else {
                close_new_project_modal(world);
                enter_project_with(world, root, false);
            }
        }
        Err(e) => {
            warn!("Set up jackdaw failed for {}: {e}", root.display());
            show_setup_jackdaw_card(world, root, Some(e.to_string()));
        }
    }
}

/// Shown after setup creates a `src/lib.rs` stub for a bin-only project. The
/// editor only sees components that live in the project's library, so the dev
/// has to move their game code out of `main.rs` into the new `GamePlugin`
/// before their components appear. Offers an automatic migration, or opening
/// the editor as-is (it just won't show their components yet).
fn show_lib_stub_warning_card(world: &mut World, root: PathBuf) {
    show_lib_stub_warning_card_with_note(world, root, None);
}

/// As [`show_lib_stub_warning_card`], with an extra red note (e.g. why an
/// automatic migration couldn't run).
fn show_lib_stub_warning_card_with_note(world: &mut World, root: PathBuf, note: Option<String>) {
    close_new_project_modal(world);
    let font = world.resource::<EditorFont>().0.clone();

    let scrim = world
        .spawn((
            NewProjectModalRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(tokens::DIALOG_BACKDROP),
            GlobalZIndex(100),
        ))
        .id();
    let card = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(24.0)),
                min_width: Val::Px(480.0),
                max_width: Val::Px(640.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(scrim),
        ))
        .id();

    world.spawn((
        Text::new("Created a library for your game code"),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_LG,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(card),
    ));
    world.spawn((
        Text::new(
            "This project had no library target, so jackdaw created `src/lib.rs` with \
             an empty `GamePlugin`. The editor only discovers components that live in \
             your library, so move your gameplay (components, systems, resources) out \
             of `main.rs` into `GamePlugin`, then have `main.rs` add it. Until you do, \
             the editor opens but the inspector won't list your components.",
        ),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(Color::srgb(0.95, 0.78, 0.45)),
        ChildOf(card),
    ));

    if let Some(note) = note {
        world.spawn((
            Text::new(note),
            TextFont {
                font: font.clone().into(),
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(Color::srgb(0.92, 0.45, 0.45)),
            ChildOf(card),
        ));
    }

    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                justify_content: JustifyContent::FlexEnd,
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();

    let open_editor =
        spawn_card_button(world, row, "Open editor (move code yourself)", &font, false);
    let migrate = spawn_card_button(world, row, "Migrate my code", &font, true);

    let root_open = root.clone();
    world
        .entity_mut(open_editor)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let root = root_open.clone();
            commands.queue(move |world: &mut World| {
                close_new_project_modal(world);
                enter_project_with(world, root, false);
            });
        });
    world
        .entity_mut(migrate)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let root = root.clone();
            commands.queue(move |world: &mut World| on_migrate_clicked(world, root));
        });
}

/// Plan an automatic migration of the project's `main.rs` into the new
/// `GamePlugin`. On success show a preview to confirm; on failure fall back to
/// the manual warning card with the reason appended.
fn on_migrate_clicked(world: &mut World, root: PathBuf) {
    let crate_name = match crate::migrate::crate_name_of(&root) {
        Ok(n) => n,
        Err(e) => {
            warn!("Migrate: {e}");
            show_lib_stub_warning_card_with_note(
                world,
                root,
                Some(format!("Couldn't read the crate name: {e}")),
            );
            return;
        }
    };
    match crate::migrate::plan_migration(&root, &crate_name) {
        Ok(plan) => show_migration_preview_card(world, root, plan),
        Err(e) => {
            warn!("Migrate: {e}");
            show_lib_stub_warning_card_with_note(
                world,
                root,
                Some(format!(
                    "Couldn't migrate automatically ({e}). Move your code into `GamePlugin` by hand."
                )),
            );
        }
    }
}

/// Preview of a planned migration: what moves into the library, what gets wired
/// into `GamePlugin`, and the safety note. "Apply migration" writes the files
/// (original `main.rs` saved as `main.rs.bak`) then builds + opens; "Back"
/// returns to the warning card.
fn show_migration_preview_card(
    world: &mut World,
    root: PathBuf,
    plan: crate::migrate::MigrationPlan,
) {
    close_new_project_modal(world);
    let font = world.resource::<EditorFont>().0.clone();

    let scrim = world
        .spawn((
            NewProjectModalRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(tokens::DIALOG_BACKDROP),
            GlobalZIndex(100),
        ))
        .id();
    let card = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(10.0),
                padding: UiRect::all(Val::Px(24.0)),
                min_width: Val::Px(520.0),
                max_width: Val::Px(680.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(scrim),
        ))
        .id();

    world.spawn((
        Text::new("Migrate your game code"),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_LG,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(card),
    ));
    world.spawn((
        Text::new("Nothing is written until you click Apply. Your original main.rs is saved as main.rs.bak."),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    let mut summary = String::new();
    if !plan.moved_items.is_empty() {
        summary.push_str(&format!(
            "Move into src/lib.rs:\n  {}\n\n",
            plan.moved_items.join(", ")
        ));
    }
    if !plan.moved_calls.is_empty() {
        summary.push_str(&format!(
            "Wire into GamePlugin::build:\n  {}\n\n",
            plan.moved_calls.join("\n  ")
        ));
    }
    summary.push_str("Slim src/main.rs to DefaultPlugins + add_plugins(GamePlugin).");
    for note in &plan.notes {
        summary.push_str(&format!("\n\nNote: {note}"));
    }
    world.spawn((
        Text::new(summary),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(card),
    ));

    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                justify_content: JustifyContent::FlexEnd,
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();

    let back = spawn_card_button(world, row, "Back", &font, false);
    let apply = spawn_card_button(world, row, "Apply migration", &font, true);

    let root_back = root.clone();
    world
        .entity_mut(back)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let root = root_back.clone();
            commands.queue(move |world: &mut World| show_lib_stub_warning_card(world, root));
        });

    // The plan is moved into the apply observer; it owns the generated contents.
    let plan = std::sync::Arc::new(plan);
    world
        .entity_mut(apply)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let root = root.clone();
            let plan = std::sync::Arc::clone(&plan);
            commands.queue(move |world: &mut World| {
                match crate::migrate::apply_migration(&root, &plan) {
                    Ok(()) => {
                        info!("Migrated {} into GamePlugin", root.display());
                        close_new_project_modal(world);
                        enter_project_with(world, root, false);
                    }
                    Err(e) => {
                        warn!("Migrate apply failed for {}: {e}", root.display());
                        show_lib_stub_warning_card_with_note(
                            world,
                            root,
                            Some(format!("Couldn't write the migration: {e}")),
                        );
                    }
                }
            });
        });
}

pub fn open_project_progress_modal(world: &mut World, project_name: &str) {
    close_new_project_modal(world);

    let editor_font = world.resource::<EditorFont>().0.clone();

    let scrim = world
        .spawn((
            NewProjectModalRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(tokens::DIALOG_BACKDROP),
            GlobalZIndex(100),
        ))
        .id();

    let card = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(24.0)),
                min_width: Val::Px(480.0),
                max_width: Val::Px(720.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(scrim),
        ))
        .id();

    world.spawn((
        Text::new(format!("Opening `{project_name}`")),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_LG,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(card),
    ));

    // Up-front hint about how long the build can take. Without this
    // users see a blank-looking launcher for several minutes on a
    // first run and assume jackdaw has hung.
    world.spawn((
        Text::new(
            "Building the project. First run with a fresh cargo cache can \
             take 5 to 10 minutes (bevy is ~500 crates). Subsequent opens \
             are incremental and finish in seconds.",
        ),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    world.spawn((
        NewProjectStatusText,
        Text::new(String::new()),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    // Progress container + children mirror the scaffold modal so
    // `refresh_build_progress_ui` walks the same marker chain.
    // `display: Flex` (not None) so the user sees the placeholder
    // text + empty progress bar right away, instead of a blank
    // card while cargo's first artifact event is pending.
    let progress_container = world
        .spawn((
            NewProjectProgressContainer,
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                margin: UiRect::top(Val::Px(8.0)),
                display: Display::Flex,
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();

    world.spawn((
        NewProjectProgressCrateLabel,
        LocalizedText::new("preparing-build"),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(progress_container),
    ));

    let bar_slot = world
        .spawn((
            NewProjectProgressBarSlot,
            Node {
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(progress_container),
        ))
        .id();
    world.spawn((
        jackdaw_feathers::progress::progress_bar(0.0),
        ChildOf(bar_slot),
    ));

    world.spawn((
        NewProjectLogText,
        Text::new(String::new()),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            max_height: Val::Px(220.0),
            overflow: Overflow::clip(),
            ..Default::default()
        },
        ChildOf(progress_container),
    ));
}

/// Show the New Project modal with the given template kind
/// pre-selected.
///
/// Callable from any `AppState`; the launcher (`ProjectSelect`)
/// and the editor's **File -> New Project** menu both invoke this.
/// The modal is a full-window overlay so it renders regardless of
/// which camera is active.
pub fn open_new_project_modal(world: &mut World, kind: TemplateKind) {
    close_new_project_modal(world);

    let location = project::read_last_new_project_location()
        .filter(|p| p.is_dir())
        .unwrap_or_else(default_projects_dir);
    {
        let mut state = world.resource_mut::<NewProjectState>();
        state.kind = Some(kind);
        state.location = location.clone();
        state.status = None;
    }

    let editor_font = world.resource::<EditorFont>().0.clone();
    let icon_font = world
        .resource::<jackdaw_feathers::icons::IconFont>()
        .0
        .clone();
    let (heading, name_placeholder) = match kind {
        TemplateKind::Extension => ("New Extension", "my_extension"),
        TemplateKind::Game => ("New Game", "my_game"),
    };
    // Match the modal heading icon to the sidebar action that opened it, so the
    // creation flow keeps a stable visual anchor.
    let heading_icon = match kind {
        TemplateKind::Extension => Icon::PackagePlus,
        TemplateKind::Game => Icon::Gamepad2,
    };

    // Full-window scrim that catches clicks behind the modal.
    let scrim = world
        .spawn((
            NewProjectModalRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            GlobalZIndex(100),
        ))
        .id();

    // Modal card root
    let card_root = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(10.0),
                padding: UiRect::all(Val::Px(18.0)),
                min_width: Val::Px(560.0),
                max_width: Val::Px(680.0),
                max_height: Val::Percent(90.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_LG)),
                overflow: Overflow::clip(),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(scrim),
        ))
        .id();
    // Inner scroll container.
    let card = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(24.0)),
                min_width: Val::Px(420.0),
                max_width: Val::Px(520.0),
                overflow: Overflow::scroll_y(),
                ..Default::default()
            },
            ScrollPosition::default(),
            bevy::picking::hover::Hovered::default(),
            ChildOf(card_root),
        ))
        .id();
    world.spawn((
        jackdaw_feathers::scroll::scrollbar(card),
        ChildOf(card_root),
    ));

    // Heading
    world.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(8.0),
            padding: UiRect::bottom(Val::Px(4.0)),
            ..Default::default()
        },
        children![
            (
                Text::new(String::from(heading_icon.unicode())),
                TextFont {
                    font: icon_font.clone().into(),
                    font_size: tokens::ICON_MD,
                    ..Default::default()
                },
                TextColor(tokens::DOC_TAB_TOOL_ACCENT),
            ),
            (
                Text::new(heading.to_string()),
                TextFont {
                    font: editor_font.clone().into(),
                    font_size: tokens::TEXT_SIZE_XS,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            ),
        ],
        ChildOf(card),
    ));

    // Name field
    world.spawn((
        Text::new("Name"),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));
    world.spawn((
        NewProjectNameInput,
        ChildOf(card),
        text_edit(
            TextEditProps::default()
                .with_placeholder(name_placeholder.to_string())
                .with_default_value(name_placeholder.to_string())
                .auto_focus(),
        ),
    ));

    // Location field
    world.spawn((
        Text::new("Location"),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));
    let location_row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();
    world.spawn((
        NewProjectLocationText,
        Text::new(location.to_string_lossy().into_owned()),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        Node {
            flex_grow: 1.0,
            ..Default::default()
        },
        ChildOf(location_row),
    ));
    let browse = world
        .spawn((
            NewProjectBrowseButton,
            Node {
                padding: UiRect::axes(Val::Px(12.0), Val::Px(6.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::TOOLBAR_BG),
            children![(
                Text::new("Browse..."),
                TextFont {
                    font: editor_font.clone().into(),
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
            ChildOf(location_row),
        ))
        .id();
    world.entity_mut(browse).observe(on_browse_new_location);

    spawn_reset_location_button(world, location_row, &editor_font, &location);

    // Status line
    world.spawn((
        NewProjectStatusText,
        Text::new(String::new()),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    // Build-progress UI (hidden until a build is in flight).
    let progress_container = world
        .spawn((
            NewProjectProgressContainer,
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                margin: UiRect::top(Val::Px(8.0)),
                display: Display::None,
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();

    // "Compiling <crate>" label.
    world.spawn((
        NewProjectProgressCrateLabel,
        Text::new(String::new()),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(progress_container),
    ));

    // Progress bar slot wrapping the `progress_bar` widget.
    let bar_slot = world
        .spawn((
            NewProjectProgressBarSlot,
            Node {
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(progress_container),
        ))
        .id();
    world.spawn((
        jackdaw_feathers::progress::progress_bar(0.0),
        ChildOf(bar_slot),
    ));

    // Log tail; fixed-height scrollable-ish (we don't enable real
    // scrolling; text wraps naturally and oldest lines age out via
    // the 20-line ring buffer).
    world.spawn((
        NewProjectLogText,
        Text::new(String::new()),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            max_height: Val::Px(220.0),
            overflow: Overflow::clip(),
            ..Default::default()
        },
        ChildOf(progress_container),
    ));

    // Action buttons
    let actions = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::FlexEnd,
                column_gap: Val::Px(8.0),
                margin: UiRect::top(Val::Px(8.0)),
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();

    let cancel = world
        .spawn((
            NewProjectCancelButton,
            Node {
                padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::TOOLBAR_BG),
            children![(
                NewProjectCancelButtonLabel,
                Text::new("Back"),
                TextFont {
                    font: editor_font.clone().into(),
                    font_size: tokens::TEXT_SIZE,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
            ChildOf(actions),
        ))
        .id();
    world.entity_mut(cancel).observe(on_cancel_new_project);

    let create = world
        .spawn((
            NewProjectCreateButton,
            Node {
                padding: UiRect::axes(Val::Px(20.0), Val::Px(8.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::SELECTED_BG),
            children![(
                Text::new("Create"),
                TextFont {
                    font: editor_font.into(),
                    font_size: tokens::TEXT_SIZE,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
            ChildOf(actions),
        ))
        .id();
    world.entity_mut(create).observe(on_create_new_project);
}

fn on_cancel_new_project(_: On<Pointer<Click>>, mut commands: Commands) {
    commands.queue(|world: &mut World| {
        let mut state = world.resource_mut::<NewProjectState>();
        if let Some(cancel) = state.build_cancel.as_ref() {
            cancel.store(true, Ordering::Release);
            state.status = Some("Cancelling build...".into());
        } else {
            close_new_project_modal(world);
        }
    });
}

fn on_browse_new_location(
    _: On<Pointer<Click>>,
    mut commands: Commands,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) {
    let mut dialog = AsyncFileDialog::new().set_title("Choose parent directory");
    if let Ok(rh) = raw_handle.single() {
        // SAFETY: called on the main thread during an observer.
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_folder().await });
    commands.queue(move |world: &mut World| {
        world.resource_mut::<NewProjectState>().folder_task = Some(task);
    });
}

fn spawn_reset_location_button(
    world: &mut World,
    location_row: Entity,
    editor_font: &Handle<Font>,
    location: &Path,
) {
    let hidden = location == default_projects_dir();
    let reset = world
        .spawn((
            NewProjectResetLocationButton,
            Node {
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                display: if hidden { Display::None } else { Display::Flex },
                ..Default::default()
            },
            BackgroundColor(Color::NONE),
            children![(
                Text::new("Reset"),
                TextFont {
                    font: editor_font.clone().into(),
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            )],
            ChildOf(location_row),
        ))
        .id();
    world.entity_mut(reset).observe(on_reset_new_location);
}

fn on_reset_new_location(_: On<Pointer<Click>>, mut commands: Commands) {
    commands.queue(|world: &mut World| {
        let default_project_location = default_projects_dir();
        world.resource_mut::<NewProjectState>().location = default_project_location;
    });
}

fn on_create_new_project(
    _: On<Pointer<Click>>,
    mut commands: Commands,
    name_inputs: Query<Entity, With<NewProjectNameInput>>,
    text_edit_values: Query<&TextEditValue>,
) {
    let Some(name_entity) = name_inputs.iter().next() else {
        return;
    };
    let raw_name = text_edit_values
        .get(name_entity)
        .map(|v| v.0.trim().to_string())
        .unwrap_or_default();

    commands.queue(move |world: &mut World| {
        let (location, kind) = {
            let state = world.resource::<NewProjectState>();
            let Some(kind) = state.kind else {
                return;
            };
            if state.scaffold_task.is_some() {
                return; // already running
            }
            (state.location.clone(), kind)
        };

        let name = crate::scaffold::sanitize_project_name(&raw_name);
        if name.is_empty() {
            world.resource_mut::<NewProjectState>().status =
                Some("Please enter a project name.".into());
            return;
        }

        {
            let mut state = world.resource_mut::<NewProjectState>();
            state.status = Some(format!("Creating `{name}`..."));
        }

        let task = AsyncComputeTaskPool::get()
            .spawn(async move { scaffold_project(&name, &location, kind) });
        world.resource_mut::<NewProjectState>().scaffold_task = Some(task);
    });
}

fn poll_new_project_tasks(
    mut commands: Commands,
    mut state: ResMut<NewProjectState>,
    mut location_texts: Query<
        &mut Text,
        (
            With<NewProjectLocationText>,
            Without<NewProjectCancelButtonLabel>,
        ),
    >,
    mut status_texts: Query<
        &mut Text,
        (
            With<NewProjectStatusText>,
            Without<NewProjectLocationText>,
            Without<NewProjectCancelButtonLabel>,
        ),
    >,
    mut reset_buttons: Query<&mut Node, With<NewProjectResetLocationButton>>,
    mut cancel_labels: Query<&mut Text, With<NewProjectCancelButtonLabel>>,
) {
    // Folder picker.
    if let Some(task) = state.folder_task.as_mut()
        && let Some(result) = future::block_on(future::poll_once(task))
    {
        state.folder_task = None;
        if let Some(handle) = result {
            state.location = handle.path().to_path_buf();
        }
    }

    let scaffold_result = state
        .scaffold_task
        .as_mut()
        .and_then(|task| future::block_on(future::poll_once(task)));
    if let Some(result) = scaffold_result {
        state.scaffold_task = None;
        match result {
            Ok(project_path) => {
                info!("Scaffolded project at {}", project_path.display());
                project::save_last_new_project_location(&state.location);
                state.status = None;
                // Open through the normal open path. The modal is
                // closed first so the open path spawns its own
                // progress modal if the project needs a build.
                commands.queue(move |world: &mut World| {
                    close_new_project_modal(world);
                    enter_project(world, project_path);
                });
            }
            Err(err) => {
                warn!("Scaffold failed: {err}");
                state.status = Some(format!("Create failed: {err}"));
            }
        }
    }

    // Build task completed: stash the artifact for the
    // install-in-Last step.
    if let Some(task) = state.build_task.as_mut()
        && let Some(result) = future::block_on(future::poll_once(task))
    {
        state.build_task = None;
        state.build_cancel = None;
        match result {
            Ok(artifact) => {
                info!("Build produced {}", artifact.display());
                state.pending_install = Some(artifact);
            }
            Err(crate::ext_build::BuildError::Cancelled { .. }) => {
                info!("Build cancelled");
                state.status = Some("Cancelled.".into());
                state.pending_project = None;
                state.build_progress = None;
                state.build_progress_snapshot = None;
            }
            Err(err) => {
                warn!("Build failed: {err}");
                state.status = Some(format!(
                    "Build failed: {err}.\n\
                         Fix the issue and try opening the project again."
                ));
                state.pending_project = None;
            }
        }
    }

    // Sync UI.
    let desired_location = state.location.to_string_lossy().into_owned();
    for mut text in location_texts.iter_mut() {
        if text.0 != desired_location {
            text.0 = desired_location.clone();
        }
    }
    let reset_display = if state.location == default_projects_dir() {
        Display::None
    } else {
        Display::Flex
    };
    for mut node in reset_buttons.iter_mut() {
        if node.display != reset_display {
            node.display = reset_display;
        }
    }
    let desired_status = state.status.as_deref().unwrap_or("").to_string();
    for mut text in status_texts.iter_mut() {
        if text.0 != desired_status {
            text.0 = desired_status.clone();
        }
    }
    let desired_cancel = if state.scaffold_task.is_some() || state.build_cancel.is_some() {
        "Cancel"
    } else {
        "Back"
    };
    for mut text in cancel_labels.iter_mut() {
        if text.0 != desired_cancel {
            text.0 = desired_cancel.to_string();
        }
    }
}

/// Copy the shared `BuildProgress` into the per-frame snapshot so
/// the UI-refresh system can read from a plain struct without
/// holding the mutex across rendering.
fn refresh_build_progress_snapshot(mut state: ResMut<NewProjectState>) {
    let Some(ref arc) = state.build_progress else {
        return;
    };
    let snap = {
        let Ok(guard) = arc.lock() else {
            return;
        };
        guard.clone()
    };
    state.build_progress_snapshot = Some(snap);
}

/// Reflect the current snapshot into the modal's progress UI:
/// toggles the container, updates the "compiling `<crate>`" label,
/// scrubs the progress-bar fill, and sets the log-tail text.
fn refresh_build_progress_ui(
    state: Res<NewProjectState>,
    mut containers: Query<&mut Node, With<NewProjectProgressContainer>>,
    mut crate_labels: Query<
        &mut Text,
        (
            With<NewProjectProgressCrateLabel>,
            Without<NewProjectLogText>,
        ),
    >,
    mut log_texts: Query<
        &mut Text,
        (
            With<NewProjectLogText>,
            Without<NewProjectProgressCrateLabel>,
        ),
    >,
    bar_slots: Query<&Children, With<NewProjectProgressBarSlot>>,
    children_q: Query<&Children>,
    mut fill_q: Query<
        &mut Node,
        (
            With<jackdaw_feathers::progress::ProgressBarFill>,
            Without<NewProjectProgressContainer>,
        ),
    >,
) {
    let snapshot = state.build_progress_snapshot.as_ref();

    // Toggle container visibility based on whether a build is active.
    let show = snapshot.is_some();
    for mut node in containers.iter_mut() {
        let desired = if show { Display::Flex } else { Display::None };
        if node.display != desired {
            node.display = desired;
        }
    }

    let Some(progress) = snapshot else {
        return;
    };

    // "Compiling <crate>" or "Preparing..." if we don't know yet.
    let crate_line = match (&progress.current_crate, progress.artifacts_total) {
        (Some(name), Some(total)) => {
            // The estimate (package count) undershoots the real artifact count
            // (proc-macros, build scripts), so clamp to never display past 100%.
            let total = total.max(progress.artifacts_done);
            format!("Compiling {name} ({}/{})", progress.artifacts_done, total)
        }
        (Some(name), None) => format!("Compiling {name} ({} so far)", progress.artifacts_done),
        (None, Some(total)) => format!("Preparing build... (0/{total})"),
        (None, None) => "Preparing build...".to_string(),
    };
    for mut t in crate_labels.iter_mut() {
        if t.0 != crate_line {
            t.0 = crate_line.clone();
        }
    }

    // Progress bar fill; walk slot -> bar -> bar children -> fill.
    let fraction = progress.fraction().unwrap_or(0.0).clamp(0.0, 1.0);
    let desired_width = Val::Percent(fraction * 100.0);
    for bar_children in bar_slots.iter() {
        for bar_entity in bar_children.iter() {
            let Ok(inner) = children_q.get(bar_entity) else {
                continue;
            };
            for fill_entity in inner.iter() {
                if let Ok(mut node) = fill_q.get_mut(fill_entity)
                    && node.width != desired_width
                {
                    node.width = desired_width;
                }
            }
        }
    }

    // Log tail.
    let mut joined = String::new();
    for (i, line) in progress.recent_log_lines.iter().enumerate() {
        if i > 0 {
            joined.push('\n');
        }
        joined.push_str(line);
    }
    for mut t in log_texts.iter_mut() {
        if t.0 != joined {
            t.0 = joined.clone();
        }
    }
}

/// Install a freshly-built game/extension dylib, running in the
/// `Last` schedule so `GameApp::add_systems(Update, ...)` inside the
/// game's build function mutates `Update` while nobody holds it in
/// `schedule_scope`. See the plugin-registration block at the top
/// of this file for context.
///
/// On success: closes the modal and transitions to the editor.
/// On `LoadError::SymbolMismatch`: closes the modal and opens an info
/// dialog with instructions to run `cargo clean -p <name> && cargo
/// build` from the project directory.
/// On any other error: closes the modal and opens an info dialog
/// with the error message.
fn apply_pending_install(world: &mut World) {
    let artifact_opt = world
        .resource_mut::<NewProjectState>()
        .pending_install
        .take();
    let Some(artifact) = artifact_opt else {
        return;
    };

    let result = crate::extensions_dialog::handle_install_from_path(world, artifact);

    match result {
        Ok(_) => {
            let project = world
                .resource_mut::<NewProjectState>()
                .pending_project
                .clone();
            close_new_project_modal(world);
            if let Some(p) = project {
                transition_to_editor(world, p);
            }
        }
        Err(ref err) if err.is_symbol_mismatch() => {
            let project_name = world
                .resource::<NewProjectState>()
                .pending_project
                .as_deref()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("project")
                .to_owned();
            warn!("Install failed (SDK mismatch) for `{project_name}`");
            close_new_project_modal(world);
            world.trigger(
                jackdaw_feathers::dialog::OpenDialogEvent::new("SDK mismatch", "OK")
                    .with_description(format!(
                        "Project `{project_name}` was built against a different jackdaw \
                         SDK build. Run this from the project directory to refresh:\n\n\
                         \tcargo clean -p {project_name}\n\
                         \tcargo build\n\n\
                         Then re-open the project."
                    ))
                    .without_cancel(),
            );
        }
        Err(err) => {
            warn!("Install failed: {err}");
            close_new_project_modal(world);
            world.trigger(
                jackdaw_feathers::dialog::OpenDialogEvent::new("Install failed", "OK")
                    .with_description(format!("{err}"))
                    .without_cancel(),
            );
        }
    }
}
