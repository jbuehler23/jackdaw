//! Linkage identity: compatibility verification with no user-authored
//! tags.
//!
//! A project dylib built as a Rust dylib keeps its `.rustc` metadata
//! section, which records every dependency crate with the exact SVH it
//! was compiled against. Comparing the dylib's recorded `jackdaw_sdk`
//! dependency hash with the running SDK dylib's own hash proves the
//! dylib links THE running SDK: a stale or foreign SDK build has a
//! different hash even at the same crate version. The hashes are read
//! with `rustc -Zls=root` using the toolchain the editor already ships
//! for project builds; a foreign compiler's artifact fails to parse
//! outright (the metadata format is versioned), which is itself the
//! toolchain-mismatch signal.

use std::path::Path;
use std::process::Command;

#[derive(Debug)]
pub enum LinkageError {
    /// `rustc -Zls` failed on the artifact (missing file, cdylib with
    /// no metadata section, or a foreign-toolchain artifact).
    Unreadable { artifact: String, detail: String },
    /// The artifact does not record a `jackdaw_sdk` dependency at all;
    /// it was not built through the SDK pipeline.
    NotSdkLinked { artifact: String },
    /// The recorded SDK hash differs from the running SDK's.
    Mismatch { expected: String, recorded: String },
}

impl std::fmt::Display for LinkageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable { artifact, detail } => {
                write!(f, "could not read crate metadata from {artifact}: {detail}")
            }
            Self::NotSdkLinked { artifact } => {
                write!(f, "{artifact} was not built against the jackdaw SDK")
            }
            Self::Mismatch { expected, recorded } => write!(
                f,
                "dylib links a different SDK build (recorded {recorded}, running {expected}); \
                 rebuild the project"
            ),
        }
    }
}

impl std::error::Error for LinkageError {}

/// Verify that `dylib` links the running SDK at `sdk_dylib`.
pub fn verify_linkage(dylib: &Path, sdk_dylib: &Path) -> Result<(), LinkageError> {
    let expected = own_hash(sdk_dylib)?;
    let recorded = recorded_sdk_dep_hash(dylib)?;
    if recorded != expected {
        return Err(LinkageError::Mismatch { expected, recorded });
    }
    Ok(())
}

fn metadata_dump(artifact: &Path) -> Result<String, LinkageError> {
    let output = Command::new("rustc")
        .arg("-Zls=root")
        .arg(artifact)
        .output()
        .map_err(|e| LinkageError::Unreadable {
            artifact: artifact.display().to_string(),
            detail: e.to_string(),
        })?;
    if !output.status.success() {
        return Err(LinkageError::Unreadable {
            artifact: artifact.display().to_string(),
            detail: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The artifact's own SVH: the `hash <H>` field of its crate info.
fn own_hash(artifact: &Path) -> Result<String, LinkageError> {
    let dump = metadata_dump(artifact)?;
    dump.lines()
        .find_map(|line| {
            let rest = line.strip_prefix("hash ")?;
            rest.split_whitespace().next().map(str::to_string)
        })
        .ok_or_else(|| LinkageError::Unreadable {
            artifact: artifact.display().to_string(),
            detail: "no own hash in metadata dump".into(),
        })
}

/// The SVH the artifact recorded for its `jackdaw_sdk` dependency.
fn recorded_sdk_dep_hash(artifact: &Path) -> Result<String, LinkageError> {
    let dump = metadata_dump(artifact)?;
    dump.lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let _index = parts.next()?;
            let name = parts.next()?;
            if !name.starts_with("jackdaw_sdk") {
                return None;
            }
            let hash_kw = parts.next()?;
            if hash_kw != "hash" {
                return None;
            }
            parts.next().map(str::to_string)
        })
        .ok_or_else(|| LinkageError::NotSdkLinked {
            artifact: artifact.display().to_string(),
        })
}
