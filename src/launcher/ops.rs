//! The launcher's operators. Each one changes the launcher's state or
//! queues the work it names; nothing here calls another operator.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use bevy::window::{PrimaryWindow, RawHandleWrapper};
use jackdaw_api::prelude::*;
use jackdaw_project_build::modules::ModuleCatalog;
use jackdaw_releases::Channel;

use super::jd::{self, JobKind};
use super::model::{EditorFilter, ProjectFilter};
use super::project_actions::{self, OpenChoice};
use super::{
    ExtensionFilter, ExtensionItem, ExtensionsView, InstallForm, Launcher, LauncherDialog,
    LauncherPage, NewProjectForm, OpenWithForm, Template,
};

pub(super) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<LauncherShowPageOp>()
        .register_operator::<LauncherProjectFilterOp>()
        .register_operator::<LauncherEditorFilterOp>()
        .register_operator::<LauncherOpenProjectOp>()
        .register_operator::<LauncherOpenWithOp>()
        .register_operator::<LauncherOpenWithChoiceOp>()
        .register_operator::<LauncherOpenWithConfirmOp>()
        .register_operator::<LauncherRemoveProjectOp>()
        .register_operator::<LauncherRevealProjectOp>()
        .register_operator::<LauncherAddProjectOp>()
        .register_operator::<LauncherOpenFolderOp>()
        .register_operator::<LauncherImportProjectOp>()
        .register_operator::<LauncherNewProjectOp>()
        .register_operator::<LauncherNewProjectTemplateOp>()
        .register_operator::<LauncherNewProjectEditorOp>()
        .register_operator::<LauncherNewProjectModuleOp>()
        .register_operator::<LauncherNewProjectGitOp>()
        .register_operator::<LauncherNewProjectBrowseOp>()
        .register_operator::<LauncherNewProjectCreateOp>()
        .register_operator::<LauncherInstallEditorOp>()
        .register_operator::<LauncherInstallChannelOp>()
        .register_operator::<LauncherInstallVersionOp>()
        .register_operator::<LauncherInstallShowOlderOp>()
        .register_operator::<LauncherInstallModuleOp>()
        .register_operator::<LauncherInstallConfirmOp>()
        .register_operator::<LauncherUpdateEditorOp>()
        .register_operator::<LauncherSetDefaultEditorOp>()
        .register_operator::<LauncherUninstallEditorOp>()
        .register_operator::<LauncherReleaseNotesOp>()
        .register_operator::<LauncherRevealEditorOp>()
        .register_operator::<LauncherVerifyEditorOp>()
        .register_operator::<LauncherEditorModulesOp>()
        .register_operator::<LauncherLocateEditorOp>()
        .register_operator::<LauncherCleanNightliesOp>()
        .register_operator::<LauncherRefreshIndexOp>()
        .register_operator::<LauncherDoctorOp>()
        .register_operator::<LauncherRecheckEnvironmentOp>()
        .register_operator::<LauncherSettingsOp>()
        .register_operator::<LauncherDownloadsOp>()
        .register_operator::<LauncherCancelJobOp>()
        .register_operator::<LauncherCloseDialogOp>()
        .register_operator::<LauncherDismissNoticeOp>()
        .register_operator::<LauncherExtensionsOp>()
        .register_operator::<LauncherExtensionsFilterOp>()
        .register_operator::<LauncherExtensionsSelectOp>()
        .register_operator::<LauncherExtensionsToggleOp>()
        .register_operator::<LauncherExtensionsRevertOp>()
        .register_operator::<LauncherExtensionsApplyOp>()
        .register_operator::<LauncherInstallExtensionOp>();
}

fn path_param(params: &OperatorParameters, id: &str) -> Option<PathBuf> {
    let path = params.as_str("path").filter(|path| !path.is_empty());
    if path.is_none() {
        warn!("{id}: name a project with path=");
    }
    path.map(PathBuf::from)
}

fn version_param(params: &OperatorParameters, id: &str) -> Option<String> {
    let version = params
        .as_str("version")
        .filter(|version| !version.is_empty());
    if version.is_none() {
        warn!("{id}: name an editor with version=");
    }
    version.map(str::to_string)
}

/// Show one of the launcher's pages.
#[operator(
    id = "launcher.show_page",
    label = "Show Launcher Page",
    description = "Show the launcher's Projects, Editors, Extensions or New project page.",
    allows_undo = false,
    params(page(String, doc = "projects, editors, extensions or new-project."))
)]
pub fn launcher_show_page(
    params: In<OperatorParameters>,
    mut page: ResMut<LauncherPage>,
) -> OperatorResult {
    let Some(next) = params.as_str("page").and_then(LauncherPage::parse) else {
        warn!("launcher.show_page: page= is projects, editors, extensions or new-project");
        return OperatorResult::Cancelled;
    };
    page.set_if_neq(next);
    OperatorResult::Finished
}

/// Filter the project list.
#[operator(
    id = "launcher.project_filter",
    label = "Filter Projects",
    description = "Show all projects, the Stable or Bleeding Edge ones, or those whose editor \
                   is not installed, optionally only those matching some text.",
    allows_undo = false,
    params(
        filter(String, doc = "all, stable, bleeding-edge or needs-editor."),
        query(String, doc = "Text the name or path contains.")
    )
)]
pub fn launcher_project_filter(
    params: In<OperatorParameters>,
    mut view: ResMut<super::ProjectListView>,
) -> OperatorResult {
    let mut next = view.clone();
    if let Some(filter) = params.as_str("filter") {
        let Some(filter) = ProjectFilter::parse(filter) else {
            warn!("launcher.project_filter: unknown filter `{filter}`");
            return OperatorResult::Cancelled;
        };
        next.filter = filter;
    }
    if let Some(query) = params.as_str("query") {
        next.query = query.to_string();
    }
    view.set_if_neq(next);
    OperatorResult::Finished
}

