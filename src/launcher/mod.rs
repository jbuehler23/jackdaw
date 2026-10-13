//! The jackdaw launcher: the editor's first window. It lists projects with
//! the editor each one opens with, the installed editors grouped by Bevy
//! line, and the extensions a project uses, and creates new projects.
//!
//! Whichever installed editor is launched shows the launcher; opening a
//! project pinned to another install relaunches into that install. Every
//! download and install runs `jd` as a subprocess, so the editor stays free
//! of TLS. Every action is an operator under `launcher.`.

pub mod jd;
pub mod model;
mod ops;
mod pages;
mod project_actions;
pub mod ui;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;
use jackdaw_api::prelude::*;
use jackdaw_feathers::text_edit::TextEditValue;
use jackdaw_project_build::modules::{ModuleCatalog, ModuleSet};
use jackdaw_releases::Channel;

pub use jd::{IndexStatus, JdJobs, JdProgram, JobKind, JobStatus};
pub use model::{EditorFilter, LauncherData, ProjectEditor, ProjectFilter};
pub use ops::*;
pub use pages::extensions::apply_project_overrides;
pub use project_actions::{EditorHandoff, OpenChoice, open_project, record_open, show_open_with};

use crate::AppState;

pub struct LauncherPlugin;

impl Plugin for LauncherPlugin {
    fn build(&self, app: &mut App) {
        let windowed = app.is_plugin_added::<bevy::winit::WinitPlugin>();
        app.init_resource::<LauncherPage>()
            .init_resource::<ProjectListView>()
            .init_resource::<EditorListView>()
            .init_resource::<LauncherDialog>()
            .init_resource::<NewProjectForm>()
            .init_resource::<ExtensionsView>()
            .init_resource::<JdJobs>()
            .init_resource::<JdProgram>()
            .init_resource::<IndexStatus>()
            .init_resource::<EditorHandoff>()
            .init_resource::<LauncherNotice>()
            .insert_resource(Launcher(LauncherData::read()))
            .add_observer(dispatch_toggle_operator)
            .add_systems(
                OnEnter(AppState::ProjectSelect),
                (
                    refresh_launcher_data,
                    start_index_refresh.run_if(move || windowed),
                ),
            )
            .add_systems(
                Update,
                (
                    poll_jd_jobs,
                    project_actions::poll_project_copies,
                    ops::poll_folder_pick,
                    ops::poll_extension_pick,
                    pages::poll_new_project,
                    sync_launcher_fields,
                    close_dialog_on_escape,
                    pages::sync_dialog,
                ),
            )
            .add_systems(
                Update,
                (
                    pages::rebuild_header,
                    pages::rebuild_nav,
                    pages::rebuild_page,
                    pages::rebuild_project_list,
                    pages::rebuild_editor_list,
                    pages::rebuild_new_project,
                    pages::rebuild_extensions,
                    pages::fill_text_slots,
                )
                    .chain()
                    .run_if(any_with_component::<LauncherShell>),
            );
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ops::add_to_extension(ctx);
}

/// Fill the launcher window: the title bar's name and header buttons, and
/// the body's sidebar and page.
pub fn fill_launcher(
    commands: &mut Commands,
    title_bar: Entity,
    body: Entity,
    icon: Handle<Image>,
) {
    commands
        .entity(title_bar)
        .with_child(crate::windowing::title_bar_repo_link(icon));
    commands
        .spawn_scene(pages::title_bar())
        .insert(ChildOf(title_bar));
    commands
        .spawn_scene(pages::body())
        .insert((LauncherShell, ChildOf(body)));
}

/// The launcher's body; while it exists, the launcher's pages are kept up
/// to date.
#[derive(Component, Default, Clone)]
pub struct LauncherShell;

/// The launcher's state from disk: projects, installs and the cached index.
#[derive(Resource, Clone, Debug, PartialEq, Eq, Deref)]
pub struct Launcher(pub LauncherData);

/// Which page the launcher shows.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LauncherPage {
    #[default]
    Projects,
    Editors,
    Extensions,
    NewProject,
}

