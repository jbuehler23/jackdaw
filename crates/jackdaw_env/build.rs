//! Records the rustup toolchain that compiles this crate, so the SDK and
//! every extension are later built by the same compiler as the editor.

use std::{env, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=RUSTC");
    println!("cargo:rerun-if-env-changed=RUSTUP_TOOLCHAIN");
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = Command::new(&rustc)
        .arg("-vV")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    let host = field(&version, "host").unwrap_or_default();
    let toolchain = toolchain_name(
        field(&version, "release"),
        env::var("RUSTUP_TOOLCHAIN").ok().as_deref(),
        host,
    );
    println!("cargo:rustc-env=JACKDAW_TOOLCHAIN={toolchain}");
}

fn field<'a>(version: &'a str, key: &str) -> Option<&'a str> {
    version
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(": "))
}

/// A stable release is named by its exact version, so a moving `stable`
/// channel never stands in for it. Anything else keeps the rustup name it
/// was built with, minus the host suffix.
fn toolchain_name(release: Option<&str>, rustup: Option<&str>, host: &str) -> String {
    if let Some(release) = release.filter(|r| !r.contains('-')) {
        return release.to_string();
    }
    if let Some(name) = rustup {
        let name = name
            .strip_suffix(host)
            .and_then(|n| n.strip_suffix('-'))
            .unwrap_or(name);
        if name == "nightly" || name == "beta" {
            println!(
                "cargo:warning=built with the moving `{name}` channel; extensions will use \
                 whatever `{name}` is when they build, which may not match this editor"
            );
        }
        return name.to_string();
    }
    println!(
        "cargo:warning=could not name the toolchain building jackdaw; extension builds will \
         use the default toolchain"
    );
    "stable".to_string()
}