/// Filter the editor list.
#[operator(
    id = "launcher.editor_filter",
    label = "Filter Editors",
    description = "Show all installed editors, or the Stable or Bleeding Edge ones.",
    allows_undo = false,
    params(filter(String, doc = "all, stable or bleeding-edge."))
)]
pub fn launcher_editor_filter(
    params: In<OperatorParameters>,
    mut view: ResMut<super::EditorListView>,
) -> OperatorResult {
    let Some(filter) = params.as_str("filter").and_then(EditorFilter::parse) else {
        warn!("launcher.editor_filter: filter= is all, stable or bleeding-edge");
        return OperatorResult::Cancelled;
    };
    view.set_if_neq(super::EditorListView { filter });
    OperatorResult::Finished
}

/// Open a project with the editor its pin selects.
#[operator(
    id = "launcher.open_project",
    label = "Open Project",
    description = "Open a project with the editor its jackdaw.toml names: here, in another \
                   installed editor, or after choosing what to do when that editor is missing.",
    allows_undo = false,
    params(path(String, doc = "The project folder."))
)]
pub fn launcher_open_project(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(path) = path_param(&params, "launcher.open_project") else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| project_actions::open_project(world, path));
    OperatorResult::Finished
}

/// Choose which editor opens a project.
#[operator(
    id = "launcher.open_with",
    label = "Open With...",
    description = "Choose how to open a project: install its editor, open a copy or convert it \
                   with an installed editor, or look at its scenes here.",
    allows_undo = false,
    params(path(String, doc = "The project folder."))
)]
pub fn launcher_open_with(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(path) = path_param(&params, "launcher.open_with") else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| project_actions::show_open_with(world, path));
    OperatorResult::Finished
}

/// Pick an option in the Open with dialog.
#[operator(
    id = "launcher.open_with_choice",
    label = "Choose How To Open",
    description = "Pick an option in the Open with a different editor dialog.",
    allows_undo = false,
    params(
        choice(String, doc = "install, copy, convert or scenes."),
        version(String, doc = "The installed editor a copy or conversion moves to.")
    )
)]
pub fn launcher_open_with_choice(
    params: In<OperatorParameters>,
    launcher: Res<Launcher>,
    mut dialog: ResMut<LauncherDialog>,
) -> OperatorResult {
    let LauncherDialog::OpenWith(form) = &*dialog else {
        return OperatorResult::Cancelled;
    };
    let version = params
        .as_str("version")
        .filter(|version| !version.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| launcher.newest_editor());
    let choice = match params.as_str("choice") {
        Some("install") => OpenChoice::InstallAndOpen,
        Some("copy") => OpenChoice::OpenCopy { version },
        Some("convert") => OpenChoice::Convert { version },
        Some("scenes") => OpenChoice::ScenesOnly,
        _ => {
            warn!("launcher.open_with_choice: choice= is install, copy, convert or scenes");
            return OperatorResult::Cancelled;
        }
    };
    let next = LauncherDialog::OpenWith(OpenWithForm {
        project: form.project.clone(),
        choice,
    });
    dialog.set_if_neq(next);
    OperatorResult::Finished
}

/// Carry out the Open with dialog's choice.
#[operator(
    id = "launcher.open_with_confirm",
    label = "Continue",
    description = "Carry out the option picked in the Open with a different editor dialog.",
    allows_undo = false
)]
pub fn launcher_open_with_confirm(
    _: In<OperatorParameters>,
    dialog: Res<LauncherDialog>,
    mut commands: Commands,
) -> OperatorResult {
    let LauncherDialog::OpenWith(form) = &*dialog else {
        return OperatorResult::Cancelled;
    };
    let form = form.clone();
    commands.queue(move |world: &mut World| project_actions::confirm_open_with(world, form));
    OperatorResult::Finished
}

/// Forget a project in the launcher's list.
#[operator(
    id = "launcher.remove_project",
    label = "Remove From List",
    description = "Forget a project in the launcher's list. Its folder is left alone.",
    allows_undo = false,
    params(path(String, doc = "The project folder."))
)]
pub fn launcher_remove_project(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(path) = path_param(&params, "launcher.remove_project") else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        crate::project::remove_recent(&path);
        refresh(world);
    });
    OperatorResult::Finished
}

/// Show a project's folder in the file manager.
#[operator(
    id = "launcher.reveal_project",
    label = "Show in Files",
    description = "Show a project's folder in the system file manager.",
    allows_undo = false,
    params(path(String, doc = "The project folder."))
)]
pub fn launcher_reveal_project(params: In<OperatorParameters>) -> OperatorResult {
    let Some(path) = path_param(&params, "launcher.reveal_project") else {
        return OperatorResult::Cancelled;
    };
    if let Err(error) = crate::project_window::open_with_desktop(&path) {
        warn!("launcher.reveal_project: {}: {error}", path.display());
    }
    OperatorResult::Finished
}

/// A `.jdext` file picker the launcher is waiting on.
#[derive(Resource)]
pub struct LauncherExtensionPick(Task<Option<rfd::FileHandle>>);

/// Install the extension bundle the person picked.
pub fn poll_extension_pick(world: &mut World) {
    let Some(mut pick) = world.get_resource_mut::<LauncherExtensionPick>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut pick.0)) else {
        return;
    };
    world.remove_resource::<LauncherExtensionPick>();
    if let Some(handle) = result {
        install_extension(world, handle.path().to_path_buf());
    }
}

