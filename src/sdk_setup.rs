//! The extension SDK: where it stands, and the build that makes it.
//!
//! Games build as their own cargo binaries and never touch the SDK; only
//! editor extensions compile against it. So nothing builds it up front. The
//! launcher shows its state with a Build SDK action, and opening an extension
//! project on an install without one starts the build in the background and
//! opens the project once it finishes. In a release bundle the SDK ships
//! prebuilt and none of this runs.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
    ui_widgets::observe,
};
use jackdaw_feathers::{
    button::{ButtonProps, button},
    icons::{EditorFont, Icon, IconFont},
    progress, tokens,
};
use jackdaw_project_build::bootstrap::{self, SdkState, SetupProgress};

use crate::AppState;
use crate::progress::{
    EditorProgress, ProgressCancelRequested, allow_cancel, begin_progress, fail_progress,
    finish_progress, progress_count, progress_stage,
};

/// The owner the SDK build reports its progress under.
const SDK_BUILD: &str = "extension sdk";

const GREEN: Color = Color::srgb(0.45, 0.80, 0.55);
const RED: Color = Color::srgb(0.92, 0.45, 0.45);

pub struct SdkSetupPlugin;

impl Plugin for SdkSetupPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SdkSetup>()
            .add_observer(on_cancel_requested)
            .add_systems(OnEnter(AppState::ProjectSelect), refresh_sdk_state)
            .add_systems(
                Update,
                (
                    poll_sdk_build,
                    refresh_sdk_row.run_if(in_state(AppState::ProjectSelect)),
                )
                    .chain(),
            );
    }
}

/// Where the SDK stands, as the launcher shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SdkStatus {
    Ready,
    NotBuilt,
    Unavailable,
    Building,
    Failed { error: String, log: Option<PathBuf> },
}

impl From<SdkState> for SdkStatus {
    fn from(state: SdkState) -> Self {
        match state {
            SdkState::Ready => Self::Ready,
            SdkState::NotBuilt => Self::NotBuilt,
            SdkState::Unavailable => Self::Unavailable,
        }
    }
}

/// Progress shared between the background build (writer) and the UI (reader).
#[derive(Default, Clone, PartialEq, Eq)]
struct SetupShared {
    phase: String,
    current_crate: Option<String>,
    done: u32,
    total: Option<u32>,
}

/// The SDK's state and the build in flight, if any.
#[derive(Resource, Default)]
pub struct SdkSetup {
    status: Option<SdkStatus>,
    shared: Option<Arc<Mutex<SetupShared>>>,
    task: Option<Task<Result<PathBuf, String>>>,
    snapshot: SetupShared,
    log: Option<PathBuf>,
    /// The extension project to open once the build finishes.
    waiting: Option<PathBuf>,
    /// Raised by the launcher's Build SDK and Retry buttons.
    start_requested: bool,
}

impl SdkSetup {
    /// The SDK's state, read from disk the first time it is asked for.
    pub fn status(&mut self) -> &SdkStatus {
        self.status
            .get_or_insert_with(|| bootstrap::sdk_state().into())
    }

    /// Ask for a build, started on the next update unless one is running.
    pub fn request_build(&mut self) {
        self.start_requested = true;
    }

    /// Whether a build is running or about to start.
    pub fn build_pending(&self) -> bool {
        self.start_requested || self.task.is_some()
    }

    /// The crate the running build last compiled and how many units it has
    /// finished.
    pub fn compiling(&self) -> (Option<&str>, u32) {
        (self.snapshot.current_crate.as_deref(), self.snapshot.done)
    }

    #[cfg(test)]
    pub(crate) fn with_status(status: SdkStatus) -> Self {
        Self {
            status: Some(status),
            ..Default::default()
        }
    }

