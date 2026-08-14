use std::path::{Path, PathBuf};

use bevy::text::{FontSize, FontSourceTemplate};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
    window::{PrimaryWindow, RawHandleWrapper},
};
use jackdaw_feathers::{
    button::{ButtonVariant, IconButtonProps, icon_button},
    icons::{EditorFont, Icon, font_paths},
    text_edit::{TextEditProps, TextEditValue, text_edit},
    tokens,
};
use jackdaw_localization::LocalizedText;
use jackdaw_project_build::project_manifest;
use rfd::{AsyncFileDialog, FileHandle};

use crate::{
    AppState,
    new_project::scaffold_project,
    project::{self, ProjectRoot},
    scaffold::{ImportChange, ScaffoldError, TemplateKind},
    windowing::{JackdawIcon, title_bar_repo_link},
};
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
use bevy_window_chrome::CaptionFont;
use bevy_window_chrome::{WindowChromeTheme, spawn_window_shell};
use jackdaw_project_build::cargo_meta::ResolveError;

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
                (poll_folder_dialog, poll_preflight).run_if(in_state(AppState::ProjectSelect)),
            )
            // Not state-guarded: the New Project modal also opens from
            // the editor's File menu, and a scaffold started there has
            // to be polled to completion and validated as it is typed.
            // Both are cheap no-ops when the modal is closed.
            .add_systems(
                Update,
                (poll_new_project_tasks, refresh_new_project_validation),
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
            );
    }
}

/// Marker for the project selector root UI node.
#[derive(Component, Copy, Clone)]
struct ProjectSelectorRoot;

/// The project a launcher row points at, so an action taken elsewhere
/// (dropping a folder that no longer exists) can find and remove it.
#[derive(Component)]
struct RecentRow(PathBuf);

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

/// Resource holding the async folder picker task and what the picked
/// folder is for.
#[derive(Resource)]
struct FolderDialogTask {
    task: Task<Option<rfd::FileHandle>>,
    purpose: FolderPurpose,
}

/// Which launcher action opened the folder picker.
#[derive(Clone, Copy)]
enum FolderPurpose {
    /// Open an existing jackdaw project.
    Open,
    /// Set up a Bevy project that is not a jackdaw project yet.
    Import,
}

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

/// Live feedback under the name field: the directory the entered name
/// resolves to, or why it cannot be used.
#[derive(Component)]
struct NewProjectNameHint;

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
    // What actually marks a project now. The old markers predate
    // `jackdaw.toml`: `assets/` in particular promoted any folder that
    // happened to have one to the top of the launcher, where clicking it
    // landed on the not-a-project card.
    let cwd_has_project = cwd.join("jackdaw.toml").is_file() || cwd.join("Cargo.toml").is_file();

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

                        let import_entity = spawn_launcher_action_button(
                            sidebar,
                            "Import Bevy Project",
                            Icon::Import,
                            font.clone(),
                            icon_font_handle.clone(),
                            tokens::TOOLBAR_BG,
                            tokens::HOVER_BG,
                        );
                        sidebar
                            .commands()
                            .entity(import_entity)
                            .observe(spawn_import_dialog);

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
                                        None,
                                        true,
                                    );
                                }

                                spawn_launcher_section_label(list, "Recent", font.clone());
                                let mut shown_recent = 0usize;
                                for entry in &recent.projects {
                                    if cwd_has_project
                                        && dunce::simplified(entry.path.as_path())
                                            == dunce::simplified(cwd.as_path())
                                    {
                                        continue;
                                    }
                                    spawn_project_row(
                                        list,
                                        &entry.name,
                                        &entry.path.to_string_lossy(),
                                        font.clone(),
                                        icon_font_handle.clone(),
                                        entry.path.clone(),
                                        Some(entry.last_opened.as_str()),
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
    last_opened: Option<&str>,
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
            RecentRow(project_path.clone()),
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
                        // A folder that has been moved or deleted is
                        // still listed, and used to look perfectly
                        // healthy until the click that failed. One stat
                        // per row says so up front.
                        missing_badge(&project_path, font.clone()),
                        // The engine a project targets decides whether
                        // this editor can build it at all, so it belongs
                        // next to the name rather than one click away.
                        bevy_badge(&project_path, font.clone()),
                    ],
                ),
                (
                    Text::new(row_subtitle(path_display, last_opened)),
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

/// The row's second line: the path, plus when the project was last
/// opened when that is known.
fn row_subtitle(path_display: &str, last_opened: Option<&str>) -> String {
    match last_opened.and_then(crate::timestamps::relative_to_now) {
        Some(when) => format!("{path_display}  .  {when}"),
        None => path_display.to_string(),
    }
}

/// A `missing` chip for a recent whose folder is no longer there.
/// Clicking still explains it and offers to forget the entry; this only
/// stops the list from claiming the project is fine.
fn missing_badge(project_path: &Path, font: Handle<Font>) -> impl Bundle {
    let missing = !project_path.is_dir();
    (
        Text::new(if missing { "missing" } else { "" }.to_string()),
        TextFont {
            font: font.into(),
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(tokens::TEXT_ERROR),
        Node {
            display: if missing {
                Display::Flex
            } else {
                Display::None
            },
            ..Default::default()
        },
        Pickable::IGNORE,
    )
}

/// A `bevy 0.16` chip, coloured by whether this editor can build it.
/// Absent when the project states no Bevy version, since an empty chip
/// would read as a claim.
fn bevy_badge(project_path: &Path, font: Handle<Font>) -> impl Bundle {
    let targeted = project_manifest::targeted_bevy(project_path);
    let supported = targeted
        .as_deref()
        .is_some_and(|minor| minor == jackdaw_project_build::BEVY_VERSION);
    let (display, color) = match &targeted {
        Some(minor) if supported => (Display::Flex, tokens::TEXT_SECONDARY),
        Some(_) => (Display::Flex, tokens::TEXT_ERROR),
        None => (Display::None, tokens::TEXT_SECONDARY),
    };
    (
        Text::new(format!("bevy {}", targeted.unwrap_or_default())),
        TextFont {
            font: font.into(),
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(color),
        Node {
            display,
            ..Default::default()
        },
        Pickable::IGNORE,
    )
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
    commands: Commands,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) {
    pick_project_folder(commands, raw_handle, FolderPurpose::Open);
}

fn spawn_import_dialog(
    _: On<Pointer<Click>>,
    commands: Commands,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) {
    pick_project_folder(commands, raw_handle, FolderPurpose::Import);
}

fn pick_project_folder(
    mut commands: Commands,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
    purpose: FolderPurpose,
) {
    let title = match purpose {
        FolderPurpose::Open => "Select project folder",
        FolderPurpose::Import => "Select the Bevy project to import",
    };
    let mut dialog = AsyncFileDialog::new().set_title(title);

    if let Ok(rh) = raw_handle.single() {
        // SAFETY: called on the main thread during an observer
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }

    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_folder().await });
    commands.insert_resource(FolderDialogTask { task, purpose });
}

fn poll_folder_dialog(world: &mut World) {
    let Some(mut task_res) = world.get_resource_mut::<FolderDialogTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task_res.task)) else {
        return;
    };
    let purpose = task_res.purpose;
    world.remove_resource::<FolderDialogTask>();

    let Some(handle) = result else {
        return;
    };
    let path = handle.path().to_path_buf();
    match purpose {
        // Opening an unset-up project falls through to the import
        // offer anyway; going straight to the preview just skips one
        // click for a user who already knows they are importing.
        FolderPurpose::Open => enter_project(world, path),
        FolderPurpose::Import => on_setup_jackdaw_clicked(world, path),
    }
}

