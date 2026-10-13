//! What the launcher shows: the recent projects with the editor each one
//! opens with, and the installed editors grouped by Bevy line, read from the
//! recent-project list, the installed-editor registry and the cached release
//! index. Building it touches only local files.

use std::path::{Path, PathBuf};

use jackdaw_project_build::modules::ModuleCatalog;
use jackdaw_project_build::project_manifest::ProjectManifest;
use jackdaw_releases::index::LineStatus;
use jackdaw_releases::{
    Channel, EditorPin, Install, Installs, Layout, PinResolution, ReleaseIndex, signing, version,
};

/// The editor this process is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningEditor {
    pub version: String,
    /// The Bevy minor it targets, or `main`.
    pub bevy: String,
    pub channel: Channel,
    pub bevy_rev: Option<String>,
}

impl RunningEditor {
    /// The registered install whose binary is this process, or this
    /// build's own version when it is not registered.
    pub fn detect(installs: &Installs) -> Self {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|exe| dunce::canonicalize(exe).ok());
        let registered = exe.and_then(|exe| {
            installs.installs.iter().find(|install| {
                dunce::canonicalize(install.executable()).is_ok_and(|known| known == exe)
            })
        });
        match registered {
            Some(install) => Self {
                version: install.version.clone(),
                bevy: install.bevy.clone(),
                channel: install.channel,
                bevy_rev: install.bevy_rev.clone(),
            },
            None => Self::of_this_build(),
        }
    }

    /// This build's version, on the stable channel.
    pub fn of_this_build() -> Self {
        Self {
            version: jackdaw_project_build::VERSION.to_string(),
            bevy: jackdaw_project_build::BEVY_VERSION.to_string(),
            channel: Channel::Stable,
            bevy_rev: None,
        }
    }

    /// `0.20.0 - Bevy 0.20`, or `nightly 2026-10-09`.
    pub fn chip(&self) -> String {
        editor_chip(&self.version, &self.bevy)
    }
}

/// The label of an editor version: `0.20.0 - Bevy 0.20`, or
/// `nightly 2026-10-09` for a Bleeding Edge build.
pub fn editor_chip(version: &str, bevy: &str) -> String {
    match version.strip_prefix("nightly-") {
        Some(date) => format!("nightly {date}"),
        None => format!("{version} - Bevy {bevy}"),
    }
}

/// `nightly-2026-10-09` as `nightly 2026-10-09`, anything else unchanged.
pub fn version_label(version: &str) -> String {
    match version.strip_prefix("nightly-") {
        Some(date) => format!("nightly {date}"),
        None => version.to_string(),
    }
}

/// The channel's name as the launcher shows it.
pub fn channel_label(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "Stable",
        Channel::BleedingEdge => "Bleeding Edge",
    }
}

/// A byte count as `180 MB` or `1.1 GB`.
pub fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let megabytes = bytes as f64 / MB;
    if megabytes >= 1024.0 {
        format!("{:.1} GB", megabytes / 1024.0)
    } else if megabytes >= 1.0 {
        format!("{} MB", megabytes.round() as u64)
    } else {
        format!("{} KB", (bytes as f64 / 1024.0).ceil() as u64)
    }
}

/// Which editor a project opens with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectEditor {
    /// This editor: the pin selects it, or names its line, or the project
    /// has no pin and no default install is set.
    Running,
    /// Another installed editor, opened by relaunching into it.
    Installed(Install),
    /// The pinned editor is not installed; `query` installs it.
    Missing { query: String },
}

/// One project in the launcher's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectEntry {
    pub name: String,
    pub path: PathBuf,
    /// When it was last opened, RFC 3339.
    pub last_opened: Option<String>,
    pub folder_missing: bool,
    /// The editor version the project was saved with, or its nightly.
    pub pinned: Option<String>,
    pub channel: Channel,
    /// The Bevy minor or `main` the pin targets.
    pub bevy: Option<String>,
    pub bevy_rev: Option<String>,
    pub editor: ProjectEditor,
    /// Display names of the modules it uses.
    pub modules: Vec<String>,
    /// How many editor extensions it turns on over the global list.
    pub extensions: usize,
    pub is_extension: bool,
}

