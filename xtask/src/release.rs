//! Packaging the workspace crates and publishing them to crates.io.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Crates that must be published: the editor and MCP server users install, and
/// the crates the project templates depend on.
const REQUIRED: [&str; 4] = [
    "jackdaw",
    "jackdaw_mcp",
    "jackdaw_runtime",
    "jackdaw_extension",
];

/// The largest `.crate` file crates.io accepts.
const UPLOAD_LIMIT: u64 = 10 * 1024 * 1024;

/// How long to wait before retrying an upload crates.io turned away.
const RETRY_WAIT: Duration = Duration::from_secs(120);

/// Uploads refused for publishing too fast are retried this many times; any
/// other failure, which may be the index not yet showing a dependency, three.
const RATE_LIMIT_ATTEMPTS: u32 = 15;
const OTHER_ATTEMPTS: u32 = 3;

/// A workspace crate that `cargo publish` uploads.
#[derive(Debug)]
pub struct Crate {
    pub name: String,
    pub version: String,
    /// Publishable crates that must be on the registry before this one.
    pub needs: BTreeSet<String>,
    pub has_lib: bool,
    pub has_bins: bool,
}

struct Workspace {
    crates: Vec<Crate>,
    target_dir: PathBuf,
}

fn workspace() -> Result<Workspace, String> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err("cargo metadata failed".to_string());
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parse cargo metadata: {error}"))?;
    let target_dir = json["target_directory"]
        .as_str()
        .ok_or("cargo metadata has no target directory")?;
    let crates = publishable(&json);
    let missing: Vec<&str> = REQUIRED
        .into_iter()
        .filter(|name| !crates.iter().any(|krate| krate.name == *name))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "these crates must be published but are not publishable: {}",
            missing.join(", ")
        ));
    }
    Ok(Workspace {
        crates,
        target_dir: PathBuf::from(target_dir),
    })
}

/// The packages in `cargo metadata` output that are not `publish = false`,
/// with the publishable crates each one depends on.
///
/// A dev-dependency counts only when it states a version: cargo drops one
/// without a version from the published manifest, and crates.io never sees it.
pub fn publishable(metadata: &serde_json::Value) -> Vec<Crate> {
    let packages = metadata["packages"].as_array().cloned().unwrap_or_default();
    let packages: Vec<&serde_json::Value> = packages
        .iter()
        .filter(|package| {
            package["publish"]
                .as_array()
                .is_none_or(|to| !to.is_empty())
        })
        .collect();
    let names: BTreeSet<&str> = packages
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect();
    packages
        .iter()
        .map(|package| {
            let needs = package["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|dep| {
                    dep["kind"].as_str() != Some("dev") || dep["req"].as_str() != Some("*")
                })
                .filter_map(|dep| dep["name"].as_str())
                .filter(|name| names.contains(name))
                .map(str::to_string)
                .collect();
            let kinds: BTreeSet<&str> = package["targets"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|target| target["kind"].as_array().into_iter().flatten())
                .filter_map(serde_json::Value::as_str)
                .collect();
            Crate {
                name: package["name"].as_str().unwrap_or_default().to_string(),
                version: package["version"].as_str().unwrap_or_default().to_string(),
                needs,
                has_lib: ["lib", "rlib", "dylib", "proc-macro"]
                    .iter()
                    .any(|kind| kinds.contains(kind)),
                has_bins: kinds.contains("bin"),
            }
        })
        .collect()
}

/// `crates` ordered so every crate comes after the crates it needs, ties
/// broken by name.
pub fn publish_order(crates: &[Crate]) -> Result<Vec<&Crate>, String> {
    let by_name: BTreeMap<&str, &Crate> = crates
        .iter()
        .map(|krate| (krate.name.as_str(), krate))
        .collect();
    let mut placed = BTreeSet::new();
    let mut order = Vec::with_capacity(crates.len());
    while order.len() < by_name.len() {
        let ready: Vec<&Crate> = by_name
            .values()
            .filter(|krate| !placed.contains(krate.name.as_str()))
            .filter(|krate| {
                krate
                    .needs
                    .iter()
                    .all(|need| placed.contains(need.as_str()))
            })
            .copied()
            .collect();
        if ready.is_empty() {
            let stuck: Vec<&str> = by_name
                .keys()
                .filter(|name| !placed.contains(*name))
                .copied()
                .collect();
            return Err(format!(
                "these crates depend on each other in a cycle: {}",
                stuck.join(", ")
            ));
        }
        for krate in ready {
            placed.insert(krate.name.as_str());
            order.push(krate);
        }
    }
    Ok(order)
}

/// The path of `name`'s file in the crates.io sparse index.
pub fn index_path(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    }
}