fn install_extension(world: &mut World, path: PathBuf) {
    match crate::extensions_dialog::handle_install_from_path(world, path.clone()) {
        Ok(id) => super::notify_info(world, format!("Installed the extension `{id}`.")),
        Err(error) => super::notify_error(
            world,
            format!("Could not install {}: {error}", path.display()),
        ),
    }
    refresh(world);
    world.resource_mut::<ExtensionsView>().set_changed();
}

/// Install an editor extension bundle.
#[operator(
    id = "launcher.install_extension",
    label = "Install Extension...",
    description = "Install a signed .jdext editor extension bundle into this editor.",
    allows_undo = false,
    params(path(String, doc = "The .jdext file. Asks for one when absent."))
)]
pub fn launcher_install_extension(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let path = params
        .as_str("path")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    commands.queue(move |world: &mut World| match path {
        Some(path) => install_extension(world, path),
        None => {
            let mut windows = world.query_filtered::<&RawHandleWrapper, With<PrimaryWindow>>();
            let window = windows.iter(world).next().cloned();
            let dialog = crate::native_dialog::dialog_at(dirs::download_dir(), window.as_ref())
                .set_title("Install an extension")
                .add_filter("Jackdaw extension", &["jdext"]);
            let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
            world.insert_resource(LauncherExtensionPick(task));
        }
    });
    OperatorResult::Finished
}

/// What a folder picked in the launcher is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickPurpose {
    AddProject,
    NewProjectLocation,
    LocateEditor,
}

/// A folder picker the launcher is waiting on.
#[derive(Resource)]
pub struct LauncherFolderPick {
    task: Task<Option<rfd::FileHandle>>,
    purpose: PickPurpose,
}

fn pick_folder(world: &mut World, title: &str, start: Option<PathBuf>, purpose: PickPurpose) {
    let mut windows = world.query_filtered::<&RawHandleWrapper, With<PrimaryWindow>>();
    let window = windows.iter(world).next().cloned();
    let dialog = crate::native_dialog::dialog_at(start, window.as_ref()).set_title(title);
    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_folder().await });
    world.insert_resource(LauncherFolderPick { task, purpose });
}

/// Act on a folder the person picked.
pub fn poll_folder_pick(world: &mut World) {
    let Some(mut pick) = world.get_resource_mut::<LauncherFolderPick>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut pick.task)) else {
        return;
    };
    let purpose = pick.purpose;
    world.remove_resource::<LauncherFolderPick>();
    let Some(handle) = result else {
        return;
    };
    let path = handle.path().to_path_buf();
    match purpose {
        PickPurpose::AddProject => add_project(world, path),
        PickPurpose::NewProjectLocation => {
            let mut form = world.resource_mut::<NewProjectForm>();
            if form.location != path {
                form.location = path;
            }
            world.insert_resource(super::pages::NewProjectFieldsStale);
        }
        PickPurpose::LocateEditor => {
            jd::start_job(
                world,
                JobKind::Locate,
                vec![
                    "editor".into(),
                    "locate".into(),
                    path.to_string_lossy().into_owned(),
                    "--json".into(),
                ],
                None,
            );
        }
    }
}

/// List an existing project in the launcher without opening it.
pub fn add_project(world: &mut World, path: PathBuf) {
    if !path.join("Cargo.toml").is_file() {
        super::notify_error(
            world,
            format!(
                "{} has no Cargo.toml; it is not a Bevy project",
                path.display()
            ),
        );
        return;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    crate::project::touch_recent(&path, &name);
    refresh(world);
}

/// Add an existing project to the list.
#[operator(
    id = "launcher.add_project",
    label = "Add Project",
    description = "List an existing project in the launcher without opening it.",
    allows_undo = false,
    params(path(String, doc = "The project folder. Asks for one when absent."))
)]
pub fn launcher_add_project(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let path = params
        .as_str("path")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    commands.queue(move |world: &mut World| match path {
        Some(path) => add_project(world, path),
        None => pick_folder(
            world,
            "Add a project",
            crate::native_dialog::launcher_project_directory(),
            PickPurpose::AddProject,
        ),
    });
    OperatorResult::Finished
}

/// Open a project folder.
#[operator(
    id = "launcher.open_folder",
    label = "Open Folder...",
    description = "Pick a project folder and open it.",
    allows_undo = false
)]
pub fn launcher_open_folder(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands
        .queue(|world: &mut World| crate::project_select::pick_project_folder_from(world, false));
    OperatorResult::Finished
}

/// Set up an existing Bevy project for jackdaw.
#[operator(
    id = "launcher.import_project",
    label = "Import Bevy Project...",
    description = "Pick a Bevy project and see what setting it up for jackdaw writes.",
    allows_undo = false
)]
pub fn launcher_import_project(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands
        .queue(|world: &mut World| crate::project_select::pick_project_folder_from(world, true));
    OperatorResult::Finished
}

/// Start a new project.
#[operator(
    id = "launcher.new_project",
    label = "New Project",
    description = "Show the New project page, starting from a template.",
    allows_undo = false,
    params(template(String, doc = "3d, 2d, empty or extension."))
)]
pub fn launcher_new_project(
    params: In<OperatorParameters>,
    mut page: ResMut<LauncherPage>,
    mut form: ResMut<NewProjectForm>,
    mut commands: Commands,
) -> OperatorResult {
    let template = params
        .as_str("template")
        .and_then(Template::parse)
        .unwrap_or(form.template);
    let mut next = NewProjectForm {
        template,
        location: form.location.clone(),
        ..NewProjectForm::default()
    };
    if template == Template::Extension {
        next.name = "my_extension".into();
    }
    form.set_if_neq(next);
    page.set_if_neq(LauncherPage::NewProject);
    commands.insert_resource(super::pages::NewProjectFieldsStale);
    OperatorResult::Finished
}

