//! The shipped `jd` binary must scaffold a working project on a machine
//! that has no jackdaw source checkout.
//!
//! That is the whole promise of the downloadable bundle: extract it and
//! create a project, offline, with no clone. Every other test in this
//! repo runs from inside the checkout, where the templates are on disk
//! next to the binary and the scaffolder rewrites the new project's
//! jackdaw dependencies to local path deps. Neither of those is true
//! for a real user, so this test copies the binary somewhere the
//! checkout cannot be found and exercises it there.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Where scratch copies live: a debug `jd` is ~500 MB, and the default
/// temp dir is often a tmpfs with far less room than that. Prefer a
/// disk-backed location when one is obvious.
fn temp_root() -> PathBuf {
    let disk_backed = PathBuf::from("/var/tmp");
    if cfg!(unix) && disk_backed.is_dir() {
        return disk_backed;
    }
    std::env::temp_dir()
}

/// One copy of `jd`, placed outside the workspace and shared by every
/// test here.
///
/// The copy is the point: dev-checkout detection keys on where the
/// running executable lives, so a binary invoked from `target/` behaves
/// like a developer's, not like a user's. `None` when the copy could
/// not be made (usually no space), which the tests report rather than
/// failing on an unrelated cause.
fn standalone_jd() -> Option<&'static PathBuf> {
    static JD: OnceLock<Option<PathBuf>> = OnceLock::new();
    JD.get_or_init(|| {
        // A fixed destination, so repeated runs overwrite one ~500 MB
        // file instead of accumulating one per run. Tests here run in
        // parallel and share it, so nothing deletes it mid-run; write
        // to a unique name and rename, which is atomic, rather than
        // truncating a binary another process may be executing.
        let dir = temp_root().join("jackdaw_standalone_jd");
        std::fs::create_dir_all(&dir).ok()?;
        let dest = dir.join(format!("jd{}", std::env::consts::EXE_SUFFIX));
        let staging = dir.join(format!("jd-{}", std::process::id()));
        std::fs::copy(env!("CARGO_BIN_EXE_jd"), &staging).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut perms = std::fs::metadata(&staging).ok()?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&staging, perms).ok()?;
        }
        std::fs::rename(&staging, &dest).ok()?;
        Some(dest)
    })
    .as_ref()
}

/// The shared binary, or a skip. A silent skip would let this break
/// unnoticed, so CI sets `JACKDAW_STANDALONE_REQUIRED` to make the
/// absence a failure.
macro_rules! jd_or_skip {
    () => {
        match standalone_jd() {
            Some(jd) => jd,
            None => {
                assert!(
                    std::env::var_os("JACKDAW_STANDALONE_REQUIRED").is_none(),
                    "standalone scaffold test was required but the jd binary could not be \
                     staged (out of space in {}?)",
                    temp_root().display()
                );
                eprintln!("SKIP: could not stage a standalone jd binary");
                return;
            }
        }
    };
}

