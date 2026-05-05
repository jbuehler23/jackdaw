use std::path::{Path, PathBuf};

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
use rfd::{AsyncFileDialog, FileHandle};

use crate::{
    AppState,
    new_project::{ScaffoldError, TemplateLinkage, TemplatePreset, scaffold_project},
    project::{self, ProjectRoot},
    scene_io,
};

pub struct ProjectSelectPlugin;

impl Plugin for ProjectSelectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NewProjectState>()
            .init_resource::<crate::build_status::BuildStatus>()
            .add_systems(OnEnter(AppState::ProjectSelect), spawn_project_selector)
            .add_systems(
                Update,
                (
                    poll_folder_dialog,
                    poll_template_folder_dialog,
                    refresh_build_progress_ui,
                )
                    .run_if(in_state(AppState::ProjectSelect)),
            )
            // Build-progress polling and task draining run in BOTH
            // states. The static-template scaffold flow transitions
            // to Editor immediately after scaffolding succeeds, but
            // the build task keeps running in the background; the
            // status_bar's progress region depends on
            // `refresh_build_progress_snapshot` to keep updating,
            // and `poll_new_project_tasks` needs to drain the build
            // task when it completes (otherwise the progress
            // indicator would never collapse). They're cheap no-ops
            // when nothing is in flight.
            //
            // `drive_static_editor_build` runs in both states too:
            // it kicks off the user's editor-binary build in the
            // background after scaffold or open-existing, polls for
            // completion, and updates `BuildStatus` so the status
            // bar / click handler can render and dispatch the
            // handoff.
            .add_systems(
                Update,
                (
                    poll_new_project_tasks,
                    refresh_build_progress_snapshot,
                    drive_static_editor_build,
                ),
            )
            // The dylib-install step MUST run outside of `Update`'s
            // `schedule_scope`. The game's `GameApp::add_systems(Update, …)`
            // inserts into `Schedules`; doing that while bevy has
            // `Update` checked out via `schedule_scope` causes the
            // modification to be overwritten when the scope re-inserts
            // at exit. `Last` has its own scope and doesn't clash.
            .add_systems(
                Last,
                (apply_pending_install, apply_pending_static_open)
                    .run_if(in_state(AppState::ProjectSelect)),
            );
    }
}

/// Marker for the project selector root UI node.
#[derive(Component)]
struct ProjectSelectorRoot;

/// When set, the project selector will skip UI and auto-open the given project.
#[derive(Resource)]
pub struct PendingAutoOpen {
    pub path: PathBuf,
    /// `true` when we got here via a post-restart auto-open ;
    /// the parent process already built + installed the dylib,
    /// so we skip that step (preventing an infinite
    /// build→restart→auto-open→build loop).
    pub skip_build: bool,
}

/// Resource holding the async folder picker task.
#[derive(Resource)]
struct FolderDialogTask(Task<Option<rfd::FileHandle>>);

/// Resource holding the async folder picker task for the template-
/// folder Browse button in the New Project modal. Kept separate from
/// `FolderDialogTask` (which routes results into the Location field).
#[derive(Resource)]
struct TemplateFolderDialogTask(Task<Option<rfd::FileHandle>>);

/// Root marker for the New Project modal overlay. Spawned when the
/// user clicks **+ New Extension** / **+ New Game**; despawned on
/// Cancel or on successful scaffold.
#[derive(Component)]
struct NewProjectModalRoot;

/// Wraps the Name `TextEdit` so the Create handler can read its
/// current value.
#[derive(Component)]
struct NewProjectNameInput;

/// Wraps the Template URL `TextEdit`. Pre-filled with the default
/// URL for the active preset; always editable so users can paste
/// any Bevy-CLI-compatible URL.
#[derive(Component)]
struct NewProjectTemplateInput;

/// Wraps the local-path `TextEdit`. Empty by default; when
/// non-empty it takes precedence over the Git URL on Create.
#[derive(Component)]
struct NewProjectLocalTemplateInput;

/// Marks the Browse button next to the local template path field.
#[derive(Component)]
struct NewProjectLocalBrowseButton;

/// Wraps the Git branch `TextEdit`. Used only when scaffolding via
/// the Git URL field; ignored for local-path scaffolds. Default
/// value comes from `template_branch()`.
#[derive(Component)]
struct NewProjectBranchInput;

#[derive(Component)]
struct NewProjectLocationText;

#[derive(Component)]
struct NewProjectStatusText;

/// Marker for the "Open editor after creating" checkbox in the New
/// Project modal. The checkbox lets the user opt out of the
/// auto-build/auto-spawn flow for static templates: when checked
/// (default) the launcher kicks off `cargo build --bin editor
/// --features editor` and hands off to the user's editor binary.
/// When unchecked, the launcher stops at "files written" and
/// surfaces a dialog with the cargo command the user should run
/// from their terminal.
#[derive(Component)]
struct BuildAfterScaffoldCheckbox;

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
struct NewProjectCreateButton;

#[derive(Component)]
struct NewProjectBrowseButton;

/// One of the two segmented buttons that pick between the Static
/// and Dylib template variants. The enum value is stored on the
/// component so the click observer knows which linkage to apply
/// without needing separate marker types per button.
#[derive(Component, Clone, Copy)]
struct NewProjectLinkageButton(TemplateLinkage);

/// State for the static-editor build pipeline. The launcher
/// background-builds `<project>/target/debug/editor` (via
/// `cargo build --bin editor --features editor`) for static-game
/// projects, then hands off to that binary. Three pieces of state
/// belong together: the queued request, the in-flight task, and
/// the auto-reload flag.
#[derive(Default)]
struct StaticEditorBuild {
    /// Queued request to start a build. The bool is `auto_reload`:
    /// `true` from the post-scaffold path (driver auto-fires the
    /// handoff once the build is `Ready`); `false` from "open
    /// existing project" (user clicks the footer indicator to
    /// reload manually). `drive_static_editor_build` consumes this
    /// on the next `Update` and spawns the cargo task.
    pending: Option<(PathBuf, bool)>,
    /// In-flight static-editor cargo task. Polled each frame by
    /// `drive_static_editor_build`; on completion the driver
    /// updates `BuildStatus` to `Ready` or `Failed`.
    task: Option<Task<Result<PathBuf, crate::ext_build::BuildError>>>,
    /// Auto-reload flag remembered between dispatch and completion.
    /// Mirrors the second member of `pending`, stashed once the task
    /// is spawned because `pending` is drained on dispatch.
    auto_reload: bool,
}