/// Pick the new project's template.
#[operator(
    id = "launcher.new_project_template",
    label = "Choose Template",
    description = "Pick the template the new project starts from.",
    allows_undo = false,
    params(template(String, doc = "3d, 2d, empty or extension."))
)]
pub fn launcher_new_project_template(
    params: In<OperatorParameters>,
    mut form: ResMut<NewProjectForm>,
) -> OperatorResult {
    let Some(template) = params.as_str("template").and_then(Template::parse) else {
        warn!("launcher.new_project_template: template= is 3d, 2d, empty or extension");
        return OperatorResult::Cancelled;
    };
    if form.template != template {
        form.template = template;
    }
    OperatorResult::Finished
}

/// Pick the editor the new project is made with.
#[operator(
    id = "launcher.new_project_editor",
    label = "Choose Editor",
    description = "Pick the installed editor the new project is made with and pinned to.",
    allows_undo = false,
    params(version(String, doc = "An installed editor's version; empty for this editor."))
)]
pub fn launcher_new_project_editor(
    params: In<OperatorParameters>,
    launcher: Res<Launcher>,
    mut form: ResMut<NewProjectForm>,
) -> OperatorResult {
    let editor = params
        .as_str("version")
        .filter(|version| !version.is_empty() && *version != launcher.running.version)
        .map(str::to_string);
    if let Some(version) = &editor
        && launcher.installs.get(version).is_none()
    {
        warn!("launcher.new_project_editor: jackdaw {version} is not installed");
        return OperatorResult::Cancelled;
    }
    if form.editor != editor {
        form.editor = editor;
    }
    OperatorResult::Finished
}

/// Turn a module on or off for the new project.
#[operator(
    id = "launcher.new_project_module",
    label = "Set New Project Module",
    description = "Turn one of the new project's modules on or off. Turning one on turns on \
                   what it needs; turning one off turns off what needs it.",
    allows_undo = false,
    params(
        id(String, doc = "The module: physics, navmesh or multiplayer."),
        enabled(bool, doc = "On or off.")
    )
)]
pub fn launcher_new_project_module(
    params: In<OperatorParameters>,
    mut form: ResMut<NewProjectForm>,
) -> OperatorResult {
    let (Some(id), Some(enabled)) = (params.as_str("id"), params.as_bool("enabled")) else {
        warn!("launcher.new_project_module: name a module and whether it is enabled");
        return OperatorResult::Cancelled;
    };
    let catalog = ModuleCatalog::builtin();
    if catalog.get(id).is_none() {
        warn!("launcher.new_project_module: there is no module `{id}`");
        return OperatorResult::Cancelled;
    }
    let mut modules = form.modules.clone();
    if enabled {
        catalog.enable(&mut modules, id);
        if let Err(reason) = catalog.available(modules.clone()) {
            warn!("launcher.new_project_module: {reason}");
            return OperatorResult::Cancelled;
        }
    } else {
        catalog.disable(&mut modules, id);
    }
    if form.modules != modules {
        form.modules = modules;
    }
    OperatorResult::Finished
}

/// Choose whether the new project gets a git repository.
#[operator(
    id = "launcher.new_project_git",
    label = "Create Git Repository",
    description = "Choose whether the new project starts as a git repository.",
    allows_undo = false,
    params(enabled(bool, doc = "Whether to run git init."))
)]
pub fn launcher_new_project_git(
    params: In<OperatorParameters>,
    mut form: ResMut<NewProjectForm>,
) -> OperatorResult {
    let Some(enabled) = params.as_bool("enabled") else {
        return OperatorResult::Cancelled;
    };
    if form.git != enabled {
        form.git = enabled;
    }
    OperatorResult::Finished
}

/// Pick the folder the new project is made in.
#[operator(
    id = "launcher.new_project_browse",
    label = "Choose Location...",
    description = "Pick the folder the new project is made in.",
    allows_undo = false
)]
pub fn launcher_new_project_browse(
    _: In<OperatorParameters>,
    form: Res<NewProjectForm>,
    mut commands: Commands,
) -> OperatorResult {
    let start = form.location.clone();
    commands.queue(move |world: &mut World| {
        pick_folder(
            world,
            "Where to create the project",
            Some(start),
            PickPurpose::NewProjectLocation,
        );
    });
    OperatorResult::Finished
}

/// Create the new project and open it.
#[operator(
    id = "launcher.new_project_create",
    label = "Create Project",
    description = "Create the project the New project page describes, pinned to the chosen \
                   editor, and open it. Only while the New project page is showing.",
    allows_undo = false,
    params(
        name(String, doc = "The project's name. The page's when absent."),
        location(String, doc = "The folder it is made in. The page's when absent.")
    )
)]
pub fn launcher_new_project_create(
    params: In<OperatorParameters>,
    page: Res<LauncherPage>,
    mut form: ResMut<NewProjectForm>,
    mut commands: Commands,
) -> OperatorResult {
    if form.creating || *page != LauncherPage::NewProject {
        return OperatorResult::Cancelled;
    }
    if let Some(name) = params.as_str("name") {
        form.name = name.to_string();
    }
    if let Some(location) = params.as_str("location") {
        form.location = PathBuf::from(location);
    }
    let snapshot = form.clone();
    commands.queue(move |world: &mut World| super::pages::create_project(world, snapshot));
    OperatorResult::Finished
}