impl ProjectEntry {
    /// The label of the editor chip on the project's row.
    pub fn editor_chip(&self, running: &RunningEditor) -> String {
        match (&self.editor, &self.pinned) {
            (ProjectEditor::Installed(install), _) => editor_chip(&install.version, &install.bevy),
            (_, Some(pinned)) => editor_chip(pinned, self.bevy.as_deref().unwrap_or("?")),
            (ProjectEditor::Running, None) => running.chip(),
            (ProjectEditor::Missing { query }, None) => version_label(query),
        }
    }

    pub fn needs_editor(&self) -> bool {
        matches!(self.editor, ProjectEditor::Missing { .. })
    }

    /// `jackdaw 0.19 not installed`, for a project whose editor is missing.
    pub fn missing_label(&self) -> Option<String> {
        let ProjectEditor::Missing { query } = &self.editor else {
            return None;
        };
        Some(format!("jackdaw {} not installed", version_label(query)))
    }
}

/// Which projects the list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectFilter {
    #[default]
    All,
    Stable,
    BleedingEdge,
    NeedsEditor,
}

impl ProjectFilter {
    pub const ALL: [Self; 4] = [
        Self::All,
        Self::Stable,
        Self::BleedingEdge,
        Self::NeedsEditor,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Stable => "stable",
            Self::BleedingEdge => "bleeding-edge",
            Self::NeedsEditor => "needs-editor",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|filter| filter.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Stable => "Stable",
            Self::BleedingEdge => "Bleeding Edge",
            Self::NeedsEditor => "Needs an editor",
        }
    }

    pub fn keeps(self, entry: &ProjectEntry) -> bool {
        match self {
            Self::All => true,
            Self::Stable => entry.channel == Channel::Stable,
            Self::BleedingEdge => entry.channel == Channel::BleedingEdge,
            Self::NeedsEditor => entry.needs_editor(),
        }
    }
}

/// The projects `filter` keeps whose name or path contains `query`,
/// ignoring case.
pub fn filter_projects<'a>(
    projects: &'a [ProjectEntry],
    filter: ProjectFilter,
    query: &str,
) -> Vec<&'a ProjectEntry> {
    let query = query.trim().to_lowercase();
    projects
        .iter()
        .filter(|entry| filter.keeps(entry))
        .filter(|entry| {
            query.is_empty()
                || entry.name.to_lowercase().contains(&query)
                || entry.path.to_string_lossy().to_lowercase().contains(&query)
        })
        .collect()
}

/// Which editors the Editors page shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditorFilter {
    #[default]
    All,
    Stable,
    BleedingEdge,
}

impl EditorFilter {
    pub const ALL: [Self; 3] = [Self::All, Self::Stable, Self::BleedingEdge];

    pub fn id(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Stable => "stable",
            Self::BleedingEdge => "bleeding-edge",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|filter| filter.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Stable => "Stable",
            Self::BleedingEdge => "Bleeding Edge",
        }
    }

    pub fn keeps(self, line: &EditorLine) -> bool {
        match self {
            Self::All => true,
            Self::Stable => !line.is_main(),
            Self::BleedingEdge => line.is_main(),
        }
    }
}

/// How the release index describes a Bevy line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Current,
    Previous,
    Unsupported,
    /// Nightly builds on Bevy main.
    Main,
    /// Not in the index, or no index is cached.
    Unlisted,
}

/// An installed editor as the Editors page lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledEditor {
    pub install: Install,
    pub is_default: bool,
    pub is_running: bool,
    /// Projects that open with it.
    pub projects: Vec<PathBuf>,
    /// A newer release of the same line, from the index.
    pub update: Option<String>,
    /// The newest installed nightly.
    pub newest: bool,
}

