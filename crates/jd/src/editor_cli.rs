//! The `jd editor`, `jd self` and `jd open` commands.

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "command-line output goes to the terminal"
)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use jackdaw_project_build::modules::ModuleCatalog;
use jackdaw_releases::index::LineStatus;
use jackdaw_releases::{Channel, Install, Installs, Layout, ReleaseIndex, version};

use crate::editors::{
    self, EditorChoice, IndexOrigin, InstallOutcome, InstallProgress, RemoveError,
};
use crate::self_update::{self, SelfUpdate};

const EDITOR_USAGE: &str = "\
usage: jd editor <command> [--json]

commands:
  list [--available]          Installed editors, or every published one
  install <version|line|stable|bleeding-edge> [--modules <list>]
                              Download, verify and install an editor
  remove <version> [--force]  Remove an editor (refused while projects use it)
  default <version>           Open unpinned projects with this editor
  locate <path>               Register an editor installed another way
  verify <version>            Check that an installed editor runs
  refresh                     Fetch the release index into the cache
  clean                       Remove nightlies no project uses

--json prints one JSON object per line instead: progress, stage, message,
warning, index, done and error events, for programs driving jd.";

fn layout() -> Result<Layout, String> {
    Layout::from_env().ok_or_else(|| "cannot find a data directory for installs".to_string())
}

fn fail(command: &str, message: impl std::fmt::Display) -> ExitCode {
    eprintln!("jd {command}: {message}");
    ExitCode::FAILURE
}

/// Where a command's output goes: lines for a person, or with `--json`
/// one JSON object per line for a program driving `jd`.
struct Report {
    json: bool,
    /// The last whole percent of a download reported, so `--json` prints
    /// one progress event per percent rather than one per chunk.
    last_percent: std::cell::Cell<Option<u64>>,
}

impl Report {
    fn new(json: bool) -> Self {
        Self {
            json,
            last_percent: std::cell::Cell::new(None),
        }
    }

    fn emit(&self, event: serde_json::Value) {
        println!("{event}");
    }

    fn say(&self, text: impl AsRef<str>) {
        let text = text.as_ref();
        if self.json {
            self.emit(serde_json::json!({ "event": "message", "text": text }));
        } else {
            println!("{text}");
        }
    }

    fn warn(&self, text: impl AsRef<str>) {
        let text = text.as_ref();
        if self.json {
            self.emit(serde_json::json!({ "event": "warning", "text": text }));
        } else {
            eprintln!("warning: {text}");
        }
    }

    fn done(&self, text: impl AsRef<str>) {
        let text = text.as_ref();
        if self.json {
            self.emit(serde_json::json!({ "event": "done", "text": text }));
        } else {
            println!("{text}");
        }
    }

    fn progress(&self, progress: InstallProgress) {
        if !self.json {
            return;
        }
        let stage = match progress {
            InstallProgress::Download { done, total } => {
                let percent = (done * 100).checked_div(total).unwrap_or(100);
                if self.last_percent.replace(Some(percent)) == Some(percent) {
                    return;
                }
                self.emit(serde_json::json!({
                    "event": "progress",
                    "stage": "download",
                    "done": done,
                    "total": total,
                }));
                return;
            }
            InstallProgress::Unpacking => "unpack",
            InstallProgress::Modules => "modules",
            InstallProgress::Registering => "register",
        };
        self.emit(serde_json::json!({ "event": "stage", "stage": stage }));
    }

    fn fail(&self, command: &str, message: impl std::fmt::Display) -> ExitCode {
        if self.json {
            self.emit(serde_json::json!({ "event": "error", "text": message.to_string() }));
        }
        fail(command, message)
    }
}

