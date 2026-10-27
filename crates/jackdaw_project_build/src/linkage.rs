//! Linkage identity: which SDK build an extension was compiled against.
//!
//! The SDK dylib is a Rust dylib, so its `.rustc` metadata carries its own
//! crate hash (SVH), which changes with any change to the SDK or anything it
//! was built from. The extension build reads that hash with `rustc -Zls=root`
//! using the SDK's toolchain and compiles it into the extension through
//! [`jackdaw_package::sdk_identity`]; rustc itself refuses to compile the
//! extension against any other build of the SDK crate. The editor compares
//! the embedded hash with its own SDK's before opening the library.
//!
//! An installed SDK records the hash in a file beside its dylib, so an
//! editor never needs a compiler to know it. A checkout's SDK is read with
//! `rustc` each time, since it is rebuilt in place.
//!
//! `-Zls` has no stable equivalent, so the dump runs with
//! `RUSTC_BOOTSTRAP=1`. It only reads metadata written by that same
//! compiler; nothing it compiles depends on the flag.

use std::path::Path;

use jackdaw_env::sdk_toolchain_command;

#[derive(Debug)]
pub enum LinkageError {
    /// The artifact or the SDK's metadata could not be read.
    Unreadable { artifact: String, detail: String },
    /// The artifact names no SDK; it was not built through the SDK
    /// pipeline.
    NotSdkLinked { artifact: String },
    /// The recorded SDK hash differs from the running SDK's.
    Mismatch { expected: String, recorded: String },
}

impl std::fmt::Display for LinkageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable { artifact, detail } => {
                write!(f, "could not read {artifact}: {detail}")
            }
            Self::NotSdkLinked { artifact } => {
                write!(f, "{artifact} was not built against the jackdaw SDK")
            }
            Self::Mismatch { expected, recorded } => write!(
                f,
                "the extension was built against a different SDK build (recorded {recorded}, \
                 running {expected}); rebuild the project"
            ),
        }
    }
}

impl std::error::Error for LinkageError {}

/// The file beside an SDK dylib that records its hash.
pub fn identity_file(sdk_dylib: &Path) -> std::path::PathBuf {
    sdk_dylib.with_file_name("jackdaw_sdk.svh")
}

/// The crate hash of the SDK dylib at `sdk_dylib`: from the file beside it
/// when the SDK was installed with one, otherwise read from its metadata.
pub fn sdk_svh(sdk_dylib: &Path, toolchain: Option<&str>) -> Result<String, LinkageError> {
    if let Ok(recorded) = std::fs::read_to_string(identity_file(sdk_dylib)) {
        let recorded = recorded.trim();
        if !recorded.is_empty() {
            return Ok(recorded.to_string());
        }
    }
    own_hash(sdk_dylib, toolchain)
}

/// Write the hash of the SDK dylib at `sdk_dylib` beside it, for an SDK that
/// is about to be shipped.
pub fn record_sdk_svh(sdk_dylib: &Path, toolchain: Option<&str>) -> Result<String, LinkageError> {
    let svh = own_hash(sdk_dylib, toolchain)?;
    std::fs::write(identity_file(sdk_dylib), format!("{svh}\n")).map_err(|e| {
        LinkageError::Unreadable {
            artifact: sdk_dylib.display().to_string(),
            detail: e.to_string(),
        }
    })?;
    Ok(svh)
}

/// Verify that the extension library at `library` was built against the SDK
/// whose hash is `svh`.
pub fn verify_linkage(library: &Path, svh: &str) -> Result<(), LinkageError> {
    let bytes = std::fs::read(library).map_err(|e| LinkageError::Unreadable {
        artifact: library.display().to_string(),
        detail: e.to_string(),
    })?;
    match jackdaw_package::sdk_identity::check(&bytes, svh) {
        Ok(()) => Ok(()),
        Err(jackdaw_package::sdk_identity::Mismatch::Unmarked) => Err(LinkageError::NotSdkLinked {
            artifact: library.display().to_string(),
        }),
        Err(jackdaw_package::sdk_identity::Mismatch::Other { built, running }) => {
            Err(LinkageError::Mismatch {
                expected: running,
                recorded: built,
            })
        }
    }
}

fn metadata_dump(artifact: &Path, toolchain: Option<&str>) -> Result<String, LinkageError> {
    let mut cmd = sdk_toolchain_command("rustc");
    // The rustup proxy selects the pinned toolchain from this variable;
    // a real rustc binary ignores it.
    if let Some(toolchain) = toolchain {
        cmd.env("RUSTUP_TOOLCHAIN", toolchain);
    }
    let output = cmd
        .env("RUSTC_BOOTSTRAP", "1")
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
fn own_hash(artifact: &Path, toolchain: Option<&str>) -> Result<String, LinkageError> {
    let dump = metadata_dump(artifact, toolchain)?;
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