/// A line or channel with no install that the launcher offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installable {
    /// The release `Install` fetches.
    pub version: String,
    /// What `jd editor install` is passed.
    pub query: String,
    /// Download size for this machine, when the index has a build for it.
    pub download: Option<u64>,
    /// Projects that need it.
    pub needed_by: Vec<String>,
}

/// One Bevy line on the Editors page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorLine {
    /// The Bevy minor, or `main`.
    pub bevy: String,
    pub kind: LineKind,
    pub installs: Vec<InstalledEditor>,
    pub installable: Option<Installable>,
    /// Nightlies `jd editor clean` would remove.
    pub rollback: Vec<String>,
    /// Their size on disk, when the index records it.
    pub rollback_bytes: Option<u64>,
}

impl EditorLine {
    pub fn is_main(&self) -> bool {
        self.bevy == "main"
    }

    /// `current line`, `previous line` and so on.
    pub fn note(&self) -> &'static str {
        match self.kind {
            LineKind::Current => "current line",
            LineKind::Previous => "previous line",
            LineKind::Unsupported => "no longer supported",
            LineKind::Main => "nightly builds on Bevy main - things will break",
            LineKind::Unlisted => "",
        }
    }
}

/// The release index as the launcher last verified it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CachedIndex {
    /// No index has been fetched yet.
    #[default]
    None,
    Verified(ReleaseIndex),
    /// A cached index that does not verify.
    Unusable(String),
}

impl CachedIndex {
    /// The cached index under `layout`, verified with the key the
    /// environment names or the official one.
    pub fn read(layout: &Layout) -> Self {
        let Some((bytes, signature)) = layout.read_cached_index() else {
            return Self::None;
        };
        let override_key = std::env::var(jackdaw_releases::INDEX_KEY_VAR)
            .ok()
            .filter(|key| !key.is_empty());
        let key = match signing::trusted_key(override_key.as_deref()) {
            Ok(key) => key,
            Err(error) => return Self::Unusable(error.to_string()),
        };
        match ReleaseIndex::from_signed(&bytes, &signature, &key) {
            Ok(index) => Self::Verified(index),
            Err(error) => Self::Unusable(error.to_string()),
        }
    }

    pub fn index(&self) -> Option<&ReleaseIndex> {
        match self {
            Self::Verified(index) => Some(index),
            _ => None,
        }
    }
}

/// One recent project as the recent-project list records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentProject {
    pub name: String,
    pub path: PathBuf,
    pub last_opened: Option<String>,
}

/// Everything the launcher pages draw from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherData {
    pub running: RunningEditor,
    pub installs: Installs,
    pub index: CachedIndex,
    pub projects: Vec<ProjectEntry>,
    pub lines: Vec<EditorLine>,
    /// Whether this editor is in the installed-editor registry.
    pub running_registered: bool,
    /// The Rust target this machine downloads builds for.
    pub host_target: &'static str,
    pub launcher_dir: Option<PathBuf>,
}

impl LauncherData {
    /// Read the launcher's state from disk.
    pub fn read() -> Self {
        let layout = Layout::from_env();
        let installs = layout
            .as_ref()
            .and_then(|layout| layout.load_installs().ok())
            .unwrap_or_default();
        let index = layout.as_ref().map(CachedIndex::read).unwrap_or_default();
        let recent = crate::project::read_recent_projects_including_missing()
            .projects
            .into_iter()
            .map(|entry| RecentProject {
                name: entry.name,
                path: entry.path,
                last_opened: Some(entry.last_opened).filter(|when| !when.is_empty()),
            })
            .collect::<Vec<_>>();
        let running = RunningEditor::detect(&installs);
        let mut data = Self::build(
            running,
            installs,
            index,
            &recent,
            jackdaw_package::host_target(),
        );
        data.launcher_dir = layout.map(|layout| layout.root().to_path_buf());
        data
    }