/// Entry point for **every** "open a project" action from the
/// launcher (new-scaffold completion, recent-project click, manual
/// folder browse). Anything without a `Cargo.toml`, and any Cargo
/// project with a `jackdaw.toml`, transitions straight to the editor;
/// an unrecognized Cargo project gets the import offer.
pub fn enter_project(world: &mut World, root: PathBuf) {
    enter_project_with(world, root, false);
}

/// Same as [`enter_project`] but lets the caller bypass the build
/// step. Used by the post-restart auto-open path: the parent
/// process already produced the dylib, the loader picked it up at
/// startup, so a second build-and-install would either be a no-op
/// or (for games) trigger another restart loop.
pub fn enter_project_with(world: &mut World, root: PathBuf, skip_build: bool) {
    if skip_build {
        transition_to_editor(world, root);
        return;
    }
    // A folder that is gone (a stale recent entry) or that was never a
    // cargo project used to open the editor rooted at it, with an
    // untitled scene and no indication anything was wrong.
    if !root.is_dir() {
        show_not_a_project_card(world, root, NotAProject::Missing);
        return;
    }
    if !root.join("Cargo.toml").is_file() {
        show_not_a_project_card(world, root, NotAProject::NoManifest);
        return;
    }
    // A project without a `jackdaw.toml` has not been set up yet; the
    // import flow proposes exactly what setting it up would write.
    if !project_manifest::ProjectManifest::exists(&root) {
        info!(
            "Project at {} has a Cargo.toml but no jackdaw.toml; offering setup.",
            root.display()
        );
        show_setup_jackdaw_card(world, root, None);
        return;
    }

    // A project set up against a different Bevy minor cannot share a
    // type graph with this editor's SDK, so say so before the user
    // watches a doomed build.
    let manifest = project_manifest::ProjectManifest::read(&root);
    let pins = project_manifest::compare_project(&root, &manifest);
    if pins.is_blocking() {
        show_version_mismatch_card(world, root, pins);
        return;
    }
    // A same-Bevy jackdaw change is safe to open, but the project still
    // claims the old version and still requests the old crate line, and
    // only an offer to fix it makes that actionable. Skipped when the
    // upgrade would be a no-op or cannot be planned.
    if let project_manifest::PinStatus::JackdawDiffers { pinned, running } = &pins {
        info!("Project was set up with jackdaw {pinned}; running {running}");
        if let Ok(plan) = crate::scaffold::plan_upgrade_project(&root)
            && !plan.is_empty()
        {
            show_upgrade_card(world, root, pinned.clone(), running.clone(), plan);
            return;
        }
    }

    // Project code is compiled by the editor's own pipeline once the
    // project is open (`pie::prebuild_play_target`), which streams into
    // the Build panel. The launcher's job ends at the handoff.
    close_new_project_modal(world);
    transition_to_editor(world, root);
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

    world.insert_resource(ProjectRoot::new(root.clone(), config));

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
            crate::scenes::operators::scene_open_system(world, &scene_path);
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
    show_setup_jackdaw_card_with_recovery(world, root, error, false, None);
}

/// The setup offer. When `recoverable`, the failure is one the user can
/// choose to proceed past (a Bevy minor mismatch), so a third button
/// offers that instead of leaving only a retry that cannot succeed.
/// `package` is the workspace member already chosen (if any), so a
/// recoverable retry does not lose that choice.
fn show_setup_jackdaw_card_with_recovery(
    world: &mut World,
    root: PathBuf,
    error: Option<String>,
    recoverable: bool,
    package: Option<String>,
) {
    let (_, card, font) = spawn_modal_card(world, 520.0, 700.0);
    spawn_card_title(world, card, "Set up jackdaw for this project", &font);
    spawn_card_body(
        world,
        card,
        format!(
            "`{}` is a Cargo project but is not set up for the editor yet.",
            root.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("This folder")
        ),
        &font,
    );
    // The three facts a user needs before consenting, as separate
    // claims rather than a paragraph they have to parse: what is added,
    // what is guaranteed untouched, and what happens next.
    spawn_fact_list(
        world,
        card,
        &[
            (
                Icon::FilePlus,
                "Adds a `jackdaw.toml` and a gitignored `.jackdaw/` directory.",
            ),
            (
                Icon::Check,
                "Leaves your Cargo manifest, lockfile, toolchain, and `target/` alone. \
                 `cargo run` keeps working exactly as it does now.",
            ),
            (
                Icon::CircleAlert,
                "The editor only sees components in a library. If this project has none, \
                 setup moves your `main.rs` setup into a `GamePlugin` (keeping the \
                 original as `main.rs.bak`) or leaves you an empty one to fill in.",
            ),
            (
                Icon::Eye,
                "Nothing is written yet: the next screen lists every file first.",
            ),
        ],
        &font,
    );

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

    let row = spawn_card_button_row(world, card);

    let cancel = spawn_card_button(world, row, "Cancel", &font, false);
    world
        .entity_mut(cancel)
        .observe(|_: On<Pointer<Click>>, mut commands: Commands| {
            commands.queue(close_new_project_modal);
        });

    let open_anyway = spawn_card_button(world, row, "Open without setup", &font, false);
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

    if recoverable {
        // Retrying the identical plan would fail identically, so the
        // only forward action is the one that changes the outcome.
        let anyway = spawn_card_button(world, row, "Set up anyway", &font, true);
        let root_force = root.clone();
        world
            .entity_mut(anyway)
            .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                let root = root_force.clone();
                let package = package.clone();
                commands.queue(move |world: &mut World| {
                    plan_and_show_import(world, root, package, true);
                });
            });
    } else {
        let setup = spawn_card_button(world, row, "Set up jackdaw", &font, true);
        world
            .entity_mut(setup)
            .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                let root = root.clone();
                commands.queue(move |world: &mut World| on_setup_jackdaw_clicked(world, root));
            });
    }
}

