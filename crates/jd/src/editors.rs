//! Installing, registering and removing editors under the launcher
//! directory, and choosing the one a project opens with.
//!
//! An install is staged in a temporary directory beside its destination:
//! download, checksum, unpack, run `--version`, fetch modules, then one
//! rename. Any failure removes the staging directory, so an interrupted
//! install leaves nothing behind and simply runs again.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use jackdaw_releases::index::{IndexError, ModulesIndex, module_id};
use jackdaw_releases::installs::EDITOR_EXECUTABLE;
use jackdaw_releases::{
    Channel, EditorPin, INDEX_KEY_VAR, INDEX_URL_VAR, Install, Installs, Layout,
    OFFICIAL_INDEX_URL, PinResolution, Release, ReleaseIndex, signing, version,
};

use crate::archive;
use crate::fetch;

/// How long `--version` may take before the binary is judged broken.
const VERSION_TIMEOUT: Duration = Duration::from_secs(60);

/// Where a release index came from.
#[derive(Debug, PartialEq, Eq)]
pub enum IndexOrigin {
    /// Fetched and verified just now.
    Fetched,
    /// The cached copy, because fetching failed for this reason.
    Cached(String),
}

/// The largest release index or modules list `jd` reads.
const INDEX_LIMIT: u64 = 8 << 20;

/// The release index URL: [`INDEX_URL_VAR`] when set, else the official one.
pub fn index_url() -> String {
    env_override(INDEX_URL_VAR).unwrap_or_else(|| OFFICIAL_INDEX_URL.to_string())
}

fn env_override(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Warnings for the environment variables that replace the official index
/// or its key, so an override is never in effect silently.
pub fn index_overrides() -> Vec<String> {
    let mut warnings = Vec::new();
    if let Some(url) = env_override(INDEX_URL_VAR) {
        warnings.push(format!(
            "using the release index at {url} ({INDEX_URL_VAR} is set)"
        ));
    }
    if env_override(INDEX_KEY_VAR).is_some() {
        warnings.push(format!(
            "verifying the release index with the key in {INDEX_KEY_VAR}"
        ));
    }
    warnings
}

fn trusted_key() -> Result<ed25519_dalek::VerifyingKey, String> {
    signing::trusted_key(env_override(INDEX_KEY_VAR).as_deref()).map_err(|error| error.to_string())
}

fn signature_url(url: &str) -> String {
    format!("{url}.sig")
}

/// Fetch and verify the release index, caching it. The verified cache is
/// used instead when the fetch fails, when the signature does not match,
/// or when the served index is older than the cached one.
pub fn load_index(layout: &Layout) -> Result<(ReleaseIndex, IndexOrigin), String> {
    let key = trusted_key()?;
    let url = index_url();
    let cached = cached_index_with(layout, &key);
    let fetched = fetch::fetch_limited(&url, INDEX_LIMIT).and_then(|bytes| {
        let signature = fetch::fetch_limited(&signature_url(&url), 1024)?;
        let index = ReleaseIndex::from_signed(&bytes, &signature, &key)
            .map_err(|error| format!("{url}: {error}"))?;
        Ok((bytes, signature, index))
    });
    let reason = match fetched {
        Ok((bytes, signature, index)) => match &cached {
            Some(Ok(known)) if index.serial < known.serial => format!(
                "{url} serves index serial {}, older than the cached serial {}",
                index.serial, known.serial
            ),
            _ => {
                layout.write_cached_index(&bytes, &signature)?;
                return Ok((index, IndexOrigin::Fetched));
            }
        },
        Err(error) => error,
    };
    match cached {
        Some(Ok(index)) => Ok((index, IndexOrigin::Cached(reason))),
        Some(Err(error)) => Err(format!("{reason}; the cached index: {error}")),
        None => Err(reason),
    }
}

/// The cached index, verified, without touching the network.
pub fn cached_index(layout: &Layout) -> Option<ReleaseIndex> {
    let key = trusted_key().ok()?;
    cached_index_with(layout, &key)?.ok()
}

fn cached_index_with(
    layout: &Layout,
    key: &ed25519_dalek::VerifyingKey,
) -> Option<Result<ReleaseIndex, IndexError>> {
    let (bytes, signature) = layout.read_cached_index()?;
    Some(ReleaseIndex::from_signed(&bytes, &signature, key))
}

/// What [`install`] did.
#[derive(Debug)]
pub enum InstallOutcome {
    Installed(Install),
    /// Already installed; any newly requested modules were added.
    AlreadyInstalled(Install),
}

/// How far an [`install`] has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallProgress {
    /// Bytes of the editor bundle downloaded, out of `total`.
    Download { done: u64, total: u64 },
    /// The bundle is downloaded and being checked and unpacked.
    Unpacking,
    /// Fetching the requested modules.
    Modules,
    /// Moving the install into place and registering it.
    Registering,
}