    /// The launcher's state from what was read.
    pub fn build(
        running: RunningEditor,
        installs: Installs,
        index: CachedIndex,
        recent: &[RecentProject],
        host_target: &'static str,
    ) -> Self {
        let projects: Vec<ProjectEntry> = recent
            .iter()
            .map(|project| project_entry(project, &installs, &running))
            .collect();
        let lines = editor_lines(&installs, index.index(), &projects, &running, host_target);
        let running_registered = installs.get(&running.version).is_some();
        Self {
            running,
            installs,
            index,
            projects,
            lines,
            running_registered,
            host_target,
            launcher_dir: None,
        }
    }

    pub fn project(&self, path: &Path) -> Option<&ProjectEntry> {
        self.projects.iter().find(|entry| entry.path == path)
    }

    pub fn install(&self, version: &str) -> Option<&InstalledEditor> {
        self.lines
            .iter()
            .flat_map(|line| &line.installs)
            .find(|editor| editor.install.version == version)
    }

    /// Installed editors other than this one a project could be opened
    /// with, newest first.
    pub fn other_installs(&self) -> Vec<&Install> {
        let mut installs: Vec<&Install> = self
            .installs
            .installs
            .iter()
            .filter(|install| install.version != self.running.version)
            .collect();
        installs.sort_by(|a, b| version::compare(&b.version, &a.version));
        installs
    }

    /// The newest stable editor on this machine, this one or an install:
    /// what a project moving to a newer Bevy is offered.
    pub fn newest_editor(&self) -> String {
        self.installs
            .installs
            .iter()
            .filter(|install| install.channel == Channel::Stable)
            .map(|install| install.version.as_str())
            .chain(
                (self.running.channel == Channel::Stable).then_some(self.running.version.as_str()),
            )
            .max_by(|a, b| version::compare(a, b))
            .unwrap_or(&self.running.version)
            .to_string()
    }

    /// The release `query` names in the index.
    pub fn release(&self, query: &str) -> Option<&jackdaw_releases::Release> {
        self.index.index()?.resolve(query).ok()
    }

    /// Download size of the release `query` names, for this machine.
    pub fn download_size(&self, query: &str) -> Option<u64> {
        self.release(query)?
            .artifact_for(self.host_target)
            .map(|artifact| artifact.size)
    }

    /// Whether any editor has an update in the index.
    pub fn update_available(&self) -> Option<String> {
        self.lines
            .iter()
            .flat_map(|line| &line.installs)
            .find_map(|editor| editor.update.clone())
    }
}

/// The pin in a project's `jackdaw.toml`.
pub fn project_pin(manifest: &ProjectManifest) -> EditorPin {
    let pins = &manifest.jackdaw;
    EditorPin::from_fields(
        pins.version.as_deref(),
        pins.channel.as_deref(),
        pins.build.as_deref(),
    )
    .unwrap_or_default()
}

/// Which editor opens a project pinned with `pin`.
///
/// Mirrors `jd open`: a matching install wins, and this editor stands in
/// for a stable pin of its own line when no install of that line exists.
/// A project without a pin opens with the default install, or here.
pub fn resolve_editor(
    pin: &EditorPin,
    installs: &Installs,
    running: &RunningEditor,
) -> ProjectEditor {
    let installed = |install: &Install| {
        if install.version == running.version {
            ProjectEditor::Running
        } else {
            ProjectEditor::Installed(install.clone())
        }
    };
    match installs.resolve_pin(pin) {
        PinResolution::Unpinned => installs
            .default_install()
            .map_or(ProjectEditor::Running, installed),
        PinResolution::Installed { install, .. } => {
            let stable = pin.channel.unwrap_or(Channel::Stable) == Channel::Stable;
            let pinned_here = pin.version.as_deref() == Some(running.version.as_str());
            if stable && pinned_here && running.channel == Channel::Stable {
                ProjectEditor::Running
            } else {
                installed(install)
            }
        }
        PinResolution::Missing {
            install_query: query,
        } => {
            let stable = pin.channel.unwrap_or(Channel::Stable) == Channel::Stable;
            let same_line = pin.version.as_deref().and_then(version::line_of)
                == version::line_of(&running.version);
            let this_line = stable && same_line && running.channel == Channel::Stable;
            let this_nightly = !stable && pin.build.as_deref() == Some(running.version.as_str());
            if this_line || this_nightly {
                ProjectEditor::Running
            } else {
                ProjectEditor::Missing { query }
            }
        }
    }
}

