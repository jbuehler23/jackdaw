//! Test-harness orchestrator. `cargo xtask <tier>` runs a tier through nextest.
use std::process::{Command, ExitCode};

mod bundle_smoke;
mod release;

/// Target triple for the heavy tier's SDK build. Reads the host from
/// `rustc -vV` so the tier runs on any host; `JACKDAW_TRIPLE` overrides it.
fn triple() -> String {
    if let Ok(explicit) = std::env::var("JACKDAW_TRIPLE") {
        return explicit;
    }
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .expect("run `rustc -vV` to resolve the host triple");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("`rustc -vV` reports a host triple")
        .to_string()
}

fn sh(program: &str, args: &[&str]) -> bool {
    eprintln!("+ {program} {}", args.join(" "));
    Command::new(program)
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn fast() -> bool {
    sh("cargo", &["fmt", "--all", "--check"])
        && sh(
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--features",
                "dylib",
                "--",
                "--deny",
                "warnings",
            ],
        )
        && sh(
            "cargo",
            &[
                "nextest",
                "run",
                "--profile",
                "ci",
                "--workspace",
                "--lib",
                "--features",
                "dylib",
            ],
        )
}

/// Integration binaries that need a built SDK or a game build; they run in
/// `heavy()` or the onboarding workflow instead.
const SDK_BINARIES: &str = "binary(bsn_game_run) | binary(editor_journey) \
    | binary(stress_reload) | binary(scaffold_e2e) \
    | binary(reflect_auto_register) \
    | binary(component_shape_refresh) | binary(dylib_linkage_identity) \
    | binary(extern_redirect_ecosystem) | binary(mcp_smoke)";

/// Where [`archive`] gathers the Rust dylibs the test binaries load. nextest
/// archives the binaries and the standard library but not `deps/*.so`, whose
/// names carry a hash, so they travel as one extra directory instead.
const ARCHIVED_DYLIBS: &str = "target/debug/archived-dylibs";

const ARCHIVE_BUILD: [&str; 5] = ["--locked", "--workspace", "--features", "dylib", "--tests"];

/// Build every test binary the integration tier runs, including library unit
/// tests, into one nextest archive at `file`.
fn archive(file: &str) -> bool {
    let mut build = vec!["test", "--no-run"];
    build.extend(ARCHIVE_BUILD);
    if !sh("cargo", &build) || !gather_dylibs() {
        return false;
    }
    let mut args = vec!["nextest", "archive", "--archive-file", file];
    args.extend(ARCHIVE_BUILD);
    sh("cargo", &args)
}

fn gather_dylibs() -> bool {
    let dest = std::path::Path::new(ARCHIVED_DYLIBS);
    let _ = std::fs::remove_dir_all(dest);
    if let Err(error) = std::fs::create_dir_all(dest) {
        eprintln!("create {}: {error}", dest.display());
        return false;
    }
    let Ok(entries) = std::fs::read_dir("target/debug/deps") else {
        eprintln!("no target/debug/deps to gather dylibs from");
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "so")
            && let Err(error) = std::fs::copy(&path, dest.join(entry.file_name()))
        {
            eprintln!("copy {}: {error}", path.display());
            return false;
        }
    }
    true
}

/// Every crate's library and integration tests that are not SDK/dylib-gated.
/// `shard` is an `N/M` slice of the run for CI to spread over `M` runners.
/// With `archive`, the binaries come from [`archive`] and are unpacked into
/// the workspace's `target/`, where the tests expect them.
fn integration(shard: Option<&str>, archive: Option<&str>) -> bool {
    let filter = format!("not ({SDK_BINARIES})");
    let mut args = vec!["nextest", "run", "--profile", "ci"];
    match archive {
        Some(file) => args.extend([
            "--archive-file",
            file,
            "--workspace-remap",
            ".",
            "--extract-to",
            ".",
            "--extract-overwrite",
        ]),
        None => args.extend(["--workspace", "--features", "dylib", "--tests"]),
    }
    args.extend(["-E", filter.as_str()]);
    let partition = shard.map(|shard| format!("slice:{shard}"));
    if let Some(partition) = &partition {
        args.extend(["--partition", partition.as_str()]);
    }
    if archive.is_none() {
        return sh("cargo", &args);
    }
    let dylibs = std::env::current_dir()
        .expect("the working directory")
        .join(ARCHIVED_DYLIBS);
    let mut search = std::ffi::OsString::from(dylibs);
    if let Some(inherited) = std::env::var_os("LD_LIBRARY_PATH") {
        search.push(":");
        search.push(inherited);
    }
    eprintln!(
        "+ LD_LIBRARY_PATH={} cargo {}",
        search.display(),
        args.join(" ")
    );
    Command::new("cargo")
        .args(&args)
        .env("LD_LIBRARY_PATH", search)
        .status()
        .is_ok_and(|status| status.success())
}