    fn start(&mut self) {
        let shared = Arc::new(Mutex::new(SetupShared {
            phase: "Preparing".to_string(),
            ..Default::default()
        }));
        let writer = Arc::clone(&shared);
        let log_path = bootstrap::setup_log_path();
        let task_log = log_path.clone();
        self.task = Some(AsyncComputeTaskPool::get().spawn(async move {
            let mut log = task_log.as_deref().and_then(open_log);
            let result = bootstrap::ensure_sdk(|event| record(&writer, log.as_mut(), event));
            if let (Err(error), Some(file)) = (&result, log.as_mut()) {
                let _ = writeln!(file, "error: {error}");
            }
            result
        }));
        self.shared = Some(shared);
        self.snapshot = SetupShared::default();
        self.log = log_path;
        self.status = Some(SdkStatus::Building);
    }
}

fn open_log(path: &Path) -> Option<File> {
    std::fs::create_dir_all(path.parent()?).ok()?;
    File::create(path).ok()
}

fn record(shared: &Mutex<SetupShared>, log: Option<&mut File>, event: SetupProgress) {
    let line = match &event {
        SetupProgress::Phase(phase) => Some(format!("== {phase}")),
        SetupProgress::Log(line) => Some(line.clone()),
        _ => None,
    };
    if let (Some(line), Some(file)) = (line, log) {
        let _ = writeln!(file, "{line}");
    }
    let Ok(mut progress) = shared.lock() else {
        return;
    };
    match event {
        SetupProgress::Phase(phase) => {
            progress.phase = phase.to_string();
            progress.current_crate = None;
        }
        SetupProgress::Total(total) => progress.total = Some(total),
        SetupProgress::Compiled { crate_name, done } => {
            progress.current_crate = Some(crate_name);
            progress.done = done;
        }
        SetupProgress::Log(_) => {}
    }
}

/// Re-read the SDK's state each time the launcher opens, unless a build is
/// running or a failure is waiting for a retry.
fn refresh_sdk_state(mut setup: ResMut<SdkSetup>) {
    if setup.task.is_none() && !matches!(setup.status, Some(SdkStatus::Failed { .. })) {
        setup.status = None;
        setup.status();
    }
}

/// Hold an open of `root` until the SDK is built, when `root` is an
/// extension and this install has no SDK yet. Starts the build if it is not
/// already running, shows its progress, and opens the project when it
/// finishes. Returns whether the open was held; a game never is.
pub fn wait_for_sdk(world: &mut World, root: &Path) -> bool {
    let Some(mut setup) = world.get_resource_mut::<SdkSetup>() else {
        return false;
    };
    let state = match setup.status() {
        SdkStatus::Ready => SdkState::Ready,
        SdkStatus::Unavailable => SdkState::Unavailable,
        SdkStatus::NotBuilt | SdkStatus::Building | SdkStatus::Failed { .. } => SdkState::NotBuilt,
    };
    if state != SdkState::NotBuilt
        || !bootstrap::opening_waits_for_sdk(bootstrap::project_needs_sdk(root), state)
    {
        return false;
    }
    setup.waiting = Some(root.to_path_buf());
    if setup.task.is_none() {
        setup.start();
    }
    begin_progress(world, SDK_BUILD, "Building the extension SDK", true);
    allow_cancel(world, SDK_BUILD);
    progress_stage(
        world,
        SDK_BUILD,
        waiting_stage(&project_name(root), "Preparing"),
        None,
    );
    true
}

fn project_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

/// The overlay's stage line while an open waits on the build.
fn waiting_stage(project: &str, phase: &str) -> String {
    format!(
        "{phase}. {project} opens when it finishes, usually in {}.",
        bootstrap::SDK_BUILD_ESTIMATE
    )
}