/// A project that is not in the recent list, described the same way.
pub fn describe_project(path: &Path, installs: &Installs, running: &RunningEditor) -> ProjectEntry {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    project_entry(
        &RecentProject {
            name,
            path: path.to_path_buf(),
            last_opened: None,
        },
        installs,
        running,
    )
}

fn project_entry(
    project: &RecentProject,
    installs: &Installs,
    running: &RunningEditor,
) -> ProjectEntry {
    let folder_missing = !project.path.is_dir();
    let manifest = if folder_missing {
        ProjectManifest::default()
    } else {
        ProjectManifest::read(&project.path)
    };
    let pin = project_pin(&manifest);
    let editor = if folder_missing {
        ProjectEditor::Running
    } else {
        resolve_editor(&pin, installs, running)
    };
    let catalog = ModuleCatalog::builtin();
    let modules = if folder_missing {
        Vec::new()
    } else {
        manifest
            .modules(&project.path)
            .iter()
            .filter_map(|id| catalog.get(id).map(|module| module.name.clone()))
            .collect()
    };
    let extensions = manifest
        .extensions
        .as_ref()
        .map_or(0, |table| table.values().filter(|on| **on).count());
    let is_extension = !folder_missing
        && !project.path.join("src").join("main.rs").is_file()
        && jackdaw_project_build::detect::detect_extension(&project.path).is_some();
    ProjectEntry {
        name: project.name.clone(),
        path: project.path.clone(),
        last_opened: project.last_opened.clone(),
        folder_missing,
        pinned: pin.build.clone().or(pin.version.clone()),
        channel: pin.channel.unwrap_or(Channel::Stable),
        bevy: manifest.jackdaw.bevy.clone(),
        bevy_rev: manifest.jackdaw.bevy_rev.clone(),
        editor,
        modules,
        extensions,
        is_extension,
    }
}

fn line_kind(bevy: &str, index: Option<&ReleaseIndex>) -> LineKind {
    if bevy == "main" {
        return LineKind::Main;
    }
    let status = index.and_then(|index| {
        index
            .channels
            .stable
            .lines
            .iter()
            .find(|line| line.bevy == bevy)
            .map(|line| line.status)
    });
    match status {
        Some(LineStatus::Current) => LineKind::Current,
        Some(LineStatus::Previous) => LineKind::Previous,
        Some(LineStatus::Unsupported) => LineKind::Unsupported,
        None => LineKind::Unlisted,
    }
}

fn bevy_order(bevy: &str) -> (bool, std::cmp::Reverse<Vec<u64>>) {
    (
        bevy == "main",
        std::cmp::Reverse(
            bevy.split('.')
                .filter_map(|part| part.parse().ok())
                .collect(),
        ),
    )
}