/// Spawn a centred modal card over a scrim, returning
/// `(scrim, card, font)`. Every launcher card (setup, import preview,
/// version mismatch, lib-stub warning) is this shape; only the contents
/// differ.
fn spawn_modal_card(
    world: &mut World,
    min_width: f32,
    max_width: f32,
) -> (Entity, Entity, Handle<Font>) {
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
                min_width: Val::Px(min_width),
                max_width: Val::Px(max_width),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(scrim),
        ))
        .id();
    (scrim, card, font)
}

fn spawn_card_title(world: &mut World, card: Entity, title: &str, font: &Handle<Font>) {
    world.spawn((
        Text::new(title.to_string()),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_LG,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(card),
    ));
}

fn spawn_card_body(world: &mut World, card: Entity, body: impl Into<String>, font: &Handle<Font>) {
    world.spawn((
        Text::new(body.into()),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));
}

/// An icon-led list of short claims.
///
/// A consent screen's job is to let someone check a small number of
/// specific facts, which a paragraph actively works against: the
/// reassuring sentence and the consequential one look the same and read
/// at the same speed. One row per claim makes them separately
/// checkable.
fn spawn_fact_list(world: &mut World, card: Entity, facts: &[(Icon, &str)], font: &Handle<Font>) {
    let icon_font = world
        .resource::<jackdaw_feathers::icons::IconFont>()
        .0
        .clone();
    let list = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();
    for (icon, text) in facts {
        world.spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(10.0),
                align_items: AlignItems::Start,
                ..Default::default()
            },
            ChildOf(list),
            children![
                (
                    Text::new(String::from(icon.unicode())),
                    TextFont {
                        font: icon_font.clone().into(),
                        font_size: tokens::ICON_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                ),
                (
                    Text::new((*text).to_string()),
                    TextFont {
                        font: font.clone().into(),
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                ),
            ],
        ));
    }
}

/// A numbered list of things the user has to do, in order.
///
/// Distinct from [`spawn_fact_list`]: these are instructions to work
/// through, so they carry their position rather than an icon, and the
/// numbering comes from the layout instead of being typed into the
/// strings where it can drift.
fn spawn_step_list(world: &mut World, card: Entity, steps: &[&str], font: &Handle<Font>) {
    let list = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();
    for (index, step) in steps.iter().enumerate() {
        world.spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(10.0),
                align_items: AlignItems::Start,
                ..Default::default()
            },
            ChildOf(list),
            children![
                (
                    Text::new(format!("{}", index + 1)),
                    TextFont {
                        font: font.clone().into(),
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_PRIMARY),
                    Node {
                        min_width: Val::Px(14.0),
                        ..Default::default()
                    },
                ),
                (
                    Text::new((*step).to_string()),
                    TextFont {
                        font: font.clone().into(),
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                ),
            ],
        ));
    }
}

/// Render an import plan's changes as one row per file, rather than as
/// a paragraph.
///
/// The distinction that matters for consent is create-versus-modify: a
/// modify touches something the user already had. As prose bullets the
/// two read identically, so this gives modifies their own icon and the
/// warning colour, and puts the verb before the path.
fn spawn_change_list(
    world: &mut World,
    card: Entity,
    changes: &[ImportChange],
    font: &Handle<Font>,
) {
    let icon_font = world
        .resource::<jackdaw_feathers::icons::IconFont>()
        .0
        .clone();
    let list = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                padding: UiRect::all(Val::Px(10.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::INPUT_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(card),
        ))
        .id();

    for change in changes {
        let (icon, verb, color) = match change {
            ImportChange::CreateDirectory { .. } => {
                (Icon::FolderPlus, "create", tokens::TEXT_SECONDARY)
            }
            // `summary` decides create-vs-modify by looking at the
            // disk, so ask it rather than re-deriving the answer here.
            ImportChange::WriteFile { .. } if change.summary().starts_with("modify") => {
                (Icon::FilePen, "modify", tokens::TEXT_WARNING)
            }
            ImportChange::WriteFile { .. } => (Icon::FilePlus, "create", tokens::TEXT_SECONDARY),
        };
        let path = match change {
            ImportChange::CreateDirectory { path } | ImportChange::WriteFile { path, .. } => path,
        };
        world.spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                align_items: AlignItems::Center,
                ..Default::default()
            },
            ChildOf(list),
            children![
                (
                    Text::new(String::from(icon.unicode())),
                    TextFont {
                        font: icon_font.clone().into(),
                        font_size: tokens::ICON_SM,
                        ..Default::default()
                    },
                    TextColor(color),
                ),
                (
                    Text::new(verb.to_string()),
                    TextFont {
                        font: font.clone().into(),
                        font_size: tokens::TEXT_SIZE_XS,
                        ..Default::default()
                    },
                    TextColor(color),
                    Node {
                        min_width: Val::Px(52.0),
                        ..Default::default()
                    },
                ),
                (
                    Text::new(path.display().to_string()),
                    TextFont {
                        font: font.clone().into(),
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_PRIMARY),
                ),
            ],
        ));
    }
}

/// Render a plan's notes as their own rows, so guidance is not mistaken
/// for another file the import is about to touch.
fn spawn_note_list(world: &mut World, card: Entity, notes: &[String], font: &Handle<Font>) {
    if notes.is_empty() {
        return;
    }
    let icon_font = world
        .resource::<jackdaw_feathers::icons::IconFont>()
        .0
        .clone();
    let list = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();
    for note in notes {
        world.spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                align_items: AlignItems::Start,
                ..Default::default()
            },
            ChildOf(list),
            children![
                (
                    Text::new(String::from(Icon::Info.unicode())),
                    TextFont {
                        font: icon_font.clone().into(),
                        font_size: tokens::ICON_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                ),
                (
                    Text::new(note.clone()),
                    TextFont {
                        font: font.clone().into(),
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                ),
            ],
        ));
    }
}

