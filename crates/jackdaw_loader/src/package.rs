//! Signed `.jdext` bundle validation and versioned installation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use ring::signature::{self, KeyPair as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionManifest {
    pub schema_version: u32,
    pub id: String,
    pub label: String,
    pub version: String,
    pub publisher: String,
    pub license: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub sdk_abi: String,
    pub target: String,
    pub library: String,
    pub library_sha256: String,
}

impl ExtensionManifest {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        version: impl Into<String>,
        publisher: impl Into<String>,
        license: impl Into<String>,
        library: impl Into<String>,
        library_bytes: &[u8],
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: id.into(),
            label: label.into(),
            version: version.into(),
            publisher: publisher.into(),
            license: license.into(),
            homepage: None,
            sdk_abi: host_sdk_abi(),
            target: host_target().to_string(),
            library: library.into(),
            library_sha256: hex_digest(library_bytes),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtensionBundle {
    pub manifest: ExtensionManifest,
    pub publisher_key: String,
    pub library_base64: String,
    pub signature: String,
}

#[derive(Clone, Debug)]
pub struct VerifiedBundle {
    pub manifest: ExtensionManifest,
    pub publisher_key: Vec<u8>,
    pub library: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustDecision {
    RequireTrusted,
    TrustPublisher,
}

#[derive(Clone, Debug)]
pub struct InstalledExtension {
    pub manifest: ExtensionManifest,
    pub library_path: PathBuf,
    pub bundle_path: PathBuf,
}

#[derive(Debug)]
pub enum PackageError {
    Io(String),
    Invalid(String),
    Signature,
    Incompatible {
        expected: String,
        found: String,
    },
    TrustRequired {
        publisher: String,
        fingerprint: String,
    },
    AlreadyInstalled(String),
}

impl std::fmt::Display for PackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) | Self::Invalid(error) => write!(f, "{error}"),
            Self::Signature => write!(f, "extension signature is missing or invalid"),
            Self::Incompatible { expected, found } => {
                write!(
                    f,
                    "incompatible extension: expected {expected}, found {found}"
                )
            }
            Self::TrustRequired {
                publisher,
                fingerprint,
            } => write!(
                f,
                "publisher `{publisher}` is not trusted (key {fingerprint}); \
                 confirm trust before installing native code"
            ),
            Self::AlreadyInstalled(version) => {
                write!(f, "extension version {version} is already installed")
            }
        }
    }
}

impl std::error::Error for PackageError {}

pub fn host_sdk_abi() -> String {
    format!(
        "jackdaw-{}-bevy-{}-{}",
        env!("CARGO_PKG_VERSION"),
        "0.19",
        env!("JACKDAW_COMPILED_RUSTC")
    )
}

pub const fn host_target() -> &'static str {
    env!("JACKDAW_COMPILED_TARGET")
}

/// Generate a new Ed25519 signing key in PKCS#8 form.
///
/// Publishers should create this once, keep it private, and use the same key
/// for updates so users only make one trust decision.
pub fn generate_signing_key() -> Result<Vec<u8>, PackageError> {
    let random = ring::rand::SystemRandom::new();
    signature::Ed25519KeyPair::generate_pkcs8(&random)
        .map(|document| document.as_ref().to_vec())
        .map_err(|_| PackageError::Invalid("could not generate an Ed25519 signing key".into()))
}

/// Build a signed bundle from a native library and an Ed25519 PKCS#8 key.
pub fn create_bundle(
    mut manifest: ExtensionManifest,
    library: &[u8],
    private_key_pkcs8: &[u8],
) -> Result<Vec<u8>, PackageError> {
    validate_manifest_shape(&manifest)?;
    manifest.schema_version = SCHEMA_VERSION;
    manifest.library_sha256 = hex_digest(library);
    let key = signature::Ed25519KeyPair::from_pkcs8(private_key_pkcs8)
        .map_err(|_| PackageError::Invalid("invalid Ed25519 PKCS#8 key".into()))?;
    let message = signing_message(&manifest, library)?;
    let bundle = ExtensionBundle {
        manifest,
        publisher_key: base64::engine::general_purpose::STANDARD.encode(key.public_key().as_ref()),
        library_base64: base64::engine::general_purpose::STANDARD.encode(library),
        signature: base64::engine::general_purpose::STANDARD.encode(key.sign(&message).as_ref()),
    };
    serde_json::to_vec_pretty(&bundle)
        .map_err(|error| PackageError::Invalid(format!("encoding bundle: {error}")))
}

