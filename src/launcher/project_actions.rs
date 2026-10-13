//! Opening a project with the editor its pin selects: here, by relaunching
//! into another install, or through the Open with a different editor
//! dialog when that editor is missing.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use jackdaw_releases::{Channel, Layout, version};

use super::model::{self, ProjectEditor};
use super::{Launcher, LauncherDialog, OpenWithForm, jd};

/// Hands a project to another installed editor. Replaces this process
/// outside tests; a test swaps in a recorder.
#[derive(Resource, Clone, Copy)]
pub struct EditorHandoff(pub fn(&Path, &Path));

impl Default for EditorHandoff {
    fn default() -> Self {
        Self(|executable, project| crate::restart::relaunch_into_editor(executable, project))
    }
}

/// What the Open with a different editor dialog does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenChoice {
    /// Install the pinned editor, then open with it.
    InstallAndOpen,
    /// Copy the project, move the copy to `version`, and open the copy.
    OpenCopy { version: String },
    /// Back the project up, then move it to `version` in place.
    Convert { version: String },
    /// Open here without moving the project.
    ScenesOnly,
}

impl OpenChoice {
    pub fn id(&self) -> &'static str {
        match self {
            Self::InstallAndOpen => "install",
            Self::OpenCopy { .. } => "copy",
            Self::Convert { .. } => "convert",
            Self::ScenesOnly => "scenes",
        }
    }
}

/// Open the project at `path` with the editor its pin selects.
///
/// A project this editor opens goes through the launcher's usual open,
/// which offers setup, upgrades and the SDK build. One pinned to another
/// install relaunches into it, and one whose editor is not installed opens
/// the Open with a different editor dialog.
pub fn open_project(world: &mut World, path: PathBuf) {
    let set_up =
        path.is_dir() && jackdaw_project_build::project_manifest::ProjectManifest::exists(&path);
    if !set_up {
        crate::project_select::enter_project(world, path);
        return;
    }
    let data = &world.resource::<Launcher>().0;
    let manifest = jackdaw_project_build::project_manifest::ProjectManifest::read(&path);
    let pin = model::project_pin(&manifest);
    match model::resolve_editor(&pin, &data.installs, &data.running) {
        ProjectEditor::Running => crate::project_select::enter_project(world, path),
        ProjectEditor::Installed(install) => hand_off(world, &install.executable(), &path),
        ProjectEditor::Missing { .. } => show_open_with(world, path),
    }
}

/// Open the Open with a different editor dialog for `project`.
pub fn show_open_with(world: &mut World, project: PathBuf) {
    if world.resource::<Launcher>().project(&project).is_none() {
        let data = &world.resource::<Launcher>().0;
        let entry = model::describe_project(&project, &data.installs, &data.running);
        world.resource_mut::<Launcher>().0.projects.push(entry);
    }
    let choice = default_choice(&world.resource::<Launcher>().0, &project);
    *world.resource_mut::<LauncherDialog>() =
        LauncherDialog::OpenWith(OpenWithForm { project, choice });
}

/// Installing the pinned editor when the index has it, else opening a copy
/// with this editor.
pub fn default_choice(data: &model::LauncherData, project: &Path) -> OpenChoice {
    let Some(entry) = data.project(project) else {
        return OpenChoice::ScenesOnly;
    };
    match &entry.editor {
        ProjectEditor::Missing { query } if data.release(query).is_some() => {
            OpenChoice::InstallAndOpen
        }
        _ => OpenChoice::OpenCopy {
            version: data.newest_editor(),
        },
    }
}

/// Open `project` with the installed editor `version`, here when that is
/// this editor.
pub fn open_with_install(world: &mut World, project: PathBuf, version: &str) {
    let data = &world.resource::<Launcher>().0;
    if version == data.running.version {
        crate::project_select::enter_project(world, project);
        return;
    }
    match data.installs.get(version) {
        Some(install) => {
            let executable = install.executable();
            hand_off(world, &executable, &project);
        }
        None => super::notify_error(world, format!("jackdaw {version} is not installed")),
    }
}