/// Whether crates.io already has `version` of `name`.
fn on_crates_io(name: &str, version: &str) -> Result<bool, String> {
    let url = format!("https://index.crates.io/{}", index_path(name));
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--write-out",
            "\n%{http_code}",
            &url,
        ])
        .output()
        .map_err(|error| format!("run curl: {error}"))?;
    let body = String::from_utf8_lossy(&output.stdout);
    let (entries, status) = body.rsplit_once('\n').unwrap_or(("", &body));
    match status.trim() {
        "200" => Ok(entries.lines().any(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .is_ok_and(|entry| entry["vers"].as_str() == Some(version))
        })),
        "404" => Ok(false),
        other => Err(format!(
            "{url} answered {other}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// Publish every publishable crate missing from crates.io, in dependency order.
///
/// Packages are not rebuilt here: `package --check` builds them from their
/// packaged sources before a tag is cut. A run cut short by crates.io rate
/// limits is resumed by running it again.
pub fn publish(dry_run: bool) -> bool {
    let workspace = match workspace() {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("{error}");
            return false;
        }
    };
    let order = match publish_order(&workspace.crates) {
        Ok(order) => order,
        Err(error) => {
            eprintln!("{error}");
            return false;
        }
    };
    for krate in order {
        match on_crates_io(&krate.name, &krate.version) {
            Ok(true) => eprintln!("{} {} is already on crates.io", krate.name, krate.version),
            Ok(false) if dry_run => eprintln!("would publish {} {}", krate.name, krate.version),
            Ok(false) => {
                if !publish_one(krate) {
                    return false;
                }
            }
            Err(error) => {
                eprintln!("{error}");
                return false;
            }
        }
    }
    true
}

fn publish_one(krate: &Crate) -> bool {
    let mut attempt = 0;
    loop {
        attempt += 1;
        eprintln!("+ cargo publish --locked --no-verify -p {}", krate.name);
        let output = Command::new("cargo")
            .args(["publish", "--locked", "--no-verify", "-p", &krate.name])
            .stdout(Stdio::inherit())
            .output();
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                eprintln!("run cargo publish: {error}");
                return false;
            }
        };
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprint!("{stderr}");
        if output.status.success() {
            return true;
        }
        let rate_limited = stderr.contains("429") || stderr.contains("Too Many Requests");
        let attempts = if rate_limited {
            RATE_LIMIT_ATTEMPTS
        } else {
            OTHER_ATTEMPTS
        };
        if attempt >= attempts {
            return false;
        }
        eprintln!(
            "retrying {} in {} s ({attempt} of {attempts})",
            krate.name,
            RETRY_WAIT.as_secs()
        );
        std::thread::sleep(RETRY_WAIT);
    }
}

/// Package every publishable crate into `out` as crates.io would receive it.
///
/// `out` gets each `.crate` file, its unpacked sources under `src/`, and a
/// `.cargo/config.toml` whose `[patch.crates-io]` table points every crate at
/// those sources, so cargo run anywhere under `out` resolves the jackdaw
/// crates from their packages as if they were on the registry. With `check`,
/// every packaged library and binary is type-checked from those sources.
pub fn package(out: &Path, check: bool) -> bool {
    let out = std::path::absolute(out).unwrap_or_else(|_| out.to_path_buf());
    match package_into(&out).and_then(|crates| {
        if check {
            check_packages(&out, &crates)
        } else {
            Ok(())
        }
    }) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("{error}");
            false
        }
    }
}

fn package_into(out: &Path) -> Result<Vec<Crate>, String> {
    let workspace = workspace()?;
    publish_order(&workspace.crates)?;
    let packaged = |krate: &Crate| {
        workspace
            .target_dir
            .join("package")
            .join(format!("{}-{}.crate", krate.name, krate.version))
    };
    let mut args = vec!["package", "--locked", "--no-verify"];
    for krate in &workspace.crates {
        let _ = std::fs::remove_file(packaged(krate));
        args.extend(["-p", krate.name.as_str()]);
    }
    if !crate::sh("cargo", &args) {
        return Err("cargo package failed".to_string());
    }
    let _ = std::fs::remove_dir_all(out.join("src"));
    let sources = out.join("src");
    std::fs::create_dir_all(&sources)
        .map_err(|error| format!("create {}: {error}", sources.display()))?;
    let mut patch = String::from("[patch.crates-io]\n");
    let mut too_big = Vec::new();
    for krate in &workspace.crates {
        let packaged = packaged(krate);
        let file_name = packaged.file_name().unwrap_or_default();
        let size = std::fs::metadata(&packaged)
            .map_err(|error| format!("{}: {error}", packaged.display()))?
            .len();
        if size > UPLOAD_LIMIT {
            too_big.push(format!("{} is {size} bytes", file_name.display()));
        }
        std::fs::copy(&packaged, out.join(file_name))
            .map_err(|error| format!("copy {}: {error}", packaged.display()))?;
        let unpacked = Command::new("tar")
            .arg("-xzf")
            .arg(&packaged)
            .arg("-C")
            .arg(&sources)
            .status()
            .is_ok_and(|status| status.success());
        if !unpacked {
            return Err(format!("unpack {}", packaged.display()));
        }
        let source = sources.join(format!("{}-{}", krate.name, krate.version));
        patch.push_str(&format!(
            "{} = {{ path = {:?} }}\n",
            krate.name,
            source.to_string_lossy()
        ));
    }
    if !too_big.is_empty() {
        return Err(format!(
            "crates.io accepts packages up to {UPLOAD_LIMIT} bytes:\n  {}",
            too_big.join("\n  ")
        ));
    }
    let config = out.join(".cargo");
    std::fs::create_dir_all(&config).map_err(|error| error.to_string())?;
    std::fs::write(config.join("config.toml"), patch).map_err(|error| error.to_string())?;
    eprintln!(
        "packaged {} crates into {}",
        workspace.crates.len(),
        out.display()
    );
    Ok(workspace.crates)
}

