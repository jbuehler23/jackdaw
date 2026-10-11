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
}