fn editor_lines(
    installs: &Installs,
    index: Option<&ReleaseIndex>,
    projects: &[ProjectEntry],
    running: &RunningEditor,
    host_target: &str,
) -> Vec<EditorLine> {
    let mut bevies: Vec<String> = installs
        .installs
        .iter()
        .map(|install| install.bevy.clone())
        .collect();
    if let Some(index) = index {
        bevies.extend(
            index
                .channels
                .stable
                .lines
                .iter()
                .filter(|line| line.status != LineStatus::Unsupported)
                .map(|line| line.bevy.clone()),
        );
        if index.channels.bleeding_edge.latest.is_some() {
            bevies.push("main".into());
        }
    }
    for project in projects {
        if let (ProjectEditor::Missing { .. }, Some(bevy)) = (&project.editor, &project.bevy) {
            bevies.push(bevy.clone());
        }
    }
    bevies.push(running.bevy.clone());
    bevies.sort_by_key(|bevy| bevy_order(bevy));
    bevies.dedup();

    let removable: Vec<String> = installs
        .removable_nightlies()
        .iter()
        .map(|install| install.version.clone())
        .collect();
    let newest_nightly = installs
        .installs
        .iter()
        .filter(|install| install.channel == Channel::BleedingEdge)
        .max_by(|a, b| version::compare(&a.version, &b.version))
        .map(|install| install.version.clone());

    bevies
        .into_iter()
        .map(|bevy| {
            let kind = line_kind(&bevy, index);
            let members: Vec<InstalledEditor> = installs
                .installs
                .iter()
                .filter(|install| install.bevy == bevy)
                .map(|install| InstalledEditor {
                    install: install.clone(),
                    is_default: installs.default.as_deref() == Some(install.version.as_str()),
                    is_running: install.version == running.version,
                    projects: projects_using(install, installs, projects),
                    update: update_for(install, installs, index),
                    newest: newest_nightly.as_deref() == Some(install.version.as_str()),
                })
                .collect();
            let installable = if members.is_empty() {
                installable_for(&bevy, index, projects, host_target)
            } else {
                None
            };
            let rollback: Vec<String> = members
                .iter()
                .filter(|editor| removable.contains(&editor.install.version))
                .map(|editor| editor.install.version.clone())
                .collect();
            let rollback_bytes = index.and_then(|index| {
                rollback
                    .iter()
                    .map(|version| {
                        index
                            .find(version)?
                            .artifact_for(host_target)?
                            .installed_size
                    })
                    .sum::<Option<u64>>()
            });
            EditorLine {
                bevy,
                kind,
                installs: members,
                installable,
                rollback,
                rollback_bytes,
            }
        })
        .collect()
}

fn projects_using(
    install: &Install,
    installs: &Installs,
    projects: &[ProjectEntry],
) -> Vec<PathBuf> {
    let mut using: Vec<PathBuf> = installs
        .pinning_projects(&install.version)
        .into_iter()
        .map(Path::to_path_buf)
        .collect();
    for project in projects {
        let opens_here = match &project.editor {
            ProjectEditor::Installed(other) => other.version == install.version,
            _ => false,
        };
        if opens_here && !using.contains(&project.path) {
            using.push(project.path.clone());
        }
    }
    using
}

fn update_for(
    install: &Install,
    installs: &Installs,
    index: Option<&ReleaseIndex>,
) -> Option<String> {
    let index = index?;
    if install.channel != Channel::Stable {
        return None;
    }
    let line = version::line_of(&install.version)?;
    let latest = &index
        .channels
        .stable
        .lines
        .iter()
        .find(|known| version::line_of(&known.latest).as_deref() == Some(line.as_str()))?
        .latest;
    let newest_installed = installs
        .installs
        .iter()
        .filter(|other| {
            other.channel == Channel::Stable
                && version::line_of(&other.version).as_deref() == Some(line.as_str())
        })
        .max_by(|a, b| version::compare(&a.version, &b.version))?;
    (newest_installed.version == install.version
        && version::compare(latest, &install.version).is_gt())
    .then(|| latest.clone())
}

