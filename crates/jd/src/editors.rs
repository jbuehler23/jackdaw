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

/// The release index URL: [`INDEX_URL_VAR`] when set, else the official one.
pub fn index_url() -> String {
    std::env::var(INDEX_URL_VAR)
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| OFFICIAL_INDEX_URL.to_string())
}

fn trusted_key() -> Result<ed25519_dalek::VerifyingKey, String> {
    let override_key = std::env::var(INDEX_KEY_VAR)
        .ok()
        .filter(|key| !key.is_empty());
    signing::trusted_key(override_key.as_deref()).map_err(|error| error.to_string())
}

fn signature_url(url: &str) -> String {
    format!("{url}.sig")
}

fn signature_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".sig");
    PathBuf::from(name)
}

/// Fetch and verify the release index, caching it; when the fetch fails,
/// fall back to the cached copy, verified again.
pub fn load_index(layout: &Layout) -> Result<(ReleaseIndex, IndexOrigin), String> {
    let key = trusted_key()?;
    let url = index_url();
    let fetched =
        fetch::fetch(&url).and_then(|bytes| Ok((bytes, fetch::fetch(&signature_url(&url))?)));
    match fetched {
        Ok((bytes, signature)) => {
            let index = ReleaseIndex::from_signed(&bytes, &signature, &key)
                .map_err(|error| format!("{url}: {error}"))?;
            let cache = layout.cached_index();
            jackdaw_releases::write_atomically(&signature_path(&cache), &signature)?;
            jackdaw_releases::write_atomically(&cache, &bytes)?;
            Ok((index, IndexOrigin::Fetched))
        }
        Err(fetch_error) => match cached_index_with(layout, &key) {
            Some(Ok(index)) => Ok((index, IndexOrigin::Cached(fetch_error))),
            Some(Err(error)) => Err(format!("{fetch_error}; the cached index: {error}")),
            None => Err(fetch_error),
        },
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
    let cache = layout.cached_index();
    let bytes = std::fs::read(&cache).ok()?;
    let signature = std::fs::read(signature_path(&cache)).ok()?;
    Some(ReleaseIndex::from_signed(&bytes, &signature, key))
}

/// What [`install`] did.
#[derive(Debug)]
pub enum InstallOutcome {
    Installed(Install),
    /// Already installed; any newly requested modules were added.
    AlreadyInstalled(Install),
}

/// Install the release `query` selects (a version, a line such as `0.20`,
/// `stable` or `bleeding-edge`) with `modules`, for `target`.
pub fn install(
    layout: &Layout,
    index: &ReleaseIndex,
    query: &str,
    modules: &[String],
    target: &str,
) -> Result<InstallOutcome, String> {
    let release = index.resolve(query)?;
    file_name_safe(&release.version)?;
    let mut installs = layout.load_installs()?;
    if let Some(existing) = installs.get(&release.version).cloned() {
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
        let modules_dir = existing.path.join("modules");
        for file in fetched {
            std::fs::create_dir_all(&modules_dir)
                .map_err(|error| format!("{}: {error}", modules_dir.display()))?;
            let dest = modules_dir.join(file.file_name().unwrap_or_default());
            std::fs::rename(&file, &dest)
                .map_err(|error| format!("{}: {error}", dest.display()))?;
        }
        let install = installs
            .get_mut(&release.version)
            .ok_or("the registry changed during the install")?;
        install.modules.extend(missing);
        install.modules.sort();
        let updated = install.clone();
        layout.save_installs(&installs)?;
        return Ok(InstallOutcome::AlreadyInstalled(updated));
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
    let (size, sha256) = fetch::download(&artifact.url, &download)?;
    if !sha256.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(format!(
            "{}: checksum mismatch (expected {}, got {sha256}); nothing was installed",
            artifact.url, artifact.sha256
        ));
    }
    if size != artifact.size {
        return Err(format!(
            "{}: expected {} bytes, got {size}; nothing was installed",
            artifact.url, artifact.size
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
    let fetched = fetch_modules(release, modules, target, staging.path())?;
    if !fetched.is_empty() {
        let modules_dir = root.join("modules");
        std::fs::create_dir_all(&modules_dir)
            .map_err(|error| format!("{}: {error}", modules_dir.display()))?;
        for file in fetched {
            let dest = modules_dir.join(file.file_name().unwrap_or_default());
            std::fs::rename(&file, &dest)
                .map_err(|error| format!("{}: {error}", dest.display()))?;
        }
    }

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
    let bytes = fetch::fetch(url)?;
    let signature = fetch::fetch(&signature_url(url))?;
    let modules = ModulesIndex::from_signed(&bytes, &signature, &trusted_key()?)
        .map_err(|error| format!("{url}: {error}"))?;
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
        let (_, sha256) = fetch::download(&artifact.url, &dest)?;
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
/// <minor>)` from it. A binary that does not exit in time is killed.
pub fn query_version(executable: &Path) -> Result<Reported, String> {
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
                return Err(format!(
                    "{} --version did not finish; is it a jackdaw editor?",
                    executable.display()
                ));
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
    parse_version_line(&output).ok_or_else(|| {
        format!(
            "{} --version printed `{}`, not a jackdaw version",
            executable.display(),
            output.trim()
        )
    })
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
/// A matching install wins. Without one, the editor beside `jd` is used
/// when it belongs to the pinned line, as it did before installs were
/// managed; otherwise the error names the `jd editor install` to run.
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
            let stable = pin.channel.is_none_or(|channel| channel == Channel::Stable);
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
