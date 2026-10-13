//! `jd editor` and `jd open` against a signed index served from `file://`
//! URLs, with every directory jd reads or writes under a temporary root.
//!
//! The bundles hold shell scripts standing in for the editor, so these run
//! on Unix only.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use jackdaw_releases::index::{ModuleRelease, ModulesIndex};
use jackdaw_releases::{Artifact, Channel, Installs, Release, ReleaseIndex, sha256_hex, signing};

const TARGET: &str = jackdaw_package::host_target();

struct Fixture {
    root: tempfile::TempDir,
    key: SigningKey,
    index: ReleaseIndex,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        for dir in ["home", "config", "data", "cache", "mirror", "projects"] {
            std::fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        Self {
            root,
            key: SigningKey::from_bytes(&[42; 32]),
            index: ReleaseIndex::default(),
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    fn mirror(&self) -> PathBuf {
        self.path("mirror")
    }

    fn launcher(&self) -> PathBuf {
        self.path("config/launcher")
    }

    fn url(&self, name: &str) -> String {
        format!("file://{}", self.mirror().join(name).display())
    }

    /// Publish a release whose bundle holds an editor reporting `reports`.
    fn publish(&mut self, version: &str, reports: &str) -> &mut Release {
        let name = format!("jackdaw-{version}-{TARGET}.tar.zst");
        let bundle = tar_zst(&[
            ("jackdaw".to_string(), fake_editor(reports).into_bytes()),
            (
                "jd".to_string(),
                format!("#!/bin/sh\necho 'jd {version}'\n").into_bytes(),
            ),
        ]);
        std::fs::write(self.mirror().join(&name), &bundle).unwrap();
        let nightly = version.starts_with("nightly-");
        let release = Release {
            version: version.to_string(),
            channel: if nightly {
                Channel::BleedingEdge
            } else {
                Channel::Stable
            },
            bevy: if nightly {
                "main".into()
            } else {
                jackdaw_releases::version::line_of(version).unwrap()
            },
            bevy_rev: None,
            jackdaw_rev: None,
            date: "2026-10-11".into(),
            rustc: None,
            sdk_id: None,
            notes_url: None,
            yanked: false,
            broken: false,
            known_good: false,
            known_issues: Vec::new(),
            artifacts: vec![Artifact {
                target: TARGET.to_string(),
                url: self.url(&name),
                sha256: sha256_hex(&bundle),
                size: bundle.len() as u64,
                installed_size: None,
                attestation: None,
            }],
            modules_index: None,
        };
        let today = jackdaw_releases::version::parse_date("2026-10-11").unwrap();
        self.index.add_release(release, today);
        self.index
            .releases
            .iter_mut()
            .find(|release| release.version == version)
            .unwrap()
    }

    /// Sign the index and write it with its signature to the mirror.
    fn sign(&mut self) {
        self.index.refresh_channels();
        let bytes = serde_json::to_vec_pretty(&self.index).unwrap();
        std::fs::write(self.mirror().join("releases.json"), &bytes).unwrap();
        std::fs::write(
            self.mirror().join("releases.json.sig"),
            signing::sign_detached(&self.key, &bytes),
        )
        .unwrap();
    }

    /// Run `jd` at `jd`. A binary copied a moment ago can still be open
    /// for writing in a child another test thread forked, which makes
    /// executing it fail with "text file busy" until that child execs.
    fn jd_at(&self, jd: &Path, args: &[&str]) -> Output {
        const TEXT_FILE_BUSY: i32 = 26;
        let started = Instant::now();
        loop {
            match self.command(jd, args).output() {
                Err(error)
                    if error.raw_os_error() == Some(TEXT_FILE_BUSY)
                        && started.elapsed() < Duration::from_secs(10) =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                result => return result.unwrap(),
            }
        }
    }

    fn command(&self, jd: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(jd);
        command
            .args(args)
            .env("HOME", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("config-xdg"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("JACKDAW_CONFIG_DIR", self.path("config"))
            .env("CARGO_HOME", self.path("cargo"))
            .env(jackdaw_releases::INDEX_URL_VAR, self.url("releases.json"))
            .env(
                jackdaw_releases::INDEX_KEY_VAR,
                signing::key_hex(&self.key.verifying_key()),
            );
        command
    }

    fn jd(&self, args: &[&str]) -> Output {
        self.jd_at(Path::new(env!("CARGO_BIN_EXE_jd")), args)
    }

    fn installs(&self) -> Installs {
        Installs::load(&self.launcher().join("installs.json")).unwrap()
    }

    fn installed_dirs(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.launcher().join("editors"))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn project(&self, name: &str, jackdaw_table: &str) -> PathBuf {
        let dir = self.path("projects").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("jackdaw.toml"), jackdaw_table).unwrap();
        dir
    }
}

/// An editor stand-in: answers `--version`, and otherwise records the
/// project it was asked to open beside itself.
fn fake_editor(reports: &str) -> String {
    format!(
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then echo 'jackdaw {reports} (targets bevy 0.20)'; exit 0; fi\n\
         echo \"$JACKDAW_OPEN_PROJECT\" > \"$(dirname \"$0\")/opened\"\n"
    )
}

fn tar_zst(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, contents) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                format!("jackdaw-{TARGET}/{name}"),
                &contents[..],
            )
            .unwrap();
    }
    let tar = builder.into_inner().unwrap();
    ruzstd::encoding::compress_to_vec(&tar[..], ruzstd::encoding::CompressionLevel::Fastest)
}