fn hand_off(world: &mut World, executable: &Path, project: &Path) {
    if !executable.is_file() {
        super::notify_error(
            world,
            format!(
                "{} is missing; verify or reinstall that editor",
                executable.display()
            ),
        );
        return;
    }
    let handoff = world.resource::<EditorHandoff>().0;
    handoff(executable, project);
}

/// Carry out the Open with a different editor dialog's choice.
pub fn confirm_open_with(world: &mut World, form: OpenWithForm) {
    *world.resource_mut::<LauncherDialog>() = LauncherDialog::None;
    let OpenWithForm { project, choice } = form;
    match choice {
        OpenChoice::InstallAndOpen => {
            let Some(query) =
                super::project_entry(world, &project).and_then(|entry| match entry.editor {
                    ProjectEditor::Missing { query } => Some(query),
                    _ => None,
                })
            else {
                open_project(world, project);
                return;
            };
            jd::start_job(
                world,
                jd::JobKind::Install {
                    query: query.clone(),
                },
                vec!["editor".into(), "install".into(), query, "--json".into()],
                Some(jd::AfterJob::OpenProject(project)),
            );
            *world.resource_mut::<LauncherDialog>() = LauncherDialog::Downloads;
        }
        OpenChoice::OpenCopy { version } => {
            let target = copy_destination(&project, &line_name(world, &version));
            start_copy(world, project, target, version, CopyPurpose::Copy);
        }
        OpenChoice::Convert { version } => {
            let backup = backup_destination(&project);
            start_copy(world, project, backup, version, CopyPurpose::Convert);
        }
        OpenChoice::ScenesOnly => crate::project_select::transition_to_editor(world, project),
    }
}

fn line_name(world: &World, version: &str) -> String {
    let data = &world.resource::<Launcher>().0;
    match data.installs.get(version) {
        Some(install) if install.channel == Channel::BleedingEdge => "main".into(),
        Some(install) => install.bevy.clone(),
        None if version == data.running.version => data.running.bevy.clone(),
        None => version::line_of(version).unwrap_or_else(|| version.to_string()),
    }
}

/// `<project>-<line>` beside the project, numbered when taken.
pub fn copy_destination(project: &Path, line: &str) -> PathBuf {
    let name = project
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    free_sibling(project, &format!("{name}-{line}"))
}

/// `<project>.backup-<date>` beside the project, numbered when taken.
pub fn backup_destination(project: &Path) -> PathBuf {
    let name = project
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    let date = version::format_date(version::today());
    free_sibling(project, &format!("{name}.backup-{date}"))
}

