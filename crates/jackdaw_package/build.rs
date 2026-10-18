use std::process::Command;

fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown-target".into());
    println!("cargo:rustc-env=JACKDAW_COMPILED_TARGET={target}");

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let release = Command::new(rustc)
        .arg("-V")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .unwrap_or_else(|| "rustc unknown".into());
    println!("cargo:rustc-env=JACKDAW_COMPILED_RUSTC={}", release.trim());

    // A build on Bevy's main branch is compatible with the one commit its
    // workspace lockfile pins, not with a Bevy minor.
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let lockfile = manifest_dir.join("../../Cargo.lock");
    println!("cargo:rerun-if-changed={}", lockfile.display());
    let bevy = std::fs::read_to_string(&lockfile)
        .ok()
        .filter(|_| !manifest_dir.join("Cargo.toml.orig").is_file())
        .and_then(|text| locked_bevy_rev(&text))
        .map_or_else(
            || {
                format!(
                    "{}.{}",
                    std::env::var("CARGO_PKG_VERSION_MAJOR").unwrap(),
                    std::env::var("CARGO_PKG_VERSION_MINOR").unwrap()
                )
            },
            |rev| format!("main-{rev}"),
        );
    println!("cargo:rustc-env=JACKDAW_COMPILED_BEVY={bevy}");
}

/// The commit the `bevy` package's git source is locked to.
fn locked_bevy_rev(lockfile: &str) -> Option<String> {
    lockfile
        .split("[[package]]")
        .find(|block| block.lines().any(|line| line.trim() == "name = \"bevy\""))?
        .lines()
        .find_map(|line| line.trim().strip_prefix("source = \"git+"))?
        .trim_end_matches('"')
        .rsplit_once('#')
        .map(|(_, rev)| rev.to_string())
}