impl LauncherPage {
    pub fn id(self) -> &'static str {
        match self {
            Self::Projects => "projects",
            Self::Editors => "editors",
            Self::Extensions => "extensions",
            Self::NewProject => "new-project",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        [
            Self::Projects,
            Self::Editors,
            Self::Extensions,
            Self::NewProject,
        ]
        .into_iter()
        .find(|page| page.id() == id)
    }
}

/// How the project list is filtered.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectListView {
    pub filter: ProjectFilter,
    pub query: String,
}

/// How the editor list is filtered.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorListView {
    pub filter: EditorFilter,
}

/// The dialog over the launcher, if any.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub enum LauncherDialog {
    #[default]
    None,
    InstallEditor(InstallForm),
    OpenWith(OpenWithForm),
    Uninstall {
        version: String,
        projects: Vec<PathBuf>,
    },
    Doctor,
    Settings,
    Downloads,
}

/// The Install editor dialog's choices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallForm {
    pub channel: Channel,
    /// The release to install, as `jd editor install` takes it.
    pub version: String,
    pub show_older: bool,
    pub modules: ModuleSet,
    /// Adding modules to this existing install rather than installing.
    pub existing: Option<String>,
}

/// The Open with a different editor dialog's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenWithForm {
    pub project: PathBuf,
    pub choice: OpenChoice,
}

/// A template in the New project gallery.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Template {
    #[default]
    ThreeD,
    TwoD,
    Empty,
    Extension,
}

impl Template {
    pub const ALL: [Self; 4] = [Self::ThreeD, Self::TwoD, Self::Empty, Self::Extension];

    pub fn id(self) -> &'static str {
        match self {
            Self::ThreeD => "3d",
            Self::TwoD => "2d",
            Self::Empty => "empty",
            Self::Extension => "extension",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|template| template.id() == id)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::ThreeD => "3D game",
            Self::TwoD => "2D game",
            Self::Empty => "Empty",
            Self::Extension => "Extension",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Self::ThreeD => "Camera, sun and scene",
            Self::TwoD => "Camera2d and a sprite",
            Self::Empty => "GamePlugin only",
            Self::Extension => "Editor tool dylib",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::ThreeD => {
                "A scene with a camera and a sun, and a GamePlugin that loads it. Plays in \
                 the editor right away."
            }
            Self::TwoD => "A scene with a 2D camera and a sprite, and a GamePlugin that loads it.",
            Self::Empty => {
                "An empty scene and a GamePlugin, for a game that builds its world in code."
            }
            Self::Extension => {
                "An editor extension: a library the editor loads to add panels, tools and \
                 operators."
            }
        }
    }

    pub fn kind(self) -> crate::scaffold::TemplateKind {
        match self {
            Self::Extension => crate::scaffold::TemplateKind::Extension,
            _ => crate::scaffold::TemplateKind::Game,
        }
    }

    pub fn starter(self) -> crate::scaffold::StarterScene {
        match self {
            Self::TwoD => crate::scaffold::StarterScene::TwoD,
            Self::Empty => crate::scaffold::StarterScene::Empty,
            _ => crate::scaffold::StarterScene::ThreeD,
        }
    }
}

/// The New project page's choices.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct NewProjectForm {
    /// The editor version to create it with; this editor when `None`.
    pub editor: Option<String>,
    pub template: Template,
    pub modules: ModuleSet,
    pub name: String,
    pub location: PathBuf,
    pub git: bool,
    /// The last error or progress line.
    pub status: Option<String>,
    pub creating: bool,
}

impl Default for NewProjectForm {
    fn default() -> Self {
        Self {
            editor: None,
            template: Template::default(),
            modules: ModuleCatalog::builtin().defaults(),
            name: "my_game".into(),
            location: crate::project::read_last_new_project_location()
                .unwrap_or_else(default_projects_dir),
            git: true,
            status: None,
            creating: false,
        }
    }
}

