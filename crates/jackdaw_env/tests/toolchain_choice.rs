//! Every toolchain process in the workspace states whose toolchain runs it.
//!
//! The editor's own toolchain is only right for the SDK and extensions. Run
//! against a user's project it fails on a machine without that exact release,
//! or has rustup download it unannounced, so its use is confined to the files
//! that build or inspect the SDK and extensions.

use std::path::{Path, PathBuf};

/// Files that may run the editor's toolchain, relative to the workspace root.
const SDK_TOOLCHAIN_FILES: &[&str] = &[
    "crates/jackdaw_env/src/lib.rs",
    "crates/jackdaw_project_build/src/bootstrap.rs",
    "crates/jackdaw_project_build/src/build.rs",
    "crates/jackdaw_project_build/src/plan.rs",
    "crates/jackdaw_project_build/src/linkage.rs",
    "crates/jackdaw_cli/src/package.rs",
    "src/preflight.rs",
];

const TOOLS: &[&str] = &["cargo", "rustc", "rustup", "rustfmt", "rustdoc"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels below the workspace root")
        .to_path_buf()
}

/// Every `.rs` file under the workspace's `src/` and each crate's `src/`.
fn workspace_sources(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.join("src")];
    for entry in std::fs::read_dir(root.join("crates")).expect("crates dir") {
        dirs.push(entry.expect("crate entry").path().join("src"));
    }
    let mut files = Vec::new();
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn the_editors_toolchain_runs_only_sdk_and_extension_work() {
    let root = workspace_root();
    for allowed in SDK_TOOLCHAIN_FILES {
        assert!(
            root.join(allowed).is_file(),
            "{allowed} is listed as running the editor's toolchain but does not exist"
        );
    }
    let offenders: Vec<String> = workspace_sources(&root)
        .into_iter()
        .filter(|path| !SDK_TOOLCHAIN_FILES.contains(&relative(&root, path).as_str()))
        .filter(|path| {
            std::fs::read_to_string(path).is_ok_and(|text| text.contains("sdk_toolchain_command("))
        })
        .map(|path| relative(&root, &path))
        .collect();
    assert!(
        offenders.is_empty(),
        "these run the editor's toolchain outside SDK and extension work; a project, game or \
         rustup command takes `user_toolchain_command`: {offenders:?}"
    );
}

#[test]
fn every_toolchain_process_names_its_toolchain() {
    let root = workspace_root();
    let patterns: Vec<String> = TOOLS
        .iter()
        .map(|tool| format!("Command::new(\"{tool}\")"))
        .collect();
    let offenders: Vec<String> = workspace_sources(&root)
        .into_iter()
        .filter(|path| {
            std::fs::read_to_string(path)
                .is_ok_and(|text| patterns.iter().any(|pattern| text.contains(pattern)))
        })
        .map(|path| relative(&root, &path))
        .collect();
    assert!(
        offenders.is_empty(),
        "these start a toolchain process that inherits whatever toolchain the editor was \
         started under; use `user_toolchain_command` or `sdk_toolchain_command`: {offenders:?}"
    );
}