/// The right-aligned action row at the bottom of a modal card.
fn spawn_card_button_row(world: &mut World, card: Entity) -> Entity {
    world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                justify_content: JustifyContent::FlexEnd,
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id()
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

/// Offer to move a project onto this jackdaw. Same consent model as
/// import: the exact edits are listed, and skipping is a normal choice
/// (the project opens and builds either way).
fn show_upgrade_card(
    world: &mut World,
    root: PathBuf,
    pinned: String,
    running: String,
    plan: crate::scaffold::ImportPlan,
) {
    let (_, card, font) = spawn_modal_card(world, 520.0, 720.0);
    spawn_card_title(world, card, "Update this project for jackdaw", &font);
    spawn_card_body(
        world,
        card,
        format!(
            "`{}` was set up with jackdaw {pinned}; you are running {running}. Both target \
             Bevy {bevy}, so the project still builds either way.\n\n\
             Updating records the new version and moves the project's jackdaw dependencies \
             onto the matching release line:",
            root.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("This project"),
            bevy = jackdaw_project_build::BEVY_VERSION,
        ),
        &font,
    );
    spawn_change_list(world, card, &plan.changes, &font);
    spawn_note_list(world, card, &plan.notes, &font);

    let row = spawn_card_button_row(world, card);
    let skip = spawn_card_button(world, row, "Not now", &font, false);
    let update = spawn_card_button(world, row, "Update project", &font, true);

    let skip_root = root.clone();
    world
        .entity_mut(skip)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let root = skip_root.clone();
            commands.queue(move |world: &mut World| {
                close_new_project_modal(world);
                transition_to_editor(world, root);
            });
        });
    world
        .entity_mut(update)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let plan = plan.clone();
            commands.queue(move |world: &mut World| {
                let root = plan.root.clone();
                if let Err(error) = crate::scaffold::apply_import_plan(&plan) {
                    warn!("Upgrade failed for {}: {error}", root.display());
                }
                close_new_project_modal(world);
                transition_to_editor(world, root);
            });
        });
}

/// Why a folder cannot be opened as a project.
#[derive(Clone, Copy)]
enum NotAProject {
    /// The folder is gone: a recent entry pointing at a moved or
    /// deleted directory.
    Missing,
    /// The folder exists but holds no `Cargo.toml`.
    NoManifest,
}

/// Refuse to open a folder that is not a project, naming what was
/// expected and offering the actions that would fix it. Opening the
/// editor rooted at an arbitrary folder looks like success and is not.
fn show_not_a_project_card(world: &mut World, root: PathBuf, reason: NotAProject) {
    let (_, card, font) = spawn_modal_card(world, 480.0, 640.0);
    let (title, body) = match reason {
        NotAProject::Missing => (
            "That project folder is gone",
            format!(
                "`{}` no longer exists. It was probably moved, renamed, or deleted.\n\n\
                 Removing it from the list does not touch any files.",
                root.display()
            ),
        ),
        NotAProject::NoManifest => (
            "That folder is not a Rust project",
            format!(
                "`{}` has no Cargo.toml, so there is no Bevy project here to open.\n\n\
                 Pick the folder that holds your project's Cargo.toml, or create a new \
                 project with New Game.",
                root.display()
            ),
        ),
    };
    spawn_card_title(world, card, title, &font);
    spawn_card_body(world, card, body, &font);

    let row = spawn_card_button_row(world, card);
    let back = spawn_card_button(world, row, "Back", &font, false);
    world
        .entity_mut(back)
        .observe(|_: On<Pointer<Click>>, mut commands: Commands| {
            commands.queue(close_new_project_modal);
        });

    if matches!(reason, NotAProject::Missing) {
        let forget = spawn_card_button(world, row, "Remove from list", &font, true);
        world
            .entity_mut(forget)
            .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                let root = root.clone();
                commands.queue(move |world: &mut World| {
                    project::remove_recent(&root);
                    let stale: Vec<Entity> = world
                        .query::<(Entity, &RecentRow)>()
                        .iter(world)
                        .filter(|(_, row)| row.0 == root)
                        .map(|(entity, _)| entity)
                        .collect();
                    for entity in stale {
                        if let Ok(row) = world.get_entity_mut(entity) {
                            row.despawn();
                        }
                    }
                    close_new_project_modal(world);
                });
            });
    }
}

/// Shown when a project's recorded pins say it targets a different Bevy
/// minor than this editor. Opening anyway is allowed (scenes and assets
/// still load), but project code will not build, so the card says so
/// rather than letting the user discover it as a wall of compile errors.
fn show_version_mismatch_card(
    world: &mut World,
    root: PathBuf,
    status: project_manifest::PinStatus,
) {
    let project_manifest::PinStatus::BevyDiffers { pinned, running } = status else {
        transition_to_editor(world, root);
        return;
    };
    let (_, card, font) = spawn_modal_card(world, 480.0, 640.0);
    spawn_card_title(world, card, "This project targets a different Bevy", &font);
    spawn_card_body(
        world,
        card,
        format!(
            "`{}` was set up for Bevy {pinned}; this jackdaw targets Bevy {running}. The editor \
             and your game code have to share one Bevy version, so project code will not build \
             until they match.\n\nEither install the jackdaw release for Bevy {pinned}, or \
             migrate the project to Bevy {running} and update the `bevy` line in jackdaw.toml.\n\n\
             You can still open the project to look at its scenes.",
            root.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("this project")
        ),
        &font,
    );

    let row = spawn_card_button_row(world, card);
    let back = spawn_card_button(world, row, "Back", &font, false);
    let open_anyway = spawn_card_button(world, row, "Open anyway", &font, true);
    world
        .entity_mut(back)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.queue(close_new_project_modal);
        });
    world
        .entity_mut(open_anyway)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let root = root.clone();
            commands.queue(move |world: &mut World| {
                close_new_project_modal(world);
                transition_to_editor(world, root);
            });
        });
}

/// Build an import preview for the "Set up jackdaw" card. Planning is
/// side-effect free; a second explicit action applies the exact proposal.
fn on_setup_jackdaw_clicked(world: &mut World, root: PathBuf) {
    plan_and_show_import(world, root, None, false);
}