/// Show the Install editor dialog.
#[operator(
    id = "launcher.install_editor",
    label = "Install Editor...",
    description = "Choose an editor release and its modules to install.",
    allows_undo = false,
    params(version(
        String,
        doc = "The release to start on: a version, a line, stable or \
                                  bleeding-edge."
    ))
)]
pub fn launcher_install_editor(
    params: In<OperatorParameters>,
    launcher: Res<Launcher>,
    mut dialog: ResMut<LauncherDialog>,
) -> OperatorResult {
    let query = params.as_str("version").unwrap_or("stable");
    let release = launcher.release(query);
    let channel = release.map_or(
        if query == "bleeding-edge" {
            Channel::BleedingEdge
        } else {
            Channel::Stable
        },
        |release| release.channel,
    );
    let version = release.map_or_else(|| query.to_string(), |release| release.version.clone());
    dialog.set_if_neq(LauncherDialog::InstallEditor(InstallForm {
        channel,
        version,
        show_older: false,
        modules: ModuleCatalog::builtin().defaults(),
        existing: None,
    }));
    OperatorResult::Finished
}

fn install_form(dialog: &mut LauncherDialog) -> Option<&mut InstallForm> {
    match dialog {
        LauncherDialog::InstallEditor(form) => Some(form),
        _ => None,
    }
}

/// Pick the channel in the Install editor dialog.
#[operator(
    id = "launcher.install_channel",
    label = "Choose Channel",
    description = "Pick Stable or Bleeding Edge in the Install editor dialog.",
    allows_undo = false,
    params(channel(String, doc = "stable or bleeding-edge."))
)]
pub fn launcher_install_channel(
    params: In<OperatorParameters>,
    launcher: Res<Launcher>,
    mut dialog: ResMut<LauncherDialog>,
) -> OperatorResult {
    let Some(channel) = params.as_str("channel").and_then(Channel::parse) else {
        warn!("launcher.install_channel: channel= is stable or bleeding-edge");
        return OperatorResult::Cancelled;
    };
    let Some(form) = install_form(dialog.bypass_change_detection()) else {
        return OperatorResult::Cancelled;
    };
    if form.channel == channel {
        return OperatorResult::Finished;
    }
    let mut next = form.clone();
    next.channel = channel;
    next.version = launcher.release(channel.as_str()).map_or_else(
        || channel.as_str().to_string(),
        |release| release.version.clone(),
    );
    *dialog = LauncherDialog::InstallEditor(next);
    OperatorResult::Finished
}

/// Pick the release in the Install editor dialog.
#[operator(
    id = "launcher.install_version",
    label = "Choose Version",
    description = "Pick the release the Install editor dialog installs.",
    allows_undo = false,
    params(version(String, doc = "A published version."))
)]
pub fn launcher_install_version(
    params: In<OperatorParameters>,
    mut dialog: ResMut<LauncherDialog>,
) -> OperatorResult {
    let Some(version) = version_param(&params, "launcher.install_version") else {
        return OperatorResult::Cancelled;
    };
    let Some(form) = install_form(dialog.bypass_change_detection()) else {
        return OperatorResult::Cancelled;
    };
    if form.version != version {
        let mut next = form.clone();
        next.version = version;
        *dialog = LauncherDialog::InstallEditor(next);
    }
    OperatorResult::Finished
}

/// Show or hide older releases in the Install editor dialog.
#[operator(
    id = "launcher.install_show_older",
    label = "Show Older Releases",
    description = "List every published release in the Install editor dialog, not only the \
                   newest of each line.",
    allows_undo = false,
    params(enabled(bool, doc = "Whether older releases are listed."))
)]
pub fn launcher_install_show_older(
    params: In<OperatorParameters>,
    mut dialog: ResMut<LauncherDialog>,
) -> OperatorResult {
    let Some(form) = install_form(dialog.bypass_change_detection()) else {
        return OperatorResult::Cancelled;
    };
    let show = params.as_bool("enabled").unwrap_or(!form.show_older);
    if form.show_older != show {
        let mut next = form.clone();
        next.show_older = show;
        *dialog = LauncherDialog::InstallEditor(next);
    }
    OperatorResult::Finished
}

/// Turn a module on or off in the Install editor dialog.
#[operator(
    id = "launcher.install_module",
    label = "Choose Module",
    description = "Turn a module on or off in the Install editor dialog.",
    allows_undo = false,
    params(
        id(String, doc = "The module: physics, navmesh or multiplayer."),
        enabled(bool, doc = "On or off.")
    )
)]
pub fn launcher_install_module(
    params: In<OperatorParameters>,
    mut dialog: ResMut<LauncherDialog>,
) -> OperatorResult {
    let (Some(id), Some(enabled)) = (params.as_str("id"), params.as_bool("enabled")) else {
        return OperatorResult::Cancelled;
    };
    let Some(form) = install_form(dialog.bypass_change_detection()) else {
        return OperatorResult::Cancelled;
    };
    let catalog = ModuleCatalog::builtin();
    let mut next = form.clone();
    if enabled {
        catalog.enable(&mut next.modules, id);
    } else {
        catalog.disable(&mut next.modules, id);
    }
    if next != *form {
        *dialog = LauncherDialog::InstallEditor(next);
    }
    OperatorResult::Finished
}

/// Install what the Install editor dialog describes.
#[operator(
    id = "launcher.install_confirm",
    label = "Install",
    description = "Download, verify and install the release the Install editor dialog names.",
    allows_undo = false
)]
pub fn launcher_install_confirm(
    _: In<OperatorParameters>,
    dialog: Res<LauncherDialog>,
    mut commands: Commands,
) -> OperatorResult {
    let LauncherDialog::InstallEditor(form) = &*dialog else {
        return OperatorResult::Cancelled;
    };
    let form = form.clone();
    commands.queue(move |world: &mut World| {
        let query = form.existing.clone().unwrap_or(form.version.clone());
        jd::start_job(
            world,
            JobKind::Install {
                query: query.clone(),
            },
            install_args(&query, &form.modules),
            None,
        );
        *world.resource_mut::<LauncherDialog>() = LauncherDialog::Downloads;
    });
    OperatorResult::Finished
}