pub fn verify_bundle(path: &Path) -> Result<VerifiedBundle, PackageError> {
    let bytes = std::fs::read(path)
        .map_err(|error| PackageError::Io(format!("{}: {error}", path.display())))?;
    verify_bytes(&bytes)
}

/// Verify a bundle already in memory.
///
/// The signature, checksum, ABI, and target checks have nothing to do
/// with where the bytes came from, so a marketplace client that fetched
/// them over the network runs exactly the same gate a local file does.
/// This crate stays transport-free; fetching is the caller's business.
pub fn verify_bytes(bytes: &[u8]) -> Result<VerifiedBundle, PackageError> {
    let bundle: ExtensionBundle = serde_json::from_slice(bytes)
        .map_err(|error| PackageError::Invalid(format!("invalid .jdext: {error}")))?;
    validate_manifest_shape(&bundle.manifest)?;
    if bundle.manifest.sdk_abi != host_sdk_abi() {
        return Err(PackageError::Incompatible {
            expected: host_sdk_abi(),
            found: bundle.manifest.sdk_abi,
        });
    }
    if bundle.manifest.target != host_target() {
        return Err(PackageError::Incompatible {
            expected: host_target().into(),
            found: bundle.manifest.target,
        });
    }
    let decode = |value: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .map_err(|error| PackageError::Invalid(format!("invalid base64 payload: {error}")))
    };
    let library = decode(&bundle.library_base64)?;
    if hex_digest(&library) != bundle.manifest.library_sha256 {
        return Err(PackageError::Invalid(
            "library checksum does not match manifest".into(),
        ));
    }
    let publisher_key = decode(&bundle.publisher_key)?;
    let signature_bytes = decode(&bundle.signature)?;
    let message = signing_message(&bundle.manifest, &library)?;
    signature::UnparsedPublicKey::new(&signature::ED25519, &publisher_key)
        .verify(&message, &signature_bytes)
        .map_err(|_| PackageError::Signature)?;
    Ok(VerifiedBundle {
        manifest: bundle.manifest,
        publisher_key,
        library,
    })
}

pub fn install_bundle(
    path: &Path,
    decision: TrustDecision,
) -> Result<InstalledExtension, PackageError> {
    let bytes = std::fs::read(path)
        .map_err(|error| PackageError::Io(format!("{}: {error}", path.display())))?;
    install_bytes(&bytes, decision)
}