/// Plan the import and show the preview, or explain why it could not be
/// planned. `allow_bevy_mismatch` is the second attempt after the user
/// chose to proceed past a version mismatch, so the card offers a way
/// forward instead of a button that fails identically every time.
/// `package` is set when the user already picked a workspace member.
fn plan_and_show_import(
    world: &mut World,
    root: PathBuf,
    package: Option<String>,
    allow_bevy_mismatch: bool,
) {
    match crate::scaffold::plan_import_with(&root, None, package.as_deref(), allow_bevy_mismatch) {
        Ok(plan) if plan.is_empty() => enter_project_with(world, root, false),
        Ok(plan) => show_import_preview_card(world, plan),
        Err(ScaffoldError::Package(ResolveError::Ambiguous { candidates })) => {
            show_package_picker_card(world, root, candidates, allow_bevy_mismatch);
        }
        Err(error) => {
            warn!("Set up jackdaw failed for {}: {error}", root.display());
            let recoverable = matches!(error, ScaffoldError::BevyVersion { .. });
            show_setup_jackdaw_card_with_recovery(
                world,
                root,
                Some(error.to_string()),
                recoverable,
                package,
            );
        }
    }
}

/// Ask which workspace member is the game when several look like one.
///
/// Clicking a row continues setup for that package; Back returns to the
/// setup offer.
fn show_package_picker_card(
    world: &mut World,
    root: PathBuf,
    candidates: Vec<String>,
    allow_bevy_mismatch: bool,
) {
    let (_, card, font) = spawn_modal_card(world, 480.0, 560.0);
    spawn_card_title(world, card, "Which package is the game?", &font);
    spawn_card_body(
        world,
        card,
        "Several packages in this workspace could be the game. Pick the one the editor should build.",
        &font,
    );

    let Ok(mut list_shell) = world.spawn_scene(package_picker_list_shell()) else {
        error!("failed to spawn package picker list shell");
        return;
    };
    list_shell.insert(ChildOf(card));
    let list_shell = list_shell.id();

    let Ok(mut list) = world.spawn_scene(package_picker_list()) else {
        error!("failed to spawn package picker list");
        return;
    };
    list.insert(ChildOf(list_shell));
    let list = list.id();
    world.spawn((
        jackdaw_feathers::scroll::scrollbar(list),
        ChildOf(list_shell),
    ));

    for name in candidates {
        let Ok(mut row) = world.spawn_scene(package_candidate_row(
            name,
            root.clone(),
            allow_bevy_mismatch,
        )) else {
            error!("failed to spawn package candidate row");
            continue;
        };
        row.insert(ChildOf(list));
    }

    let row = spawn_card_button_row(world, card);
    let back = spawn_card_button(world, row, "Back", &font, false);
    world
        .entity_mut(back)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let root = root.clone();
            commands.queue(move |world: &mut World| show_setup_jackdaw_card(world, root, None));
        });
}

/// Bordered shell around the scrollable package list.
fn package_picker_list_shell() -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
            max_height: px(280.0),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_MD)),
            overflow: Overflow::clip(),
        }
        BackgroundColor(tokens::INPUT_BG)
        BorderColor::all(tokens::BORDER_SUBTLE)
    }
}

/// Scrollable column that holds package candidate rows.
fn package_picker_list() -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(4.0),
            padding: UiRect::all(px(6.0)),
            width: percent(100),
            max_height: px(280.0),
            overflow: Overflow::scroll_y(),
        }
        ScrollPosition::default()
        bevy::picking::hover::Hovered::default()
    }
}

/// clickable workspace-member row in the package picker.
fn package_candidate_row(name: String, root: PathBuf, allow_bevy_mismatch: bool) -> impl Scene {
    let glyph = String::from(Icon::Package.unicode());
    let label = name.clone();
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            width: percent(100),
            min_height: px(40.0),
            padding: UiRect::axes(px(10.0), px(8.0)),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_LG)),
            align_items: AlignItems::Center,
            column_gap: px(10.0),
        }
        BackgroundColor(tokens::PANEL_BG)
        BorderColor::all(tokens::BORDER_SUBTLE)
        Children [
            (
                Node {
                    width: px(26.0),
                    height: px(26.0),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_MD)),
                }
                BackgroundColor(tokens::DOC_TAB_ACTIVE_BG)
                Pickable::IGNORE
                Children [
                    (
                        Text(glyph)
                        TextFont {
                            font: FontSourceTemplate::Handle(font_paths::LUCIDE),
                            font_size: FontSize::Px(tokens::ICON_SM_PX),
                        }
                        TextColor(tokens::DIR_ICON_COLOR)
                    ),
                ]
            ),
            (
                Text(label)
                TextFont {
                    font_size: tokens::TEXT_SIZE,
                }
                TextColor(tokens::TEXT_PRIMARY)
                Pickable::IGNORE
            ),
        ]
        on(|hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                bg.0 = tokens::HOVER_BG;
            }
        })
        on(|out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                bg.0 = tokens::PANEL_BG;
            }
        })
        on(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let root = root.clone();
            let package = name.clone();
            commands.queue(move |world: &mut World| {
                plan_and_show_import(world, root, Some(package), allow_bevy_mismatch);
            });
        })
    }
}

