//! Publisher keys and bundle signing for `jd extension keygen` and `pack`.
//!
//! Keys are Ed25519 in the PKCS#8 v2 layout earlier releases wrote, so an
//! existing publisher key keeps producing the same signatures.

use ed25519_dalek::{Signer as _, SigningKey};
use jackdaw_package::{ExtensionManifest, PackageError};
use jackdaw_releases::signing::{decode_pkcs8, encode_pkcs8};

/// Generate a new Ed25519 signing key in PKCS#8 form.
///
/// Publishers should create this once, keep it private, and use the same key
/// for updates so users only make one trust decision.
pub fn generate_signing_key() -> Result<Vec<u8>, PackageError> {
    let mut seed = [0; 32];
    getrandom::fill(&mut seed)
        .map_err(|_| PackageError::Invalid("could not generate an Ed25519 signing key".into()))?;
    Ok(encode_pkcs8(&SigningKey::from_bytes(&seed)))
}

/// Build a signed bundle from a native library and an Ed25519 PKCS#8 key.
pub fn create_bundle(
    manifest: ExtensionManifest,
    library: &[u8],
    private_key_pkcs8: &[u8],
) -> Result<Vec<u8>, PackageError> {
    let key = decode_pkcs8(private_key_pkcs8)
        .ok_or_else(|| PackageError::Invalid("invalid Ed25519 PKCS#8 key".into()))?;
    jackdaw_package::encode_bundle(
        manifest,
        library,
        key.verifying_key().as_bytes(),
        |message| key.sign(message).to_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use jackdaw_package::{ExtensionBundle, PackageError, verify_bytes};

    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../jackdaw_package/tests/fixtures");
        std::fs::read(dir.join(name)).unwrap()
    }

    #[test]
    fn existing_publisher_key_reproduces_its_signature() {
        let key = fixture("publisher.pk8");
        assert_eq!(encode_pkcs8(&decode_pkcs8(&key).unwrap()), key);

        let original: ExtensionBundle = serde_json::from_slice(&fixture("signed.jdext")).unwrap();
        let resigned = create_bundle(original.manifest, &fixture("library.bin"), &key).unwrap();
        assert_eq!(resigned, fixture("signed.jdext"));
    }

    #[test]
    fn generated_keys_round_trip_through_pkcs8() {
        let document = generate_signing_key().unwrap();
        assert_eq!(document.len(), 83);
        let key = decode_pkcs8(&document).unwrap();
        assert_eq!(encode_pkcs8(&key), document);

        let mut mismatched = document.clone();
        *mismatched.last_mut().unwrap() ^= 1;
        assert!(decode_pkcs8(&mismatched).is_none());
        assert!(decode_pkcs8(&document[..40]).is_none());
    }

    #[test]
    fn packed_bundles_verify_and_detect_tampering() {
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
        assert_eq!(verify_bytes(&bundle).unwrap().library, b"native library");

        let mut decoded: ExtensionBundle = serde_json::from_slice(&bundle).unwrap();
        decoded.manifest.label = "Tampered".into();
        let tampered = serde_json::to_vec(&decoded).unwrap();
        assert!(matches!(
            verify_bytes(&tampered),
            Err(PackageError::Signature)
        ));
    }
}
