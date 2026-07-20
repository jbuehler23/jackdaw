//! Launcher-side scaffolding entry point plus dev-checkout helpers.
//!
//! New projects come from the templates embedded in the binary (see
//! [`crate::scaffold`]); this module adds the launcher's thin wrapper
//! and the dev-only manifest rewrite that points scaffolded projects
//! at a local jackdaw source checkout.

use std::path::{Path, PathBuf};

use bevy::log::{info, warn};

/// Resolve the path to the jackdaw source checkout the running
/// editor was built from, if any. Returns `Some(path)` when the
/// path exists on disk and contains a `templates/` directory (i.e.,
/// the editor is a dev build, not an installed binary). Honours
/// `JACKDAW_DEV_CHECKOUT` as a runtime override for unusual
/// setups.
pub fn jackdaw_dev_checkout() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("JACKDAW_DEV_CHECKOUT") {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return Some(path);
        }
    }
    let compile_time = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidate = compile_time.as_path();
    loop {
        if candidate.join("templates").is_dir() && candidate.join("Cargo.toml").is_file() {
            return Some(candidate.to_path_buf());
        }
        candidate = candidate.parent()?;
    }
}

/// Create `<location>/<name>` from an embedded template and return
/// the created project directory.
pub fn scaffold_project(
    name: &str,
    location: &Path,
    kind: crate::scaffold::TemplateKind,
) -> Result<PathBuf, crate::scaffold::ScaffoldError> {
    if !location.is_dir() {
        return Err(crate::scaffold::ScaffoldError::Io(format!(
            "location does not exist: {}",
            location.display()
        )));
    }
    let dest = location.join(name);
    crate::scaffold::scaffold_new_project(&dest, name, kind)?;
    Ok(dest)
}

/// When the editor runs from a jackdaw source checkout, rewrite the
/// scaffolded project's `jackdaw_runtime` / `jackdaw_api` version
/// deps into path deps under `<checkout>/crates/`, so the project
/// builds against the in-development workspace instead of an
/// unpublished crates.io version. Idempotent; a no-op for installed
/// binaries and for manifests that already use path deps.
pub(crate) fn rewrite_jackdaw_dep_for_dev_checkout(dest: &Path) {
    let Some(checkout) = jackdaw_dev_checkout() else {
        return;
    };
    let manifest_path = dest.join("Cargo.toml");
    let Ok(original) = std::fs::read_to_string(&manifest_path) else {
        return;
    };
    let Some(contents) = rewrite_dep_lines(&original, &checkout) else {
        return;
    };
    match std::fs::write(&manifest_path, contents) {
        Ok(()) => info!(
            "Rewrote jackdaw deps in {} to path deps under {}",
            manifest_path.display(),
            checkout.display()
        ),
        Err(e) => warn!(
            "Failed to rewrite jackdaw deps in {}: {e}",
            manifest_path.display()
        ),
    }
}

/// The jackdaw crates a scaffolded project may depend on directly.
const DEV_PATH_DEPS: [&str; 2] = ["jackdaw_runtime", "jackdaw_api"];

