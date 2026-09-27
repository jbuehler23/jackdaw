//! End-to-end scaffolding regression: scaffold a game project, build it as a
//! cargo binary, and assert the template's component reaches the extracted
//! schema. The nested build is almost all dependency graph, which only changes
//! with the template or the workspace lockfile, so the target dir it compiles
//! into is keyed on both and kept between runs.

use std::path::{Path, PathBuf};

use jackdaw::scaffold::{TemplateKind, scaffold_new_project};
use jackdaw_project_build::{BuildEvent, build_project_binary, shim_spec_for_project};

#[test]
fn scaffold_game_builds_and_exposes_component() {
    let key = template_revision();
    let work = scratch_root().join(&key);
    let dest = work.join("mygame");
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&work).expect("a scratch dir for the scaffolded project");

    scaffold_new_project(&dest, "mygame", TemplateKind::Game).expect("scaffold a game project");
    reuse_dependency_build(&dest, &dependency_cache(&key));

    let spec = shim_spec_for_project(&dest).expect("scaffolded project is a jackdaw project");
    let jackdaw_dir = dest.join(".jackdaw");
    let mut log = Vec::new();
    let mut collect_log = |event: BuildEvent| {
        if let BuildEvent::Log(line) = event {
            log.push(line);
        }
    };
    let build = build_project_binary(&spec, &jackdaw_dir, &mut collect_log)
        .expect("build the scaffolded project binary");

    let schema = build.schema.unwrap_or_else(|| {
        panic!(
            "no schema extracted from the built binary; the build said:\n{}",
            log.join("\n")
        )
    });
    let spinning_cube = schema
        .components
        .iter()
        .find(|c| c.type_path.ends_with("::SpinningCube"))
        .unwrap_or_else(|| {
            panic!(
                "template component `SpinningCube` missing from schema; got: {:?}",
                schema
                    .components
                    .iter()
                    .map(|c| c.type_path.as_str())
                    .collect::<Vec<_>>()
            )
        });
    let field_names: Vec<&str> = spinning_cube
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(
        field_names,
        ["speed"],
        "the fields match the Reflect derive"
    );
    assert!(
        spinning_cube.default.is_some(),
        "ReflectDefault gives the component picker a default value"
    );

    let on_disk = jackdaw_schema::read_schema(&jackdaw_dir).expect("the schema is written");
    assert!(
        on_disk
            .components
            .iter()
            .any(|c| c.type_path == spinning_cube.type_path),
        "the schema the editor watches on disk holds the component"
    );
}

/// Where this checkout scaffolds its project: under the system temp dir and keyed
/// on the checkout, so two worktrees do not write the same path and one run's
/// `remove_dir_all` land in the middle of the other's build.
fn scratch_root() -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    env!("CARGO_MANIFEST_DIR").hash(&mut hasher);
    std::env::temp_dir().join(format!("jackdaw_scaffold_e2e_{:016x}", hasher.finish()))
}

/// A digest of the game template and the workspace lockfile.
///
/// Two runs sharing this key scaffold byte-identical sources and resolve
/// the same dependency versions, so the second can reuse what the first
/// compiled. Editing a template file or the lockfile changes the key and
/// the next run builds from scratch.
fn template_revision() -> String {
    use std::hash::{Hash as _, Hasher as _};

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_files(&workspace.join("templates/game"), &mut files);
    files.sort();
    files.push(workspace.join("Cargo.lock"));

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for file in files {
        file.file_name().hash(&mut hasher);
        std::fs::read(&file)
            .unwrap_or_else(|e| panic!("read {} for the cache key: {e}", file.display()))
            .hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read the template dir") {
        let path = entry.expect("template dir entry").path();
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The target dir this template revision compiles into, with the dirs
/// earlier revisions left behind removed.
fn dependency_cache(key: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/scaffold-e2e");
    std::fs::create_dir_all(&root).expect("a cache root for the nested build");
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            if entry.file_name() != *key {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    root.join(key)
}

/// Point the scaffolded project's build at the shared target dir.
/// `build_project_binary` strips `CARGO_TARGET_DIR` from the environment it hands
/// cargo, so the project's own config is what remains, read from the project dir
/// upwards - under the system temp dir, out of reach of the workspace's config.
fn reuse_dependency_build(project: &Path, target_dir: &Path) {
    let dir = project.join(".cargo");
    std::fs::create_dir_all(&dir).expect("a cargo config dir in the scaffolded project");
    let contents = format!("[build]\ntarget-dir = {:?}\n", target_dir.to_string_lossy());
    std::fs::write(dir.join("config.toml"), contents).expect("write the nested build's config");
}