fn free_sibling(project: &Path, name: &str) -> PathBuf {
    let parent = project.parent().unwrap_or(Path::new("."));
    let first = parent.join(name);
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| parent.join(format!("{name}-{n}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

/// Why a project is being copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyPurpose {
    /// Open the copy, moved to the chosen editor.
    Copy,
    /// The copy is a backup; the original moves to the chosen editor.
    Convert,
}

/// A project copy running off the main thread.
struct ProjectCopy {
    task: Task<Result<(), String>>,
    source: PathBuf,
    target: PathBuf,
    version: String,
    purpose: CopyPurpose,
}

/// Copies in flight.
#[derive(Resource, Default)]
pub struct ProjectCopies(Vec<ProjectCopy>);

fn start_copy(
    world: &mut World,
    source: PathBuf,
    target: PathBuf,
    version: String,
    purpose: CopyPurpose,
) {
    let (from, to) = (source.clone(), target.clone());
    let task = AsyncComputeTaskPool::get().spawn(async move { copy_project(&from, &to) });
    super::notify_info(
        world,
        format!("Copying {} to {}", source.display(), target.display()),
    );
    world
        .get_resource_or_insert_with(ProjectCopies::default)
        .0
        .push(ProjectCopy {
            task,
            source,
            target,
            version,
            purpose,
        });
}

/// Finish the copies that are done: move the project to its editor and
/// open it.
pub fn poll_project_copies(world: &mut World) {
    let Some(mut copies) = world.get_resource_mut::<ProjectCopies>() else {
        return;
    };
    if copies.0.is_empty() {
        return;
    }
    let mut done = Vec::new();
    let mut index = 0;
    while index < copies.0.len() {
        let finished = future::block_on(future::poll_once(&mut copies.0[index].task));
        match finished {
            Some(result) => done.push((copies.0.remove(index), result)),
            None => index += 1,
        }
    }
    for (copy, result) in done {
        if let Err(error) = result {
            super::notify_error(world, format!("Copy failed: {error}"));
            continue;
        }
        let moved = match copy.purpose {
            CopyPurpose::Copy => copy.target.clone(),
            CopyPurpose::Convert => copy.source.clone(),
        };
        let editor = editor_identity(world, &copy.version);
        if let Err(error) = repin(&moved, &editor) {
            super::notify_error(world, format!("Could not move the project: {error}"));
            continue;
        }
        let name = moved
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        crate::project::touch_recent(&moved, &name);
        open_with_install(world, moved, &copy.version);
    }
}

/// The version, channel and Bevy line `version` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorIdentity {
    pub version: String,
    pub channel: Channel,
    pub bevy: String,
    pub bevy_rev: Option<String>,
}

fn editor_identity(world: &World, version: &str) -> EditorIdentity {
    let data = &world.resource::<Launcher>().0;
    match data.installs.get(version) {
        Some(install) => EditorIdentity {
            version: install.version.clone(),
            channel: install.channel,
            bevy: install.bevy.clone(),
            bevy_rev: install.bevy_rev.clone(),
        },
        None => EditorIdentity {
            version: data.running.version.clone(),
            channel: data.running.channel,
            bevy: data.running.bevy.clone(),
            bevy_rev: data.running.bevy_rev.clone(),
        },
    }
}

/// Point the project's `[jackdaw]` table at `editor`.
pub fn repin(project: &Path, editor: &EditorIdentity) -> Result<(), String> {
    let path = jackdaw_project_build::project_manifest::path(project);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let written = pinned_toml(&text, editor)?;
    std::fs::write(&path, written).map_err(|error| format!("{}: {error}", path.display()))
}

/// `jackdaw_toml` with its `[jackdaw]` table naming `editor`.
pub fn pinned_toml(jackdaw_toml: &str, editor: &EditorIdentity) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = jackdaw_toml
        .parse()
        .map_err(|error: toml_edit::TomlError| error.to_string())?;
    if !doc.contains_key("jackdaw") {
        doc.insert("jackdaw", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let table = doc["jackdaw"]
        .as_table_mut()
        .ok_or_else(|| "`jackdaw` in jackdaw.toml is not a table".to_string())?;
    match editor.channel {
        Channel::Stable => {
            table.insert("version", toml_edit::value(&editor.version));
            table.insert("bevy", toml_edit::value(&editor.bevy));
            table.insert("channel", toml_edit::value("stable"));
            table.remove("build");
            table.remove("bevy_rev");
        }
        Channel::BleedingEdge => {
            table.insert("bevy", toml_edit::value("main"));
            table.insert("channel", toml_edit::value("bleeding-edge"));
            table.insert("build", toml_edit::value(&editor.version));
            match &editor.bevy_rev {
                Some(rev) => {
                    table.insert("bevy_rev", toml_edit::value(rev));
                }
                None => {
                    table.remove("bevy_rev");
                }
            }
        }
    }
    Ok(doc.to_string())
}

/// Copy a project folder, leaving out build output and version control.
fn copy_project(from: &Path, to: &Path) -> Result<(), String> {
    const SKIPPED: [&str; 2] = ["target", ".git"];
    std::fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
    let entries =
        std::fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", from.display()))?;
        let name = entry.file_name();
        if SKIPPED.iter().any(|skipped| name == *skipped) {
            continue;
        }
        copy_tree(&entry.path(), &to.join(&name))?;
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let kind = std::fs::symlink_metadata(from)
        .map_err(|error| format!("{}: {error}", from.display()))?
        .file_type();
    if kind.is_dir() {
        std::fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
        let entries =
            std::fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("{}: {error}", from.display()))?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else if kind.is_file() {
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|error| format!("{}: {error}", from.display()))
    } else {
        Ok(())
    }
}

/// Remember in the installed-editor registry that this editor opened
/// `project`, so uninstalling it warns about the project. Skipped for an
/// editor that is not registered.
pub fn record_open(project: PathBuf) {
    let Some(layout) = Layout::from_env() else {
        return;
    };
    std::thread::spawn(move || {
        let Ok(installs) = layout.load_installs() else {
            return;
        };
        let running = model::RunningEditor::detect(&installs);
        if installs.get(&running.version).is_none() {
            return;
        }
        let Ok(_lock) = layout.lock() else {
            return;
        };
        let Ok(mut installs) = layout.load_installs() else {
            return;
        };
        if installs.projects.get(&project) == Some(&running.version) {
            return;
        }
        installs.record_project(&project, &running.version);
        if let Err(error) = layout.save_installs(&installs) {
            warn!(
                "Could not record {} in the editor registry: {error}",
                project.display()
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stable(version: &str) -> EditorIdentity {
        EditorIdentity {
            version: version.into(),
            channel: Channel::Stable,
            bevy: version::line_of(version).unwrap(),
            bevy_rev: None,
        }
    }

    #[test]
    fn repinning_rewrites_the_jackdaw_table_and_keeps_the_rest() {
        let original = "plugin = \"GamePlugin\"\n\n[jackdaw]\nversion = \"0.19.0\"\nbevy = \"0.19\"\nchannel = \"stable\"\n\n[modules]\nphysics = true\n";
        let written = pinned_toml(original, &stable("0.20.1")).unwrap();
        let manifest: jackdaw_project_build::project_manifest::ProjectManifest =
            toml::from_str(&written).unwrap();
        assert_eq!(manifest.jackdaw.version.as_deref(), Some("0.20.1"));
        assert_eq!(manifest.jackdaw.bevy.as_deref(), Some("0.20"));
        assert_eq!(manifest.plugin.as_deref(), Some("GamePlugin"));
        assert!(written.contains("physics = true"));

        let nightly = EditorIdentity {
            version: "nightly-2026-10-12".into(),
            channel: Channel::BleedingEdge,
            bevy: "main".into(),
            bevy_rev: Some("9be004d".into()),
        };
        let written = pinned_toml(&written, &nightly).unwrap();
        let manifest: jackdaw_project_build::project_manifest::ProjectManifest =
            toml::from_str(&written).unwrap();
        assert_eq!(manifest.jackdaw.channel.as_deref(), Some("bleeding-edge"));
        assert_eq!(
            manifest.jackdaw.build.as_deref(),
            Some("nightly-2026-10-12")
        );
        assert_eq!(manifest.jackdaw.bevy_rev.as_deref(), Some("9be004d"));
    }

    #[test]
    fn copies_and_backups_land_beside_the_project_under_a_free_name() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("village");
        std::fs::create_dir_all(project.join("target")).unwrap();
        std::fs::create_dir_all(project.join("assets")).unwrap();
        std::fs::write(project.join("assets/scene.bsn"), "scene").unwrap();
        std::fs::write(project.join("target/big"), "build output").unwrap();

        let copy = copy_destination(&project, "0.20");
        assert_eq!(copy, dir.path().join("village-0.20"));
        copy_project(&project, &copy).unwrap();
        assert!(copy.join("assets/scene.bsn").is_file());
        assert!(!copy.join("target").exists(), "build output is not copied");
        assert_eq!(
            copy_destination(&project, "0.20"),
            dir.path().join("village-0.20-2")
        );

        let backup = backup_destination(&project);
        let name = backup.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("village.backup-"), "{name}");
    }
}