fn heavy() -> bool {
    let triple = triple();
    let triple = triple.as_str();
    // Same feature set as the test step below; a different one recompiles the
    // whole editor between the two.
    sh(
        "cargo",
        &[
            "build",
            "-p",
            "jackdaw",
            "--features",
            "dylib",
            "--target",
            triple,
        ],
    ) && sh("cargo", &["build", "-p", "jackdaw_rustc_wrapper"])
        // `jd mcp` executes `jd-mcp` from beside `jd`, and it lives in
        // another package, so nothing else in this tier builds it. Without
        // it `mcp_smoke` has no binary to drive and skips.
        && sh(
            "cargo",
            &[
                "build",
                "-p",
                "jackdaw_mcp",
                "--bin",
                "jd-mcp",
                "--target",
                triple,
            ],
        )
        && sh(
            "cargo",
            &[
                "nextest",
                "run",
                "--profile",
                "heavy",
                // The SDK and wrapper above use Cargo's dev profile.
                // Keep the test harnesses on that profile too: SDK manifest
                // generation performs a nested dev build, and Rust dylibs from
                // different profiles have incompatible symbol identities even
                // though Cargo writes them to the same unhashed filename.
                "--cargo-profile",
                "dev",
                "-p",
                "jackdaw",
                "--features",
                "dylib",
                "--target",
                triple,
                // `--test` rather than an `-E` filter: `-E` selects what runs but
                // cargo still builds every test target in the package.
                //
                // mcp_smoke belongs here rather than in `integration`: it
                // launches a windowed editor process and waits minutes on it,
                // which is what this tier's deadlines and thread budget are
                // sized for.
                "--test",
                "bsn_game_run",
                "--test",
                "editor_journey",
                "--test",
                "mcp_smoke",
            ],
        )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tier = args.first().map(String::as_str).unwrap_or_default();
    let ok = match tier {
        "fast" => fast(),
        "archive" => archive(args.get(1).map_or("target/tests.tar.zst", String::as_str)),
        "integration" => {
            let mut shard = None;
            let mut archive = None;
            let mut rest = args[1..].iter().map(String::as_str);
            while let Some(arg) = rest.next() {
                match arg {
                    "--archive" => archive = rest.next(),
                    _ => shard = Some(arg),
                }
            }
            integration(shard, archive)
        }
        "heavy" => heavy(),
        "release-gate" => fast() && integration(None, None) && heavy(),
        "package-sdk" => {
            return jackdaw_cli_internal::package::cmd_package_sdk(&args[1..]);
        }
        "bundle" => {
            return jackdaw_cli_internal::package::cmd_bundle(&args[1..]);
        }
        "bundle-smoke" => bundle_smoke::cmd(&args[1..]),
        "package" => match args.get(1) {
            Some(out) => release::package(
                std::path::Path::new(out),
                args[2..].iter().any(|arg| arg == "--check"),
            ),
            None => {
                eprintln!("usage: cargo xtask package <DIR> [--check]");
                false
            }
        },
        "publish" => release::publish(args[1..].iter().any(|arg| arg == "--dry-run")),
        other => {
            eprintln!(
                "usage: cargo xtask <fast|archive [FILE]|integration [N/M] [--archive FILE]|heavy|\
                 release-gate|package-sdk|bundle|bundle-smoke|package DIR [--check]|publish [--dry-run]> \
                 (got {other:?})"
            );
            false
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