/// Copy the build's progress, hand it to a waiting open's overlay, and settle
/// the build when it ends: open the waiting project, or report the failure.
fn poll_sdk_build(world: &mut World) {
    let (finished, snapshot, waiting) = {
        let mut setup = world.resource_mut::<SdkSetup>();
        if std::mem::take(&mut setup.start_requested) && setup.task.is_none() {
            setup.start();
        }
        if let Some(shared) = setup.shared.clone()
            && let Ok(latest) = shared.lock()
            && setup.snapshot != *latest
        {
            setup.snapshot = latest.clone();
        }
        let finished = setup
            .task
            .as_mut()
            .and_then(|task| future::block_on(future::poll_once(task)));
        if finished.is_some() {
            setup.task = None;
            setup.shared = None;
        }
        (finished, setup.snapshot.clone(), setup.waiting.clone())
    };

    match finished {
        None => {
            if let Some(root) = waiting {
                forward_progress(world, &project_name(&root), &snapshot);
            }
        }
        Some(Ok(cache)) => {
            info!("Extension SDK ready: {}", cache.display());
            // Nothing further in this process is owed a build, whatever the
            // stamp on disk goes on to say.
            bootstrap::skip_setup_check();
            let mut setup = world.resource_mut::<SdkSetup>();
            setup.status = Some(SdkStatus::Ready);
            if let Some(root) = setup.waiting.take() {
                finish_progress(world, SDK_BUILD);
                crate::project_select::enter_project(world, root);
            }
        }
        Some(Err(error)) => {
            warn!("Extension SDK build failed: {error}");
            let mut setup = world.resource_mut::<SdkSetup>();
            let log = setup.log.clone();
            setup.status = Some(SdkStatus::Failed {
                error: error.clone(),
                log: log.clone(),
            });
            if setup.waiting.take().is_some() {
                let mut message = format!("The extension SDK did not build: {error}");
                if let Some(log) = log {
                    message.push_str(&format!(" (log: {})", log.display()));
                }
                fail_progress(world, SDK_BUILD, message);
            }
        }
    }
}

fn forward_progress(world: &mut World, project: &str, snapshot: &SetupShared) {
    let stage = waiting_stage(project, &snapshot.phase);
    let current = world
        .get_resource::<EditorProgress>()
        .and_then(|progress| progress.get(SDK_BUILD))
        .map(|task| task.stage.clone());
    if current.as_deref() != Some(stage.as_str()) {
        progress_stage(world, SDK_BUILD, stage, None);
    }
    if let Some(total) = snapshot.total {
        let done = snapshot.done as usize;
        progress_count(world, SDK_BUILD, done, Some((total as usize).max(done)));
    }
}

/// Cancelling the wait keeps the build running; the launcher row goes on
/// showing it, and the project stays closed.
fn on_cancel_requested(
    cancel: On<ProgressCancelRequested>,
    mut setup: ResMut<SdkSetup>,
    mut commands: Commands,
) {
    if cancel.event().owner != SDK_BUILD {
        return;
    }
    setup.waiting = None;
    commands.queue(|world: &mut World| finish_progress(world, SDK_BUILD));
}

/// The launcher row the SDK's state is drawn into, under the environment
/// checks.
#[derive(Component)]
pub struct SdkStatusRow;

#[derive(Component)]
struct SdkProgressText;

#[derive(Component)]
struct SdkProgressBar;

/// What the launcher row says for one state.
#[derive(Clone, Debug)]
struct RowContent {
    icon: Icon,
    color: Color,
    headline: String,
    details: Vec<String>,
    action: Option<&'static str>,
    building: bool,
}

impl PartialEq for RowContent {
    fn eq(&self, other: &Self) -> bool {
        self.icon.unicode() == other.icon.unicode()
            && self.color == other.color
            && self.headline == other.headline
            && self.details == other.details
            && self.action == other.action
            && self.building == other.building
    }
}

