//! Jackdaw's signed release index and the registry of installed editors.
//!
//! `jd` fetches `releases.json` and its detached signature, verifies them
//! with the official key, and installs editors into the launcher directory
//! shared by every installed editor:
//!
//! ```text
//! <launcher>/installs.json           registry: installs, default, projects
//! <launcher>/editors/<version>/      one directory per installed editor
//! <launcher>/cache/releases.json     last verified index, for offline use
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

    /// The last verified copy of the release index.
    pub fn cached_index(&self) -> PathBuf {
        self.root.join("cache").join("releases.json")
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