fn ok(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "jd failed\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

fn failed(output: &Output) -> String {
    assert!(
        !output.status.success(),
        "jd succeeded unexpectedly: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn wait_for(path: &Path) -> String {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        if let Ok(text) = std::fs::read_to_string(path)
            && text.ends_with('\n')
        {
            return text;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{} was never written", path.display());
}

#[test]
fn installing_a_line_registers_its_newest_patch_as_the_default() {
    let mut fixture = Fixture::new();
    fixture.publish("0.20.1", "0.20.1");
    fixture.publish("0.20.2", "0.20.2");
    fixture.sign();

    let stdout = ok(&fixture.jd(&["editor", "install", "0.20"]));
    assert!(stdout.contains("installed jackdaw 0.20.2"), "{stdout}");
    assert_eq!(fixture.installed_dirs(), ["0.20.2"]);
    let installs = fixture.installs();
    assert_eq!(installs.default.as_deref(), Some("0.20.2"));
    assert!(installs.get("0.20.2").unwrap().executable().is_file());

    let stdout = ok(&fixture.jd(&["editor", "install", "0.20"]));
    assert!(stdout.contains("already installed"), "{stdout}");

    let stdout = ok(&fixture.jd(&["editor", "list"]));
    assert!(stdout.contains("Bevy 0.20"), "{stdout}");
    assert!(stdout.contains("default"), "{stdout}");
}

fn events(stdout: &str) -> Vec<serde_json::Value> {
    stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|_| panic!("not JSON: {line}")))
        .collect()
}

#[test]
fn json_output_reports_download_progress_stages_and_the_result() {
    let mut fixture = Fixture::new();
    fixture.publish("0.20.1", "0.20.1");
    fixture.sign();

    let events = events(&ok(&fixture.jd(&["editor", "install", "0.20", "--json"])));
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event["event"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"progress"), "{kinds:?}");
    let last_download = events
        .iter()
        .rev()
        .find(|event| event["stage"] == "download")
        .unwrap();
    assert_eq!(last_download["done"], last_download["total"]);
    let stages: Vec<&str> = events
        .iter()
        .filter(|event| event["event"] == "stage")
        .map(|event| event["stage"].as_str().unwrap())
        .collect();
    assert_eq!(stages, ["unpack", "register"]);
    let done = events.last().unwrap();
    assert_eq!(done["event"], "done");
    assert!(
        done["text"]
            .as_str()
            .unwrap()
            .contains("installed jackdaw 0.20.1"),
        "{done}"
    );

    let verified = events_of(&fixture, &["editor", "verify", "0.20.1", "--json"]);
    assert_eq!(verified.last().unwrap()["event"], "done");
}

fn events_of(fixture: &Fixture, args: &[&str]) -> Vec<serde_json::Value> {
    events(&ok(&fixture.jd(args)))
}

#[test]
fn json_refresh_caches_the_index_and_says_where_it_came_from() {
    let mut fixture = Fixture::new();
    fixture.publish("0.20.1", "0.20.1");
    fixture.sign();

    let fetched = events_of(&fixture, &["editor", "refresh", "--json"]);
    let index = fetched
        .iter()
        .find(|event| event["event"] == "index")
        .expect("an index event");
    assert_eq!(index["origin"], "fetched");
    assert!(fixture.launcher().join("cache/releases.signed").is_file());

    std::fs::remove_file(fixture.mirror().join("releases.json")).unwrap();
    let cached = events_of(&fixture, &["editor", "refresh", "--json"]);
    let index = cached
        .iter()
        .find(|event| event["event"] == "index")
        .expect("an index event");
    assert_eq!(index["origin"], "cached");
    assert!(index["reason"].is_string(), "{index}");
}