/// Run `jd editor <args>`.
pub fn editor_command(args: &[String]) -> ExitCode {
    let layout = match layout() {
        Ok(layout) => layout,
        Err(error) => return fail("editor", error),
    };
    let positional = |index: usize| {
        args.iter()
            .skip(1)
            .filter(|arg| !arg.starts_with('-'))
            .nth(index)
            .cloned()
    };
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    if has("--help") || has("-h") {
        println!("{EDITOR_USAGE}");
        return ExitCode::SUCCESS;
    }
    let report = Report::new(has("--json"));
    let result = match args.first().map(String::as_str) {
        Some("list") if has("--available") => list_available(&layout),
        Some("list") => list_installed(&layout),
        Some("install") => match args.get(1).filter(|arg| !arg.starts_with('-')) {
            Some(query) => install(
                &layout,
                query,
                flag_value(args, "--modules").as_deref(),
                &report,
            ),
            None => Err("usage: jd editor install <version|line|stable|bleeding-edge>".into()),
        },
        Some("remove") => match positional(0) {
            Some(version) => remove(&layout, &version, has("--force"), &report),
            None => Err("usage: jd editor remove <version> [--force]".into()),
        },
        Some("default") => match positional(0) {
            Some(version) => set_default(&layout, &version, &report),
            None => Err("usage: jd editor default <version>".into()),
        },
        Some("locate") => match positional(0) {
            Some(path) => locate(&layout, Path::new(&path), &report),
            None => Err("usage: jd editor locate <path>".into()),
        },
        Some("verify") => match positional(0) {
            Some(version) => verify(&layout, &version, &report),
            None => Err("usage: jd editor verify <version>".into()),
        },
        Some("refresh") => refresh(&layout, &report),
        Some("clean") => clean(&layout, &report),
        _ => {
            eprintln!("{EDITOR_USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => report.fail(
            &format!("editor {}", args.first().map_or("", String::as_str)),
            error,
        ),
    }
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn describe(install: &Install, installs: &Installs) -> String {
    let mut notes = vec![install.channel.as_str().to_string()];
    if installs.default.as_deref() == Some(install.version.as_str()) {
        notes.push("default".into());
    }
    if install.known_good {
        notes.push("known good".into());
    }
    if !install.managed {
        notes.push("located".into());
    }
    if !install.modules.is_empty() {
        notes.push(format!("modules: {}", install.modules.join(", ")));
    }
    format!(
        "  {:<20} {:<40} {}",
        install.version,
        notes.join(", "),
        install.path.display()
    )
}

/// Bevy lines newest first, `main` (Bleeding Edge) on top.
fn by_bevy_line(installs: &Installs) -> Vec<(String, Vec<&Install>)> {
    let mut lines: Vec<(String, Vec<&Install>)> = Vec::new();
    for install in &installs.installs {
        match lines.iter_mut().find(|(bevy, _)| *bevy == install.bevy) {
            Some((_, members)) => members.push(install),
            None => lines.push((install.bevy.clone(), vec![install])),
        }
    }
    lines.sort_by(|(a, _), (b, _)| {
        let rank = |bevy: &str| (bevy != "main", std::cmp::Reverse(bevy_sort_key(bevy)));
        rank(a).cmp(&rank(b))
    });
    lines
}

fn bevy_sort_key(bevy: &str) -> Vec<u64> {
    bevy.split('.')
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn list_installed(layout: &Layout) -> Result<(), String> {
    let installs = layout.load_installs()?;
    if installs.installs.is_empty() {
        println!("No editors installed. Run `jd editor install stable` to install one.");
        return Ok(());
    }
    for (bevy, members) in by_bevy_line(&installs) {
        println!("Bevy {bevy}");
        for install in members {
            println!("{}", describe(install, &installs));
        }
    }
    if let Some(index) = editors::cached_index(layout) {
        for hint in update_hints(&installs, &index) {
            println!("{hint}");
        }
    }
    Ok(())
}

/// Newer releases of the lines and channel already installed, from the
/// cached index. Only reported; nothing is downloaded.
fn update_hints(installs: &Installs, index: &ReleaseIndex) -> Vec<String> {
    let mut hints = Vec::new();
    for line in &index.channels.stable.lines {
        let newest_installed = installs
            .installs
            .iter()
            .filter(|install| install.channel == Channel::Stable)
            .filter(|install| version::line_of(&install.version) == version::line_of(&line.latest))
            .max_by(|a, b| version::compare(&a.version, &b.version));
        if let Some(installed) = newest_installed
            && version::compare(&line.latest, &installed.version).is_gt()
        {
            let query = version::line_of(&line.latest).unwrap_or_default();
            hints.push(format!(
                "{} is available: jd editor install {query}",
                line.latest
            ));
        }
    }
    let newest_nightly = installs
        .installs
        .iter()
        .filter(|install| install.channel == Channel::BleedingEdge)
        .max_by(|a, b| version::compare(&a.version, &b.version));
    if let (Some(installed), Some(latest)) = (
        newest_nightly,
        index.channels.bleeding_edge.latest.as_deref(),
    ) && version::compare(latest, &installed.version).is_gt()
    {
        hints.push(format!(
            "{latest} is available: jd editor install bleeding-edge"
        ));
    }
    hints
}

/// Fetch the index, reporting any environment override and a fall back to
/// the cached copy.
fn load_index(layout: &Layout) -> Result<ReleaseIndex, String> {
    load_index_reporting(layout, &Report::new(false)).map(|(index, _)| index)
}

fn load_index_reporting(
    layout: &Layout,
    report: &Report,
) -> Result<(ReleaseIndex, IndexOrigin), String> {
    for warning in editors::index_overrides() {
        report.warn(warning);
    }
    let (index, origin) = editors::load_index(layout)?;
    if let IndexOrigin::Cached(reason) = &origin {
        report.warn(format!("using the cached release index ({reason})"));
    }
    Ok((index, origin))
}

fn refresh(layout: &Layout, report: &Report) -> Result<(), String> {
    let (index, origin) = load_index_reporting(layout, report)?;
    let (origin, reason) = match origin {
        IndexOrigin::Fetched => ("fetched", None),
        IndexOrigin::Cached(reason) => ("cached", Some(reason)),
    };
    if report.json {
        report.emit(serde_json::json!({
            "event": "index",
            "origin": origin,
            "serial": index.serial,
            "reason": reason,
            "jd": jackdaw_project_build::VERSION,
        }));
    } else {
        println!(
            "release index serial {} with {} releases ({origin})",
            index.serial,
            index.releases.len()
        );
    }
    Ok(())
}

fn verify(layout: &Layout, version: &str, report: &Report) -> Result<(), String> {
    let installs = layout.load_installs()?;
    let install = installs
        .get(version)
        .ok_or_else(|| format!("{version} is not installed"))?;
    let reported = editors::query_version(&install.executable())?;
    if install.channel == Channel::Stable && reported.version != install.version {
        return Err(format!(
            "{} reports version {}, not {}",
            install.executable().display(),
            reported.version,
            install.version
        ));
    }
    report.done(format!(
        "jackdaw {} at {} runs and reports {} for Bevy {}",
        install.version,
        install.path.display(),
        reported.version,
        reported.bevy
    ));
    Ok(())
}

fn list_available(layout: &Layout) -> Result<(), String> {
    let index = load_index(layout)?;
    let installs = layout.load_installs()?;
    let installed = |version: &str| {
        if installs.get(version).is_some() {
            ", installed"
        } else {
            ""
        }
    };
    println!("Stable");
    for line in &index.channels.stable.lines {
        let status = match line.status {
            LineStatus::Current => "current",
            LineStatus::Previous => "previous",
            LineStatus::Unsupported => "unsupported",
        };
        println!(
            "  Bevy {:<8} {:<20} {status}{}",
            line.bevy,
            line.latest,
            installed(&line.latest)
        );
    }
    let nightlies: Vec<_> = index
        .releases
        .iter()
        .filter(|release| release.channel == Channel::BleedingEdge)
        .collect();
    if !nightlies.is_empty() {
        println!("Bleeding Edge (Bevy main)");
        for release in nightlies {
            let mut notes = Vec::new();
            if index.channels.bleeding_edge.latest.as_deref() == Some(release.version.as_str()) {
                notes.push("latest");
            }
            if release.known_good {
                notes.push("known good");
            }
            if release.broken {
                notes.push("broken");
            }
            println!(
                "  {:<20} {}{}",
                release.version,
                notes.join(", "),
                installed(&release.version)
            );
        }
    }
    Ok(())
}

fn install(
    layout: &Layout,
    query: &str,
    modules: Option<&str>,
    report: &Report,
) -> Result<(), String> {
    let modules: Vec<String> = match modules {
        Some(list) => ModuleCatalog::builtin()
            .parse_list(list)?
            .into_iter()
            .collect(),
        None => Vec::new(),
    };
    let (index, _) = load_index_reporting(layout, report)?;
    let target = jackdaw_package::host_target();
    let release = index.resolve(query)?;
    if release.channel == Channel::BleedingEdge {
        report.say(format!(
            "{} is a Bleeding Edge build against Bevy main. Projects saved with it may not \
             open in a stable editor.",
            release.version
        ));
        for issue in &release.known_issues {
            report.say(format!("  known issue: {issue}"));
        }
    }
    report.say(format!(
        "installing jackdaw {} for {target}",
        release.version
    ));
    let outcome = editors::install(layout, &index, query, &modules, target, &mut |progress| {
        report.progress(progress);
    })?;
    match outcome {
        InstallOutcome::Installed(install) => report.done(format!(
            "installed jackdaw {} (Bevy {}) at {}",
            install.version,
            install.bevy,
            install.path.display()
        )),
        InstallOutcome::AlreadyInstalled(install) => report.done(format!(
            "jackdaw {} is already installed at {}",
            install.version,
            install.path.display()
        )),
    }
    Ok(())
}

fn remove(layout: &Layout, version: &str, force: bool, report: &Report) -> Result<(), String> {
    match editors::remove(layout, version, force) {
        Ok(install) if install.managed => {
            report.done(format!("removed jackdaw {version}"));
            Ok(())
        }
        Ok(install) => {
            report.done(format!(
                "unregistered jackdaw {version}; its files at {} were left in place",
                install.path.display()
            ));
            Ok(())
        }
        Err(RemoveError::NotInstalled) => Err(format!("{version} is not installed")),
        Err(RemoveError::Pinned(projects)) => {
            let list: Vec<String> = projects
                .iter()
                .map(|project| format!("  {}", project.display()))
                .collect();
            Err(format!(
                "these projects open with {version}:\n{}\npass --force to remove it anyway",
                list.join("\n")
            ))
        }
        Err(RemoveError::Failed(error)) => Err(error),
    }
}

fn set_default(layout: &Layout, version: &str, report: &Report) -> Result<(), String> {
    editors::set_default(layout, version)?;
    report.done(format!(
        "projects without a pinned editor now open with jackdaw {version}"
    ));
    Ok(())
}

fn locate(layout: &Layout, path: &Path, report: &Report) -> Result<(), String> {
    let install = editors::locate(layout, path)?;
    report.done(format!(
        "registered jackdaw {} (Bevy {}) at {}",
        install.version,
        install.bevy,
        install.path.display()
    ));
    Ok(())
}

fn clean(layout: &Layout, report: &Report) -> Result<(), String> {
    let removed = editors::clean(layout)?;
    let removed: Vec<String> = removed
        .into_iter()
        .map(|install| format!("removed jackdaw {}", install.version))
        .collect();
    if removed.is_empty() {
        report.done("no nightlies to remove");
    } else {
        report.done(removed.join("\n"));
    }
    Ok(())
}

/// Run `jd self <args>`.
pub fn self_command(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) != Some("update") {
        eprintln!("usage: jd self update");
        return ExitCode::FAILURE;
    }
    match self_update_inner() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail("self update", error),
    }
}

fn self_update_inner() -> Result<(), String> {
    let layout = layout()?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let cargo_bin = self_update::cargo_bin_dir();
    let current = jackdaw_project_build::VERSION;
    let refusal = self_update::refusal(&executable, current, &layout, cargo_bin.as_deref());
    let reason = match refusal {
        None => None,
        Some(SelfUpdate::CargoInstalled) => {
            println!("jd was installed with cargo; update it with `cargo install jd --locked`");
            return Ok(());
        }
        Some(SelfUpdate::PartOfInstall(version)) => {
            Some(format!("this jd belongs to the jackdaw {version} install"))
        }
        Some(SelfUpdate::InBundle(dir)) => Some(format!(
            "this jd is part of the editor bundle in {}, and is updated with it; rerun the \
             install script",
            dir.display()
        )),
        Some(SelfUpdate::DevelopmentBuild) => {
            Some("this jd is a development build; rebuild it from the source checkout".into())
        }
        Some(_) => Some(format!("jd {current} is a prerelease")),
    };
    if let Some(reason) = reason {
        return Err(format!(
            "{reason}. For the newest editor and its jd, run `jd editor install stable`"
        ));
    }
    let index = load_index(&layout)?;
    match self_update::update(&executable, current, &index, jackdaw_package::host_target())? {
        SelfUpdate::Updated { from, to } => println!("updated jd from {from} to {to}"),
        _ => println!("jd {current} is up to date"),
    }
    Ok(())
}

/// Run `jd open [path]`: launch the editor the project's pin selects.
pub fn open_command(args: &[String]) -> ExitCode {
    match open_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail("open", error),
    }
}

