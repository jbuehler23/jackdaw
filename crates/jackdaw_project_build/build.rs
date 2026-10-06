//! Assemble the embedded SDK-builder recipe.
//!
//! Under the `embed-recipe` feature this copies every workspace library crate
//! and a pure `[workspace]` root manifest into an `include_bytes!` table plus a
//! content hash. A shipped jackdaw extracts this at first use and builds the SDK
//! against the user's toolchain; cargo compiles only the SDK targets' closure,
//! so shipping every crate's source keeps the manifest correct without per-crate
//! dependency surgery.
//!
//! Built from a published package there is no workspace to copy, so the recipe
//! is instead a manifest asking crates.io for the SDK crates at exactly this
//! version.
//!
//! A no-op without the feature, and a no-op when compiled in any other
//! workspace, which is what stops the extracted recipe recursing.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::{env, fs};

use path_slash::PathExt as _;

#[path = "src/registry_recipe.rs"]
mod registry_recipe;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let recipe = out_dir.join("recipe");
    let _ = fs::remove_dir_all(&recipe);
    fs::create_dir_all(&recipe).unwrap();

    let embed = env::var_os("CARGO_FEATURE_EMBED_RECIPE").is_some();
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // crates/jackdaw_project_build -> crates -> workspace root
    let workspace = manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf);

    let packaged = is_packaged(&manifest_dir);
    let assembled = embed
        && !packaged
        && workspace
            .as_deref()
            .is_some_and(|ws| is_main_workspace(ws) && assemble_recipe(ws, &recipe));
    if embed && packaged {
        write_registry_recipe(&recipe);
    }

    let (data_rs, hash) = generate_data(&recipe);
    fs::write(out_dir.join("recipe_data.rs"), data_rs).unwrap();
    println!("cargo:rustc-env=RECIPE_HASH={hash}");

    if assembled && let Some(ws) = workspace.as_deref() {
        // Deep-watch every recipe source. `rerun-if-changed` on a directory
        // tracks only that directory entry, so an edit inside a crate would
        // leave the embedded recipe and its hash stale until a clean build.
        emit_rerun_for_tree(&ws.join("crates"));
        println!("cargo:rerun-if-changed={}", ws.join("Cargo.toml").display());
        println!("cargo:rerun-if-changed={}", ws.join("Cargo.lock").display());
    }
    emit_build_source(&manifest_dir, workspace.as_deref(), packaged);

    println!("cargo:rerun-if-changed=build.rs");
    // The embed is toggled only through this feature env, which gates no
    // code, so tell cargo to re-run the script when it flips.
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_EMBED_RECIPE");
}

/// Record where the jackdaw crates this build is made of can be fetched from, so
/// a project it scaffolds can ask for the same ones.
///
/// A build from a published package is a release. A release bundle, built from
/// a checkout, says so through `JACKDAW_RELEASE_BUILD`: nothing readable from
/// the source tree distinguishes the commit a release is cut from. Otherwise
/// the revision, and failing that the workspace on this machine.
fn emit_build_source(manifest_dir: &Path, workspace: Option<&Path>, packaged: bool) {
    println!("cargo:rerun-if-env-changed=JACKDAW_RELEASE_BUILD");
    let source = if packaged || env::var_os("JACKDAW_RELEASE_BUILD").is_some_and(|flag| flag == "1")
    {
        "release".to_string()
    } else if let Some(rev) = git_head(manifest_dir) {
        for file in head_files(manifest_dir) {
            println!("cargo:rerun-if-changed={}", file.display());
        }
        format!("git:{rev}")
    } else {
        format!("path:{}", workspace.unwrap_or(manifest_dir).display())
    };
    println!("cargo:rustc-env=JACKDAW_BUILD_SOURCE={source}");
}

/// The full revision `dir` is checked out at, and nothing when it is not
/// in a repository or git is not installed.
fn git_head(dir: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let rev = String::from_utf8(output.stdout).ok()?.trim().to_string();
    let full = rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit());
    full.then_some(rev)
}

/// The files that change when `dir`'s checkout moves: `HEAD`, and the branch
/// file it names, for cargo to watch so a checkout or a commit moves the
/// embedded revision.
///
/// In a linked worktree these live in the main repository's git directory, not
/// under a `.git` folder in the worktree. Only files that exist are returned:
/// cargo reruns a script on every build while a file it watches is missing.
fn head_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Some(head) = git_path(dir, "HEAD") else {
        return files;
    };
    let branch = fs::read_to_string(&head).ok().and_then(|text| {
        text.strip_prefix("ref: ")
            .map(|name| name.trim().to_string())
    });
    files.push(head);
    if let Some(branch) = branch.and_then(|name| git_path(dir, &name)) {
        files.push(branch);
    }
    files.retain(|file| file.is_file());
    files
}