/// Drives the modal's async operations. Internal to this module;
/// external systems that need to observe build progress read the
/// public `BuildStatus` resource instead.
#[derive(Resource, Default)]
struct NewProjectState {
    /// Which preset the user opened the dialog with. `None` when
    /// the modal isn't open.
    preset: Option<TemplatePreset>,
    /// Static vs dylib template choice. Ignored when `preset` is
    /// `Custom` (the user pastes a raw URL).
    linkage: TemplateLinkage,
    /// Parent directory the new project will be placed under.
    /// Scaffolder produces `location/name/`.
    location: PathBuf,
    /// In-flight folder picker (rfd).
    folder_task: Option<Task<Option<FileHandle>>>,
    /// In-flight scaffold (bevy-cli subprocess).
    scaffold_task: Option<Task<Result<PathBuf, ScaffoldError>>>,
    /// In-flight initial build after scaffold. Queued immediately
    /// after the scaffold task succeeds so the user lands in the
    /// editor with the game/extension dylib already installed.
    /// In-flight build task plus the linkage it was kicked off with.
    /// The linkage travels with the task so the poller branches on
    /// the actual in-flight linkage, not on the modal's stale
    /// `state.linkage` (which describes the modal's current selection,
    /// not whatever build is running).
    build_task: Option<(
        Task<Result<PathBuf, crate::ext_build::BuildError>>,
        TemplateLinkage,
    )>,
    /// Artifact waiting to be installed by `apply_pending_install`
    /// (runs in `Last`, not `Update`, so modifications to the
    /// `Update` schedule by the game's `GameApp::add_systems` don't
    /// collide with `Update`'s active `schedule_scope`).
    pending_install: Option<PathBuf>,
    /// Static scaffold whose pre-build finished. Picked up in `Last`
    /// by `apply_pending_static_open`, which calls `enter_project`.
    pending_static_open: Option<PathBuf>,
    /// Static-editor build pipeline state grouped into one field.
    /// See [`StaticEditorBuild`].
    static_editor: StaticEditorBuild,
    /// Whether the post-scaffold path should kick off the editor
    /// build immediately (default `true` — the user lands in their
    /// editor without leaving the launcher) or stop at "files
    /// written, run `cargo editor` from the project root yourself"
    /// (`false` — for users who'd rather drive the build from
    /// their own terminal). Read by the scaffold-success branch
    /// in `poll_new_project_tasks`; set from the New Project
    /// modal's checkbox in `on_create_new_project`.
    build_after_scaffold: bool,
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

fn spawn_project_selector(
    mut commands: Commands,
    editor_font: Res<EditorFont>,
    icon_font: Res<jackdaw_feathers::icons::IconFont>,
    pending: Option<Res<PendingAutoOpen>>,
) {
    // UI camera for the project selector screen. Spawned BEFORE the
    // auto-open early-return so the build-progress modal renders
    // against a valid camera even when the user lands directly in
    // an opening project. Without this, the launcher window stays
    // fully blank for several minutes during a static-editor build
    // because the modal had no camera to draw on.
    commands.spawn((ProjectSelectorRoot, Camera2d));

    if let Some(pending) = pending {
        let path = pending.path.clone();
        let skip_build = pending.skip_build;
        commands.remove_resource::<PendingAutoOpen>();
        commands.queue(move |world: &mut World| {
            enter_project_with(world, path, skip_build);
        });
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

    commands
        .spawn((
            ProjectSelectorRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(tokens::WINDOW_BG),
        ))
        .with_children(|parent| {
            // Card container
            parent
                .spawn(Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    padding: UiRect::all(Val::Px(32.0)),
                    row_gap: Val::Px(24.0),
                    min_width: Val::Px(420.0),
                    max_width: Val::Px(520.0),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    ..Default::default()
                })
                .insert(BackgroundColor(tokens::PANEL_BG))
                .insert(BorderColor::all(tokens::BORDER_SUBTLE))
                .with_children(|card| {
                    // Title
                    card.spawn((
                        Text::new("jackdaw"),
                        TextFont {
                            font: font.clone(),
                            font_size: 28.0,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_PRIMARY),
                    ));

                    // Subtitle
                    card.spawn((
                        Text::new("Select a project to open"),
                        TextFont {
                            font: font.clone(),
                            font_size: tokens::FONT_LG,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_SECONDARY),
                    ));

                    // CWD option (if it looks like a project)
                    if cwd_has_project {
                        let cwd_name = cwd
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| cwd.to_string_lossy().to_string());
                        let cwd_clone = cwd.clone();
                        spawn_project_row(
                            card,
                            &cwd_name,
                            &cwd.to_string_lossy(),
                            font.clone(),
                            icon_font_handle.clone(),
                            cwd_clone,
                            true,
                        );
                    }

                    // Recent projects
                    if !recent.projects.is_empty() {
                        card.spawn((
                            Text::new("Recent Projects"),
                            TextFont {
                                font: font.clone(),
                                font_size: tokens::FONT_MD,
                                ..Default::default()
                            },
                            TextColor(tokens::TEXT_SECONDARY),
                            Node {
                                margin: UiRect::top(Val::Px(8.0)),
                                ..Default::default()
                            },
                        ));

                        for entry in &recent.projects {
                            // Skip CWD if already shown above
                            if cwd_has_project && entry.path == cwd {
                                continue;
                            }
                            spawn_project_row(
                                card,
                                &entry.name,
                                &entry.path.to_string_lossy(),
                                font.clone(),
                                icon_font_handle.clone(),
                                entry.path.clone(),
                                false,
                            );
                        }
                    }

                    // New Extension / New Game row
                    let new_row = card
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(8.0),
                            margin: UiRect::top(Val::Px(8.0)),
                            ..Default::default()
                        })
                        .id();
                    spawn_new_project_button(
                        card,
                        new_row,
                        "+ New Extension",
                        font.clone(),
                        TemplatePreset::Extension,
                    );
                    spawn_new_project_button(
                        card,
                        new_row,
                        "+ New Game",
                        font.clone(),
                        TemplatePreset::Game,
                    );
                    spawn_new_project_button(
                        card,
                        new_row,
                        "+ From URL…",
                        font.clone(),
                        TemplatePreset::Custom(String::new()),
                    );

                    // Browse button
                    let browse_entity = card
                        .spawn((
                            Node {
                                padding: UiRect::axes(Val::Px(20.0), Val::Px(10.0)),
                                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                                margin: UiRect::top(Val::Px(4.0)),
                                justify_content: JustifyContent::Center,
                                ..Default::default()
                            },
                            BackgroundColor(tokens::SELECTED_BG),
                            children![(
                                Text::new("Open existing project..."),
                                TextFont {
                                    font: font.clone(),
                                    font_size: tokens::FONT_LG,
                                    ..Default::default()
                                },
                                TextColor(tokens::TEXT_PRIMARY),
                            )],
                        ))
                        .id();

                    // Hover effects for browse button
                    card.commands().entity(browse_entity).observe(
                        |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
                            if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                                bg.0 = tokens::SELECTED_BORDER;
                            }
                        },
                    );
                    card.commands().entity(browse_entity).observe(
                        |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
                            if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                                bg.0 = tokens::SELECTED_BG;
                            }
                        },
                    );
                    card.commands()
                        .entity(browse_entity)
                        .observe(spawn_browse_dialog);
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
    // Outer row: info column on left, optional X button on right
    let row_entity = parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                width: Val::Percent(100.0),
                padding: UiRect::all(Val::Px(10.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(tokens::TOOLBAR_BG),
        ))
        .id();

    // Left side: info column (flex_grow so it fills space)
    let info_column = parent
        .commands()
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                row_gap: Val::Px(2.0),
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
                                font: font.clone(),
                                font_size: tokens::FONT_LG,
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
                        font: font.clone(),
                        font_size: tokens::FONT_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                ),
            ],
            Pickable::IGNORE,
        ))
        .id();

    parent.commands().entity(row_entity).add_child(info_column);

    // Right side: X button (only for recent projects, not CWD)
    if !is_cwd {
        let remove_path = project_path.clone();
        let x_button = parent
            .commands()
            .spawn(icon_button(
                IconButtonProps::new(Icon::X).variant(ButtonVariant::Ghost),
                &icon_font,
            ))
            .id();

        // X button click: remove from recent + despawn row
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

    // Hover effects on the row
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
                bg.0 = tokens::TOOLBAR_BG;
            }
        },
    );

    // Click: select project
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
            font,
            font_size: tokens::FONT_SM,
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
/// folder browse). If the project has a `Cargo.toml`, we kick off a
/// `cargo build` task and the poller decides whether to restart
/// (game) or transition into the editor (extension / non-building
/// project) once it finishes. If there's no `Cargo.toml`, we
/// transition straight to the editor.
///
/// All per-session rebuilds therefore happen at the launcher, never
/// mid-edit. Games' restart-to-activate requirement becomes
/// invisible; the launcher → editor transition already carries a
/// build step, so folding a process restart into it is just a
/// slightly-longer wait.
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
    // Static-template games: don't open the launcher's editor at
    // all. The launcher's editor doesn't carry the user's
    // `MyGamePlugin` types (different binary, different
    // `AppTypeRegistry`), so the inspector + PIE Play would be
    // missing exactly the user-defined components the user wants
    // to author. Build the user's `editor` binary first, then
    // hand off (`drive_static_editor_build` -> `do_handoff`).
    if matches!(
        crate::new_project::detect_template_kind(&root),
        crate::new_project::TemplateKind::StaticGameWithEditorFeature
    ) {
        let project_name = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .to_owned();
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
        let mut state = world.resource_mut::<NewProjectState>();
        state.pending_project = Some(root.clone());
        state.status = Some(format!("Building editor for `{project_name}`…"));
        state.build_progress = Some(std::sync::Arc::clone(&progress));
        state.build_progress_snapshot = Some(crate::ext_build::BuildProgress::default());
        state.static_editor.pending = Some((root, true));
        return;
    }
    // If the Cargo.toml is a plain binary crate (e.g., the editor's
    // own source tree, or any non-extension cargo project the user
    // points at) there's no cdylib for the loader to pick up. Skip
    // the build rather than compile the whole dep graph just to fail
    // the artifact check at the end.
    if !crate::ext_build::manifest_declares_cdylib(&root) {
        info!(
            "Project at {} has a Cargo.toml but no cdylib target; \
             opening without building.",
            root.display()
        );
        transition_to_editor(world, root);
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
        state.status = Some(format!("Building `{project_name}`…"));
        state.build_progress = Some(std::sync::Arc::clone(&progress));
        state.build_progress_snapshot = Some(crate::ext_build::BuildProgress::default());
    }

    let root_for_task = root;
    let progress_for_task = std::sync::Arc::clone(&progress);
    // Non-cdylib projects took the early-out in `enter_project_with`.
    let task = AsyncComputeTaskPool::get().spawn(async move {
        crate::ext_build::build_extension_project_with_progress(
            &root_for_task,
            Some(progress_for_task),
            TemplateLinkage::Dylib,
        )
    });
    world.resource_mut::<NewProjectState>().build_task = Some((task, TemplateLinkage::Dylib));
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

    project::touch_recent(&root, &config.project.name);

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

    // Convention: auto-load `assets/scene.jsn` if present. The game
    // template ships one so scaffolded projects open populated.
    // Per-project last-opened-scene persistence is a follow-up.
    let scene_path = root.join("assets").join("scene.jsn");
    if scene_path.is_file() {
        crate::scene_io::load_scene_from_file(world, &scene_path);
    }
}

