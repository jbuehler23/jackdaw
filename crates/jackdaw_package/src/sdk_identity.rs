//! Which SDK build an extension library was compiled against.
//!
//! An extension is built as a `cdylib`, which exports only its entry points
//! and carries no crate metadata. So the generated shim embeds the SDK's
//! crate hash (its SVH) after [`MARKER`], and a library is matched to an
//! editor by reading those bytes from the file before it is opened: a
//! library built against another SDK never runs a single initializer.

/// The bytes that precede the SDK hash inside an extension library.
pub const MARKER: &[u8] = b"jackdaw-sdk-svh:";

/// The static the shim declares to carry the SDK hash `svh`, followed by an
/// exported accessor that keeps it in the linked library.
pub fn shim_source(svh: &str) -> String {
    let marker = std::str::from_utf8(MARKER).unwrap_or_default();
    let bytes = format!("{marker}{svh}\0");
    format!(
        r#"
static JACKDAW_SDK: [u8; {len}] = *b"{escaped}";

#[unsafe(no_mangle)]
pub fn jackdaw_extension_sdk() -> &'static [u8] {{
    &JACKDAW_SDK
}}
"#,
        len = bytes.len(),
        escaped = bytes.replace('\0', "\\0"),
    )
}

/// The SDK hash embedded in the library bytes `library`, if any.
pub fn embedded(library: &[u8]) -> Option<String> {
    library
        .windows(MARKER.len())
        .enumerate()
        .filter(|(_, window)| *window == MARKER)
        .find_map(|(start, _)| {
            let rest = &library[start + MARKER.len()..];
            let end = rest.iter().position(|&byte| byte == 0)?;
            let svh = &rest[..end];
            (!svh.is_empty() && svh.iter().all(u8::is_ascii_alphanumeric))
                .then(|| String::from_utf8_lossy(svh).into_owned())
        })
}

/// Why an extension library does not match the running editor's SDK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    /// The library names no SDK: it was built before extensions carried one.
    Unmarked,
    /// The library was built against another SDK build.
    Other { built: String, running: String },
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unmarked => write!(
                f,
                "it was built by an older jackdaw; rebuild the extension with this editor"
            ),
            Self::Other { built, running } => write!(
                f,
                "it was built against another jackdaw SDK (built {built}, running {running}); \
                 rebuild the extension with this editor"
            ),
        }
    }
}

impl std::error::Error for Mismatch {}

/// Check that `library` was built against the SDK whose hash is `running`.
pub fn check(library: &[u8], running: &str) -> Result<(), Mismatch> {
    match embedded(library) {
        None => Err(Mismatch::Unmarked),
        Some(built) if built == running => Ok(()),
        Some(built) => Err(Mismatch::Other {
            built,
            running: running.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(svh: &str) -> Vec<u8> {
        let mut bytes = b"\x7fELF padding jackdaw-sdk-svh:not hex!\0 more".to_vec();
        bytes.extend_from_slice(MARKER);
        bytes.extend_from_slice(svh.as_bytes());
        bytes.extend_from_slice(b"\0trailing");
        bytes
    }

    #[test]
    fn the_embedded_hash_is_read_past_lookalike_text() {
        assert_eq!(embedded(&library("3f2a9c")).as_deref(), Some("3f2a9c"));
    }

    #[test]
    fn a_library_for_this_sdk_passes() {
        assert_eq!(check(&library("3f2a9c"), "3f2a9c"), Ok(()));
    }

    #[test]
    fn a_library_for_another_sdk_is_refused() {
        let refused = check(&library("3f2a9c"), "77aa01").unwrap_err();
        assert_eq!(
            refused,
            Mismatch::Other {
                built: "3f2a9c".into(),
                running: "77aa01".into()
            }
        );
        assert!(refused.to_string().contains("rebuild"));
    }

    #[test]
    fn a_library_from_before_the_hash_is_told_to_rebuild() {
        let refused = check(b"\x7fELF an old extension", "77aa01").unwrap_err();
        assert_eq!(refused, Mismatch::Unmarked);
        assert!(refused.to_string().contains("older jackdaw"));
    }

    #[test]
    fn the_shim_source_carries_the_marker_and_hash() {
        let source = shim_source("3f2a9c");
        assert!(
            source.contains(r#"*b"jackdaw-sdk-svh:3f2a9c\0""#),
            "{source}"
        );
        assert!(source.contains("[u8; 23]"), "{source}");
        assert!(source.contains("jackdaw_extension_sdk"), "{source}");
    }
}
