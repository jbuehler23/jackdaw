//! Publisher keys and bundle signing for `jd extension keygen` and `pack`.
//!
//! Keys are Ed25519 in the PKCS#8 v2 layout earlier releases wrote, so an
//! existing publisher key keeps producing the same signatures.

use ed25519_dalek::{Signer as _, SigningKey};
use jackdaw_package::{ExtensionManifest, PackageError};

/// DER prefix of a PKCS#8 v1 Ed25519 private key, up to the 32-byte seed.
const PKCS8_V1_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];
/// DER prefix of a PKCS#8 v2 Ed25519 key, which also carries the public key.
const PKCS8_V2_PREFIX: [u8; 16] = [
    0x30, 0x51, 0x02, 0x01, 0x01, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];
/// DER header between the seed and the public key in a v2 document.
const PKCS8_V2_PUBLIC_KEY_TAG: [u8; 3] = [0x81, 0x21, 0x00];

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

/// Encode a key as a PKCS#8 v2 document.
fn encode_pkcs8(key: &SigningKey) -> Vec<u8> {
    let mut document = PKCS8_V2_PREFIX.to_vec();
    document.extend_from_slice(key.as_bytes());
    document.extend_from_slice(&PKCS8_V2_PUBLIC_KEY_TAG);
    document.extend_from_slice(key.verifying_key().as_bytes());
    document
}

/// Read an Ed25519 key from a PKCS#8 v1 or v2 document.
fn decode_pkcs8(document: &[u8]) -> Option<SigningKey> {
    let seed_key = |rest: &[u8]| -> Option<SigningKey> {
        Some(SigningKey::from_bytes(rest.get(..32)?.try_into().ok()?))
    };
    if let Some(rest) = document.strip_prefix(&PKCS8_V1_PREFIX) {
        return if rest.len() == 32 {
            seed_key(rest)
        } else {
            None
        };
    }
    let rest = document.strip_prefix(&PKCS8_V2_PREFIX)?;
    let key = seed_key(rest)?;
    let public = rest[32..].strip_prefix(&PKCS8_V2_PUBLIC_KEY_TAG)?;
    (public == key.verifying_key().as_bytes()).then_some(key)
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

        let mut v1 = PKCS8_V1_PREFIX.to_vec();
        v1.extend_from_slice(key.as_bytes());
        assert_eq!(decode_pkcs8(&v1).unwrap().as_bytes(), key.as_bytes());

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