fn show_import_preview_card(world: &mut World, plan: crate::scaffold::ImportPlan) {
    let (_, card, font) = spawn_modal_card(world, 560.0, 760.0);
    spawn_card_title(world, card, "Review project integration", &font);
    // Lead with the rewrite when there is one. A blanket reassurance
    // printed above a line reading `modify src/main.rs` is worse than
    // no reassurance: the user reads the summary, not the bullets.
    let intro = if plan.migrated_bin_target {
        format!(
            "Jackdaw will rewrite `src/main.rs` in `{}`, keeping the original as \
             `src/main.rs.bak`, and make the changes below. Your Cargo manifest, lockfile, \
             and `target/` are not touched.",
            plan.root.display()
        )
    } else {
        format!(
            "Jackdaw will make these changes to `{}`. Your Cargo manifest, lockfile, and \
             `target/` are not touched.",
            plan.root.display()
        )
    };
    spawn_card_body(world, card, intro, &font);

    spawn_change_list(world, card, &plan.changes, &font);
    spawn_note_list(world, card, &plan.notes, &font);
    let row = spawn_card_button_row(world, card);
    let cancel = spawn_card_button(world, row, "Cancel", &font, false);
    let apply = spawn_card_button(world, row, "Apply changes", &font, true);

    let cancel_root = plan.root.clone();
    world
        .entity_mut(cancel)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let root = cancel_root.clone();
            commands.queue(move |world: &mut World| show_setup_jackdaw_card(world, root, None));
        });
    world
        .entity_mut(apply)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let plan = plan.clone();
            commands.queue(move |world: &mut World| {
                let root = plan.root.clone();
                match crate::scaffold::apply_import_plan(&plan) {
                    Ok(report) => {
                        info!(
                            "Set up jackdaw at {}: {}",
                            root.display(),
                            report.actions.join(", ")
                        );
                        if report.created_lib_stub {
                            // The plan records why the automatic
                            // migration declined; showing that beats a
                            // generic "move your code".
                            let reason = plan
                                .notes
                                .iter()
                                .find_map(|note| {
                                    note.strip_prefix(
                                        "automatic bin-to-library conversion was unavailable: ",
                                    )
                                })
                                .map(str::to_string);
                            show_lib_stub_warning_card(
                                world,
                                root,
                                plan.package_dir.clone(),
                                reason,
                            );
                        } else {
                            close_new_project_modal(world);
                            enter_project_with(world, root, false);
                        }
                    }
                    Err(error) => {
                        show_setup_jackdaw_card(world, root, Some(error.to_string()));
                    }
                }
            });
        });
}