/// Where git keeps `name` for the checkout at `dir`, as `git rev-parse
/// --git-path` reports it.
fn git_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--git-path", name])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    Some(if path.is_absolute() {
        path
    } else {
        dir.join(path)
    })
}

/// Whether this crate is built from a package (a registry download or an
/// unpacked `.crate`) rather than a checkout. Cargo writes `Cargo.toml.orig`
/// into every package it creates.
fn is_packaged(manifest_dir: &Path) -> bool {
    manifest_dir.join("Cargo.toml.orig").is_file()
}

/// Write the recipe for a build from a published package: a manifest that
/// depends on the SDK crates at exactly this crate's version, which every
/// jackdaw crate shares.
fn write_registry_recipe(recipe: &Path) {
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    fs::write(
        recipe.join("Cargo.toml"),
        registry_recipe::manifest(&version),
    )
    .unwrap();
    fs::create_dir_all(recipe.join("src")).unwrap();
    fs::write(recipe.join("src/lib.rs"), "").unwrap();
    write_cargo_config(recipe);
}

/// The extracted first-run SDK has no checkout-level `.cargo/config.toml`.
/// Preserve the Mach-O/PE dylib codegen rule there too, or a prepared macOS or
/// Windows SDK can contain unresolved shared-generic instantiations.
fn write_cargo_config(recipe: &Path) {
    fs::create_dir_all(recipe.join(".cargo")).unwrap();
    fs::write(
        recipe.join(".cargo/config.toml"),
        "[target.'cfg(any(target_os = \"macos\", target_os = \"windows\"))']\n\
         rustflags = [\"-Zshare-generics=no\"]\n",
    )
    .unwrap();
}

/// True only for the jackdaw editor workspace with the crates present. False in
/// an extracted recipe, so assembly is skipped and there is no recursion.
fn is_main_workspace(ws: &Path) -> bool {
    let Ok(text) = fs::read_to_string(ws.join("Cargo.toml")) else {
        return false;
    };
    text.contains("[workspace]")
        && text.contains("name = \"jackdaw\"")
        && ws.join("crates/jackdaw_sdk/Cargo.toml").is_file()
}

fn assemble_recipe(ws: &Path, recipe: &Path) -> bool {
    // Every workspace library crate. `cargo build -p <sdk targets>` compiles only
    // those targets' closure; the rest ride along so all `.workspace = true` path
    // deps resolve.
    //
    // Except any crate depending on the editor package itself: the recipe's root
    // is a pure virtual workspace with no `jackdaw` package, so such a path
    // dependency cannot resolve and cargo rejects the whole workspace.
    copy_dir_filtered(&ws.join("crates"), &recipe.join("crates"), &|crate_dir| {
        !depends_on_editor(crate_dir)
    });
    let Some(manifest) = generate_root_manifest(ws) else {
        return false;
    };
    fs::write(recipe.join("Cargo.toml"), manifest).unwrap();
    if let Ok(lock) = fs::read(ws.join("Cargo.lock")) {
        fs::write(recipe.join("Cargo.lock"), lock).unwrap();
    }
    write_cargo_config(recipe);
    true
}

/// Emit `cargo:rerun-if-changed` for every file the recipe ships, recursively,
/// skipping build output, VCS metadata and the target directories the recipe
/// leaves out.
///
/// The watch has to match [`copy_dir_filtered`]'s filter exactly: watch less and
/// the hash goes stale silently, watch more and the script re-runs for nothing.
fn emit_rerun_for_tree(dir: &Path) {
    emit_rerun_at_depth(dir, 0);
}