fn row_content(status: &SdkStatus, waiting: Option<&Path>) -> RowContent {
    match status {
        SdkStatus::Ready => RowContent {
            icon: Icon::CircleCheck,
            color: GREEN,
            headline: "Extension SDK ready".to_string(),
            details: Vec::new(),
            action: None,
            building: false,
        },
        SdkStatus::NotBuilt => RowContent {
            icon: Icon::Info,
            color: tokens::TEXT_SECONDARY,
            headline: "Extension SDK not built".to_string(),
            details: vec![format!(
                "Only extension projects need it, and opening one builds it. \
                 Building takes {}.",
                bootstrap::SDK_BUILD_ESTIMATE
            )],
            action: Some("Build SDK"),
            building: false,
        },
        SdkStatus::Unavailable => RowContent {
            icon: Icon::Info,
            color: tokens::TEXT_SECONDARY,
            headline: "Extension SDK not available".to_string(),
            details: vec![
                "Only extension projects need it. This build of jackdaw cannot make one; \
                 jd doctor says why."
                    .to_string(),
            ],
            action: None,
            building: false,
        },
        SdkStatus::Building => RowContent {
            icon: Icon::Loader,
            color: tokens::TEXT_SECONDARY,
            headline: match waiting {
                Some(root) => format!(
                    "Building the extension SDK; {} opens when it finishes",
                    project_name(root)
                ),
                None => "Building the extension SDK".to_string(),
            },
            details: Vec::new(),
            action: None,
            building: true,
        },
        SdkStatus::Failed { error, log } => RowContent {
            icon: Icon::CircleAlert,
            color: RED,
            headline: "Extension SDK build failed".to_string(),
            details: std::iter::once(error.clone())
                .chain(log.as_ref().map(|log| format!("Log: {}", log.display())))
                .collect(),
            action: Some("Retry"),
            building: false,
        },
    }
}

/// The running build's progress, one line.
fn progress_line(snapshot: &SetupShared) -> String {
    match (&snapshot.current_crate, snapshot.total) {
        (Some(name), Some(total)) => format!(
            "{}: compiling {name} ({}/{})",
            snapshot.phase,
            snapshot.done,
            total.max(snapshot.done)
        ),
        (Some(name), None) => format!("{}: compiling {name}", snapshot.phase),
        (None, _) => snapshot.phase.clone(),
    }
}

fn progress_fraction(snapshot: &SetupShared) -> f32 {
    match snapshot.total {
        Some(total) if total > 0 => (snapshot.done as f32 / total as f32).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// Redraw the row when what it says changes, and move its progress text and
/// bar along while a build runs.
fn refresh_sdk_row(
    mut commands: Commands,
    mut setup: ResMut<SdkSetup>,
    rows: Query<(Entity, Option<&Children>), With<SdkStatusRow>>,
    added: Query<(), Added<SdkStatusRow>>,
    mut shown: Local<Option<RowContent>>,
    editor_font: Res<EditorFont>,
    icon_font: Res<IconFont>,
    mut texts: Query<&mut Text, With<SdkProgressText>>,
    bars: Query<&Children, With<SdkProgressBar>>,
    children_q: Query<&Children>,
    mut fills: Query<&mut Node, With<progress::ProgressBarFill>>,
) {
    let Ok((row, children)) = rows.single() else {
        return;
    };
    let waiting = setup.waiting.clone();
    let content = row_content(setup.status(), waiting.as_deref());
    if added.contains(row) || shown.as_ref() != Some(&content) {
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).despawn();
            }
        }
        spawn_row(&mut commands, row, &content, &editor_font.0, &icon_font.0);
        *shown = Some(content.clone());
    }
    if !content.building {
        return;
    }
    let line = progress_line(&setup.snapshot);
    for mut text in &mut texts {
        if text.0 != line {
            text.0.clone_from(&line);
        }
    }
    let width = Val::Percent(progress_fraction(&setup.snapshot) * 100.0);
    for bar_children in &bars {
        for bar in bar_children.iter() {
            let Ok(inner) = children_q.get(bar) else {
                continue;
            };
            for fill in inner.iter() {
                if let Ok(mut node) = fills.get_mut(fill)
                    && node.width != width
                {
                    node.width = width;
                }
            }
        }
    }
}

fn spawn_row(
    commands: &mut Commands,
    row: Entity,
    content: &RowContent,
    font: &Handle<Font>,
    icon_font: &Handle<Font>,
) {
    let line = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                ..Default::default()
            },
            ChildOf(row),
        ))
        .id();
    commands.spawn((
        Text::new(String::from(content.icon.unicode())),
        TextFont {
            font: icon_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(content.color),
        ChildOf(line),
    ));
    commands.spawn((
        Text::new(content.headline.clone()),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(content.color),
        ChildOf(line),
    ));
    if let Some(label) = content.action {
        commands.spawn((
            button(ButtonProps::new(label)),
            observe(|click: On<Pointer<Click>>, mut setup: ResMut<SdkSetup>| {
                if click.event().button == PointerButton::Primary {
                    setup.start_requested = true;
                }
            }),
            ChildOf(line),
        ));
    }
    for detail in &content.details {
        spawn_detail(commands, row, Text::new(detail.clone()), font);
    }
    if content.building {
        spawn_detail(
            commands,
            row,
            (SdkProgressText, Text::new(String::new())),
            font,
        );
        let slot = commands
            .spawn((
                SdkProgressBar,
                Node {
                    width: Val::Px(320.0),
                    margin: UiRect::left(Val::Px(22.0)),
                    ..Default::default()
                },
                ChildOf(row),
            ))
            .id();
        commands.spawn((progress::progress_bar(0.0), ChildOf(slot)));
    }
}