#[test]
fn a_failed_json_command_ends_with_an_error_event() {
    let mut fixture = Fixture::new();
    fixture.publish("0.20.1", "0.20.1");
    fixture.sign();

    let output = fixture.jd(&["editor", "remove", "0.30.0", "--json"]);
    assert!(!output.status.success());
    let events = events(&String::from_utf8_lossy(&output.stdout));
    let error = events.last().expect("an event");
    assert_eq!(error["event"], "error");
    assert!(
        error["text"].as_str().unwrap().contains("not installed"),
        "{error}"
    );
}

#[test]
fn a_bundle_with_a_wrong_checksum_leaves_nothing_installed() {
    let mut fixture = Fixture::new();
    fixture.publish("0.20.1", "0.20.1").artifacts[0].sha256 = "00".repeat(32);
    fixture.sign();

    let stderr = failed(&fixture.jd(&["editor", "install", "0.20.1"]));
    assert!(stderr.contains("checksum mismatch"), "{stderr}");
    assert!(fixture.installed_dirs().is_empty());
    assert!(fixture.installs().installs.is_empty());
}

#[test]
fn a_bundle_reporting_another_version_is_not_installed() {
    let mut fixture = Fixture::new();
    fixture.publish("0.20.1", "0.19.0");
    fixture.sign();

    let stderr = failed(&fixture.jd(&["editor", "install", "stable"]));
    assert!(stderr.contains("reports version 0.19.0"), "{stderr}");
    assert!(fixture.installed_dirs().is_empty());
}

#[test]
fn a_tampered_index_is_refused() {
    let mut fixture = Fixture::new();
    fixture.publish("0.20.1", "0.20.1");
    fixture.sign();
    let index = fixture.mirror().join("releases.json");
    let text = std::fs::read_to_string(&index).unwrap();
    std::fs::write(&index, text.replace("0.20.1", "0.20.9")).unwrap();

    let stderr = failed(&fixture.jd(&["editor", "list", "--available"]));
    assert!(stderr.contains("signature"), "{stderr}");
}