/// A scratch directory outside the workspace, so nothing above it is a
/// jackdaw checkout or a cargo workspace.
fn scratch(name: &str) -> PathBuf {
    let dir = temp_root().join(format!("jackdaw_standalone_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

#[test]
fn the_shipped_binary_scaffolds_without_a_checkout() {
    let jd = jd_or_skip!();
    let dir = scratch("new");
    let projects = dir.join("projects");
    std::fs::create_dir_all(&projects).expect("create projects dir");

    let output = Command::new(jd)
        .args(["new", "standalone-game", "--no-git"])
        .arg("--path")
        .arg(&projects)
        // Make sure nothing in the environment points back at a checkout.
        .env_remove("JACKDAW_DEV_CHECKOUT")
        .current_dir(&projects)
        .output()
        .expect("run jd new");
    assert!(
        output.status.success(),
        "jd new failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let project = projects.join("standalone-game");
    for expected in [
        "Cargo.toml",
        "jackdaw.toml",
        "src/lib.rs",
        "src/main.rs",
        "assets/scene.bsn",
        ".gitignore",
    ] {
        assert!(
            project.join(expected).exists(),
            "the templates are embedded in the binary, so {expected} must exist without a \
             checkout"
        );
    }

    let manifest = std::fs::read_to_string(project.join("Cargo.toml")).expect("read manifest");
    // A path dependency here would point at a directory that does not
    // exist on the user's machine.
    assert!(
        !manifest.contains("path ="),
        "a shipped binary must scaffold registry dependencies, not local paths:\n{manifest}"
    );
    assert!(
        manifest.contains(&format!(
            "bevy = \"{}\"",
            jackdaw_project_build::BEVY_VERSION
        )),
        "got:\n{manifest}"
    );
    assert!(
        !manifest.contains("{{"),
        "unsubstituted placeholder:\n{manifest}"
    );

    // Scaffolded games expose a detectable root Bevy Plugin so import,
    // doctor, and the template tutorial agree on the project shape.
    assert_eq!(
        jackdaw_project_build::detect::detect_plugin(&project, "standalone_game").as_deref(),
        Some("standalone_game::GamePlugin"),
        "the scaffolded plugin must be detectable"
    );

    let settings = jackdaw_project_build::project_manifest::ProjectManifest::read(&project);
    assert_eq!(
        jackdaw_project_build::project_manifest::compare_pins(&settings.jackdaw),
        jackdaw_project_build::project_manifest::PinStatus::Match,
        "a scaffolded project records the versions it was created with"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_shipped_binary_scaffolds_an_extension_without_a_checkout() {
    let jd = jd_or_skip!();
    let dir = scratch("ext");
    let projects = dir.join("projects");
    std::fs::create_dir_all(&projects).expect("create projects dir");

    let output = Command::new(jd)
        .args(["new", "standalone-ext", "--extension", "--no-git"])
        .arg("--path")
        .arg(&projects)
        .env_remove("JACKDAW_DEV_CHECKOUT")
        .current_dir(&projects)
        .output()
        .expect("run jd new --extension");
    assert!(output.status.success(), "jd new --extension failed");

    let project = projects.join("standalone-ext");
    let manifest = std::fs::read_to_string(project.join("Cargo.toml")).expect("read manifest");
    assert!(!manifest.contains("path ="), "got:\n{manifest}");
    assert!(
        jackdaw_project_build::detect::detect_extension(&project).is_some(),
        "the scaffolded extension type must be detectable"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Import is the other half of the promise, and it also has to work
/// with no checkout: the plan is computed from the user's own sources.
#[test]
fn the_shipped_binary_imports_without_a_checkout() {
    let jd = jd_or_skip!();
    let dir = scratch("import");
    let project = dir.join("their-game");
    std::fs::create_dir_all(project.join("src")).expect("create src");
    std::fs::write(
        project.join("Cargo.toml"),
        format!(
            "[package]\nname = \"their-game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"{}\"\n",
            jackdaw_project_build::BEVY_VERSION
        ),
    )
    .expect("write manifest");
    std::fs::write(
        project.join("src/lib.rs"),
        "use bevy::prelude::*;\npub struct TheirPlugin;\n\
         impl Plugin for TheirPlugin { fn build(&self, _: &mut App) {} }\n",
    )
    .expect("write lib.rs");

    let output = Command::new(jd)
        .arg("import")
        .arg("--apply")
        .arg(&project)
        .env_remove("JACKDAW_DEV_CHECKOUT")
        .output()
        .expect("run jd import");
    assert!(
        output.status.success(),
        "jd import failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let settings = jackdaw_project_build::project_manifest::ProjectManifest::read(&project);
    assert_eq!(
        settings.plugin.as_deref(),
        Some("TheirPlugin"),
        "import must record the plugin it detected"
    );
    assert!(project.join(".jackdaw").is_dir());

    let _ = std::fs::remove_dir_all(&dir);
}