/// Spawn a pill-style button inside the "+ New Extension / + New
/// Game" row. Clicking opens the New Project modal with the given
/// preset already selected.
fn spawn_new_project_button(
    card: &mut ChildSpawnerCommands,
    parent: Entity,
    label: &str,
    font: Handle<Font>,
    preset: TemplatePreset,
) {
    let button = card
        .commands()
        .spawn((
            Node {
                padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                justify_content: JustifyContent::Center,
                ..Default::default()
            },
            BackgroundColor(tokens::TOOLBAR_BG),
            children![(
                Text::new(label.to_string()),
                TextFont {
                    font,
                    font_size: tokens::FONT_MD,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
        ))
        .id();

    card.commands().entity(parent).add_child(button);

    card.commands().entity(button).observe(
        |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                bg.0 = tokens::HOVER_BG;
            }
        },
    );
    card.commands().entity(button).observe(
        |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                bg.0 = tokens::TOOLBAR_BG;
            }
        },
    );
    card.commands()
        .entity(button)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let preset = preset.clone();
            commands.queue(move |world: &mut World| {
                open_new_project_modal(world, preset);
            });
        });
}

/// Spawn one segment of the Static/Dylib selector. `initial` picks
/// the starting highlighted button; subsequent clicks repaint via
/// `on_linkage_button_click`.
///
/// The project picker runs before any extension has registered
/// operators, so there's no rich hover tooltip available here. The
/// button label is the single visible affordance; explanatory text
/// is rendered as a subtitle below the row by the calling dialog.
fn spawn_linkage_button(
    world: &mut World,
    parent: Entity,
    label: &str,
    linkage: TemplateLinkage,
    initial: TemplateLinkage,
    font: Handle<Font>,
) {
    let selected = linkage == initial;
    let bg_color = if selected {
        tokens::SELECTED_BG
    } else {
        tokens::TOOLBAR_BG
    };
    let border_color = if selected {
        tokens::SELECTED_BORDER
    } else {
        tokens::BORDER_SUBTLE
    };

    let button = world
        .spawn((
            NewProjectLinkageButton(linkage),
            Node {
                padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                justify_content: JustifyContent::Center,
                ..Default::default()
            },
            BackgroundColor(bg_color),
            BorderColor::all(border_color),
            children![(
                Text::new(label.to_string()),
                TextFont {
                    font,
                    font_size: tokens::FONT_MD,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
            ChildOf(parent),
        ))
        .id();

    // Hover and out handlers skip the currently-selected button so
    // its highlight isn't clobbered by hover/idle paints.
    world
        .entity_mut(button)
        .observe(on_linkage_button_click)
        .observe(
            |hover: On<Pointer<Over>>,
             buttons: Query<&NewProjectLinkageButton>,
             state: Res<NewProjectState>,
             mut bg: Query<&mut BackgroundColor>| {
                let Ok(button) = buttons.get(hover.event_target()) else {
                    return;
                };
                if button.0 == state.linkage {
                    return;
                }
                if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                    bg.0 = tokens::HOVER_BG;
                }
            },
        )
        .observe(
            |out: On<Pointer<Out>>,
             buttons: Query<&NewProjectLinkageButton>,
             state: Res<NewProjectState>,
             mut bg: Query<&mut BackgroundColor>| {
                let Ok(button) = buttons.get(out.event_target()) else {
                    return;
                };
                if button.0 == state.linkage {
                    return;
                }
                if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                    bg.0 = tokens::TOOLBAR_BG;
                }
            },
        );
}

/// Click handler for Static/Dylib segmented buttons. Updates
/// `NewProjectState.linkage`, repaints the two buttons, and
/// rewrites the Template URL input to the new preset URL so the
/// user sees the change immediately. If the user has manually
/// edited the URL to a custom value, this overwrites it; by
/// design: toggling the linkage is a "reset to preset" action.
fn on_linkage_button_click(
    click: On<Pointer<Click>>,
    buttons: Query<&NewProjectLinkageButton>,
    mut commands: Commands,
) {
    let Ok(button) = buttons.get(click.event_target()) else {
        return;
    };
    let linkage = button.0;
    commands.queue(move |world: &mut World| {
        world.resource_mut::<NewProjectState>().linkage = linkage;

        // Repaint every linkage button against the new selection.
        let mut repaint: Vec<(Entity, bool)> = Vec::new();
        {
            let mut q = world.query::<(Entity, &NewProjectLinkageButton)>();
            for (entity, btn) in q.iter(world) {
                repaint.push((entity, btn.0 == linkage));
            }
        }
        for (entity, is_selected) in repaint {
            let bg_color = if is_selected {
                tokens::SELECTED_BG
            } else {
                tokens::TOOLBAR_BG
            };
            let border_color = if is_selected {
                tokens::SELECTED_BORDER
            } else {
                tokens::BORDER_SUBTLE
            };
            if let Ok(mut ec) = world.get_entity_mut(entity) {
                ec.insert(BackgroundColor(bg_color));
                ec.insert(BorderColor::all(border_color));
            }
        }

        let Some(preset) = world.resource::<NewProjectState>().preset.clone() else {
            return;
        };
        // Re-derive both fields so dev users see the right
        // pre-fills after toggling Static/Dylib: Local field
        // tracks `<checkout>/templates/<subdir>` and the Git URL
        // field stays a real remote.
        let new_local = preset
            .local_template_path(linkage)
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        set_local_template_input_text(world, new_local);
        let new_url = preset.git_url_with_subdir(linkage);
        set_template_input_text(world, new_url);
    });
}

/// Push a new string into the Template URL text input.
fn set_template_input_text(world: &mut World, new_text: String) {
    use jackdaw_feathers::text_edit::{TextInputQueue, set_text_input_value};

    let mut q = world.query_filtered::<Entity, With<NewProjectTemplateInput>>();
    let Some(outer) = q.iter(world).next() else {
        return;
    };
    let Some((_wrapper, inner)) = find_text_edit_entities_for_template(world, outer) else {
        return;
    };
    if let Some(mut queue) = world.get_mut::<TextInputQueue>(inner) {
        set_text_input_value(&mut queue, new_text);
    }
}

/// Walk from the outer Template-field entity to its inner
/// `TextInputQueue`-bearing entity. Mirror of
/// `inspector::find_text_edit_entities_local`.
fn find_text_edit_entities_for_template(world: &World, outer: Entity) -> Option<(Entity, Entity)> {
    use jackdaw_feathers::text_edit::TextEditWrapper;
    let children = world.get::<Children>(outer)?;
    for child in children.iter() {
        if let Some(wrapper) = world.get::<TextEditWrapper>(child) {
            return Some((child, wrapper.0));
        }
        if let Some(grandchildren) = world.get::<Children>(child) {
            for gc in grandchildren.iter() {
                if let Some(wrapper) = world.get::<TextEditWrapper>(gc) {
                    return Some((gc, wrapper.0));
                }
            }
        }
    }
    None
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
    state.preset = None;
    state.linkage = TemplateLinkage::default();
    state.folder_task = None;
    state.scaffold_task = None;
    state.status = None;
    // `pending_static_open` is already drained by
    // `apply_pending_static_open` on the happy path, and isn't set
    // on cancel.
}

/// Lightweight modal shown while `enter_project_with` builds an
/// **existing** project; the user picked a recent entry or
/// browsed to a folder and we need something visual while cargo
/// runs + the auto-recovery retry may fire. Reuses the same
/// `NewProjectProgressContainer` / `NewProjectProgressCrateLabel`
/// / progress-bar / log-tail markers as the scaffold modal, so
/// the existing `refresh_build_progress_ui` system drives it
/// without extra wiring. Despawned via `close_new_project_modal`.
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
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
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
            font: editor_font.clone(),
            font_size: tokens::FONT_LG,
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
            "Building the per-project editor binary. First run with a fresh \
             cargo cache can take 5 to 10 minutes (bevy is ~500 crates). \
             Subsequent opens are incremental and finish in seconds.",
        ),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    world.spawn((
        NewProjectStatusText,
        Text::new(String::new()),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
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
        Text::new("Preparing build...".to_string()),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
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
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
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

/// Show the New Project modal with the given preset pre-selected.
///
/// Callable from any `AppState`; the launcher (`ProjectSelect`)
/// and the editor's **File → New Project** menu both invoke this.
/// The modal is a full-window overlay so it renders regardless of
/// which camera is active.
pub fn open_new_project_modal(world: &mut World, preset: TemplatePreset) {
    close_new_project_modal(world);

    let location = default_projects_dir();
    let initial_linkage = TemplateLinkage::default();
    {
        let mut state = world.resource_mut::<NewProjectState>();
        state.preset = Some(preset.clone());
        state.linkage = initial_linkage;
        state.location = location.clone();
        state.status = None;
        state.build_after_scaffold = true;
    }

    let editor_font = world.resource::<EditorFont>().0.clone();
    let icon_font = world
        .resource::<jackdaw_feathers::icons::IconFont>()
        .0
        .clone();
    let (heading, name_placeholder) = match preset {
        TemplatePreset::Extension => ("New Extension", "my_extension"),
        TemplatePreset::Game => ("New Game", "my_game"),
        TemplatePreset::Custom(_) => ("New Project", "my_project"),
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

    // Modal card.
    let card = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(24.0)),
                min_width: Val::Px(420.0),
                max_width: Val::Px(520.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(8.0)),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(scrim),
        ))
        .id();

    // Heading
    world.spawn((
        Text::new(heading.to_string()),
        TextFont {
            font: editor_font.clone(),
            font_size: 24.0,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(card),
    ));

    // Name field
    world.spawn((
        Text::new("Name"),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
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
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
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
            font: editor_font.clone(),
            font_size: tokens::FONT_MD,
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
                Text::new("Browse…"),
                TextFont {
                    font: editor_font.clone(),
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
            ChildOf(location_row),
        ))
        .id();
    world.entity_mut(browse).observe(on_browse_new_location);

    // Linkage selector only appears for the Extension and Game
    // presets; Custom pastes its own URL.
    if preset.supports_linkage_selector() {
        world.spawn((
            Text::new("Template type"),
            TextFont {
                font: editor_font.clone(),
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(card),
        ));
        let linkage_row = world
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(8.0),
                    ..Default::default()
                },
                ChildOf(card),
            ))
            .id();
        spawn_linkage_button(
            world,
            linkage_row,
            "Static",
            TemplateLinkage::Static,
            initial_linkage,
            editor_font.clone(),
        );
        spawn_linkage_button(
            world,
            linkage_row,
            "Dylib (experimental)",
            TemplateLinkage::Dylib,
            initial_linkage,
            editor_font.clone(),
        );
        // Inline subtitle (visible always, no hover needed) so the
        // user knows what the two linkage options do without relying
        // on an operator-registered tooltip; operators aren't loaded
        // yet at the project-select stage.
        world.spawn((
            Text::new(
                "Static: plainly-compiled rlib/bin (recommended, ships with the bundled \
                 templates/game-static and templates/extension-static). \
                 Dylib: hot-reloadable cdylib, experimental and requires the editor's \
                 `dylib` feature.",
            ),
            TextFont {
                font: editor_font.clone(),
                font_size: tokens::FONT_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(card),
        ));
    }

    // "Open editor after creating" checkbox: lets the user opt
    // out of the auto-build/auto-spawn flow. Default on. When off,
    // the post-scaffold path stops at "files written" and shows a
    // dialog with the cargo command to run from the project root.
    //
    // Hidden for static-extension scaffolds: there's no `editor`
    // binary to build (extensions launch via `cargo run` against
    // their own bin), so the checkbox would be a no-op. See
    // `on_create_new_project` for the consumer side; if the
    // checkbox isn't spawned, it falls back to `true` and the
    // post-scaffold instructions modal renders for the extension.
    let show_build_checkbox = !matches!(
        (initial_linkage, &preset),
        (TemplateLinkage::Static, TemplatePreset::Extension)
    );
    if show_build_checkbox {
        world.spawn((
            BuildAfterScaffoldCheckbox,
            ChildOf(card),
            jackdaw_feathers::checkbox::checkbox(
                jackdaw_feathers::checkbox::CheckboxProps::new("Open editor after creating")
                    .checked(true),
                &editor_font,
                &icon_font,
            ),
        ));
    }

    // Template section: shared parent label, then two sub-labelled
    // rows (Local path and Git URL). The Local path field is empty
    // by default; when non-empty it takes precedence over the Git
    // URL on Create.
    world.spawn((
        Text::new("Template"),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    // Sub-label: Local path
    world.spawn((
        Text::new("Local path"),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            margin: UiRect::top(Val::Px(4.0)),
            ..Default::default()
        },
        ChildOf(card),
    ));
    // Row: local-path text input + Browse button
    let local_row = world
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
    // Mirror the NewProjectTemplateInput pattern: marker + ChildOf
    // + text_edit in a single bundle. The text_edit widget supplies
    // its own Node, so we cannot stack another Node onto this entity
    // (Bevy rejects duplicate components). Layout falls back to the
    // text_edit's intrinsic width; flex_grow on the input is a
    // future polish if this looks too narrow next to the Browse
    // button.
    let initial_local_path = preset
        .local_template_path(initial_linkage)
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    world.spawn((
        NewProjectLocalTemplateInput,
        ChildOf(local_row),
        text_edit(
            TextEditProps::default()
                .with_placeholder(
                    "Local folder (e.g. ~/Workspace/jackdaw/templates/game)".to_string(),
                )
                .with_default_value(initial_local_path)
                .allow_empty(),
        ),
    ));
    let local_browse = world
        .spawn((
            NewProjectLocalBrowseButton,
            Node {
                padding: UiRect::axes(Val::Px(12.0), Val::Px(6.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::TOOLBAR_BG),
            children![(
                Text::new("Browse..."),
                TextFont {
                    font: editor_font.clone(),
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
            ChildOf(local_row),
        ))
        .id();
    world
        .entity_mut(local_browse)
        .observe(on_browse_template_folder);

    // Sub-label: Git URL
    world.spawn((
        Text::new("Git URL"),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            margin: UiRect::top(Val::Px(4.0)),
            ..Default::default()
        },
        ChildOf(card),
    ));
    // Git URL text input. Prefilled from preset+linkage, editable
    // so power users can point at a fork or custom template.
    world.spawn((
        NewProjectTemplateInput,
        ChildOf(card),
        text_edit(
            TextEditProps::default()
                .with_placeholder("https://github.com/.../your_template".to_string())
                .with_default_value(preset.git_url_with_subdir(initial_linkage))
                .allow_empty(),
        ),
    ));

    // Sub-label: Branch
    world.spawn((
        Text::new("Branch"),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            margin: UiRect::top(Val::Px(4.0)),
            ..Default::default()
        },
        ChildOf(card),
    ));
    // Git branch text input. Pre-filled with the configured default
    // (`template_branch()` honors `JACKDAW_TEMPLATE_BRANCH` and
    // otherwise returns the compile-time default). Ignored for the
    // local-path scaffold; the local checkout's working tree IS the
    // branch.
    world.spawn((
        NewProjectBranchInput,
        ChildOf(card),
        text_edit(
            TextEditProps::default()
                .with_placeholder("main".to_string())
                .with_default_value(crate::new_project::template_branch())
                .allow_empty(),
        ),
    ));

    // Precedence hint. With both fields populated, the local path
    // wins; this line tells the user so they aren't surprised.
    world.spawn((
        Text::new("If both are filled, the local path is used."),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            margin: UiRect::top(Val::Px(4.0)),
            ..Default::default()
        },
        ChildOf(card),
    ));

    // Status line
    world.spawn((
        NewProjectStatusText,
        Text::new(String::new()),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
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
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
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
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
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
                Text::new("Cancel"),
                TextFont {
                    font: editor_font.clone(),
                    font_size: tokens::FONT_MD,
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
                    font: editor_font,
                    font_size: tokens::FONT_MD,
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

/// Observer for the Browse button on the local template path field.
/// Opens a folder picker; on completion, writes the picked path into
/// the `NewProjectLocalTemplateInput` text field via `TextInputQueue`.
fn on_browse_template_folder(
    _: On<Pointer<Click>>,
    mut commands: Commands,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) {
    let mut dialog = AsyncFileDialog::new().set_title("Select template folder");
    if let Ok(rh) = raw_handle.single() {
        // SAFETY: called on the main thread during an observer.
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    // Default starting directory: in-tree templates dir for dev
    // checkouts, otherwise the user's home directory.
    let start_dir = crate::new_project::jackdaw_dev_checkout()
        .map(|p| p.join("templates"))
        .or_else(dirs::home_dir);
    if let Some(dir) = start_dir {
        dialog = dialog.set_directory(dir);
    }
    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_folder().await });
    commands.queue(move |world: &mut World| {
        world.insert_resource(TemplateFolderDialogTask(task));
    });
}

/// Push a new string into the local template path text input.
fn set_local_template_input_text(world: &mut World, new_text: String) {
    use jackdaw_feathers::text_edit::{TextInputQueue, set_text_input_value};

    let mut q = world.query_filtered::<Entity, With<NewProjectLocalTemplateInput>>();
    let Some(outer) = q.iter(world).next() else {
        return;
    };
    let Some((_wrapper, inner)) = find_text_edit_entities_for_template(world, outer) else {
        return;
    };
    if let Some(mut queue) = world.get_mut::<TextInputQueue>(inner) {
        set_text_input_value(&mut queue, new_text);
    }
}

/// Polling system for `TemplateFolderDialogTask`. Drains the task
/// when the picker resolves and writes the picked path into the
/// local template text input.
fn poll_template_folder_dialog(world: &mut World) {
    let Some(mut task_res) = world.get_resource_mut::<TemplateFolderDialogTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task_res.0)) else {
        return;
    };
    world.remove_resource::<TemplateFolderDialogTask>();
    if let Some(handle) = result {
        let path = handle.path().to_path_buf();
        set_local_template_input_text(world, path.to_string_lossy().into_owned());
    }
}

fn on_create_new_project(
    _: On<Pointer<Click>>,
    mut commands: Commands,
    name_inputs: Query<Entity, With<NewProjectNameInput>>,
    template_inputs: Query<Entity, With<NewProjectTemplateInput>>,
    local_template_inputs: Query<Entity, With<NewProjectLocalTemplateInput>>,
    branch_inputs: Query<Entity, With<NewProjectBranchInput>>,
    text_edit_values: Query<&TextEditValue>,
    build_checkbox: Query<
        &jackdaw_feathers::checkbox::CheckboxState,
        With<BuildAfterScaffoldCheckbox>,
    >,
) {
    // Read the name, local template path, git URL, and branch from
    // the text inputs' synced TextEditValue.
    let Some(name_entity) = name_inputs.iter().next() else {
        return;
    };
    let name = text_edit_values
        .get(name_entity)
        .map(|v| v.0.trim().to_string())
        .unwrap_or_default();
    let local_path_from_input = local_template_inputs
        .iter()
        .next()
        .and_then(|e| text_edit_values.get(e).ok())
        .map(|v| v.0.trim().to_string())
        .unwrap_or_default();
    let git_url_from_input = template_inputs
        .iter()
        .next()
        .and_then(|e| text_edit_values.get(e).ok())
        .map(|v| v.0.trim().to_string())
        .unwrap_or_default();
    let branch_from_input = branch_inputs
        .iter()
        .next()
        .and_then(|e| text_edit_values.get(e).ok())
        .map(|v| v.0.trim().to_string())
        .unwrap_or_default();
    // Snapshot the "Open editor after creating" checkbox into the
    // resource so the post-scaffold poller can branch on it. Falls
    // back to `true` if the checkbox isn't found (defensive, for
    // presets that don't render the checkbox UI). For static-
    // extension we forcibly reset to `false` below: extensions
    // don't ship an editor binary to build, so the auto-open flow
    // would fail and the instructions modal is the right path.
    let checkbox_value = build_checkbox
        .iter()
        .next()
        .map(|state| state.checked)
        .unwrap_or(true);

    commands.queue(move |world: &mut World| {
        let (location, linkage) = {
            let mut state = world.resource_mut::<NewProjectState>();
            if state.preset.is_none() {
                return;
            }
            if state.scaffold_task.is_some() {
                return; // already running
            }
            // Force `build_after_scaffold = false` for static-
            // extension scaffolds so the post-scaffold path goes to
            // the instructions modal rather than the (nonexistent)
            // editor-binary build.
            let build_after_scaffold = if matches!(
                (state.linkage, state.preset.as_ref()),
                (TemplateLinkage::Static, Some(TemplatePreset::Extension))
            ) {
                false
            } else {
                checkbox_value
            };
            state.build_after_scaffold = build_after_scaffold;
            (state.location.clone(), state.linkage)
        };

        let name = name.clone();
        if name.is_empty() {
            world.resource_mut::<NewProjectState>().status =
                Some("Please enter a project name.".into());
            return;
        }

        // Precedence: local path wins when non-empty; fall through to
        // git URL otherwise. Both empty is an error.
        let local_path = local_path_from_input.clone();
        let git_url = git_url_from_input.clone();
        let template_url = if !local_path.is_empty() {
            // Validate that the local path exists as a directory before
            // launching the scaffold subprocess.
            if !std::path::Path::new(&local_path).is_dir() {
                world.resource_mut::<NewProjectState>().status =
                    Some(format!("Local template path does not exist: {local_path}"));
                return;
            }
            local_path
        } else if !git_url.is_empty() {
            git_url
        } else {
            world.resource_mut::<NewProjectState>().status =
                Some("Provide a template (local path or git URL).".into());
            return;
        };

        let name_for_task = name.clone();
        let location_for_task = location.clone();
        let url_for_task = template_url.clone();
        // Branch field only matters for git URL scaffolds; the
        // local-path branch ignores it (the working tree IS the
        // branch). An empty branch falls through to
        // `template_branch()` inside `scaffold_project`.
        let branch_for_task = if branch_from_input.is_empty() {
            None
        } else {
            Some(branch_from_input.clone())
        };

        world.resource_mut::<NewProjectState>().status = Some(format!("Scaffolding `{name}`..."));

        let task = AsyncComputeTaskPool::get().spawn(async move {
            scaffold_project(
                &name_for_task,
                &location_for_task,
                &url_for_task,
                branch_for_task.as_deref(),
                linkage,
            )
        });
        world.resource_mut::<NewProjectState>().scaffold_task = Some(task);
    });
}

fn poll_new_project_tasks(
    mut commands: Commands,
    mut state: ResMut<NewProjectState>,
    mut location_texts: Query<&mut Text, With<NewProjectLocationText>>,
    mut status_texts: Query<
        &mut Text,
        (With<NewProjectStatusText>, Without<NewProjectLocationText>),
    >,
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

    // Scaffold.
    if let Some(task) = state.scaffold_task.as_mut()
        && let Some(result) = future::block_on(future::poll_once(task))
    {
        state.scaffold_task = None;
        match result {
            Ok(project_path) => {
                info!("Scaffolded project at {}", project_path.display());
                let project_name = project_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("project")
                    .to_owned();

                // "Open editor after creating" was unchecked in the
                // modal: stop here. Close the modal and open the
                // info dialog inline via commands.queue, which runs
                // in apply_deferred (full World access, no
                // schedule_scope conflict).
                if !state.build_after_scaffold {
                    info!(
                        "Scaffolded `{project_name}` at {}; skipping build per modal toggle.",
                        project_path.display()
                    );
                    let dialog_root = project_path.clone();
                    let dialog_linkage = state.linkage;
                    let dialog_preset = state.preset.clone();
                    commands.queue(move |world: &mut World| {
                        close_new_project_modal(world);
                        open_skip_build_dialog(
                            world,
                            &dialog_root,
                            dialog_linkage,
                            dialog_preset.as_ref(),
                        );
                    });
                    state.pending_project = None;
                    state.scaffold_task = None;
                    state.status = None;
                    return;
                }

                state.status = Some(format!("Building `{project_name}` in background…"));
                state.pending_project = Some(project_path.clone());

                // Static linkage: open the editor immediately, and
                // background-build the user's `editor` binary so
                // they can later hand off into the full inspector +
                // PIE without leaving the editor session. The
                // launcher's own editor handles authoring with
                // built-in components in the meantime.
                //
                // Dylib linkage: still needs the build to finish
                // before transitioning, because the launcher
                // dlopens the cdylib at install time. The
                // build-completion branch below sets
                // `pending_install` for that path.
                let linkage = state.linkage;
                match linkage {
                    TemplateLinkage::Static => {
                        // Build the user's editor binary first;
                        // the launcher's modal stays up showing
                        // progress through the existing modal
                        // infrastructure (`refresh_build_progress_ui`
                        // reads `state.build_progress_snapshot`).
                        // When the build is `Ready`, the driver in
                        // `drive_static_editor_build` closes the
                        // modal, spawns the user's binary, and
                        // exits the launcher. We don't transition
                        // into the launcher's own editor for
                        // static templates — the launcher's editor
                        // doesn't carry the user's component types,
                        // so opening it would just confuse users.
                        //
                        // The shared `Arc<Mutex<BuildProgress>>` is
                        // assigned to `state.build_progress` here
                        // and re-used by the driver as the cargo
                        // task's sink, so both the modal and the
                        // `BuildStatus::Building` variant point at
                        // the same data.
                        let progress = std::sync::Arc::new(std::sync::Mutex::new(
                            crate::ext_build::BuildProgress::default(),
                        ));
                        state.build_progress = Some(std::sync::Arc::clone(&progress));
                        state.build_progress_snapshot =
                            Some(crate::ext_build::BuildProgress::default());
                        state.static_editor.pending = Some((project_path.clone(), true));
                    }
                    TemplateLinkage::Dylib => {
                        let progress = std::sync::Arc::new(std::sync::Mutex::new(
                            crate::ext_build::BuildProgress::default(),
                        ));
                        state.build_progress = Some(std::sync::Arc::clone(&progress));
                        state.build_progress_snapshot =
                            Some(crate::ext_build::BuildProgress::default());

                        let project_for_task = project_path;
                        let progress_for_task = std::sync::Arc::clone(&progress);
                        let task = AsyncComputeTaskPool::get().spawn(async move {
                            crate::ext_build::build_extension_project_with_progress(
                                &project_for_task,
                                Some(progress_for_task),
                                linkage,
                            )
                        });
                        state.build_task = Some((task, linkage));
                    }
                }
            }
            Err(err) => {
                warn!("Scaffold failed: {err}");
                state.status = Some(format!("Create failed: {err}"));
            }
        }
    }

    // Build task completed. Dylib: stash the artifact for the
    // install-in-Last step. Static: stash the project dir for
    // `apply_pending_static_open`, which calls `enter_project`.
    // The linkage comes off the task tuple so we branch on the
    // actual in-flight build's linkage, not on `state.linkage`
    // which describes the modal's current selection.
    if let Some((task, build_linkage)) = state.build_task.as_mut()
        && let Some(result) = future::block_on(future::poll_once(task))
    {
        let linkage = *build_linkage;
        state.build_task = None;
        match result {
            Ok(artifact_or_project) => match linkage {
                TemplateLinkage::Dylib => {
                    info!("Build produced {}", artifact_or_project.display());
                    state.pending_install = Some(artifact_or_project);
                }
                TemplateLinkage::Static => {
                    // Editor was already opened at scaffold time;
                    // this branch just logs success and clears the
                    // build-progress state so the footer collapses.
                    info!("Static build succeeded: {}", artifact_or_project.display());
                    state.pending_project = None;
                }
            },
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
    let desired_status = state.status.as_deref().unwrap_or("").to_string();
    for mut text in status_texts.iter_mut() {
        if text.0 != desired_status {
            text.0 = desired_status.clone();
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

    // "Compiling <crate>" or "Preparing…" if we don't know yet.
    let crate_line = match (&progress.current_crate, progress.artifacts_total) {
        (Some(name), Some(total)) => {
            format!("Compiling {name} ({}/{})", progress.artifacts_done, total)
        }
        (Some(name), None) => format!("Compiling {name} ({} so far)", progress.artifacts_done),
        (None, Some(total)) => format!("Preparing build… (0/{total})"),
        (None, None) => "Preparing build…".to_string(),
    };
    for mut t in crate_labels.iter_mut() {
        if t.0 != crate_line {
            t.0 = crate_line.clone();
        }
    }

    // Progress bar fill; walk slot → bar → bar children → fill.
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
/// `Last` schedule so `GameApp::add_systems(Update, …)` inside the
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

/// Open the post-scaffold info dialog telling the user how to
/// launch the project from their terminal. Called inline from
/// `poll_new_project_tasks` via `commands.queue` when the user
/// unchecked "Open editor after creating" in the modal. Replaces
/// the older `apply_pending_skip_build` system that drained a
/// `pending_skip_build_dialog` resource field.
///
/// The body adapts per linkage and preset:
///
/// - Dylib: "open from launcher when ready" (no specific command).
/// - Static + Game: `cd <path> && cargo editor`.
/// - Static + Extension: `cd <path> && cargo run`.
/// - Static + Custom: `cd <path>` with a generic "follow the
///   template's README" pointer.
fn open_skip_build_dialog(
    world: &mut World,
    root: &Path,
    linkage: TemplateLinkage,
    preset: Option<&TemplatePreset>,
) {
    let display_path = root.display().to_string();
    let project_name = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_owned();
    let description = match (linkage, preset) {
        (TemplateLinkage::Dylib, _) => format!(
            "Files written to {display_path}.\n\nOpen the project from the launcher's recent-projects list when you're ready to build the cdylib and load it into the editor."
        ),
        (TemplateLinkage::Static, Some(TemplatePreset::Game)) => format!(
            "Files written to {display_path}.\n\nTo open in jackdaw with custom components, run:\n\n    cd {display_path}\n    cargo editor\n\n(equivalent to `cargo run --bin editor --features editor`).\n\nThe launcher's recent-projects list opens the project later."
        ),
        (TemplateLinkage::Static, Some(TemplatePreset::Extension)) => format!(
            "Files written to {display_path}.\n\nTo run your extension in jackdaw, run:\n\n    cd {display_path}\n    cargo run\n\nThe launcher's recent-projects list opens the project for level editing only and does NOT auto-build the extension binary."
        ),
        (TemplateLinkage::Static, Some(TemplatePreset::Custom(_)) | None) => format!(
            "Files written to {display_path}.\n\nFollow the template's README for the right cargo command. The launcher's recent-projects list can open the project for level editing later."
        ),
    };

    world.trigger(
        jackdaw_feathers::dialog::OpenDialogEvent::new(format!("`{project_name}` created"), "OK")
            .with_description(description)
            .without_cancel(),
    );
}

/// Static-scaffold sibling of [`apply_pending_install`]. Runs in
/// `Last` and hands off to `enter_project`, which opens the fresh
/// project directly (non-cdylib, so no second build).
fn apply_pending_static_open(world: &mut World) {
    let project = world
        .resource_mut::<NewProjectState>()
        .pending_static_open
        .take();
    let Some(project) = project else {
        return;
    };
    close_new_project_modal(world);
    enter_project(world, project);
}

/// Background-build the user's static editor binary so the
/// inspector can pick up `MyGamePlugin`'s reflected types and PIE
/// can run game code in-process. Runs in `Update` in BOTH
/// `AppState`s so the build keeps progressing after the launcher
/// transitions into the editor.
///
/// Three steps:
///   1. If `pending_static_editor_build` is queued and no task is
///      running, kick off `cargo build --bin editor --features
///      editor` and flip [`BuildStatus`](crate::build_status::BuildStatus) to `Building`.
///   2. If a task is running, poll it. On completion, flip
///      `BuildStatus` to `Ready { auto_reload }` or `Failed`.
///   3. If `BuildStatus` is `Ready` AND `auto_reload`, fire
///      [`do_handoff`] once and clear the flag. Otherwise wait
///      for the user to click the footer indicator.
fn drive_static_editor_build(world: &mut World) {
    use crate::build_status::BuildState;

    // Step 1: dispatch a queued request.
    let pending = world
        .resource_mut::<NewProjectState>()
        .static_editor
        .pending
        .take();
    if let Some((root, auto_reload)) = pending {
        // Don't dispatch if another build is already running.
        let already_running = world
            .resource::<NewProjectState>()
            .static_editor
            .task
            .is_some();
        if already_running {
            // Drop the request silently; the in-flight build will
            // produce a binary the next handoff can use. A cleaner
            // design would queue, but it'd just stack same-project
            // requests; one in-flight is enough.
        } else {
            // Re-use the `Arc<Mutex<BuildProgress>>` already
            // installed on `state.build_progress` (set by the
            // post-scaffold path or the recents-click path) so the
            // launcher's modal — already wired to render
            // `state.build_progress_snapshot` — and
            // `BuildStatus::Building` both observe the same sink.
            // If neither path set one, allocate a fresh sink and
            // install it ourselves so the modal still renders if
            // it's open.
            let progress = world
                .resource::<NewProjectState>()
                .build_progress
                .as_ref()
                .map(std::sync::Arc::clone)
                .unwrap_or_else(|| {
                    std::sync::Arc::new(std::sync::Mutex::new(
                        crate::ext_build::BuildProgress::default(),
                    ))
                });
            let progress_for_task = std::sync::Arc::clone(&progress);
            let root_for_task = root.clone();
            let task = AsyncComputeTaskPool::get().spawn(async move {
                crate::ext_build::build_static_editor_with_progress(
                    &root_for_task,
                    Some(progress_for_task),
                )
            });

            {
                let mut state = world.resource_mut::<NewProjectState>();
                state.static_editor.task = Some(task);
                state.static_editor.auto_reload = auto_reload;
                if state.build_progress.is_none() {
                    state.build_progress = Some(std::sync::Arc::clone(&progress));
                    state.build_progress_snapshot =
                        Some(crate::ext_build::BuildProgress::default());
                }
            }

            world
                .resource_mut::<crate::build_status::BuildStatus>()
                .state = BuildState::Building {
                project: root,
                started: std::time::Instant::now(),
                progress,
            };
        }
    }

    // Step 2: poll the in-flight task.
    let result_opt = {
        let mut state = world.resource_mut::<NewProjectState>();
        state
            .static_editor
            .task
            .as_mut()
            .and_then(|task| future::block_on(future::poll_once(task)))
    };
    if let Some(result) = result_opt {
        let auto_reload = {
            let mut state = world.resource_mut::<NewProjectState>();
            state.static_editor.task = None;
            state.static_editor.auto_reload
        };

        let project = match &world.resource::<crate::build_status::BuildStatus>().state {
            BuildState::Building { project, .. } => project.clone(),
            _ => return,
        };

        match result {
            Ok(bin) => {
                info!("Static editor build finished: {}", bin.display());
                world
                    .resource_mut::<crate::build_status::BuildStatus>()
                    .state = BuildState::Ready {
                    project,
                    bin,
                    auto_reload,
                };
            }
            Err(err) => {
                warn!("Static editor build failed: {err}");
                world
                    .resource_mut::<crate::build_status::BuildStatus>()
                    .state = BuildState::Failed {
                    project,
                    log_tail: format!("{err}"),
                };
            }
        }
    }

    // Step 3: auto-fire the handoff once for the scaffold case.
    let auto_handoff = matches!(
        world.resource::<crate::build_status::BuildStatus>().state,
        BuildState::Ready {
            auto_reload: true,
            ..
        }
    );
    if auto_handoff {
        // Drain the Ready, do the handoff. After this `do_handoff`
        // writes `AppExit`, so we won't loop.
        let (bin, project) = match std::mem::take(
            &mut world
                .resource_mut::<crate::build_status::BuildStatus>()
                .state,
        ) {
            BuildState::Ready { bin, project, .. } => (bin, project),
            other => {
                world
                    .resource_mut::<crate::build_status::BuildStatus>()
                    .state = other;
                return;
            }
        };
        // Close the launcher modal before spawning so the user's
        // editor window comes up against a clean launcher viewport
        // (briefly, until `AppExit` tears it down).
        close_new_project_modal(world);
        do_handoff(world, &bin, &project);
    }
}

/// Save the active scene if dirty, spawn the user's static editor
/// binary, and exit the launcher process. The user's binary loads
/// the same scene file from disk on its first frame, so anything
/// the user authored in the launcher's editor round-trips
/// transparently.
///
/// Called from [`drive_static_editor_build`]'s auto-fire branch
/// (post-scaffold case) and from the status-bar click observer
/// (user-driven reload).
pub(crate) fn do_handoff(world: &mut World, bin: &Path, project_root: &Path) {
    info!(
        "Handing off to static editor {} (cwd={})",
        bin.display(),
        project_root.display()
    );
    // The user's editor binary reads its scene from disk on its
    // first frame; if the launcher has unsaved edits and a path
    // is already set, persist them first so the round-trip is
    // transparent. If no path is set (untouched scaffold or
    // unsaved-as-yet scene), skip — opening a Save As dialog
    // mid-handoff would be jarring; the user can save manually
    // before clicking Reload.
    let has_scene_path = world
        .get_resource::<scene_io::SceneFilePath>()
        .is_some_and(|p| p.path.is_some());
    if has_scene_path {
        scene_io::save_scene(world);
    }

    // Touch the project in the launcher's recents list so it
    // shows up next launch. The static-handoff path doesn't go
    // through `transition_to_editor` (which is where dylib /
    // launcher-internal opens get touch_recent'd), so without
    // this the freshly-scaffolded project disappears from the
    // launcher between sessions.
    let project_name = project::load_project_config(project_root)
        .map(|cfg| cfg.project.name)
        .unwrap_or_else(|| {
            project_root
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("project")
                .to_string()
        });
    project::touch_recent(project_root, &project_name);

    // `JACKDAW_PROJECT` tells the spawned editor binary which
    // project to auto-open on its first frame. Without this, the
    // editor's `main()` defaults to `AppState::ProjectSelect` and
    // the user lands on the launcher's picker again instead of
    // the editor view for `project_root`.
    let spawn_result = std::process::Command::new(bin)
        .current_dir(project_root)
        .env("JACKDAW_PROJECT", project_root)
        .spawn();
    match spawn_result {
        Ok(_child) => {
            info!("Static editor spawned; exiting launcher");
            world.write_message(bevy::app::AppExit::Success);
        }
        Err(e) => {
            warn!(
                "Failed to spawn static editor binary {}: {e}",
                bin.display()
            );
            // Surface to the UI so the user can retry. Reset
            // BuildStatus to Ready (without auto_reload) so the
            // footer keeps offering the click target.
            let mut status = world.resource_mut::<crate::build_status::BuildStatus>();
            if let crate::build_status::BuildState::Ready { auto_reload, .. } = &mut status.state {
                *auto_reload = false;
            }
            world.resource_mut::<NewProjectState>().status =
                Some(format!("Failed to spawn editor binary: {e}"));
        }
    }
}
