//! Maintains the published release index.
//!
//! ```text
//! jackdaw-release-index add --index <releases.json> --tag <tag> --dist <dir>
//!     --base-url <url> --bevy <minor> [--rustc <version>] [--notes-url <url>]
//!     [--bevy-rev <sha>] [--jackdaw-rev <sha>] [--known-good]
//! jackdaw-release-index sign <file>
//! jackdaw-release-index public-key
//! ```
//!
//! `add` records the bundles in `--dist` (named `jackdaw-<target>.<ext>`)
//! as a release and rewrites the index in place. `sign` writes `<file>.sig`
//! with the PKCS#8 key held base64-encoded in `JACKDAW_INDEX_SIGNING_KEY`,
//! and `public-key` prints that key's public half for
//! `OFFICIAL_INDEX_KEY`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use base64::Engine as _;
use jackdaw_releases::index::{Artifact, Channel, Release, ReleaseIndex};
use jackdaw_releases::{sha256_hex, signing, version, write_atomically};

const SIGNING_KEY_VAR: &str = "JACKDAW_INDEX_SIGNING_KEY";

const ARCHIVE_EXTENSIONS: [&str; 3] = [".tar.zst", ".tar.gz", ".zip"];

#[expect(clippy::print_stderr, reason = "a command-line tool reports to stderr")]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("add") => add(&args[1..]),
        Some("sign") => match args.get(1) {
            Some(file) => sign(Path::new(file)),
            None => Err("usage: jackdaw-release-index sign <file>".into()),
        },
        Some("public-key") => public_key(),
        _ => Err("usage: jackdaw-release-index <add|sign|public-key>".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("jackdaw-release-index: {error}");
            ExitCode::FAILURE
        }
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn required(args: &[String], name: &str) -> Result<String, String> {
    flag(args, name).ok_or_else(|| format!("{name} is required"))
}

#[expect(clippy::print_stdout, reason = "reports what was added")]
fn add(args: &[String]) -> Result<(), String> {
    let index_path = PathBuf::from(required(args, "--index")?);
    let tag = required(args, "--tag")?;
    let dist = PathBuf::from(required(args, "--dist")?);
    let base_url = required(args, "--base-url")?;
    let release_version = tag.strip_prefix('v').unwrap_or(&tag).to_string();
    let channel = if version::is_nightly(&release_version) {
        Channel::BleedingEdge
    } else {
        Channel::Stable
    };

    let mut index = match std::fs::read(&index_path) {
        Ok(bytes) => ReleaseIndex::parse(&bytes).map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ReleaseIndex::default(),
        Err(error) => return Err(format!("{}: {error}", index_path.display())),
    };

    let today = version::today();
    let release = Release {
        version: release_version.clone(),
        channel,
        bevy: match channel {
            Channel::Stable => required(args, "--bevy")?,
            Channel::BleedingEdge => flag(args, "--bevy").unwrap_or_else(|| "main".into()),
        },
        bevy_rev: flag(args, "--bevy-rev"),
        jackdaw_rev: flag(args, "--jackdaw-rev"),
        date: version::format_date(today),
        rustc: flag(args, "--rustc"),
        sdk_id: None,
        notes_url: flag(args, "--notes-url"),
        yanked: false,
        broken: false,
        known_good: args.iter().any(|arg| arg == "--known-good"),
        known_issues: Vec::new(),
        artifacts: artifacts(&dist, base_url.trim_end_matches('/'))?,
        modules_index: None,
    };
    if release.artifacts.is_empty() {
        return Err(format!("no jackdaw-<target> bundles in {}", dist.display()));
    }
    let targets: Vec<_> = release
        .artifacts
        .iter()
        .map(|artifact| artifact.target.clone())
        .collect();
    index.add_release(release, today);
    index.generated = Some(format!("{}T00:00:00Z", version::format_date(today)));
    let text = serde_json::to_string_pretty(&index).map_err(|error| error.to_string())?;
    write_atomically(&index_path, text.as_bytes())?;
    println!("added {release_version} for {}", targets.join(", "));
    Ok(())
}

fn artifacts(dist: &Path, base_url: &str) -> Result<Vec<Artifact>, String> {
    let entries =
        std::fs::read_dir(dist).map_err(|error| format!("{}: {error}", dist.display()))?;
    let mut artifacts = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(target) = bundle_target(&name) else {
            continue;
        };
        let bytes = std::fs::read(entry.path()).map_err(|error| format!("{name}: {error}"))?;
        artifacts.push(Artifact {
            target: target.to_string(),
            url: format!("{base_url}/{name}"),
            sha256: sha256_hex(&bytes),
            size: bytes.len() as u64,
            installed_size: None,
            attestation: None,
        });
    }
    artifacts.sort_by(|a, b| a.target.cmp(&b.target));
    Ok(artifacts)
}

/// The target of a bundle file named `jackdaw-<target>.<ext>`.
fn bundle_target(name: &str) -> Option<&str> {
    let stem = name.strip_prefix("jackdaw-")?;
    ARCHIVE_EXTENSIONS
        .iter()
        .find_map(|extension| stem.strip_suffix(extension))
        .filter(|target| target.contains('-'))
}

fn signing_key() -> Result<ed25519_dalek::SigningKey, String> {
    let encoded =
        std::env::var(SIGNING_KEY_VAR).map_err(|_| format!("{SIGNING_KEY_VAR} is not set"))?;
    let document = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|error| format!("{SIGNING_KEY_VAR} is not base64: {error}"))?;
    signing::decode_pkcs8(&document)
        .ok_or_else(|| format!("{SIGNING_KEY_VAR} is not an Ed25519 PKCS#8 key"))
}

#[expect(clippy::print_stdout, reason = "reports the signature written")]
fn sign(file: &Path) -> Result<(), String> {
    let key = signing_key()?;
    let bytes = std::fs::read(file).map_err(|error| format!("{}: {error}", file.display()))?;
    let mut signature_path = file.as_os_str().to_owned();
    signature_path.push(".sig");
    let signature_path = PathBuf::from(signature_path);
    write_atomically(&signature_path, &signing::sign_detached(&key, &bytes))?;
    println!("signed {}", file.display());
    Ok(())
}

#[expect(clippy::print_stdout, reason = "prints the public key")]
fn public_key() -> Result<(), String> {
    println!("{}", signing::key_hex(&signing_key()?.verifying_key()));
    Ok(())
}