/// Rewrite the known jackdaw dep lines to path deps, preserving
/// `features` and `optional` keys. Returns `None` when nothing
/// changed. Paths are single-quoted so Windows backslashes are not
/// parsed as TOML escapes.
fn rewrite_dep_lines(contents: &str, checkout: &Path) -> Option<String> {
    let mut out = String::with_capacity(contents.len());
    let mut rewritten = false;
    for line in contents.lines() {
        let trimmed = line.trim_start();
        let dep = DEV_PATH_DEPS.iter().copied().find(|name| {
            trimmed
                .strip_prefix(name)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        });
        match dep {
            Some(name) if !trimmed.contains("path =") && !trimmed.contains("path=") => {
                let path = checkout.join("crates").join(name);
                let mut replacement = format!("{name} = {{ path = '{}'", path.display());
                if let Some(features) = extract_features_list(trimmed) {
                    replacement.push_str(&format!(", features = {features}"));
                }
                if trimmed.contains("optional = true") {
                    replacement.push_str(", optional = true");
                }
                replacement.push_str(" }");
                out.push_str(&replacement);
                rewritten = true;
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    rewritten.then_some(out)
}

/// The `[...]` value of a `features = [...]` key, skipping
/// `default-features`.
fn extract_features_list(line: &str) -> Option<&str> {
    let mut search_from = 0;
    while let Some(rel) = line[search_from..].find("features") {
        let idx = search_from + rel;
        let preceded_by_dash = idx > 0 && line.as_bytes()[idx - 1] == b'-';
        if !preceded_by_dash {
            let rest = &line[idx..];
            let start = rest.find('[')?;
            let end = rest.find(']')?;
            return Some(&rest[start..=end]);
        }
        search_from = idx + "features".len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn rewrite_dep_lines_handles_windows_backslashes() {
        let template = "[package]\n\
                        name = \"my_game\"\n\
                        \n\
                        [dependencies]\n\
                        jackdaw_runtime = { version = \"0.5\" }\n";
        let checkout = PathBuf::from(r"C:\Code\jackdaw");
        let out = rewrite_dep_lines(template, &checkout).expect("should rewrite");
        // Path separators come from the host platform; the backslashes
        // in the checkout prefix must survive inside a literal string.
        assert!(
            out.contains(r"path = 'C:\Code\jackdaw"),
            "expected literal-string path, got:\n{out}"
        );
        assert!(
            !out.contains(r#"path = "C:\Code"#),
            "must not emit basic string with raw backslashes:\n{out}"
        );
    }

    #[test]
    fn rewrite_dep_lines_preserves_features_and_optional() {
        let template =
            "jackdaw_runtime = { version = \"0.5\", features = [\"physics\"], optional = true }\n";
        let out = rewrite_dep_lines(template, Path::new("/dev/checkout")).unwrap();
        assert!(out.contains("path = '/dev/checkout/crates/jackdaw_runtime'"));
        assert!(out.contains("features = [\"physics\"]"));
        assert!(out.contains("optional = true"));
    }

    #[test]
    fn rewrite_dep_lines_ignores_default_features_key() {
        let template = "jackdaw_api = { version = \"0.5\", default-features = false }\n";
        let out = rewrite_dep_lines(template, Path::new("/dev/checkout")).unwrap();
        assert!(out.contains("path = '/dev/checkout/crates/jackdaw_api'"));
        assert!(!out.contains("features ="), "got:\n{out}");
    }

    #[test]
    fn rewrite_dep_lines_rewrites_plain_version_strings() {
        let template = "[dependencies]\njackdaw_api = \"0.5\"\n";
        let out = rewrite_dep_lines(template, Path::new("/dev/checkout")).unwrap();
        assert!(out.contains("jackdaw_api = { path = '/dev/checkout/crates/jackdaw_api' }"));
    }

    #[test]
    fn rewrite_dep_lines_is_idempotent() {
        let template = "jackdaw_runtime = { path = '/dev/checkout/crates/jackdaw_runtime' }\n";
        assert!(rewrite_dep_lines(template, Path::new("/dev/checkout")).is_none());
    }

    #[test]
    fn rewrite_dep_lines_returns_none_when_no_dep() {
        let template = "[package]\nname = \"my_game\"\n\n[dependencies]\nbevy = \"0.19\"\n";
        assert!(rewrite_dep_lines(template, Path::new("/dev/checkout")).is_none());
    }

    #[test]
    fn rewrite_dep_lines_leaves_other_lines_intact() {
        let template = "[package]\n\
                        name = \"my_game\"\n\
                        \n\
                        [dependencies]\n\
                        bevy = \"0.19\"\n\
                        jackdaw_runtime = { version = \"0.5\", features = [\"physics\"] }\n\
                        avian3d = \"0.7\"\n";
        let out = rewrite_dep_lines(template, Path::new("/dev/checkout")).unwrap();
        assert!(out.contains("name = \"my_game\""));
        assert!(out.contains("bevy = \"0.19\""));
        assert!(out.contains("avian3d = \"0.7\""));
    }

    #[test]
    fn dev_checkout_rewrites_runtime_and_api_deps() {
        // Only meaningful inside a jackdaw source checkout; a no-op otherwise.
        if jackdaw_dev_checkout().is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("jackdaw_devpatch_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\n\
             jackdaw_api = \"0.5\"\n\
             jackdaw_runtime = { version = \"0.5\", features = [\"physics\"] }\n",
        )
        .unwrap();

        rewrite_jackdaw_dep_for_dev_checkout(&dir);

        let out = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(out.contains("jackdaw_api = { path ="));
        assert!(out.contains("jackdaw_runtime = { path ="));
        assert!(out.contains("features = [\"physics\"]"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