/// Install the release `query` selects (a version, a line such as `0.20`,
/// `stable` or `bleeding-edge`) with `modules`, for `target`.
///
/// The download and checks run without the registry lock; only the final
/// rename and registration hold it, and a version another `jd` registered
/// in the meantime is kept rather than replaced.
pub fn install(
    layout: &Layout,
    index: &ReleaseIndex,
    query: &str,
    modules: &[String],
    target: &str,
    progress: &mut dyn FnMut(InstallProgress),
) -> Result<InstallOutcome, String> {
    let release = index.resolve(query)?;
    file_name_safe(&release.version)?;
    if let Some(existing) = layout.load_installs()?.get(&release.version).cloned() {
        progress(InstallProgress::Modules);
        return add_modules(layout, release, existing, modules, target);
    }

    let artifact = release.artifact_for(target).ok_or_else(|| {
        format!(
            "{} has no build for {target}; available: {}",
            release.version,
            release
                .artifacts
                .iter()
                .map(|artifact| artifact.target.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let format = archive::Format::of(&artifact.url)
        .ok_or_else(|| format!("{}: not a .tar.zst, .tar.gz or .zip bundle", artifact.url))?;

    let staging = Staging::new(&layout.editors(), &release.version)?;
    let download = staging.path().join("bundle");
    let total = artifact.size;
    let sha256 = fetch::download_reporting(&artifact.url, &download, total, &mut |done| {
        progress(InstallProgress::Download { done, total });
    })?;
    progress(InstallProgress::Unpacking);
    if !sha256.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(format!(
            "{}: checksum mismatch (expected {}, got {sha256}); nothing was installed",
            artifact.url, artifact.sha256
        ));
    }
    let unpacked = staging.path().join("unpacked");
    std::fs::create_dir(&unpacked).map_err(|error| format!("{}: {error}", unpacked.display()))?;
    archive::unpack(&download, format, &unpacked)?;
    let root = archive::bundle_root(&unpacked)?;

    let reported = query_version(&root.join(EDITOR_EXECUTABLE))?;
    if release.channel == Channel::Stable && reported.version != release.version {
        return Err(format!(
            "the {} bundle reports version {}; nothing was installed",
            release.version, reported.version
        ));
    }
    if !modules.is_empty() {
        progress(InstallProgress::Modules);
    }
    let fetched = fetch_modules(release, modules, target, staging.path())?;
    move_into(&fetched, &root.join("modules"))?;

    progress(InstallProgress::Registering);
    let _lock = layout.lock()?;
    let mut installs = layout.load_installs()?;
    if let Some(existing) = installs.get(&release.version) {
        return Ok(InstallOutcome::AlreadyInstalled(existing.clone()));
    }
    sweep_stale(&layout.editors());
    let dest = layout.editor(&release.version);
    if dest.exists() {
        remove_tree(&dest)?;
    }
    std::fs::rename(&root, &dest).map_err(|error| format!("{}: {error}", dest.display()))?;
    let mut install = Install::from_release(release, dest);
    install.modules = modules.to_vec();
    install.modules.sort();
    installs.add(install.clone());
    layout.save_installs(&installs)?;
    Ok(InstallOutcome::Installed(install))
}

/// Add the modules in `modules` that `existing` lacks.
fn add_modules(
    layout: &Layout,
    release: &Release,
    existing: Install,
    modules: &[String],
    target: &str,
) -> Result<InstallOutcome, String> {
    let missing: Vec<String> = modules
        .iter()
        .filter(|module| !existing.modules.contains(module))
        .cloned()
        .collect();
    if missing.is_empty() {
        return Ok(InstallOutcome::AlreadyInstalled(existing));
    }
    let staging = Staging::new(&layout.editors(), &release.version)?;
    let fetched = fetch_modules(release, &missing, target, staging.path())?;
    let _lock = layout.lock()?;
    let mut installs = layout.load_installs()?;
    let install = installs
        .get_mut(&release.version)
        .ok_or_else(|| format!("{} was removed during the install", release.version))?;
    move_into(&fetched, &install.path.join("modules"))?;
    for module in missing {
        if !install.modules.contains(&module) {
            install.modules.push(module);
        }
    }
    install.modules.sort();
    let updated = install.clone();
    layout.save_installs(&installs)?;
    Ok(InstallOutcome::AlreadyInstalled(updated))
}

fn move_into(files: &[PathBuf], dir: &Path) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for file in files {
        let dest = dir.join(file.file_name().unwrap_or_default());
        std::fs::rename(file, &dest).map_err(|error| format!("{}: {error}", dest.display()))?;
    }
    Ok(())
}

/// Remove staging and half-removed directories a killed `jd` left behind
/// more than a day ago.
fn sweep_stale(editors: &Path) {
    let Ok(entries) = std::fs::read_dir(editors) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let leftover = name.starts_with(".staging-") || name.contains(".removing-");
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(86_400));
        if leftover && old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Download the `.jdext` of each module in `names` from the release's
/// signed modules list into `dir`. A release that publishes no modules
/// list ships its modules inside the editor, so there is nothing to fetch.
fn fetch_modules(
    release: &Release,
    names: &[String],
    target: &str,
    dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    let Some(url) = release.modules_index.as_deref() else {
        return Ok(Vec::new());
    };
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let bytes = fetch::fetch_limited(url, INDEX_LIMIT)?;
    let signature = fetch::fetch_limited(&signature_url(url), 1024)?;
    let modules = ModulesIndex::from_signed(&bytes, &signature, &trusted_key()?)
        .map_err(|error| format!("{url}: {error}"))?;
    if modules.editor != release.version {
        return Err(format!(
            "{url} lists modules for {}, not {}",
            modules.editor, release.version
        ));
    }
    let mut files = Vec::new();
    for name in names {
        let module = modules.module(name).ok_or_else(|| {
            format!(
                "{} is not built for {}; this build has: {}",
                module_id(name),
                release.version,
                modules
                    .modules
                    .iter()
                    .map(|module| module.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        let artifact = module
            .artifacts
            .iter()
            .find(|artifact| artifact.target == target)
            .ok_or_else(|| format!("{} has no build for {target}", module.id))?;
        file_name_safe(&module.id)?;
        let dest = dir.join(format!("{}.jdext", module.id));
        let sha256 = fetch::download(&artifact.url, &dest, artifact.size)?;
        if !sha256.eq_ignore_ascii_case(&artifact.sha256) {
            return Err(format!(
                "{}: checksum mismatch; nothing was installed",
                artifact.url
            ));
        }
        files.push(dest);
    }
    Ok(files)
}

/// Refuse a version or module id that could name a path outside its
/// directory.
fn file_name_safe(name: &str) -> Result<(), String> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_');
    if name.is_empty() || name.starts_with('.') || !name.chars().all(allowed) {
        return Err(format!("`{name}` is not a valid release or module name"));
    }
    Ok(())
}

/// A temporary directory under the editors directory, removed on drop.
struct Staging(PathBuf);

impl Staging {
    fn new(editors: &Path, version: &str) -> Result<Self, String> {
        let path = editors.join(format!(".staging-{version}-{}", std::process::id()));
        if path.exists() {
            remove_tree(&path)?;
        }
        std::fs::create_dir_all(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Delete a directory tree by first renaming it aside, so a removal that
/// fails half way never leaves a half-deleted install under its real name.
fn remove_tree(path: &Path) -> Result<(), String> {
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".removing-{}", std::process::id()));
    let aside = PathBuf::from(aside);
    std::fs::rename(path, &aside).map_err(|error| format!("{}: {error}", path.display()))?;
    std::fs::remove_dir_all(&aside).map_err(|error| format!("{}: {error}", aside.display()))
}

/// What an editor binary says about itself.
#[derive(Debug, PartialEq, Eq)]
pub struct Reported {
    pub version: String,
    /// The Bevy minor it targets.
    pub bevy: String,
}

/// Run `<editor> --version` and read `jackdaw <version> (targets bevy
/// <minor>)` from it.
pub fn query_version(executable: &Path) -> Result<Reported, String> {
    let output = version_output(executable)?;
    parse_version_line(&output).ok_or_else(|| {
        format!(
            "{} --version printed `{}`, not a jackdaw version",
            executable.display(),
            output.trim()
        )
    })
}

/// What `<executable> --version` prints. A binary that does not exit in
/// time is killed.
pub fn version_output(executable: &Path) -> Result<String, String> {
    let mut child = std::process::Command::new(executable)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot run {}: {error}", executable.display()))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < VERSION_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} --version did not finish", executable.display()));
            }
            Err(error) => return Err(format!("{}: {error}", executable.display())),
        }
    };
    let mut output = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut stdout, &mut output);
    }
    if !status.success() {
        return Err(format!(
            "{} --version failed with {status}",
            executable.display()
        ));
    }
    Ok(output)
}