fn installable_for(
    bevy: &str,
    index: Option<&ReleaseIndex>,
    projects: &[ProjectEntry],
    host_target: &str,
) -> Option<Installable> {
    let needed_by: Vec<String> = projects
        .iter()
        .filter(|project| project.needs_editor() && project.bevy.as_deref() == Some(bevy))
        .map(|project| project.name.clone())
        .collect();
    let (version, query) = if bevy == "main" {
        let latest = index?.channels.bleeding_edge.latest.clone()?;
        (latest, "bleeding-edge".to_string())
    } else {
        let latest = index
            .and_then(|index| {
                index
                    .channels
                    .stable
                    .lines
                    .iter()
                    .find(|line| line.bevy == bevy)
            })
            .map(|line| line.latest.clone());
        match latest {
            Some(latest) => {
                let query = version::line_of(&latest).unwrap_or_else(|| latest.clone());
                (latest, query)
            }
            None if !needed_by.is_empty() => (bevy.to_string(), bevy.to_string()),
            None => return None,
        }
    };
    let download = index
        .and_then(|index| index.find(&version))
        .and_then(|release| release.artifact_for(host_target))
        .map(|artifact| artifact.size);
    Some(Installable {
        version,
        query,
        download,
        needed_by,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_releases::{Artifact, Release};

    const TARGET: &str = "x86_64-unknown-linux-gnu";

    fn install(version: &str) -> Install {
        let nightly = version.starts_with("nightly-");
        Install {
            version: version.into(),
            channel: if nightly {
                Channel::BleedingEdge
            } else {
                Channel::Stable
            },
            bevy: if nightly {
                "main".into()
            } else {
                version::line_of(version).unwrap()
            },
            bevy_rev: None,
            path: PathBuf::from(format!("/editors/{version}")),
            managed: true,
            modules: Vec::new(),
            known_good: false,
        }
    }

    fn release(version: &str, size: u64) -> Release {
        let nightly = version.starts_with("nightly-");
        Release {
            version: version.into(),
            channel: if nightly {
                Channel::BleedingEdge
            } else {
                Channel::Stable
            },
            bevy: if nightly {
                "main".into()
            } else {
                version::line_of(version).unwrap()
            },
            bevy_rev: None,
            jackdaw_rev: None,
            date: "2026-10-09".into(),
            rustc: None,
            sdk_id: None,
            notes_url: None,
            yanked: false,
            broken: false,
            known_good: false,
            known_issues: Vec::new(),
            artifacts: vec![Artifact {
                target: TARGET.into(),
                url: String::new(),
                sha256: String::new(),
                size,
                installed_size: Some(size * 3),
                attestation: None,
            }],
            modules_index: None,
        }
    }

    fn index(releases: &[Release]) -> ReleaseIndex {
        let mut index = ReleaseIndex::default();
        let today = version::parse_date("2026-10-12").unwrap();
        for release in releases {
            index.add_release(release.clone(), today);
        }
        index.refresh_channels();
        index
    }

    fn running(version: &str) -> RunningEditor {
        RunningEditor {
            version: version.into(),
            bevy: version::line_of(version).unwrap(),
            channel: Channel::Stable,
            bevy_rev: None,
        }
    }

    fn pin(version: &str) -> EditorPin {
        EditorPin {
            version: Some(version.into()),
            ..EditorPin::default()
        }
    }

    #[test]
    fn a_stable_pin_of_this_editors_line_opens_here_without_an_install() {
        let installs = Installs::default();
        assert_eq!(
            resolve_editor(&pin("0.20.0"), &installs, &running("0.20.1")),
            ProjectEditor::Running
        );
        assert_eq!(
            resolve_editor(&pin("0.19.0"), &installs, &running("0.20.1")),
            ProjectEditor::Missing {
                query: "0.19".into()
            }
        );
    }

    #[test]
    fn a_pin_another_install_satisfies_relaunches_into_it() {
        let mut installs = Installs::default();
        installs.add(install("0.19.2"));
        installs.add(install("0.20.1"));
        assert_eq!(
            resolve_editor(&pin("0.19.0"), &installs, &running("0.20.1")),
            ProjectEditor::Installed(installs.get("0.19.2").unwrap().clone())
        );
        assert_eq!(
            resolve_editor(&pin("0.20.0"), &installs, &running("0.20.1")),
            ProjectEditor::Running
        );
    }

    #[test]
    fn a_nightly_pin_needs_its_exact_build() {
        let mut installs = Installs::default();
        installs.add(install("nightly-2026-10-12"));
        let nightly = EditorPin {
            version: Some("0.21.0-dev".into()),
            channel: Some(Channel::BleedingEdge),
            build: Some("nightly-2026-10-09".into()),
        };
        assert_eq!(
            resolve_editor(&nightly, &installs, &running("0.20.1")),
            ProjectEditor::Missing {
                query: "nightly-2026-10-09".into()
            }
        );
        installs.add(install("nightly-2026-10-09"));
        assert!(matches!(
            resolve_editor(&nightly, &installs, &running("0.20.1")),
            ProjectEditor::Installed(found) if found.version == "nightly-2026-10-09"
        ));
    }

    #[test]
    fn chips_name_the_version_and_bevy_line_or_the_nightly_date() {
        assert_eq!(editor_chip("0.20.0", "0.20"), "0.20.0 - Bevy 0.20");
        assert_eq!(
            editor_chip("nightly-2026-10-09", "main"),
            "nightly 2026-10-09"
        );
        assert_eq!(format_bytes(180 * 1024 * 1024), "180 MB");
        assert_eq!(format_bytes(1181 * 1024 * 1024), "1.2 GB");
    }

    fn entry(name: &str, channel: Channel, editor: ProjectEditor) -> ProjectEntry {
        ProjectEntry {
            name: name.into(),
            path: PathBuf::from(format!("/work/{name}")),
            last_opened: None,
            folder_missing: false,
            pinned: None,
            channel,
            bevy: None,
            bevy_rev: None,
            editor,
            modules: Vec::new(),
            extensions: 0,
            is_extension: false,
        }
    }

    #[test]
    fn filters_keep_a_channel_or_the_projects_missing_an_editor_and_match_text() {
        let projects = vec![
            entry("village", Channel::Stable, ProjectEditor::Running),
            entry("fells", Channel::BleedingEdge, ProjectEditor::Running),
            entry(
                "old-demo",
                Channel::Stable,
                ProjectEditor::Missing {
                    query: "0.19".into(),
                },
            ),
        ];
        let names = |filter, query| {
            filter_projects(&projects, filter, query)
                .into_iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(ProjectFilter::All, ""),
            ["village", "fells", "old-demo"]
        );
        assert_eq!(names(ProjectFilter::Stable, ""), ["village", "old-demo"]);
        assert_eq!(names(ProjectFilter::BleedingEdge, ""), ["fells"]);
        assert_eq!(names(ProjectFilter::NeedsEditor, ""), ["old-demo"]);
        assert_eq!(names(ProjectFilter::All, "DEMO"), ["old-demo"]);
    }

    #[test]
    fn editor_lines_group_installs_offer_missing_lines_and_report_updates() {
        let mut installs = Installs::default();
        installs.add(install("0.20.0"));
        installs.add(install("nightly-2026-10-12"));
        installs.add(install("nightly-2026-10-08"));
        let index = index(&[
            release("0.19.0", 180 << 20),
            release("0.20.0", 186 << 20),
            release("0.20.1", 186 << 20),
            release("nightly-2026-10-08", 190 << 20),
            release("nightly-2026-10-12", 190 << 20),
        ]);
        let mut missing = entry(
            "bk-village-demo",
            Channel::Stable,
            ProjectEditor::Missing {
                query: "0.19".into(),
            },
        );
        missing.bevy = Some("0.19".into());
        let data = LauncherData::build(
            running("0.20.0"),
            installs,
            CachedIndex::Verified(index),
            &[],
            TARGET,
        );
        let lines = editor_lines(
            &data.installs,
            data.index.index(),
            &[missing],
            &data.running,
            TARGET,
        );
        let bevies: Vec<&str> = lines.iter().map(|line| line.bevy.as_str()).collect();
        assert_eq!(bevies, ["0.20", "0.19", "main"]);

        let current = &lines[0];
        assert_eq!(current.installs[0].update.as_deref(), Some("0.20.1"));
        assert!(current.installs[0].is_default && current.installs[0].is_running);

        let previous = &lines[1];
        let offer = previous.installable.as_ref().expect("an install offer");
        assert_eq!(offer.version, "0.19.0");
        assert_eq!(offer.query, "0.19");
        assert_eq!(offer.download, Some(180 << 20));
        assert_eq!(offer.needed_by, ["bk-village-demo"]);

        let main = &lines[2];
        assert!(main.installs.iter().any(|editor| editor.newest));
        assert_eq!(main.rollback, ["nightly-2026-10-08"]);
        assert_eq!(main.rollback_bytes, Some(3 * (190 << 20)));
    }
}
