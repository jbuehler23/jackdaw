//! Detached Ed25519 signatures over release index files.
//!
//! A signed file `releases.json` is published beside `releases.json.sig`,
//! which holds the 64 raw signature bytes over the file's exact bytes.

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};

/// The public half of the key the release workflow signs the official index
/// with, as 64 hex digits, or `None` in a build made before that key was
/// provisioned, which then trusts only a key supplied through the
/// environment.
pub const OFFICIAL_INDEX_KEY: Option<&str> = None;

/// Length of a `.sig` file.
pub const SIGNATURE_LEN: usize = 64;

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

/// Why a signed file was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureError {
    /// No key to check against.
    NoTrustedKey,
    /// The key text is not 64 hex digits.
    BadKey,
    /// The signature is malformed or does not match the bytes.
    Mismatch,
}

impl std::fmt::Display for SignatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTrustedKey => write!(
                f,
                "no official release index is configured in this build yet"
            ),
            Self::BadKey => write!(f, "the release index key is not 64 hexadecimal digits"),
            Self::Mismatch => write!(f, "the signature does not match"),
        }
    }
}

impl std::error::Error for SignatureError {}

/// The key to verify the index with: `override_hex` when given, otherwise
/// [`OFFICIAL_INDEX_KEY`].
pub fn trusted_key(override_hex: Option<&str>) -> Result<VerifyingKey, SignatureError> {
    let text = override_hex
        .or(OFFICIAL_INDEX_KEY)
        .ok_or(SignatureError::NoTrustedKey)?;
    let bytes = parse_hex_key(text)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| SignatureError::BadKey)
}

/// Check `signature` (the raw contents of a `.sig` file) over `bytes`.
pub fn verify_detached(
    key: &VerifyingKey,
    bytes: &[u8],
    signature: &[u8],
) -> Result<(), SignatureError> {
    let signature = Signature::from_slice(signature).map_err(|_| SignatureError::Mismatch)?;
    key.verify_strict(bytes, &signature)
        .map_err(|_| SignatureError::Mismatch)
}

/// Sign `bytes`, returning the contents of their `.sig` file.
pub fn sign_detached(key: &SigningKey, bytes: &[u8]) -> [u8; 64] {
    key.sign(bytes).to_bytes()
}

/// A public key as lowercase hex, the form [`trusted_key`] reads.
pub fn key_hex(key: &VerifyingKey) -> String {
    crate::hex(key.as_bytes())
}

fn parse_hex_key(text: &str) -> Result<[u8; 32], SignatureError> {
    let text = text.trim();
    if text.len() != 64 || !text.is_ascii() {
        return Err(SignatureError::BadKey);
    }
    let mut key = [0; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| SignatureError::BadKey)?;
    }
    Ok(key)
}

/// Encode a key as a PKCS#8 v2 document.
pub fn encode_pkcs8(key: &SigningKey) -> Vec<u8> {
    let mut document = PKCS8_V2_PREFIX.to_vec();
    document.extend_from_slice(key.as_bytes());
    document.extend_from_slice(&PKCS8_V2_PUBLIC_KEY_TAG);
    document.extend_from_slice(key.verifying_key().as_bytes());
    document
}

/// Read an Ed25519 key from a PKCS#8 v1 or v2 document.
pub fn decode_pkcs8(document: &[u8]) -> Option<SigningKey> {
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
    use super::*;

    fn test_key() -> SigningKey {
        SigningKey::from_bytes(&[7; 32])
    }

    #[test]
    fn signed_bytes_verify_and_altered_bytes_do_not() {
        let key = test_key();
        let signature = sign_detached(&key, b"{\"schema\":1}");
        let public = key.verifying_key();
        assert_eq!(
            verify_detached(&public, b"{\"schema\":1}", &signature),
            Ok(())
        );
        assert_eq!(
            verify_detached(&public, b"{\"schema\":2}", &signature),
            Err(SignatureError::Mismatch)
        );
        assert_eq!(
            verify_detached(&public, b"{\"schema\":1}", &signature[..10]),
            Err(SignatureError::Mismatch)
        );
    }

    #[test]
    fn a_signature_from_another_key_is_rejected() {
        let signature = sign_detached(&SigningKey::from_bytes(&[9; 32]), b"index");
        assert_eq!(
            verify_detached(&test_key().verifying_key(), b"index", &signature),
            Err(SignatureError::Mismatch)
        );
    }

    #[test]
    fn trusted_key_reads_hex_and_rejects_malformed_text() {
        let public = test_key().verifying_key();
        assert_eq!(trusted_key(Some(&key_hex(&public))), Ok(public));
        assert_eq!(trusted_key(Some("abc")), Err(SignatureError::BadKey));
        assert_eq!(
            trusted_key(Some(&"zz".repeat(32))),
            Err(SignatureError::BadKey)
        );
    }

    #[test]
    fn pkcs8_documents_round_trip() {
        let key = test_key();
        let document = encode_pkcs8(&key);
        assert_eq!(document.len(), 83);
        assert_eq!(decode_pkcs8(&document).unwrap().as_bytes(), key.as_bytes());

        let mut v1 = PKCS8_V1_PREFIX.to_vec();
        v1.extend_from_slice(key.as_bytes());
        assert_eq!(decode_pkcs8(&v1).unwrap().as_bytes(), key.as_bytes());

        let mut mismatched = document.clone();
        *mismatched.last_mut().unwrap() ^= 1;
        assert!(decode_pkcs8(&mismatched).is_none());
        assert!(decode_pkcs8(&document[..40]).is_none());
    }
}