/// Install a bundle already in memory, under the same verification and
/// trust rules as a local file.
pub fn install_bytes(
    bytes: &[u8],
    decision: TrustDecision,
) -> Result<InstalledExtension, PackageError> {
    let verified = verify_bytes(bytes)?;
    let root = extension_root()?;
    std::fs::create_dir_all(&root)
        .map_err(|error| PackageError::Io(format!("{}: {error}", root.display())))?;
    let fingerprint = fingerprint(&verified.publisher_key);
    let mut trust = read_json::<TrustStore>(&trust_path()?).unwrap_or_default();
    if trust.publishers.get(&fingerprint) != Some(&verified.manifest.publisher) {
        if decision == TrustDecision::RequireTrusted {
            return Err(PackageError::TrustRequired {
                publisher: verified.manifest.publisher,
                fingerprint,
            });
        }
        trust
            .publishers
            .insert(fingerprint, verified.manifest.publisher.clone());
        write_json_atomic(&trust_path()?, &trust)?;
    }

    let id_root = root.join(&verified.manifest.id);
    let version_root = id_root.join(&verified.manifest.version);
    if version_root.exists() {
        return Err(PackageError::AlreadyInstalled(verified.manifest.version));
    }
    let staging = id_root.join(format!(
        ".stage-{}-{}",
        verified.manifest.version,
        std::process::id()
    ));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .map_err(|error| PackageError::Io(format!("{}: {error}", staging.display())))?;
    }
    std::fs::create_dir_all(&staging)
        .map_err(|error| PackageError::Io(format!("{}: {error}", staging.display())))?;
    std::fs::write(staging.join(&verified.manifest.library), &verified.library)
        .map_err(|error| PackageError::Io(format!("writing extension library: {error}")))?;
    let bundle_name = format!(
        "{}-{}.jdext",
        verified.manifest.id, verified.manifest.version
    );
    // Keep the bundle beside what it installed, so a later rollback or
    // audit has the signed original. Written from the verified bytes
    // rather than copied from a path, which a remote install has none of.
    std::fs::write(staging.join(&bundle_name), bytes)
        .map_err(|error| PackageError::Io(format!("saving extension bundle: {error}")))?;
    std::fs::rename(&staging, &version_root)
        .map_err(|error| PackageError::Io(format!("activating extension: {error}")))?;

    let mut active = read_json::<ActiveIndex>(&root.join("active.json")).unwrap_or_default();
    if let Some(previous) = active.extensions.insert(
        verified.manifest.id.clone(),
        ActiveEntry {
            version: verified.manifest.version.clone(),
            library: verified.manifest.library.clone(),
            bundle: bundle_name.clone(),
        },
    ) {
        let mut garbage = read_json::<GarbageIndex>(&root.join("garbage.json")).unwrap_or_default();
        garbage
            .paths
            .push(PathBuf::from(&verified.manifest.id).join(previous.version));
        write_json_atomic(&root.join("garbage.json"), &garbage)?;
    }
    write_json_atomic(&root.join("active.json"), &active)?;
    jackdaw_api_internal::extensions_config::set_extension_enabled(&verified.manifest.id, true);

    Ok(InstalledExtension {
        library_path: version_root.join(&verified.manifest.library),
        bundle_path: version_root.join(bundle_name),
        manifest: verified.manifest,
    })
}

pub fn uninstall(id: &str) -> Result<bool, PackageError> {
    validate_identifier(id, "extension id")?;
    let root = extension_root()?;
    let mut active = read_json::<ActiveIndex>(&root.join("active.json")).unwrap_or_default();
    let Some(previous) = active.extensions.remove(id) else {
        return Ok(false);
    };
    let mut garbage = read_json::<GarbageIndex>(&root.join("garbage.json")).unwrap_or_default();
    garbage.paths.push(PathBuf::from(id).join(previous.version));
    write_json_atomic(&root.join("garbage.json"), &garbage)?;
    write_json_atomic(&root.join("active.json"), &active)?;
    Ok(true)
}

/// Restore the most recently retired version after a failed live activation.
pub fn rollback(id: &str) -> Result<Option<InstalledExtension>, PackageError> {
    validate_identifier(id, "extension id")?;
    let root = extension_root()?;
    let garbage_path = root.join("garbage.json");
    let mut garbage = read_json::<GarbageIndex>(&garbage_path).unwrap_or_default();
    let Some(index) = garbage.paths.iter().rposition(|path| {
        is_version_path(path)
            && path
                .components()
                .next()
                .is_some_and(|part| part.as_os_str() == id)
    }) else {
        return Ok(None);
    };
    let previous_relative = garbage.paths.remove(index);
    let previous_root = root.join(&previous_relative);
    let bundle_path = std::fs::read_dir(&previous_root)
        .map_err(|error| PackageError::Io(format!("{}: {error}", previous_root.display())))?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("jdext"))
        .ok_or_else(|| {
            PackageError::Invalid(format!(
                "retired extension at {} has no bundle",
                previous_root.display()
            ))
        })?;
    let verified = verify_bundle(&bundle_path)?;
    let mut active = read_json::<ActiveIndex>(&root.join("active.json")).unwrap_or_default();
    if let Some(failed) = active.extensions.insert(
        id.to_string(),
        ActiveEntry {
            version: verified.manifest.version.clone(),
            library: verified.manifest.library.clone(),
            bundle: bundle_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string(),
        },
    ) {
        garbage.paths.push(PathBuf::from(id).join(failed.version));
    }
    write_json_atomic(&root.join("active.json"), &active)?;
    write_json_atomic(&garbage_path, &garbage)?;
    Ok(Some(InstalledExtension {
        library_path: previous_root.join(&verified.manifest.library),
        bundle_path,
        manifest: verified.manifest,
    }))
}

