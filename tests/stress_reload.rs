#![expect(clippy::print_stdout, reason = "test prints progress diagnostics")]
//! Stress test: measure the resident-memory cost of reloading a
//! dylib repeatedly (the edit-mode live-reload leak).
//!
//! A loaded dylib is never unloaded, so each reload leaves its mapped
//! pages resident. This builds one representative SDK-linked dylib
//! (bevy + avian), then dlopens fresh copies in a
//! loop and reports the RSS growth per load, so we know whether
//! auto-reload-per-save is viable and how aggressively reloads must
//! be bounded.
//!
//! ```text
//! cargo test --features dylib --target <host-triple> \
//!     --test stress_reload -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;

use bevy::reflect::TypeRegistry;
use jackdaw::project_build::plan::{SdkManifest, write_plan};
use jackdaw::sdk_paths::SdkPaths;

mod util;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Current resident set size in MB, from `/proc/self/status`.
fn rss_mb() -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: f64 = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            return kb / 1024.0;
        }
    }
    0.0
}

#[test]
fn measure_reload_memory_growth() {
    let sdk = SdkPaths::for_workspace(&workspace_root());
    let triple = sdk.triple.clone();
    assert!(
        sdk.dylib_exists(),
        "SDK dylib missing; build --features dylib --target {triple}"
    );

    // Build the representative SDK-linked dylib through the production
    // pipeline (bevy + avian).
    let fixture_dir = util::stage_fixture("ecosystem_game");
    std::fs::copy(&sdk.lockfile, fixture_dir.join("Cargo.lock")).expect("seed lock");
    let manifest = SdkManifest::generate_dev(&workspace_root(), &sdk).expect("manifest");
    let map_path = fixture_dir.join("extern_map.txt");
    write_plan(&fixture_dir, &manifest, &sdk.deps, &map_path).expect("plan");

    let fixture_target = fixture_dir.join("target-fixture");
    let _ = std::fs::remove_dir_all(&fixture_target);
    let status = std::process::Command::new("cargo")
        .args(["rustc", "--crate-type", "dylib", "--target", &triple])
        .current_dir(&fixture_dir)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &fixture_target)
        .env("RUSTC_WRAPPER", &sdk.wrapper)
        .env("JACKDAW_SDK_DYLIB", &sdk.dylib)
        .env("JACKDAW_SDK_DEPS", &sdk.deps)
        .env("JACKDAW_SDK_HOST_DEPS", &sdk.host_deps)
        .env("JACKDAW_SDK_EXTERN_MAP", &map_path)
        .status()
        .expect("spawn cargo");
    assert!(status.success(), "build failed");
    let dylib = fixture_target.join(format!(
        "{triple}/debug/{}ecosystem_game{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    assert!(dylib.exists(), "dylib missing");

    let size_mb = std::fs::metadata(&dylib).unwrap().len() as f64 / 1024.0 / 1024.0;

    const RELOADS: usize = 20;
    let load_dir = std::env::temp_dir().join("jackdaw-stress-reload");
    let _ = std::fs::remove_dir_all(&load_dir);
    std::fs::create_dir_all(&load_dir).unwrap();

    let baseline = rss_mb();
    println!("\nshim dylib on disk: {size_mb:.1} MB");
    println!("baseline RSS: {baseline:.1} MB");
    println!("simulating {RELOADS} edit-mode reloads (dlopen + drain, never unload):\n");

    let mut last = baseline;
    for i in 0..RELOADS {
        let copy = load_dir.join(format!("shim-{i}{}", std::env::consts::DLL_SUFFIX));
        std::fs::copy(&dylib, &copy).unwrap();
        let lib = unsafe { libloading::Library::new(&copy) }.expect("dlopen");
        std::mem::forget(lib);
        let mut reg = TypeRegistry::default();
        reg.register_derived_types();
        let now = rss_mb();
        println!(
            "  reload {:>2}: RSS {:>7.1} MB  (+{:.1} MB this load)",
            i + 1,
            now,
            now - last
        );
        last = now;
    }

    let total = last - baseline;
    let per = total / RELOADS as f64;
    println!("\ntotal RSS growth over {RELOADS} reloads: {total:.1} MB  ({per:.1} MB per reload)");
    println!(
        "at {per:.1} MB/reload, a session hits 2 GB of leaked dylibs after ~{} reloads",
        (2048.0 / per.max(0.1)) as u64
    );
}
