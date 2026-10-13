//! Jackdaw's signed release index and the registry of installed editors.
//!
//! `jd` fetches `releases.json` and its detached signature, verifies them
//! with the official key, and installs editors into the launcher directory
//! shared by every installed editor:
//!
//! ```text
//! <launcher>/installs.json           registry: installs, default, projects
//! <launcher>/editors/<version>/      one directory per installed editor
//! <launcher>/installs.lock           held while the registry changes
//! <launcher>/cache/releases.signed   last verified index, for offline use
//! ```
//!
//! Nothing here touches the network, so the editor can read the same state
//! without linking TLS.

pub mod index;
pub mod installs;
pub mod signing;
pub mod version;

use std::path::{Path, PathBuf};

pub use index::{Artifact, Channel, ModulesIndex, Release, ReleaseIndex};
pub use installs::{EditorPin, Install, Installs, PinResolution};

/// Where the official index is published.
pub const OFFICIAL_INDEX_URL: &str =
    "https://github.com/jbuehler23/jackdaw/releases/download/index/releases.json";

/// Environment variable naming another index URL, for a mirror or a test
/// fixture. Its signature is read from the same URL with `.sig` appended.
pub const INDEX_URL_VAR: &str = "JACKDAW_RELEASE_INDEX";

/// Environment variable holding the hex public key to verify the index
/// with in place of [`signing::OFFICIAL_INDEX_KEY`].
pub const INDEX_KEY_VAR: &str = "JACKDAW_RELEASE_INDEX_KEY";

/// The files under the launcher directory.
#[derive(Clone, Debug)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// The layout under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The layout under [`jackdaw_env::paths::launcher_dir`].
    pub fn from_env() -> Option<Self> {
        jackdaw_env::paths::launcher_dir().map(Self::new)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The registry file.
    pub fn registry(&self) -> PathBuf {
        self.root.join(installs::REGISTRY_FILE)
    }

    /// The directory managed installs live in.
    pub fn editors(&self) -> PathBuf {
        self.root.join(installs::EDITORS_DIR)
    }

    /// Where a managed install of `version` lives.
    pub fn editor(&self, version: &str) -> PathBuf {
        self.editors().join(version)
    }

    /// The last verified copy of the release index: its 64 signature
    /// bytes followed by the index, so the pair is replaced in one write.
    pub fn cached_index(&self) -> PathBuf {
        self.root.join("cache").join("releases.signed")
    }

    /// Read the cached index as `(index bytes, signature)`.
    pub fn read_cached_index(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        let mut bytes = std::fs::read(self.cached_index()).ok()?;
        if bytes.len() < signing::SIGNATURE_LEN {
            return None;
        }
        let index = bytes.split_off(signing::SIGNATURE_LEN);
        Some((index, bytes))
    }

    /// Replace the cached index with `index` and its `signature`.
    pub fn write_cached_index(&self, index: &[u8], signature: &[u8]) -> Result<(), String> {
        if signature.len() != signing::SIGNATURE_LEN {
            return Err("a release index signature is 64 bytes".into());
        }
        let mut bytes = signature.to_vec();
        bytes.extend_from_slice(index);
        write_atomically(&self.cached_index(), &bytes)
    }

    /// Take the registry lock, waiting for another `jd` that holds it.
    /// Released when the guard is dropped.
    pub fn lock(&self) -> Result<RegistryLock, String> {
        let path = self.root.join("installs.lock");
        let describe = |error: std::io::Error| format!("{}: {error}", path.display());
        std::fs::create_dir_all(&self.root).map_err(describe)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(describe)?;
        file.lock().map_err(describe)?;
        Ok(RegistryLock { _file: file })
    }

    /// The registry, or an empty one when none was written yet.
    pub fn load_installs(&self) -> Result<Installs, String> {
        Installs::load(&self.registry())
    }

    /// Write the registry.
    pub fn save_installs(&self, installs: &Installs) -> Result<(), String> {
        installs.save(&self.registry())
    }
}

/// Holds the registry lock from [`Layout::lock`].
#[derive(Debug)]
pub struct RegistryLock {
    _file: std::fs::File,
}

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex(&sha2::Sha256::digest(bytes))
}

/// Replace `path` with `bytes` through a sibling temporary file, so the old
/// contents stay whole until the new ones are complete.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let describe = |error: std::io::Error| format!("{}: {error}", path.display());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(describe)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, bytes).map_err(describe)?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        describe(error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cached_index_keeps_its_signature_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path());
        assert!(layout.read_cached_index().is_none());
        layout.write_cached_index(b"{}", &[7; 64]).unwrap();
        assert_eq!(
            layout.read_cached_index(),
            Some((b"{}".to_vec(), vec![7; 64]))
        );
        assert!(layout.write_cached_index(b"{}", &[7; 10]).is_err());
    }

    #[test]
    fn the_registry_lock_can_be_taken_again_once_released() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path().join("launcher"));
        drop(layout.lock().unwrap());
        let _held = layout.lock().unwrap();
    }
}