fn default_projects_dir() -> PathBuf {
    dirs::home_dir()
        .map(|home| home.join("Projects"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// What an Extensions page entry is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExtensionItem {
    Module(String),
    Extension(String),
}

impl ExtensionItem {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Module(_) => "module",
            Self::Extension(_) => "extension",
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Module(id) | Self::Extension(id) => id,
        }
    }

    pub fn parse(kind: &str, id: &str) -> Option<Self> {
        match kind {
            "module" => Some(Self::Module(id.to_string())),
            "extension" => Some(Self::Extension(id.to_string())),
            _ => None,
        }
    }
}

/// Which Extensions page entries are listed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtensionFilter {
    #[default]
    All,
    On,
    Off,
    NeedsAttention,
}

impl ExtensionFilter {
    pub const ALL: [Self; 4] = [Self::All, Self::On, Self::Off, Self::NeedsAttention];

    pub fn id(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::On => "on",
            Self::Off => "off",
            Self::NeedsAttention => "needs-attention",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|filter| filter.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::On => "On",
            Self::Off => "Off",
            Self::NeedsAttention => "Needs attention",
        }
    }
}

/// The Extensions page's state.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtensionsView {
    /// The project whose overrides are edited; the global defaults when
    /// `None`.
    pub project: Option<PathBuf>,
    pub filter: ExtensionFilter,
    pub query: String,
    pub selected: Option<ExtensionItem>,
    /// Changes not applied yet: each entry's new state.
    pub pending: BTreeMap<ExtensionItem, bool>,
    /// What the last apply did, or why it failed.
    pub status: Option<String>,
}

/// The message shown across the top of the launcher's page.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct LauncherNotice(pub Option<Notice>);

/// One launcher message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub error: bool,
}

/// Tell the person at the launcher, and log it.
pub fn notify_info(world: &mut World, text: impl Into<String>) {
    let text = text.into();
    info!("{text}");
    set_notice(world, text, false);
}

/// Tell the person at the launcher that something failed, and log it.
pub fn notify_error(world: &mut World, text: impl Into<String>) {
    let text = text.into();
    error!("{text}");
    set_notice(world, text, true);
}

fn set_notice(world: &mut World, text: String, error: bool) {
    let notice = Some(Notice { text, error });
    let mut current = world.get_resource_or_insert_with(LauncherNotice::default);
    if current.0 != notice {
        current.0 = notice;
    }
}

/// A text field on a launcher page, and which state it edits.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LauncherField {
    #[default]
    ProjectQuery,
    ExtensionQuery,
    NewProjectName,
    NewProjectLocation,
}

/// Read the launcher's state from disk again, from a world.
pub fn refresh_launcher(world: &mut World) {
    let data = LauncherData::read();
    match world.get_resource_mut::<Launcher>() {
        Some(mut launcher) if launcher.0 != data => launcher.0 = data,
        Some(_) => {}
        None => world.insert_resource(Launcher(data)),
    }
}

/// Read the launcher's state from disk again.
pub fn refresh_launcher_data(mut launcher: ResMut<Launcher>) {
    let data = LauncherData::read();
    if launcher.0 != data {
        launcher.0 = data;
    }
}

/// Ask `jd` for the release index once the launcher opens. Only reported;
/// nothing is downloaded.
fn start_index_refresh(world: &mut World) {
    if world.contains_resource::<crate::project_select::PendingAutoOpen>() {
        return;
    }
    if world.resource::<IndexStatus>() != &IndexStatus::Unknown {
        return;
    }
    *world.resource_mut::<IndexStatus>() = IndexStatus::Checking;
    jd::start_job(
        world,
        JobKind::RefreshIndex,
        vec!["editor".into(), "refresh".into(), "--json".into()],
        None,
    );
}

