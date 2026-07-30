//! Assemble the embedded SDK-builder recipe.
//!
//! When built in the jackdaw workspace with the `embed-recipe` feature,
//! this copies every workspace library crate and a self-contained pure
//! `[workspace]` root manifest into an `include_bytes!` table plus a
//! content hash. A shipped jackdaw extracts this at first use and builds
//! the SDK (`cargo build -p jackdaw_sdk -p jackdaw_runner -p
//! jackdaw_rustc_wrapper`) against the user's toolchain; cargo compiles
//! only those three targets and their closure, so shipping every crate's
//! source (but not the editor's) keeps the manifest correct without
//! per-crate dependency surgery.
//!
//! No-op unless `embed-recipe` is set, so ordinary dev builds pay nothing.
//! Also a no-op when compiled OUTSIDE the workspace, which is what happens
//! when the extracted recipe rebuilds this crate: no workspace to read, so
//! it embeds nothing and does not recurse.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::{env, fs};

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

    let assembled = embed
        && workspace
            .as_deref()
            .is_some_and(|ws| is_main_workspace(ws) && assemble_recipe(ws, &recipe));

    let (data_rs, hash) = generate_data(&recipe);
    fs::write(out_dir.join("recipe_data.rs"), data_rs).unwrap();
    println!("cargo:rustc-env=RECIPE_HASH={hash}");

    if assembled && let Some(ws) = workspace {
        // Deep-watch every recipe source. `rerun-if-changed` on a
        // directory tracks only that directory entry, not nested file
        // contents, so an edit inside a crate would leave the embedded
        // recipe (and its hash) stale until a clean build. Emit one line
        // per file so any source change re-runs assembly and re-hashes.
        emit_rerun_for_tree(&ws.join("crates"));
        println!("cargo:rerun-if-changed={}", ws.join("Cargo.toml").display());
        println!("cargo:rerun-if-changed={}", ws.join("Cargo.lock").display());
    }
    println!("cargo:rerun-if-changed=build.rs");
    // The embed is toggled only through this feature env, which gates no
    // code, so tell cargo to re-run the script when it flips.
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_EMBED_RECIPE");
}

/// True only for the jackdaw editor workspace with the crates present. In
/// an extracted recipe (a pure virtual workspace) this is false, so
/// assembly is skipped and there is no recursion.
fn is_main_workspace(ws: &Path) -> bool {
    let Ok(text) = fs::read_to_string(ws.join("Cargo.toml")) else {
        return false;
    };
    text.contains("[workspace]")
        && text.contains("name = \"jackdaw\"")
        && ws.join("crates/jackdaw_sdk/Cargo.toml").is_file()
}

fn assemble_recipe(ws: &Path, recipe: &Path) -> bool {
    // Every workspace library crate. `cargo build -p <sdk targets>` only
    // compiles the three targets' closure; the rest ride along so all
    // `.workspace = true` path deps (including optional ones) resolve.
    //
    // Except any crate that depends on the editor package itself. The
    // recipe's root is a pure virtual workspace with no `jackdaw`
    // package in it, so such a crate's path dependency cannot resolve
    // and cargo rejects the whole workspace before compiling anything:
    //   found a virtual manifest at .../build/Cargo.toml
    //   instead of a package manifest
    // That aborts SDK setup entirely, which leaves the editor unable to
    // build any project.
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
    true
}

/// Emit `cargo:rerun-if-changed` for every file under `dir`, recursively,
/// skipping build output and VCS metadata, so a change to any recipe
/// source re-runs the build script and refreshes the embedded hash.
fn emit_rerun_for_tree(dir: &Path) {
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
            emit_rerun_for_tree(&path);
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

/// Copy `src` into `dst`, skipping `target`/`.git` and any top-level
/// entry `keep` rejects. `keep` is consulted only for the immediate
/// children, which are the crate directories.
fn copy_dir_filtered(src: &Path, dst: &Path, keep: &dyn Fn(&Path) -> bool) {
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
        if from.is_dir() && !keep(&from) {
            continue;
        }
        let to = dst.join(&*name);
        if from.is_dir() {
            copy_dir_filtered(&from, &to, &|_| true);
        } else {
            fs::copy(&from, &to).unwrap();
        }
    }
}

/// The recipe's root manifest: the workspace's own `[workspace]` table
/// (its `[workspace.dependencies]`, `[workspace.lints]`,
/// `[workspace.package]`, resolver) with `members` narrowed to the
/// embedded crates and the editor package dropped, so it is a pure
/// virtual workspace of exactly the library crates.
fn generate_root_manifest(ws: &Path) -> Option<String> {
    let text = fs::read_to_string(ws.join("Cargo.toml")).ok()?;
    let doc: toml::Value = toml::from_str(&text).ok()?;
    let mut workspace = doc.get("workspace")?.as_table()?.clone();
    workspace.insert(
        "members".to_string(),
        toml::Value::Array(vec![toml::Value::String("crates/*".to_string())]),
    );
    // A virtual workspace has no root package edition to imply a resolver,
    // so it would default to resolver 1 and unify features differently than
    // the editor (edition 2024 => resolver 3). Pin it to match, or the SDK
    // dylib is built with a different feature set and every build that
    // touches a different package subset re-resolves and re-links.
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
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
}