/// Type-check the packaged crates from their unpacked sources through a
/// throwaway crate depending on every library at its exact version.
fn check_packages(out: &Path, crates: &[Crate]) -> Result<(), String> {
    let root = out.join("check");
    std::fs::create_dir_all(root.join("src")).map_err(|error| error.to_string())?;
    let mut manifest = String::from(
        "[package]\nname = \"packaged-crates\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
         publish = false\n\n[workspace]\n\n[dependencies]\n",
    );
    for krate in crates.iter().filter(|krate| krate.has_lib) {
        manifest.push_str(&format!("{} = \"={}\"\n", krate.name, krate.version));
    }
    std::fs::write(root.join("Cargo.toml"), manifest).map_err(|error| error.to_string())?;
    std::fs::write(root.join("src/lib.rs"), "").map_err(|error| error.to_string())?;

    let check = |args: &[&str], what: &str| {
        eprintln!("+ (in {}) cargo {}", root.display(), args.join(" "));
        let ok = Command::new("cargo")
            .args(args)
            .current_dir(&root)
            .status()
            .is_ok_and(|status| status.success());
        if ok {
            Ok(())
        } else {
            Err(format!("{what} does not build from its package"))
        }
    };
    check(&["check"], "a packaged library")?;
    let mut bins = vec!["check", "--bins"];
    for krate in crates
        .iter()
        .filter(|krate| krate.has_bins && krate.has_lib)
    {
        bins.extend(["-p", krate.name.as_str()]);
    }
    if bins.len() > 2 {
        check(&bins, "a packaged binary")?;
    }
    for krate in crates.iter().filter(|krate| !krate.has_lib) {
        let manifest = out
            .join("src")
            .join(format!("{}-{}", krate.name, krate.version))
            .join("Cargo.toml");
        let manifest = manifest.to_string_lossy();
        check(
            &[
                "check",
                "--manifest-path",
                &manifest,
                "--target-dir",
                "target",
            ],
            &krate.name,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn krate(name: &str, needs: &[&str]) -> Crate {
        Crate {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            needs: needs.iter().map(|need| (*need).to_string()).collect(),
            has_lib: true,
            has_bins: false,
        }
    }

    fn names(order: &[&Crate]) -> Vec<String> {
        order.iter().map(|krate| krate.name.clone()).collect()
    }

    #[test]
    fn dependencies_come_before_their_dependents() {
        let crates = [
            krate("editor", &["runtime", "widgets"]),
            krate("widgets", &["runtime"]),
            krate("runtime", &[]),
            krate("cli", &[]),
        ];
        let order = publish_order(&crates).unwrap();
        assert_eq!(names(&order), ["cli", "runtime", "widgets", "editor"]);
    }

    #[test]
    fn a_cycle_names_the_crates_in_it() {
        let crates = [krate("a", &["b"]), krate("b", &["a"]), krate("c", &[])];
        let error = publish_order(&crates).unwrap_err();
        assert!(error.contains("a, b"), "{error}");
    }

    #[test]
    fn index_paths_follow_the_sparse_index_layout() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("Jackdaw_SDK"), "ja/ck/jackdaw_sdk");
    }

    #[test]
    fn unpublished_packages_and_version_less_dev_dependencies_are_left_out() {
        let metadata = serde_json::json!({
            "packages": [
                {
                    "name": "geometry", "version": "0.1.0", "publish": null,
                    "targets": [{ "kind": ["lib"] }],
                    "dependencies": [
                        { "name": "scene", "kind": "dev", "req": "*" },
                        { "name": "serde", "kind": null, "req": "^1" }
                    ]
                },
                {
                    "name": "scene", "version": "0.1.0", "publish": null,
                    "targets": [{ "kind": ["lib"] }, { "kind": ["bin"] }],
                    "dependencies": [
                        { "name": "geometry", "kind": null, "req": "^0.1.0" },
                        { "name": "fixture", "kind": "dev", "req": "^0.1.0" }
                    ]
                },
                {
                    "name": "fixture", "version": "0.1.0", "publish": [],
                    "targets": [{ "kind": ["cdylib"] }],
                    "dependencies": []
                }
            ]
        });
        let crates = publishable(&metadata);
        assert_eq!(crates.len(), 2);
        let scene = crates.iter().find(|krate| krate.name == "scene").unwrap();
        assert_eq!(scene.needs, BTreeSet::from(["geometry".to_string()]));
        assert!(scene.has_lib && scene.has_bins);
        let geometry = crates
            .iter()
            .find(|krate| krate.name == "geometry")
            .unwrap();
        assert!(geometry.needs.is_empty());
        assert_eq!(
            names(&publish_order(&crates).unwrap()),
            ["geometry", "scene"]
        );
    }
}