/// The `jd` arguments that install `query` with `modules`.
pub fn install_args(
    query: &str,
    modules: &jackdaw_project_build::modules::ModuleSet,
) -> Vec<String> {
    let mut args = vec!["editor".into(), "install".into(), query.to_string()];
    if !modules.is_empty() {
        args.push("--modules".into());
        args.push(modules.iter().cloned().collect::<Vec<_>>().join(","));
    }
    args.push("--json".into());
    args
}

/// Install the newest release of an editor's line.
#[operator(
    id = "launcher.update_editor",
    label = "Update",
    description = "Install the newest release of an installed editor's line, beside it.",
    allows_undo = false,
    params(version(String, doc = "The installed editor."))
)]
pub fn launcher_update_editor(
    params: In<OperatorParameters>,
    launcher: Res<Launcher>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(version) = version_param(&params, "launcher.update_editor") else {
        return OperatorResult::Cancelled;
    };
    let Some(editor) = launcher.install(&version) else {
        warn!("launcher.update_editor: jackdaw {version} is not installed");
        return OperatorResult::Cancelled;
    };
    let Some(update) = editor.update.clone() else {
        return OperatorResult::Cancelled;
    };
    let modules = editor.install.modules.iter().cloned().collect();
    commands.queue(move |world: &mut World| {
        jd::start_job(
            world,
            JobKind::Install {
                query: update.clone(),
            },
            install_args(&update, &modules),
            None,
        );
        *world.resource_mut::<LauncherDialog>() = LauncherDialog::Downloads;
    });
    OperatorResult::Finished
}

/// Make an installed editor the default.
#[operator(
    id = "launcher.set_default_editor",
    label = "Set as Default",
    description = "Open projects that name no editor with this one.",
    allows_undo = false,
    params(version(String, doc = "The installed editor."))
)]
pub fn launcher_set_default_editor(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(version) = version_param(&params, "launcher.set_default_editor") else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        jd::start_job(
            world,
            JobKind::SetDefault {
                version: version.clone(),
            },
            vec!["editor".into(), "default".into(), version, "--json".into()],
            None,
        );
    });
    OperatorResult::Finished
}

/// Uninstall an editor.
#[operator(
    id = "launcher.uninstall_editor",
    label = "Uninstall...",
    description = "Remove an installed editor. When projects still open with it, the launcher \
                   lists them and asks first.",
    allows_undo = false,
    params(
        version(String, doc = "The installed editor."),
        force(
            bool,
            default = false,
            doc = "Uninstall even though projects open with it."
        )
    )
)]
pub fn launcher_uninstall_editor(
    params: In<OperatorParameters>,
    launcher: Res<Launcher>,
    mut dialog: ResMut<LauncherDialog>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(version) = version_param(&params, "launcher.uninstall_editor") else {
        return OperatorResult::Cancelled;
    };
    let Some(editor) = launcher.install(&version) else {
        warn!("launcher.uninstall_editor: jackdaw {version} is not installed");
        return OperatorResult::Cancelled;
    };
    if editor.is_running {
        commands.queue(|world: &mut World| {
            super::notify_error(
                world,
                "This editor is running; uninstall it from another one",
            );
        });
        return OperatorResult::Cancelled;
    }
    let force = params.as_bool("force").unwrap_or(false);
    if !editor.projects.is_empty() && !force {
        dialog.set_if_neq(LauncherDialog::Uninstall {
            version,
            projects: editor.projects.clone(),
        });
        return OperatorResult::Finished;
    }
    dialog.set_if_neq(LauncherDialog::Downloads);
    commands.queue(move |world: &mut World| {
        let mut args = vec!["editor".into(), "remove".into(), version.clone()];
        if force {
            args.push("--force".into());
        }
        args.push("--json".into());
        jd::start_job(world, JobKind::Remove { version }, args, None);
    });
    OperatorResult::Finished
}

/// Open an editor release's notes.
#[operator(
    id = "launcher.release_notes",
    label = "Release Notes",
    description = "Open a release's notes in the web browser.",
    allows_undo = false,
    params(version(String, doc = "The release."))
)]
pub fn launcher_release_notes(
    params: In<OperatorParameters>,
    launcher: Res<Launcher>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(version) = version_param(&params, "launcher.release_notes") else {
        return OperatorResult::Cancelled;
    };
    let Some(url) = launcher
        .release(&version)
        .and_then(|release| release.notes_url.clone())
    else {
        commands.queue(move |world: &mut World| {
            super::notify_error(
                world,
                format!("The release index has no notes for {version}"),
            );
        });
        return OperatorResult::Cancelled;
    };
    if let Err(error) = crate::project_window::open_with_desktop(&url) {
        warn!("launcher.release_notes: {url}: {error}");
    }
    OperatorResult::Finished
}

/// Show an installed editor's folder.
#[operator(
    id = "launcher.reveal_editor",
    label = "Show in Files",
    description = "Show an installed editor's folder in the system file manager.",
    allows_undo = false,
    params(version(String, doc = "The installed editor."))
)]
pub fn launcher_reveal_editor(
    params: In<OperatorParameters>,
    launcher: Res<Launcher>,
) -> OperatorResult {
    let Some(version) = version_param(&params, "launcher.reveal_editor") else {
        return OperatorResult::Cancelled;
    };
    let Some(install) = launcher.installs.get(&version) else {
        return OperatorResult::Cancelled;
    };
    if let Err(error) = crate::project_window::open_with_desktop(&install.path) {
        warn!(
            "launcher.reveal_editor: {}: {error}",
            install.path.display()
        );
    }
    OperatorResult::Finished
}

