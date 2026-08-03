//! Test-harness orchestrator. `cargo xtask <tier>` runs a tier through nextest.
use std::process::{Command, ExitCode};

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

fn integration() -> bool {
    // Every crate's integration tests that are not SDK/dylib-gated. Matches the
    // pre-harness `--workspace ... --tests` coverage; the excluded binaries need
    // a built SDK and run in `heavy()` or the onboarding workflow instead.
    sh(
        "cargo",
        &[
            "nextest",
            "run",
            "--profile",
            "ci",
            "--workspace",
            "--features",
            "dylib",
            "--tests",
            "-E",
            "not (binary(bsn_game_run) | binary(editor_journey) | binary(bundle_smoke) \
               | binary(stress_reload) | binary(scaffold_e2e) \
               | binary(runner_boots_project_dylib) | binary(schema_extract) | binary(reflect_auto_register) \
               | binary(component_shape_refresh) | binary(dylib_linkage_identity) | binary(extern_redirect_ecosystem))",
        ],
    )
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
            "dylib runner",
            "--target",
            triple,
        ],
    ) && sh("cargo", &["build", "-p", "jackdaw_rustc_wrapper"])
        && sh(
            "cargo",
            &["build", "-p", "jackdaw_runner", "--target", triple],
        )
        && sh(
            "cargo",
            &[
                "nextest",
                "run",
                "--profile",
                "heavy",
                // The SDK, runner, and wrapper above use Cargo's dev profile.
                // Keep the test harnesses on that profile too: SDK manifest
                // generation performs a nested dev build, and Rust dylibs from
                // different profiles have incompatible symbol identities even
                // though Cargo writes them to the same unhashed filename.
                "--cargo-profile",
                "dev",
                "-p",
                "jackdaw",
                "--features",
                "dylib runner",
                "--target",
                triple,
                // `--test` rather than an `-E` filter: `-E` selects what runs but
                // cargo still builds every test target in the package.
                //
                // bundle_smoke is deliberately absent: it needs a release SDK,
                // which this tier does not build, so it would only self-skip
                // here. It runs on the real release artifacts in release.yaml.
                "--test",
                "bsn_game_run",
                "--test",
                "editor_journey",
            ],
        )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tier = args.first().map(String::as_str).unwrap_or_default();
    let ok = match tier {
        "fast" => fast(),
        "integration" => integration(),
        "heavy" => heavy(),
        "release-gate" => fast() && integration() && heavy(),
        "package-sdk" => {
            return jackdaw_cli_internal::package::cmd_package_sdk(&args[1..]);
        }
        "bundle" => {
            return jackdaw_cli_internal::package::cmd_bundle(&args[1..]);
        }
        other => {
            eprintln!(
                "usage: cargo xtask <fast|integration|heavy|release-gate|package-sdk|bundle> \
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