/// An indented secondary line under the row's headline.
fn spawn_detail(commands: &mut Commands, row: Entity, text: impl Bundle, font: &Handle<Font>) {
    commands.spawn((
        text,
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            margin: UiRect::left(Val::Px(22.0)),
            ..Default::default()
        },
        ChildOf(row),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unbuilt_sdk_offers_a_build_and_says_only_extensions_need_it() {
        let content = row_content(&SdkStatus::NotBuilt, None);
        assert_eq!(content.action, Some("Build SDK"));
        let details = content.details.join(" ");
        assert!(details.contains("Only extension projects need it"));
        assert!(details.contains(bootstrap::SDK_BUILD_ESTIMATE));
    }

    #[test]
    fn a_ready_sdk_offers_nothing() {
        let content = row_content(&SdkStatus::Ready, None);
        assert_eq!(content.headline, "Extension SDK ready");
        assert_eq!(content.action, None);
        assert!(content.details.is_empty());
    }

    #[test]
    fn a_failed_build_names_its_log_and_offers_a_retry() {
        let status = SdkStatus::Failed {
            error: "missing prerequisites: cmake".to_string(),
            log: Some(PathBuf::from("/data/jackdaw/sdk/setup.log")),
        };
        let content = row_content(&status, None);
        assert_eq!(content.action, Some("Retry"));
        assert!(content.details.iter().any(|line| line.contains("cmake")));
        assert!(
            content
                .details
                .iter()
                .any(|line| line == "Log: /data/jackdaw/sdk/setup.log")
        );
    }

    #[test]
    fn a_running_build_names_the_project_waiting_on_it() {
        let content = row_content(&SdkStatus::Building, Some(Path::new("/p/my_tool")));
        assert!(content.building);
        assert!(content.headline.contains("my_tool opens when it finishes"));
    }

    #[test]
    fn build_progress_reads_as_phase_crate_and_count() {
        let snapshot = SetupShared {
            phase: "Building the SDK".to_string(),
            current_crate: Some("bevy_render".to_string()),
            done: 120,
            total: Some(100),
        };
        assert_eq!(
            progress_line(&snapshot),
            "Building the SDK: compiling bevy_render (120/120)"
        );
        assert_eq!(progress_fraction(&snapshot), 1.0);
    }

    #[test]
    fn opening_a_game_with_no_sdk_proceeds() {
        let root =
            std::env::temp_dir().join(format!("jackdaw-sdk-game-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"sdk-game-open\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [workspace]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "use bevy::prelude::*;\npub struct GamePlugin;\n\
             impl Plugin for GamePlugin {\n    fn build(&self, _app: &mut App) {}\n}\n",
        )
        .unwrap();

        let mut world = World::new();
        world.insert_resource(SdkSetup {
            status: Some(SdkStatus::NotBuilt),
            ..Default::default()
        });
        assert!(!wait_for_sdk(&mut world, &root));
        let setup = world.resource::<SdkSetup>();
        assert!(setup.task.is_none(), "no build starts for a game");
        assert!(setup.waiting.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn opening_anything_with_a_ready_sdk_proceeds() {
        let mut world = World::new();
        world.insert_resource(SdkSetup {
            status: Some(SdkStatus::Ready),
            ..Default::default()
        });
        assert!(!wait_for_sdk(
            &mut world,
            Path::new("/nonexistent/extension")
        ));
    }
}