pub fn list_installed() -> Result<Vec<InstalledExtension>, PackageError> {
    let root = extension_root()?;
    let active = read_json::<ActiveIndex>(&root.join("active.json")).unwrap_or_default();
    let mut installed = Vec::new();
    for (id, entry) in active.extensions {
        let version_root = root.join(&id).join(&entry.version);
        let bundle_path = version_root.join(&entry.bundle);
        let verified = verify_bundle(&bundle_path)?;
        installed.push(InstalledExtension {
            manifest: verified.manifest,
            library_path: version_root.join(entry.library),
            bundle_path,
        });
    }
    Ok(installed)
}

/// Delete versions retired by a previous Jackdaw process.
pub fn garbage_collect() -> Result<usize, PackageError> {
    let root = extension_root()?;
    let path = root.join("garbage.json");
    let garbage = read_json::<GarbageIndex>(&path).unwrap_or_default();
    let mut removed = 0;
    let mut retained = Vec::new();
    for relative in garbage.paths {
        if !is_version_path(&relative) {
            continue;
        }
        let candidate = root.join(&relative);
        match std::fs::remove_dir_all(&candidate) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => retained.push(relative),
        }
    }
    write_json_atomic(&path, &GarbageIndex { paths: retained })?;
    Ok(removed)
}

fn is_version_path(path: &Path) -> bool {
    let parts: Vec<_> = path.components().collect();
    parts.len() == 2
        && parts.iter().all(|part| match part {
            std::path::Component::Normal(value) => value
                .to_str()
                .is_some_and(|value| validate_identifier(value, "extension storage path").is_ok()),
            _ => false,
        })
}

pub fn extension_root() -> Result<PathBuf, PackageError> {
    dirs::data_dir()
        .map(|path| path.join("jackdaw/extensions"))
        .ok_or_else(|| PackageError::Io("platform data directory is unavailable".into()))
}

fn trust_path() -> Result<PathBuf, PackageError> {
    dirs::config_dir()
        .map(|path| path.join("jackdaw/trusted_publishers.json"))
        .ok_or_else(|| PackageError::Io("platform config directory is unavailable".into()))
}

fn validate_manifest_shape(manifest: &ExtensionManifest) -> Result<(), PackageError> {
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(PackageError::Invalid(format!(
            "unsupported .jdext schema {}",
            manifest.schema_version
        )));
    }
    validate_identifier(&manifest.id, "extension id")?;
    validate_identifier(&manifest.version, "extension version")?;
    let library = Path::new(&manifest.library);
    if library.file_name().and_then(|value| value.to_str()) != Some(&manifest.library)
        || !matches!(
            library.extension().and_then(|value| value.to_str()),
            Some("so" | "dylib" | "dll")
        )
    {
        return Err(PackageError::Invalid(
            "manifest library must be a bare .so, .dylib, or .dll filename".into(),
        ));
    }
    if manifest.publisher.trim().is_empty() || manifest.label.trim().is_empty() {
        return Err(PackageError::Invalid(
            "manifest publisher and label must not be empty".into(),
        ));
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), PackageError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(PackageError::Invalid(format!(
            "{label} may contain only letters, numbers, dot, underscore, and hyphen"
        )));
    }
    Ok(())
}