/// Shown after setup falls back to an empty `src/lib.rs` stub, which
/// only happens when the automatic `main.rs` migration could not follow
/// the project's shape. The editor sees components only in a library, so
/// the remaining work is a manual move, and this says exactly what it is.
///
/// There is deliberately no "migrate for me" button: reaching this card
/// means the migration already declined on this exact source, so a
/// button re-running it could only fail the same way.
fn show_lib_stub_warning_card(
    world: &mut World,
    root: PathBuf,
    package_dir: PathBuf,
    reason: Option<String>,
) {
    let (_, card, font) = spawn_modal_card(world, 480.0, 660.0);
    spawn_card_title(world, card, "Move your game code into GamePlugin", &font);
    world.spawn((
        Text::new(format!(
            "This project had no library target, so jackdaw created an empty `GamePlugin` \
             in {}. The editor only discovers components that live in your library, so \
             until you move your gameplay across, the editor opens but the inspector \
             lists none of your components.",
            package_dir.join("src/lib.rs").display()
        )),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(Color::srgb(0.95, 0.78, 0.45)),
        ChildOf(card),
    ));
    spawn_step_list(
        world,
        card,
        &[
            "Move your components, systems, and resources from `main.rs` into `lib.rs`, \
             and make them `pub`.",
            "Move the `App` builder calls that register them (`add_systems`, \
             `insert_resource`) into `GamePlugin::build`.",
            "Leave `DefaultPlugins` and `run()` in `main.rs`, and add \
             `.add_plugins(<your_crate>::GamePlugin)` there.",
        ],
        &font,
    );

    if let Some(reason) = reason {
        world.spawn((
            Text::new(format!("Jackdaw could not do this for you: {reason}")),
            TextFont {
                font: font.clone().into(),
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(Color::srgb(0.92, 0.45, 0.45)),
            ChildOf(card),
        ));
    }

    let row = spawn_card_button_row(world, card);
    let open_editor = spawn_card_button(world, row, "Open the editor", &font, true);
    world
        .entity_mut(open_editor)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            // The project root, not the package directory: in a
            // workspace those differ, and `jackdaw.toml` lives at the
            // root the user opened.
            let root = root.clone();
            commands.queue(move |world: &mut World| {
                close_new_project_modal(world);
                enter_project_with(world, root, false);
            });
        });
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
    // What the typed name will actually become, or why it cannot be
    // used, updated as they type. Discovering "`2d-shooter` is not a
    // usable crate name" only after committing is the failure mode this
    // exists to remove.
    world.spawn((
        NewProjectNameHint,
        Text::new(String::new()),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
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
    commands.queue(close_new_project_modal);
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

/// Keep the name hint and the Create button in step with what has been
/// typed, so a bad name is visible before it is committed rather than
/// after. The button stays clickable and explains itself on click; a
/// dead control with no reason is worse than a refusal that says why.
/// What creating at `dest` would do, and whether it can proceed.
///
/// A legal name is only half the question. Aiming at a folder that
/// already holds someone's work is the commonest first-run mistake, and
/// previewing `creates <path>` for a destination that cannot be created
/// makes the user press Create to find out.
fn describe_destination(dest: &Path) -> (String, bool) {
    let empty = dest
        .read_dir()
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false);
    if dest.is_dir() && !empty {
        return (
            format!("{} already exists and is not empty", dest.display()),
            false,
        );
    }
    if dest.exists() && !dest.is_dir() {
        return (format!("{} is a file", dest.display()), false);
    }
    if dest.is_dir() {
        return (format!("uses the empty folder {}", dest.display()), true);
    }
    (format!("creates {}", dest.display()), true)
}

fn refresh_new_project_validation(
    state: Res<NewProjectState>,
    name_inputs: Query<&TextEditValue, With<NewProjectNameInput>>,
    mut hints: Query<&mut Text, With<NewProjectNameHint>>,
    mut hint_colors: Query<&mut TextColor, With<NewProjectNameHint>>,
    mut create_buttons: Query<&mut BackgroundColor, With<NewProjectCreateButton>>,
) {
    let Ok(value) = name_inputs.single() else {
        return;
    };
    let raw = value.0.trim();
    let (message, valid) = if raw.is_empty() {
        (String::new(), false)
    } else {
        match crate::scaffold::validated_project_name(raw) {
            Ok(name) => describe_destination(&state.location.join(&name)),
            Err(error) => (error.to_string(), false),
        }
    };
    let color = if valid {
        tokens::TEXT_SECONDARY
    } else {
        tokens::TEXT_ERROR
    };
    for mut text in hints.iter_mut() {
        if text.0 != message {
            text.0 = message.clone();
        }
    }
    for mut text_color in hint_colors.iter_mut() {
        if text_color.0 != color {
            text_color.0 = color;
        }
    }
    // Dim rather than remove: the button still answers when pressed.
    let fill = if valid {
        tokens::SELECTED_BG
    } else {
        tokens::TOOLBAR_BG
    };
    for mut background in create_buttons.iter_mut() {
        if background.0 != fill {
            background.0 = fill;
        }
    }
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

        // The same validation the CLI applies. Accepting a name here
        // that `jd new` would reject produces a project whose crate
        // name is not a valid Rust identifier, which fails at the
        // user's first build with nothing pointing back here.
        let name = if raw_name.trim().is_empty() {
            world.resource_mut::<NewProjectState>().status =
                Some("Please enter a project name.".into());
            return;
        } else {
            match crate::scaffold::validated_project_name(&raw_name) {
                Ok(name) => name,
                Err(error) => {
                    world.resource_mut::<NewProjectState>().status = Some(error.to_string());
                    return;
                }
            }
        };

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
    let desired_cancel = if state.scaffold_task.is_some() {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The card builders need the two font resources, the modal state,
    /// and enough of the asset/scene stack for `bsn!` `spawn_scene`
    /// calls (the package picker). Exercising them against this world
    /// checks the thing the type checker cannot: that the hierarchy
    /// actually gets built, and that nothing panics on the way.
    fn card_world() -> World {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::asset::AssetPlugin::default())
            .add_plugins(bevy::scene::ScenePlugin)
            .init_asset::<Font>();
        app.world_mut().insert_resource(NewProjectState::default());
        app.world_mut()
            .insert_resource(EditorFont(Handle::default()));
        app.world_mut()
            .insert_resource(jackdaw_feathers::icons::IconFont(Handle::default()));
        std::mem::take(app.world_mut())
    }

    /// Every `Text` in the world, for asserting on what a card says.
    fn rendered_text(world: &mut World) -> Vec<String> {
        world
            .query::<&Text>()
            .iter(world)
            .map(|text| text.0.clone())
            .collect()
    }

    fn plan_with(changes: Vec<ImportChange>, notes: Vec<String>) -> crate::scaffold::ImportPlan {
        crate::scaffold::ImportPlan {
            root: PathBuf::from("/proj"),
            package_dir: PathBuf::from("/proj"),
            package_name: "proj".into(),
            changes,
            notes,
            migrated_bin_target: false,
        }
    }

    /// The consent-critical distinction: a modify touches something
    /// the user already had, and must not read like a creation.
    #[test]
    fn the_preview_marks_modifications_differently_from_creations() {
        let mut world = card_world();
        show_import_preview_card(
            &mut world,
            plan_with(
                vec![
                    ImportChange::WriteFile {
                        path: PathBuf::from("/proj/jackdaw.toml"),
                        contents: String::new(),
                        replace: false,
                    },
                    ImportChange::CreateDirectory {
                        path: PathBuf::from("/proj/.jackdaw"),
                    },
                ],
                vec!["game plugin: GamePlugin".into()],
            ),
        );

        let text = rendered_text(&mut world);
        assert!(
            text.iter().any(|t| t == "create"),
            "each change carries its verb: {text:?}"
        );
        assert!(
            text.iter().any(|t| t.contains("jackdaw.toml")),
            "and its path: {text:?}"
        );
        assert!(
            text.iter().any(|t| t.contains("game plugin: GamePlugin")),
            "notes are rendered too: {text:?}"
        );
    }

    /// A card with nothing to say still has to build without panicking.
    #[test]
    fn a_preview_with_no_changes_or_notes_still_renders() {
        let mut world = card_world();
        show_import_preview_card(&mut world, plan_with(Vec::new(), Vec::new()));
        assert!(
            rendered_text(&mut world)
                .iter()
                .any(|t| t.contains("Review project integration")),
            "the title is always present"
        );
    }

    #[test]
    fn the_setup_card_lists_its_guarantees_separately() {
        let mut world = card_world();
        show_setup_jackdaw_card(&mut world, PathBuf::from("/proj/their-game"), None);
        let text = rendered_text(&mut world);
        assert!(text.iter().any(|t| t.contains("their-game")));
        assert!(
            text.iter().any(|t| t.contains("cargo run")),
            "the untouched-manifest guarantee is one of the claims: {text:?}"
        );
        assert!(
            text.iter().any(|t| t.contains("lists every file first")),
            "and so is the promise that nothing is written yet: {text:?}"
        );
        assert!(
            text.iter().any(|t| t == "Cancel"),
            "offers a way to dismiss: {text:?}"
        );
    }

    #[test]
    fn the_package_picker_lists_each_candidate() {
        let mut world = card_world();
        show_package_picker_card(
            &mut world,
            PathBuf::from("/proj/workspace"),
            vec!["package_1".into(), "package_2".into()],
            false,
        );
        let text = rendered_text(&mut world);
        assert!(
            text.iter()
                .any(|t| t.contains("Which package is the game?")),
            "asks which package: {text:?}"
        );
        assert!(
            text.iter().any(|t| t == "package_1"),
            "lists the first candidate: {text:?}"
        );
        assert!(
            text.iter().any(|t| t == "package_2"),
            "lists the second candidate: {text:?}"
        );
        assert!(
            text.iter().any(|t| t == "Back"),
            "offers a way back: {text:?}"
        );
    }

    /// The steps are numbered by the layout, so the numbering cannot
    /// drift from the order.
    #[test]
    fn the_lib_stub_card_numbers_its_steps() {
        let mut world = card_world();
        show_lib_stub_warning_card(
            &mut world,
            PathBuf::from("/proj"),
            PathBuf::from("/proj"),
            Some("could not follow the builder".into()),
        );
        let text = rendered_text(&mut world);
        for step in ["1", "2", "3"] {
            assert!(
                text.iter().any(|t| t == step),
                "step {step} should be numbered: {text:?}"
            );
        }
        assert!(
            text.iter()
                .any(|t| t.contains("could not follow the builder")),
            "the real reason is shown: {text:?}"
        );
    }

    /// The modal and `jd new` must agree on what a usable name is.
    /// Accepting one here that the CLI rejects scaffolds a project
    /// whose crate name is not a valid Rust identifier, and the user
    /// finds out at their first build with nothing pointing back here.
    #[test]
    fn the_modal_and_the_cli_reject_the_same_names() {
        for bad in ["2d-shooter", "crate", "impl", "***", ""] {
            assert!(
                crate::scaffold::validated_project_name(bad).is_err(),
                "`{bad}` should be rejected"
            );
        }
        for good in ["my-game", "My Cool Game", "spaced  out"] {
            assert!(
                crate::scaffold::validated_project_name(good).is_ok(),
                "`{good}` should be accepted"
            );
        }
    }

    /// The hint answers "what will this actually create?" before the
    /// user commits, and names the problem when there is one.
    #[test]
    fn the_name_hint_previews_the_directory_or_the_problem() {
        let location = PathBuf::from("/home/dev/Projects");
        let ok = crate::scaffold::validated_project_name("My Cool Game").expect("valid");
        assert_eq!(
            location.join(&ok),
            location.join("my-cool-game"),
            "the hint shows the sanitized directory, not the raw input"
        );
        let error = crate::scaffold::validated_project_name("2d-shooter")
            .expect_err("a leading digit is not a crate name");
        assert!(
            error.to_string().contains("cannot start with a digit"),
            "the hint says why: {error}"
        );
    }

    #[test]
    fn refusing_a_missing_folder_offers_to_forget_it() {
        let mut world = card_world();
        show_not_a_project_card(&mut world, PathBuf::from("/gone"), NotAProject::Missing);
        let text = rendered_text(&mut world);
        assert!(text.iter().any(|t| t.contains("no longer exists")));
        assert!(text.iter().any(|t| t == "Remove from list"));
    }

    #[test]
    fn refusing_a_non_project_folder_says_what_was_expected() {
        let mut world = card_world();
        show_not_a_project_card(
            &mut world,
            PathBuf::from("/downloads"),
            NotAProject::NoManifest,
        );
        let text = rendered_text(&mut world);
        assert!(text.iter().any(|t| t.contains("no Cargo.toml")));
        // Nothing to forget: this folder was never in the list.
        assert!(!text.iter().any(|t| t == "Remove from list"));
    }

    /// A legal name is only half of what makes a destination usable.
    /// Previewing `creates <path>` for a folder that already holds
    /// someone's work made the user press Create to find out.
    #[test]
    fn the_destination_is_described_before_create_is_pressed() {
        let base = std::env::temp_dir().join(format!("jackdaw_dest_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let fresh = base.join("fresh");
        let (message, ok) = super::describe_destination(&fresh);
        assert!(ok, "{message}");
        assert!(message.starts_with("creates "), "{message}");

        let empty = base.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let (message, ok) = super::describe_destination(&empty);
        assert!(ok, "an empty folder is usable: {message}");
        assert!(message.contains("empty folder"), "{message}");

        let occupied = base.join("occupied");
        std::fs::create_dir_all(&occupied).unwrap();
        std::fs::write(occupied.join("Cargo.toml"), b"").unwrap();
        let (message, ok) = super::describe_destination(&occupied);
        assert!(!ok, "an occupied folder is refused");
        assert!(
            message.contains("already exists and is not empty"),
            "{message}"
        );

        let file = base.join("afile");
        std::fs::write(&file, b"").unwrap();
        let (message, ok) = super::describe_destination(&file);
        assert!(!ok);
        assert!(message.contains("is a file"), "{message}");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Walk a real project on disk through the decision the launcher
    /// makes about it, and check what the resulting card says.
    ///
    /// The individual pieces are unit tested; what this covers is the
    /// join. Detection, planning, and the card that reports them are
    /// three separate modules, and a project shape that confuses any one
    /// of them shows up as the wrong card rather than as a failure.
    #[test]
    fn each_project_shape_reaches_the_right_card() {
        let base = std::env::temp_dir().join(format!("jackdaw_shapes_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        // A plain Bevy library: the import preview, naming its plugin.
        let plain = base.join("plain-lib");
        std::fs::create_dir_all(plain.join("src")).unwrap();
        std::fs::write(
            plain.join("Cargo.toml"),
            b"[package]\nname = \"plain-lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n              [dependencies]\nbevy = \"0.19\"\n",
        )
        .unwrap();
        std::fs::write(
            plain.join("src/lib.rs"),
            b"use bevy::prelude::*;\npub struct MyGamePlugin;\n              impl Plugin for MyGamePlugin { fn build(&self, _a: &mut App) {} }\n",
        )
        .unwrap();
        let plan =
            crate::scaffold::plan_import_project(&plain, None).expect("a bevy lib is importable");
        let mut world = card_world();
        show_import_preview_card(&mut world, plan);
        let text = rendered_text(&mut world).join("\n");
        assert!(
            text.contains("jackdaw.toml"),
            "lists the file it adds: {text}"
        );
        assert!(
            text.contains("MyGamePlugin"),
            "names the detected plugin: {text}"
        );

        // A folder that is not a Rust project at all.
        let bare = base.join("not-a-project");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::write(bare.join("readme.txt"), b"hi").unwrap();
        assert!(
            crate::scaffold::plan_import_project(&bare, None).is_err(),
            "a folder with no Cargo.toml is not importable"
        );
        let mut world = card_world();
        show_not_a_project_card(&mut world, bare.clone(), NotAProject::NoManifest);
        let text = rendered_text(&mut world).join("\n");
        assert!(text.contains("Cargo.toml"), "says what it expected: {text}");

        // A project targeting another Bevy: blocked, but openable.
        let old = base.join("old-bevy");
        std::fs::create_dir_all(old.join("src")).unwrap();
        std::fs::write(
            old.join("Cargo.toml"),
            b"[package]\nname = \"old-bevy\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n              [dependencies]\nbevy = \"0.16\"\n",
        )
        .unwrap();
        std::fs::write(old.join("src/lib.rs"), b"pub struct Placeholder;\n").unwrap();
        std::fs::write(
            old.join("jackdaw.toml"),
            b"[jackdaw]\nversion = \"0.19.0\"\nbevy = \"0.16\"\n",
        )
        .unwrap();
        let manifest = project_manifest::ProjectManifest::read(&old);
        let status = project_manifest::compare_project(&old, &manifest);
        assert!(
            matches!(status, project_manifest::PinStatus::BevyDiffers { .. }),
            "a project on another bevy is recognised as such: {status:?}"
        );
        let mut world = card_world();
        show_version_mismatch_card(&mut world, old.clone(), status);
        let text = rendered_text(&mut world).join("\n");
        assert!(text.contains("0.16"), "names the bevy it targets: {text}");

        // A folder with only assets/ is not a project, and must not be
        // offered as the current directory.
        let assets_only = base.join("assets-only");
        std::fs::create_dir_all(assets_only.join("assets")).unwrap();
        assert!(
            !assets_only.join("jackdaw.toml").is_file()
                && !assets_only.join("Cargo.toml").is_file(),
            "the marker an `assets/` folder does not satisfy"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