/// Check that an installed editor runs.
#[operator(
    id = "launcher.verify_editor",
    label = "Verify Install",
    description = "Check that an installed editor starts and reports its version.",
    allows_undo = false,
    params(version(String, doc = "The installed editor."))
)]
pub fn launcher_verify_editor(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(version) = version_param(&params, "launcher.verify_editor") else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        jd::start_job(
            world,
            JobKind::Verify {
                version: version.clone(),
            },
            vec!["editor".into(), "verify".into(), version, "--json".into()],
            None,
        );
        *world.resource_mut::<LauncherDialog>() = LauncherDialog::Downloads;
    });
    OperatorResult::Finished
}

/// Add modules to an installed editor.
#[operator(
    id = "launcher.editor_modules",
    label = "Add or Remove Modules...",
    description = "Choose the modules fetched for an installed editor.",
    allows_undo = false,
    params(version(String, doc = "The installed editor."))
)]
pub fn launcher_editor_modules(
    params: In<OperatorParameters>,
    launcher: Res<Launcher>,
    mut dialog: ResMut<LauncherDialog>,
) -> OperatorResult {
    let Some(version) = version_param(&params, "launcher.editor_modules") else {
        return OperatorResult::Cancelled;
    };
    let Some(install) = launcher.installs.get(&version) else {
        return OperatorResult::Cancelled;
    };
    dialog.set_if_neq(LauncherDialog::InstallEditor(InstallForm {
        channel: install.channel,
        version: install.version.clone(),
        show_older: false,
        modules: install.modules.iter().cloned().collect(),
        existing: Some(install.version.clone()),
    }));
    OperatorResult::Finished
}

/// Register an editor installed some other way.
#[operator(
    id = "launcher.locate_editor",
    label = "Locate...",
    description = "Pick an editor installed some other way, such as with cargo install, and \
                   list it with the installed editors.",
    allows_undo = false,
    params(path(
        String,
        doc = "The editor binary or its folder. Asks for one when absent."
    ))
)]
pub fn launcher_locate_editor(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let path = params
        .as_str("path")
        .filter(|path| !path.is_empty())
        .map(str::to_string);
    commands.queue(move |world: &mut World| match path {
        Some(path) => {
            jd::start_job(
                world,
                JobKind::Locate,
                vec!["editor".into(), "locate".into(), path, "--json".into()],
                None,
            );
        }
        None => pick_folder(
            world,
            "Locate an installed editor",
            dirs::home_dir(),
            PickPurpose::LocateEditor,
        ),
    });
    OperatorResult::Finished
}

/// Remove nightlies nothing uses.
#[operator(
    id = "launcher.clean_nightlies",
    label = "Clean Up",
    description = "Remove the Bleeding Edge builds kept for rollback that no project opens with.",
    allows_undo = false
)]
pub fn launcher_clean_nightlies(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        jd::start_job(
            world,
            JobKind::Clean,
            vec!["editor".into(), "clean".into(), "--json".into()],
            None,
        );
    });
    OperatorResult::Finished
}

/// Fetch the release index again.
#[operator(
    id = "launcher.refresh_index",
    label = "Check for Updates",
    description = "Fetch the signed release index, to see new editor releases. Nothing is \
                   downloaded.",
    allows_undo = false
)]
pub fn launcher_refresh_index(
    _: In<OperatorParameters>,
    mut status: ResMut<super::IndexStatus>,
    mut commands: Commands,
) -> OperatorResult {
    status.set_if_neq(super::IndexStatus::Checking);
    commands.queue(|world: &mut World| {
        jd::start_job(
            world,
            JobKind::RefreshIndex,
            vec!["editor".into(), "refresh".into(), "--json".into()],
            None,
        );
    });
    OperatorResult::Finished
}

fn show_dialog(dialog: &mut ResMut<LauncherDialog>, next: LauncherDialog) {
    dialog.set_if_neq(next);
}

/// Show the environment checks.
#[operator(
    id = "launcher.doctor",
    label = "Doctor",
    description = "Show whether the toolchain and SDK can build projects, with fixes.",
    allows_undo = false
)]
pub fn launcher_doctor(
    _: In<OperatorParameters>,
    mut dialog: ResMut<LauncherDialog>,
) -> OperatorResult {
    show_dialog(&mut dialog, LauncherDialog::Doctor);
    OperatorResult::Finished
}

/// Run the environment checks again.
#[operator(
    id = "launcher.recheck_environment",
    label = "Recheck",
    description = "Run the environment checks again after fixing something.",
    allows_undo = false
)]
pub fn launcher_recheck_environment(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| crate::project_select::restart_preflight(world));
    OperatorResult::Finished
}

/// Show the launcher's settings.
#[operator(
    id = "launcher.settings",
    label = "Settings",
    description = "Show where the launcher keeps editors and which release index it reads.",
    allows_undo = false
)]
pub fn launcher_settings(
    _: In<OperatorParameters>,
    mut dialog: ResMut<LauncherDialog>,
) -> OperatorResult {
    show_dialog(&mut dialog, LauncherDialog::Settings);
    OperatorResult::Finished
}

/// Show downloads and installs.
#[operator(
    id = "launcher.downloads",
    label = "Downloads",
    description = "Show the editor downloads and installs of this session.",
    allows_undo = false
)]
pub fn launcher_downloads(
    _: In<OperatorParameters>,
    mut dialog: ResMut<LauncherDialog>,
) -> OperatorResult {
    show_dialog(&mut dialog, LauncherDialog::Downloads);
    OperatorResult::Finished
}