fn open_inner(args: &[String]) -> Result<(), String> {
    let root = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let root =
        dunce::canonicalize(&root).map_err(|error| format!("{}: {error}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let layout = layout()?;
    let installs = layout.load_installs()?;
    let pin = editors::project_pin(&root)?;
    let beside = jackdaw_scaffold::editor_executable();
    let choice = editors::choose_editor(&installs, &pin, &beside, jackdaw_project_build::VERSION)?;
    let executable = match choice {
        EditorChoice::Installed {
            version,
            executable,
            older_than,
        } => {
            if let Some(pinned) = older_than {
                eprintln!(
                    "this project was saved with jackdaw {pinned}; opening it with the older \
                     {version}"
                );
            }
            if pin.version.is_some() || pin.build.is_some() {
                editors::record_project(&layout, &root, &version)?;
            }
            executable
        }
        EditorChoice::Beside(executable) => executable,
    };
    std::process::Command::new(&executable)
        .env(jackdaw_env::ENV_OPEN_PROJECT, &root)
        .spawn()
        .map_err(|error| format!("could not launch {}: {error}", executable.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_releases::index::{Channels, StableChannel, StableLine};

    fn install(version: &str) -> Install {
        Install {
            version: version.into(),
            channel: if version::is_nightly(version) {
                Channel::BleedingEdge
            } else {
                Channel::Stable
            },
            bevy: version::line_of(version).unwrap_or_else(|| "main".into()),
            bevy_rev: None,
            path: PathBuf::from("/editors").join(version),
            managed: true,
            modules: Vec::new(),
            known_good: false,
        }
    }

    #[test]
    fn newer_patches_and_nightlies_are_reported_for_installed_lines_only() {
        let mut installs = Installs::default();
        installs.add(install("0.20.1"));
        installs.add(install("nightly-2026-10-01"));
        let mut index = ReleaseIndex {
            channels: Channels {
                stable: StableChannel {
                    lines: vec![
                        StableLine {
                            bevy: "0.21".into(),
                            latest: "0.21.0".into(),
                            status: LineStatus::Current,
                        },
                        StableLine {
                            bevy: "0.20".into(),
                            latest: "0.20.3".into(),
                            status: LineStatus::Previous,
                        },
                    ],
                },
                ..Channels::default()
            },
            ..ReleaseIndex::default()
        };
        index.channels.bleeding_edge.latest = Some("nightly-2026-10-11".into());
        assert_eq!(
            update_hints(&installs, &index),
            [
                "0.20.3 is available: jd editor install 0.20",
                "nightly-2026-10-11 is available: jd editor install bleeding-edge"
            ]
        );
    }

    #[test]
    fn installs_are_grouped_by_bevy_line_with_main_first() {
        let mut installs = Installs::default();
        for version in ["0.19.4", "0.20.1", "nightly-2026-10-01", "0.20.0"] {
            installs.add(install(version));
        }
        let lines: Vec<_> = by_bevy_line(&installs)
            .into_iter()
            .map(|(bevy, members)| (bevy, members.len()))
            .collect();
        assert_eq!(
            lines,
            [
                ("main".to_string(), 1),
                ("0.20".to_string(), 2),
                ("0.19".to_string(), 1)
            ]
        );
    }
}