fn parse_version_line(output: &str) -> Option<Reported> {
    let line = output.lines().next()?.trim();
    let rest = line.strip_prefix("jackdaw ")?;
    let (version, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    let bevy = rest
        .trim()
        .strip_prefix("(targets bevy ")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_default();
    Some(Reported {
        version: version.to_string(),
        bevy: bevy.to_string(),
    })
}

/// Register an editor that was installed some other way, such as with
/// `cargo install`. `path` is the binary or the directory holding it.
pub fn locate(layout: &Layout, path: &Path) -> Result<Install, String> {
    let path = dunce::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let (dir, executable) = if path.is_dir() {
        (path.clone(), path.join(EDITOR_EXECUTABLE))
    } else {
        let dir = path
            .parent()
            .ok_or_else(|| format!("{} has no parent directory", path.display()))?
            .to_path_buf();
        (dir, path.clone())
    };
    if executable.file_name() != Some(std::ffi::OsStr::new(EDITOR_EXECUTABLE)) {
        return Err(format!(
            "{} is not a jackdaw editor binary (expected {EDITOR_EXECUTABLE})",
            executable.display()
        ));
    }
    let reported = query_version(&executable)?;
    file_name_safe(&reported.version)?;
    let _lock = layout.lock()?;
    let mut installs = layout.load_installs()?;
    if let Some(existing) = installs.get(&reported.version)
        && existing.path != dir
    {
        return Err(format!(
            "{} is already registered at {}; remove it first",
            reported.version,
            existing.path.display()
        ));
    }
    let install = Install {
        channel: if version::is_nightly(&reported.version) {
            Channel::BleedingEdge
        } else {
            Channel::Stable
        },
        version: reported.version,
        bevy: reported.bevy,
        bevy_rev: None,
        path: dir,
        managed: false,
        modules: Vec::new(),
        known_good: false,
    };
    installs.add(install.clone());
    layout.save_installs(&installs)?;
    Ok(install)
}

/// Why [`remove`] refused.
#[derive(Debug)]
pub enum RemoveError {
    NotInstalled,
    /// Projects that still open with this version.
    Pinned(Vec<PathBuf>),
    Failed(String),
}

/// Remove an install: delete a managed one, unregister a located one.
/// Refuses while a project still opens with it, unless `force`.
pub fn remove(layout: &Layout, version: &str, force: bool) -> Result<Install, RemoveError> {
    let _lock = layout.lock().map_err(RemoveError::Failed)?;
    let mut installs = layout.load_installs().map_err(RemoveError::Failed)?;
    if installs.get(version).is_none() {
        return Err(RemoveError::NotInstalled);
    }
    let pinned: Vec<PathBuf> = installs
        .pinning_projects(version)
        .into_iter()
        .map(Path::to_path_buf)
        .collect();
    if !pinned.is_empty() && !force {
        return Err(RemoveError::Pinned(pinned));
    }
    let removed = installs.remove(version).ok_or(RemoveError::NotInstalled)?;
    delete_managed(layout, &removed).map_err(RemoveError::Failed)?;
    layout
        .save_installs(&installs)
        .map_err(RemoveError::Failed)?;
    Ok(removed)
}

/// Delete a managed install's directory; leave anything else alone.
fn delete_managed(layout: &Layout, install: &Install) -> Result<(), String> {
    if !install.managed || !install.path.starts_with(layout.editors()) {
        return Ok(());
    }
    if install.path.exists() {
        remove_tree(&install.path)?;
    }
    Ok(())
}

/// Remove the nightlies no longer needed: all but the newest, the default,
/// known-good builds and those a project opens with.
pub fn clean(layout: &Layout) -> Result<Vec<Install>, String> {
    let _lock = layout.lock()?;
    let mut installs = layout.load_installs()?;
    let removable: Vec<String> = installs
        .removable_nightlies()
        .iter()
        .map(|install| install.version.clone())
        .collect();
    let mut removed = Vec::new();
    for version in removable {
        if let Some(install) = installs.remove(&version) {
            delete_managed(layout, &install)?;
            layout.save_installs(&installs)?;
            removed.push(install);
        }
    }
    Ok(removed)
}

/// Make `version` the default editor.
pub fn set_default(layout: &Layout, version: &str) -> Result<(), String> {
    let _lock = layout.lock()?;
    let mut installs = layout.load_installs()?;
    installs.set_default(version)?;
    layout.save_installs(&installs)
}

/// Remember that the project at `root` opens with `version`, so `remove`
/// and `clean` keep that editor.
pub fn record_project(layout: &Layout, root: &Path, version: &str) -> Result<(), String> {
    let _lock = layout.lock()?;
    let mut installs = layout.load_installs()?;
    installs.record_project(root, version);
    layout.save_installs(&installs)
}

/// The editor a project opens with.
#[derive(Debug, PartialEq, Eq)]
pub enum EditorChoice {
    /// A registered install.
    Installed {
        version: String,
        executable: PathBuf,
        /// Only an older patch of the pinned line is installed.
        older_than: Option<String>,
    },
    /// The editor installed beside `jd`.
    Beside(PathBuf),
}

/// The pin in a project's `jackdaw.toml`.
pub fn project_pin(root: &Path) -> Result<EditorPin, String> {
    let manifest = jackdaw_project_build::project_manifest::ProjectManifest::read_checked(root)
        .map_err(|error| format!("{}: {error}", root.join("jackdaw.toml").display()))?;
    let pins = manifest.jackdaw;
    EditorPin::from_fields(
        pins.version.as_deref(),
        pins.channel.as_deref(),
        pins.build.as_deref(),
    )
}

/// Pick the editor for a project pinned with `pin`.
///
/// A matching install wins, except that the editor beside `jd` is
/// preferred when it is exactly the pinned version and the install is
/// older. Without an install, the editor beside `jd` is used when it
/// belongs to the pinned line, as it did before installs were managed;
/// otherwise the error names the `jd editor install` to run.
pub fn choose_editor(
    installs: &Installs,
    pin: &EditorPin,
    beside: &Path,
    beside_version: &str,
) -> Result<EditorChoice, String> {
    let installed = |install: &Install, older_than: Option<String>| EditorChoice::Installed {
        version: install.version.clone(),
        executable: install.executable(),
        older_than,
    };
    match installs.resolve_pin(pin) {
        PinResolution::Installed { install, .. }
            if stable_pin(pin)
                && pin.version.as_deref() == Some(beside_version)
                && version::compare(&install.version, beside_version).is_lt()
                && beside.is_file() =>
        {
            Ok(EditorChoice::Beside(beside.to_path_buf()))
        }
        PinResolution::Installed {
            install,
            older_than_pin,
        } => Ok(installed(
            install,
            older_than_pin.then(|| pin.version.clone().unwrap_or_default()),
        )),
        PinResolution::Unpinned => Ok(installs.default_install().map_or_else(
            || EditorChoice::Beside(beside.to_path_buf()),
            |install| installed(install, None),
        )),
        PinResolution::Missing { install_query } => {
            let stable = stable_pin(pin);
            let same_line = pin.version.as_deref().and_then(version::line_of)
                == version::line_of(beside_version);
            if stable && same_line && beside.is_file() {
                return Ok(EditorChoice::Beside(beside.to_path_buf()));
            }
            Err(format!(
                "this project uses jackdaw {}, which is not installed; run `jd editor install \
                 {install_query}`, or `jd upgrade` to move it to jackdaw {beside_version}",
                pin.build
                    .as_deref()
                    .or(pin.version.as_deref())
                    .unwrap_or(&install_query)
            ))
        }
    }
}

fn stable_pin(pin: &EditorPin) -> bool {
    pin.channel.is_none_or(|channel| channel == Channel::Stable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stable(version: &str, path: &Path) -> Install {
        Install {
            version: version.into(),
            channel: Channel::Stable,
            bevy: version::line_of(version).unwrap(),
            bevy_rev: None,
            path: path.join(version),
            managed: true,
            modules: Vec::new(),
            known_good: false,
        }
    }

    fn pin(version: &str) -> EditorPin {
        EditorPin::from_fields(Some(version), None, None).unwrap()
    }

    #[test]
    fn names_that_could_leave_their_directory_are_refused() {
        assert!(file_name_safe("0.20.1").is_ok());
        assert!(file_name_safe("nightly-2026-10-11").is_ok());
        assert!(file_name_safe("jackdaw.physics").is_ok());
        assert!(file_name_safe("../0.20.1").is_err());
        assert!(file_name_safe("0.20/1").is_err());
        assert!(file_name_safe("..").is_err());
        assert!(file_name_safe("").is_err());
    }

    #[test]
    fn version_lines_are_read_from_editor_output() {
        assert_eq!(
            parse_version_line("jackdaw 0.20.1 (targets bevy 0.20)\n"),
            Some(Reported {
                version: "0.20.1".into(),
                bevy: "0.20".into()
            })
        );
        assert_eq!(
            parse_version_line("jackdaw 0.19.0-rc.10\n").map(|reported| reported.version),
            Some("0.19.0-rc.10".into())
        );
        assert_eq!(parse_version_line("jd 0.20.1 (targets bevy 0.20)"), None);
    }

    #[test]
    fn a_pinned_project_prefers_its_install_over_the_editor_beside_jd() {
        let mut installs = Installs::default();
        installs.add(stable("0.20.2", Path::new("/editors")));
        let beside = Path::new("/bin/jackdaw");
        assert_eq!(
            choose_editor(&installs, &pin("0.20.1"), beside, "0.20.1"),
            Ok(EditorChoice::Installed {
                version: "0.20.2".into(),
                executable: PathBuf::from("/editors/0.20.2").join(EDITOR_EXECUTABLE),
                older_than: None,
            })
        );
    }

    #[test]
    fn a_missing_line_names_the_install_command_unless_jd_ships_that_line() {
        let dir = tempfile::tempdir().unwrap();
        let beside = dir.path().join(EDITOR_EXECUTABLE);
        std::fs::write(&beside, b"").unwrap();
        let installs = Installs::default();

        assert_eq!(
            choose_editor(&installs, &pin("0.20.1"), &beside, "0.20.0"),
            Ok(EditorChoice::Beside(beside.clone()))
        );
        let error = choose_editor(&installs, &pin("0.21.0"), &beside, "0.20.0").unwrap_err();
        assert!(error.contains("jd editor install 0.21"), "{error}");
    }

    #[test]
    fn the_exact_editor_beside_jd_wins_over_an_older_install_of_its_line() {
        let dir = tempfile::tempdir().unwrap();
        let beside = dir.path().join(EDITOR_EXECUTABLE);
        std::fs::write(&beside, b"").unwrap();
        let mut installs = Installs::default();
        installs.add(stable("0.20.0", Path::new("/editors")));
        assert_eq!(
            choose_editor(&installs, &pin("0.20.1"), &beside, "0.20.1"),
            Ok(EditorChoice::Beside(beside.clone()))
        );
        installs.add(stable("0.20.2", Path::new("/editors")));
        assert!(matches!(
            choose_editor(&installs, &pin("0.20.1"), &beside, "0.20.1"),
            Ok(EditorChoice::Installed { version, .. }) if version == "0.20.2"
        ));
    }

    #[test]
    fn an_unpinned_project_opens_with_the_default_editor() {
        let mut installs = Installs::default();
        installs.add(stable("0.20.2", Path::new("/editors")));
        let choice = choose_editor(
            &installs,
            &EditorPin::default(),
            Path::new("/bin/jackdaw"),
            "0.19.0",
        );
        assert!(
            matches!(choice, Ok(EditorChoice::Installed { version, .. }) if version == "0.20.2")
        );
    }
}