/// Stop a download or install.
#[operator(
    id = "launcher.cancel_job",
    label = "Cancel",
    description = "Stop a running download or install. Nothing half-installed is kept.",
    allows_undo = false,
    params(job(i64, doc = "The job's number in Downloads."))
)]
pub fn launcher_cancel_job(
    params: In<OperatorParameters>,
    mut jobs: ResMut<super::JdJobs>,
) -> OperatorResult {
    let Some(id) = params.as_int("job").and_then(|id| u64::try_from(id).ok()) else {
        return OperatorResult::Cancelled;
    };
    jd::cancel_job(&mut jobs, id);
    OperatorResult::Finished
}

/// Close the launcher's dialog.
#[operator(
    id = "launcher.close_dialog",
    label = "Close",
    description = "Close the launcher's open dialog.",
    allows_undo = false
)]
pub fn launcher_close_dialog(
    _: In<OperatorParameters>,
    mut dialog: ResMut<LauncherDialog>,
) -> OperatorResult {
    show_dialog(&mut dialog, LauncherDialog::None);
    OperatorResult::Finished
}

/// Dismiss the launcher's message.
#[operator(
    id = "launcher.dismiss_notice",
    label = "Dismiss",
    description = "Hide the message across the top of the launcher.",
    allows_undo = false
)]
pub fn launcher_dismiss_notice(
    _: In<OperatorParameters>,
    mut notice: ResMut<super::LauncherNotice>,
) -> OperatorResult {
    notice.set_if_neq(super::LauncherNotice(None));
    OperatorResult::Finished
}

/// Show the Extensions page.
#[operator(
    id = "launcher.extensions",
    label = "Extensions and Plugins",
    description = "Show the modules and editor extensions for a project, or the global \
                   defaults every project starts from.",
    allows_undo = false,
    params(path(String, doc = "The project; the global defaults when absent."))
)]
pub fn launcher_extensions(
    params: In<OperatorParameters>,
    mut view: ResMut<ExtensionsView>,
    mut page: ResMut<LauncherPage>,
) -> OperatorResult {
    let project = params
        .as_str("path")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    if view.project != project {
        *view = ExtensionsView {
            project,
            ..ExtensionsView::default()
        };
    }
    page.set_if_neq(LauncherPage::Extensions);
    OperatorResult::Finished
}

/// Filter the Extensions page.
#[operator(
    id = "launcher.extensions_filter",
    label = "Filter Extensions",
    description = "Show all entries on the Extensions page, those on or off, or those that need \
                   attention.",
    allows_undo = false,
    params(filter(String, doc = "all, on, off or needs-attention."))
)]
pub fn launcher_extensions_filter(
    params: In<OperatorParameters>,
    mut view: ResMut<ExtensionsView>,
) -> OperatorResult {
    let Some(filter) = params.as_str("filter").and_then(ExtensionFilter::parse) else {
        return OperatorResult::Cancelled;
    };
    if view.filter != filter {
        view.filter = filter;
    }
    OperatorResult::Finished
}

/// Show an entry's details on the Extensions page.
#[operator(
    id = "launcher.extensions_select",
    label = "Select Extension",
    description = "Show a module's or extension's details on the Extensions page.",
    allows_undo = false,
    params(
        kind(String, doc = "module or extension."),
        id(String, doc = "Its id.")
    )
)]
pub fn launcher_extensions_select(
    params: In<OperatorParameters>,
    mut view: ResMut<ExtensionsView>,
) -> OperatorResult {
    let (Some(kind), Some(id)) = (params.as_str("kind"), params.as_str("id")) else {
        return OperatorResult::Cancelled;
    };
    let Some(item) = ExtensionItem::parse(kind, id) else {
        return OperatorResult::Cancelled;
    };
    if view.selected.as_ref() != Some(&item) {
        view.selected = Some(item);
    }
    OperatorResult::Finished
}

/// Turn an entry on or off on the Extensions page, as a pending change.
#[operator(
    id = "launcher.extensions_toggle",
    label = "Turn Extension On or Off",
    description = "Turn a module or extension on or off. The change waits until Apply.",
    allows_undo = false,
    params(
        kind(String, doc = "module or extension."),
        id(String, doc = "Its id."),
        enabled(bool, doc = "On or off.")
    )
)]
pub fn launcher_extensions_toggle(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (Some(kind), Some(id), Some(enabled)) = (
        params.as_str("kind"),
        params.as_str("id"),
        params.as_bool("enabled"),
    ) else {
        return OperatorResult::Cancelled;
    };
    let Some(item) = ExtensionItem::parse(kind, id) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        super::pages::extensions::stage_toggle(world, item, enabled);
    });
    OperatorResult::Finished
}

/// Drop the Extensions page's pending changes.
#[operator(
    id = "launcher.extensions_revert",
    label = "Revert",
    description = "Drop the Extensions page's changes that are not applied yet.",
    allows_undo = false
)]
pub fn launcher_extensions_revert(
    _: In<OperatorParameters>,
    mut view: ResMut<ExtensionsView>,
) -> OperatorResult {
    if !view.pending.is_empty() {
        view.pending.clear();
    }
    OperatorResult::Finished
}

/// Apply the Extensions page's pending changes.
#[operator(
    id = "launcher.extensions_apply",
    label = "Apply",
    description = "Write the Extensions page's changes: a project's to its jackdaw.toml and \
                   Cargo.toml, the global defaults to the editor's extension list.",
    allows_undo = false,
    params(restart(
        bool,
        default = false,
        doc = "Restart the editor afterwards so extension \
                                                  changes load."
    ))
)]
pub fn launcher_extensions_apply(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let restart = params.as_bool("restart").unwrap_or(false);
    commands.queue(move |world: &mut World| super::pages::extensions::apply(world, restart));
    OperatorResult::Finished
}

fn refresh(world: &mut World) {
    let data = super::LauncherData::read();
    if world.resource::<Launcher>().0 != data {
        world.resource_mut::<Launcher>().0 = data;
    }
}