/// Apply what running `jd` jobs printed, then refresh the launcher's state
/// after each one finishes and run what was queued after it.
fn poll_jd_jobs(world: &mut World) {
    if !world
        .resource::<JdJobs>()
        .jobs
        .iter()
        .any(jd::JdJob::is_running)
    {
        return;
    }
    let mut status = world.resource::<IndexStatus>().clone();
    let (finished, changed) = {
        let mut jobs = world.resource_mut::<JdJobs>();
        jd::drain_jobs(jobs.bypass_change_detection(), &mut status)
    };
    if changed {
        world.resource_mut::<JdJobs>().set_changed();
    }
    if world.resource::<IndexStatus>() != &status {
        *world.resource_mut::<IndexStatus>() = status;
    }
    if finished.is_empty() {
        return;
    }
    let data = LauncherData::read();
    if world.resource::<Launcher>().0 != data {
        world.resource_mut::<Launcher>().0 = data;
    }
    for job in finished {
        if !job.succeeded {
            if matches!(job.kind, JobKind::NewProject { .. }) {
                let reason = match world
                    .resource::<JdJobs>()
                    .get(job.id)
                    .map(|job| &job.status)
                {
                    Some(JobStatus::Failed(reason)) => reason.clone(),
                    _ => "The project was not created".into(),
                };
                let mut form = world.resource_mut::<NewProjectForm>();
                form.creating = false;
                form.status = Some(reason);
            }
            continue;
        }
        match job.then {
            Some(jd::AfterJob::OpenProject(project)) => open_project(world, project),
            Some(jd::AfterJob::OpenWith { project, version }) => {
                project_actions::open_with_install(world, project, &version);
            }
            Some(jd::AfterJob::NewProject {
                project,
                version,
                starter,
            }) => {
                if let Err(error) = crate::scaffold::write_starter_scene(&project, starter) {
                    notify_error(world, error.to_string());
                }
                let name = project
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                crate::project::touch_recent(&project, &name);
                world.resource_mut::<NewProjectForm>().creating = false;
                project_actions::open_with_install(world, project, &version);
            }
            None => {}
        }
    }
}

/// Copy what was typed into a launcher text field into the state it edits.
fn sync_launcher_fields(
    fields: Query<(&LauncherField, &TextEditValue), Changed<TextEditValue>>,
    mut projects: ResMut<ProjectListView>,
    mut extensions: ResMut<ExtensionsView>,
    mut form: ResMut<NewProjectForm>,
) {
    for (field, value) in &fields {
        let text = value.0.clone();
        match field {
            LauncherField::ProjectQuery => {
                if projects.query != text {
                    projects.query = text;
                }
            }
            LauncherField::ExtensionQuery => {
                if extensions.query != text {
                    extensions.query = text;
                }
            }
            LauncherField::NewProjectName => {
                if form.name != text {
                    form.name = text;
                }
            }
            LauncherField::NewProjectLocation => {
                let location = PathBuf::from(text);
                if form.location != location {
                    form.location = location;
                }
            }
        }
    }
}

fn close_dialog_on_escape(keys: Res<ButtonInput<KeyCode>>, mut dialog: ResMut<LauncherDialog>) {
    if keys.just_pressed(KeyCode::Escape) && *dialog != LauncherDialog::None {
        *dialog = LauncherDialog::None;
    }
}

/// Run the operator a checkbox, radio or switch names when it changes.
fn dispatch_toggle_operator(
    change: On<ValueChange<bool>>,
    calls: Query<&ui::ToggleOperatorCall>,
    mut commands: Commands,
) {
    let Ok(ui::ToggleOperatorCall(call)) = calls.get(change.source) else {
        return;
    };
    let id = call.id.to_string();
    let mut params: Vec<(String, jackdaw_scene_types::PropertyValue)> = call
        .params
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect();
    params.push(("enabled".into(), change.value.into()));
    commands.queue(move |world: &mut World| {
        let mut call = world.operator(id.clone());
        for (key, value) in params {
            call = call.param(key, value);
        }
        if let Err(error) = call.call() {
            error!("operator dispatch failed for `{id}`: {error}");
        }
    });
}

/// The project at `path`, as the launcher's last read of it describes it.
pub(crate) fn project_entry(world: &World, path: &Path) -> Option<model::ProjectEntry> {
    world.resource::<Launcher>().project(path).cloned()
}