fn emit_rerun_at_depth(dir: &Path, depth: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name == ".git" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            if is_unshipped_target_dir(&name, depth) {
                continue;
            }
            emit_rerun_at_depth(&path, depth + 1);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

/// Copy a directory tree, skipping build output and VCS metadata.
/// Whether a crate directory's manifest depends on the editor package,
/// which the recipe does not ship.
fn depends_on_editor(crate_dir: &Path) -> bool {
    let Ok(text) = fs::read_to_string(crate_dir.join("Cargo.toml")) else {
        return false;
    };
    let Ok(doc) = toml::from_str::<toml::Value>(&text) else {
        return false;
    };
    ["dependencies", "dev-dependencies", "build-dependencies"]
        .iter()
        .any(|table| {
            doc.get(table)
                .and_then(toml::Value::as_table)
                .is_some_and(|deps| deps.contains_key("jackdaw"))
        })
}

/// Cargo target directories the SDK build never compiles.
///
/// A crate's tests, examples and benches are separate targets that build only
/// when asked for, so shipping them adds nothing the SDK can use -- and, because
/// the recipe's content hash is the SDK cache's stamp, editing one of them cost a
/// full SDK rebuild on the next launch.
///
/// Excluded by directory name rather than by reading each manifest, which is
/// sound only while no crate declares an explicit `[[test]]`, `[[example]]` or
/// `[[bench]]` target pointing outside them;
/// `no_shipped_crate_declares_an_explicit_test_or_example_target` guards that.
const UNSHIPPED_TARGET_DIRS: &[&str] = &["tests", "examples", "benches"];

/// Whether `dir` is one of [`UNSHIPPED_TARGET_DIRS`], directly inside a
/// crate root. Nested directories of those names (a `src/tests` module
/// directory, say) are ordinary sources and are kept.
fn is_unshipped_target_dir(name: &str, depth: usize) -> bool {
    depth == 1 && UNSHIPPED_TARGET_DIRS.contains(&name)
}

/// Copy `src` into `dst`, skipping `target`/`.git` and any top-level
/// entry `keep` rejects. `keep` is consulted only for the immediate
/// children, which are the crate directories.
fn copy_dir_filtered(src: &Path, dst: &Path, keep: &dyn Fn(&Path) -> bool) {
    copy_dir_at_depth(src, dst, keep, 0);
}

/// `depth` counts directories below the `crates/` root, so depth 1 is a
/// crate's own top level: the only place a cargo target directory
/// counts.
fn copy_dir_at_depth(src: &Path, dst: &Path, keep: &dyn Fn(&Path) -> bool, depth: usize) {
    fs::create_dir_all(dst).unwrap();
    let Ok(entries) = fs::read_dir(src) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name == ".git" {
            continue;
        }
        let from = entry.path();
        if from.is_dir() {
            if depth == 0 && !keep(&from) {
                continue;
            }
            if is_unshipped_target_dir(&name, depth) {
                continue;
            }
        }
        let to = dst.join(&*name);
        if from.is_dir() {
            copy_dir_at_depth(&from, &to, &|_| true, depth + 1);
        } else {
            fs::copy(&from, &to).unwrap();
        }
    }
}

/// The recipe's root manifest: the workspace's own `[workspace]` table with
/// `members` narrowed to the embedded crates and the editor package dropped, so
/// it is a pure virtual workspace of exactly the library crates.
fn generate_root_manifest(ws: &Path) -> Option<String> {
    let text = fs::read_to_string(ws.join("Cargo.toml")).ok()?;
    let doc: toml::Value = toml::from_str(&text).ok()?;
    let mut workspace = doc.get("workspace")?.as_table()?.clone();
    workspace.insert(
        "members".to_string(),
        toml::Value::Array(vec![toml::Value::String("crates/*".to_string())]),
    );
    // A virtual workspace has no root package edition to imply a resolver, so it
    // would default to resolver 1 and unify features differently than the editor
    // (edition 2024 => resolver 3).
    workspace.insert("resolver".to_string(), toml::Value::String("3".to_string()));
    // These reference paths outside the embedded crates.
    workspace.remove("exclude");
    workspace.remove("default-members");

    let mut root = toml::value::Table::new();
    root.insert("workspace".to_string(), toml::Value::Table(workspace));
    toml::to_string_pretty(&toml::Value::Table(root)).ok()
}

/// Emit `RECIPE_FILES` (relative path + `include_bytes!`) and a stable
/// content hash over the assembled recipe.
fn generate_data(recipe: &Path) -> (String, String) {
    let mut files = Vec::new();
    collect_files(recipe, recipe, &mut files);
    files.sort();

    let mut hasher = DefaultHasher::new();
    let mut out = String::from("pub static RECIPE_FILES: &[(&str, &[u8])] = &[\n");
    for (rel, abs) in &files {
        rel.hash(&mut hasher);
        fs::read(abs).unwrap_or_default().hash(&mut hasher);
        out.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            rel,
            abs.to_string_lossy()
        ));
    }
    out.push_str("];\n");
    (out, format!("{:016x}", hasher.finish()))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_slash_lossy()
                .into_owned();
            out.push((rel, path));
        }
    }
}