#[test]
fn the_cached_index_serves_listings_when_the_mirror_is_unreachable() {
    let mut fixture = Fixture::new();
    fixture.publish("0.20.1", "0.20.1");
    fixture.publish("nightly-2026-10-11", "0.21.0-dev");
    fixture.sign();
    let stdout = ok(&fixture.jd(&["editor", "list", "--available"]));
    assert!(stdout.contains("0.20.1"), "{stdout}");
    assert!(stdout.contains("nightly-2026-10-11"), "{stdout}");

    let index = fixture.mirror().join("releases.json");
    let signed = std::fs::read_to_string(&index).unwrap();
    std::fs::write(&index, signed.replace("0.20.1", "0.20.9")).unwrap();
    let output = fixture.jd(&["editor", "list", "--available"]);
    assert!(ok(&output).contains("0.20.1"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("signature"));

    std::fs::remove_file(&index).unwrap();
    let output = fixture.jd(&["editor", "list", "--available"]);
    assert!(ok(&output).contains("0.20.1"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cached release index"));
}

#[test]
fn an_index_older_than_the_cached_one_is_not_used() {
    let mut fixture = Fixture::new();
    fixture.publish("0.20.1", "0.20.1");
    fixture.index.serial = 5;
    fixture.sign();
    ok(&fixture.jd(&["editor", "list", "--available"]));

    fixture.publish("0.20.2", "0.20.2");
    fixture.index.serial = 4;
    fixture.sign();
    let output = fixture.jd(&["editor", "list", "--available"]);
    let stdout = ok(&output);
    assert!(!stdout.contains("0.20.2"), "{stdout}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("older than the cached"));
}

#[test]
fn an_index_override_is_announced() {
    let mut fixture = Fixture::new();
    fixture.publish("0.20.1", "0.20.1");
    fixture.sign();
    let output = fixture.jd(&["editor", "list", "--available"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.contains("JACKDAW_RELEASE_INDEX is set"), "{stderr}");
    assert!(stderr.contains("JACKDAW_RELEASE_INDEX_KEY"), "{stderr}");
}

#[test]
fn open_launches_the_pinned_line_and_remove_then_needs_force() {
    let mut fixture = Fixture::new();
    fixture.publish("0.20.2", "0.20.2");
    fixture.sign();
    ok(&fixture.jd(&["editor", "install", "stable"]));
    let project = fixture.project("game", "[jackdaw]\nversion = \"0.20.1\"\nbevy = \"0.20\"\n");

    ok(&fixture.jd(&["open", project.to_str().unwrap()]));
    let opened = wait_for(&fixture.launcher().join("editors/0.20.2/opened"));
    assert_eq!(
        opened.trim(),
        dunce::canonicalize(&project).unwrap().to_str().unwrap()
    );

    let stderr = failed(&fixture.jd(&["editor", "remove", "0.20.2"]));
    assert!(stderr.contains("game"), "{stderr}");
    assert!(stderr.contains("--force"), "{stderr}");
    assert_eq!(fixture.installed_dirs(), ["0.20.2"]);

    ok(&fixture.jd(&["editor", "remove", "0.20.2", "--force"]));
    assert!(fixture.installed_dirs().is_empty());
    assert!(fixture.installs().projects.is_empty());
}

#[test]
fn open_names_the_install_command_for_a_line_that_is_not_installed() {
    let fixture = Fixture::new();
    let project = fixture.project("future", "[jackdaw]\nversion = \"7.3.0\"\n");
    let stderr = failed(&fixture.jd(&["open", project.to_str().unwrap()]));
    assert!(stderr.contains("jd editor install 7.3"), "{stderr}");

    let nightly = fixture.project(
        "edge",
        "[jackdaw]\nversion = \"7.4.0-dev\"\nchannel = \"bleeding-edge\"\nbuild = \"nightly-2026-10-09\"\n",
    );
    let stderr = failed(&fixture.jd(&["open", nightly.to_str().unwrap()]));
    assert!(
        stderr.contains("jd editor install nightly-2026-10-09"),
        "{stderr}"
    );
}

#[test]
fn clean_keeps_the_newest_known_good_and_pinned_nightlies() {
    let mut fixture = Fixture::new();
    for day in ["07", "08", "09", "10", "11"] {
        let version = format!("nightly-2026-10-{day}");
        let release = fixture.publish(&version, "0.21.0-dev");
        release.known_good = day == "08";
    }
    fixture.sign();
    for day in ["07", "08", "09", "10"] {
        ok(&fixture.jd(&["editor", "install", &format!("nightly-2026-10-{day}")]));
    }
    ok(&fixture.jd(&["editor", "install", "bleeding-edge"]));
    let project = fixture.project(
        "edge",
        "[jackdaw]\nchannel = \"bleeding-edge\"\nbuild = \"nightly-2026-10-09\"\n",
    );
    ok(&fixture.jd(&["open", project.to_str().unwrap()]));
    wait_for(&fixture.launcher().join("editors/nightly-2026-10-09/opened"));

    let stdout = ok(&fixture.jd(&["editor", "clean"]));
    assert!(stdout.contains("nightly-2026-10-07"), "{stdout}");
    assert_eq!(
        fixture.installed_dirs(),
        [
            "nightly-2026-10-08",
            "nightly-2026-10-09",
            "nightly-2026-10-11"
        ]
    );
}

#[test]
fn default_and_locate_manage_editors_installed_elsewhere() {
    let mut fixture = Fixture::new();
    fixture.publish("0.20.2", "0.20.2");
    fixture.sign();
    ok(&fixture.jd(&["editor", "install", "0.20.2"]));

    let elsewhere = fixture.path("cargo-bin");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let editor = elsewhere.join("jackdaw");
    std::fs::write(&editor, fake_editor("0.19.4")).unwrap();
    std::fs::set_permissions(&editor, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let stdout = ok(&fixture.jd(&["editor", "locate", editor.to_str().unwrap()]));
    assert!(stdout.contains("registered jackdaw 0.19.4"), "{stdout}");
    ok(&fixture.jd(&["editor", "default", "0.19.4"]));
    assert_eq!(fixture.installs().default.as_deref(), Some("0.19.4"));
    let stderr = failed(&fixture.jd(&["editor", "default", "0.18.0"]));
    assert!(stderr.contains("not installed"), "{stderr}");

    let unpinned = fixture.project("unpinned", "plugin = \"GamePlugin\"\n");
    ok(&fixture.jd(&["open", unpinned.to_str().unwrap()]));
    wait_for(&elsewhere.join("opened"));

    ok(&fixture.jd(&["editor", "remove", "0.19.4"]));
    assert!(editor.is_file(), "a located editor's files are left alone");
    assert_eq!(fixture.installs().default.as_deref(), Some("0.20.2"));
}

#[test]
fn modules_are_fetched_from_the_signed_modules_list() {
    let mut fixture = Fixture::new();
    let module_bytes = b"physics module".to_vec();
    std::fs::write(fixture.mirror().join("physics.jdext"), &module_bytes).unwrap();
    let modules = ModulesIndex {
        schema: 1,
        kind: jackdaw_releases::index::MODULES_KIND.into(),
        editor: "0.20.2".into(),
        sdk_id: None,
        modules: vec![ModuleRelease {
            id: "jackdaw.physics".into(),
            label: "Physics".into(),
            version: "0.20.2".into(),
            requires_restart: true,
            depends: Vec::new(),
            artifacts: vec![Artifact {
                target: TARGET.to_string(),
                url: fixture.url("physics.jdext"),
                sha256: sha256_hex(&module_bytes),
                size: module_bytes.len() as u64,
                installed_size: None,
                attestation: None,
            }],
        }],
    };
    let bytes = serde_json::to_vec(&modules).unwrap();
    std::fs::write(fixture.mirror().join("modules.json"), &bytes).unwrap();
    std::fs::write(
        fixture.mirror().join("modules.json.sig"),
        signing::sign_detached(&fixture.key, &bytes),
    )
    .unwrap();
    let modules_url = fixture.url("modules.json");
    fixture.publish("0.20.2", "0.20.2").modules_index = Some(modules_url);
    fixture.sign();

    ok(&fixture.jd(&["editor", "install", "0.20.2"]));
    ok(&fixture.jd(&["editor", "install", "0.20.2", "--modules", "physics"]));
    let installed = fixture
        .launcher()
        .join("editors/0.20.2/modules/jackdaw.physics.jdext");
    assert_eq!(std::fs::read(installed).unwrap(), module_bytes);
    assert_eq!(
        fixture.installs().get("0.20.2").unwrap().modules,
        ["physics"]
    );

    let stderr = failed(&fixture.jd(&["editor", "install", "0.20.2", "--modules", "teleport"]));
    assert!(stderr.contains("unknown module"), "{stderr}");
}

#[test]
fn self_update_swaps_in_the_new_jd_only_after_it_reports_its_version() {
    let mut fixture = Fixture::new();
    fixture.publish("99.0.0", "99.0.0");
    fixture.sign();
    let bin = fixture.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let jd = bin.join("jd");
    std::fs::write(&jd, b"old jd").unwrap();

    let outcome = jd::self_update::update(&jd, "0.20.0", &fixture.index, TARGET).unwrap();
    assert_eq!(
        outcome,
        jd::self_update::SelfUpdate::Updated {
            from: "0.20.0".into(),
            to: "99.0.0".into()
        }
    );
    assert_eq!(
        std::fs::read_to_string(&jd).unwrap(),
        "#!/bin/sh\necho 'jd 99.0.0'\n"
    );
    let leftovers: Vec<_> = std::fs::read_dir(&bin).unwrap().flatten().collect();
    assert_eq!(leftovers.len(), 1);

    fixture.publish("99.0.1", "99.0.1").artifacts[0].size += 1;
    let error = jd::self_update::update(&jd, "99.0.0", &fixture.index, TARGET).unwrap_err();
    assert!(error.contains("bytes"), "{error}");
    assert!(std::fs::read_to_string(&jd).unwrap().contains("99.0.0"));
}

#[test]
fn self_update_refuses_a_prerelease_or_bundled_jd() {
    let mut fixture = Fixture::new();
    fixture.publish("99.0.0", "99.0.0");
    fixture.sign();
    let bin = fixture.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let jd = bin.join("jd");
    std::fs::copy(env!("CARGO_BIN_EXE_jd"), &jd).unwrap();
    if jackdaw_project_build::VERSION.contains('-') {
        let stderr = failed(&fixture.jd_at(&jd, &["self", "update"]));
        assert!(stderr.contains("prerelease"), "{stderr}");
        assert!(stderr.contains("jd editor install stable"), "{stderr}");
    }

    std::fs::write(bin.join("jackdaw"), b"").unwrap();
    let stderr = failed(&fixture.jd_at(&jd, &["self", "update"]));
    assert!(stderr.contains("editor bundle"), "{stderr}");
}

#[test]
fn self_update_leaves_a_cargo_installed_jd_to_cargo() {
    let fixture = Fixture::new();
    let bin = fixture.path("cargo/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let jd = bin.join("jd");
    std::fs::copy(env!("CARGO_BIN_EXE_jd"), &jd).unwrap();

    let stdout = ok(&fixture.jd_at(&jd, &["self", "update"]));
    assert!(stdout.contains("cargo install jd --locked"), "{stdout}");
}