fn signing_message(manifest: &ExtensionManifest, library: &[u8]) -> Result<Vec<u8>, PackageError> {
    let mut message = serde_json::to_vec(manifest)
        .map_err(|error| PackageError::Invalid(format!("encoding manifest: {error}")))?;
    message.push(0);
    message.extend_from_slice(library);
    Ok(message)
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn fingerprint(public_key: &[u8]) -> String {
    hex_digest(public_key)[..16].to_string()
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), PackageError> {
    let parent = path
        .parent()
        .ok_or_else(|| PackageError::Io(format!("{} has no parent", path.display())))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| PackageError::Io(format!("{}: {error}", parent.display())))?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| PackageError::Invalid(format!("encoding {}: {error}", path.display())))?;
    std::fs::write(&temporary, bytes)
        .map_err(|error| PackageError::Io(format!("{}: {error}", temporary.display())))?;
    if path.exists() {
        let backup = path.with_extension(format!("bak-{}", std::process::id()));
        std::fs::rename(path, &backup)
            .map_err(|error| PackageError::Io(format!("{}: {error}", path.display())))?;
        if let Err(error) = std::fs::rename(&temporary, path) {
            let _ = std::fs::rename(&backup, path);
            return Err(PackageError::Io(format!("{}: {error}", path.display())));
        }
        let _ = std::fs::remove_file(backup);
        Ok(())
    } else {
        std::fs::rename(&temporary, path)
            .map_err(|error| PackageError::Io(format!("{}: {error}", path.display())))
    }
}

#[derive(Default, Serialize, Deserialize)]
struct TrustStore {
    publishers: BTreeMap<String, String>,
}

#[derive(Default, Serialize, Deserialize)]
struct ActiveIndex {
    extensions: BTreeMap<String, ActiveEntry>,
}

#[derive(Serialize, Deserialize)]
struct ActiveEntry {
    version: String,
    library: String,
    bundle: String,
}

#[derive(Default, Serialize, Deserialize)]
struct GarbageIndex {
    paths: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_rejects_path_traversal() {
        let manifest = ExtensionManifest::new(
            "example.ext",
            "Example",
            "1.0.0",
            "Acme",
            "MIT",
            "../bad.so",
            b"x",
        );
        assert!(validate_manifest_shape(&manifest).is_err());
    }

    #[test]
    fn manifest_uses_exact_host_compatibility() {
        let manifest = ExtensionManifest::new(
            "example.ext",
            "Example",
            "1.0.0",
            "Acme",
            "MIT",
            "ext.so",
            b"x",
        );
        assert_eq!(manifest.sdk_abi, host_sdk_abi());
        assert_eq!(manifest.target, host_target());
    }

    #[test]
    fn signed_bundle_round_trips_and_detects_tampering() {
        let key = generate_signing_key().unwrap();
        let manifest = ExtensionManifest::new(
            "example.ext",
            "Example",
            "1.0.0",
            "Acme",
            "MIT",
            format!("ext.{}", std::env::consts::DLL_EXTENSION),
            b"native library",
        );
        let bundle = create_bundle(manifest, b"native library", &key).unwrap();
        let root =
            std::env::temp_dir().join(format!("jackdaw-signed-bundle-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("example.jdext");
        std::fs::write(&path, &bundle).unwrap();
        assert_eq!(verify_bundle(&path).unwrap().library, b"native library");

        let mut decoded: ExtensionBundle = serde_json::from_slice(&bundle).unwrap();
        decoded.manifest.label = "Tampered".into();
        std::fs::write(&path, serde_json::to_vec(&decoded).unwrap()).unwrap();
        assert!(matches!(verify_bundle(&path), Err(PackageError::Signature)));
    }

    #[test]
    fn garbage_paths_are_exactly_id_and_version() {
        assert!(is_version_path(Path::new("example.ext/1.0.0")));
        assert!(!is_version_path(Path::new("../outside")));
        assert!(!is_version_path(Path::new("example.ext/../../outside")));
        assert!(!is_version_path(Path::new("/absolute/path")));
    }
}
