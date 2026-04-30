//! Host-vs-dylib compatibility checking.
//!
//! Every dylib (extension or game) embeds an API version, a Bevy
//! version string, and a build profile (debug/release). The loader
//! refuses to bring one in unless all three match the host's
//! constants exactly. That catches the three most common reasons a
//! trait-object call across the dylib boundary would go sideways.

use core::ffi::CStr;
use std::ffi::c_char;

use jackdaw_api_internal::ffi::{
    API_VERSION, BEVY_VERSION, ExtensionEntry, FINGERPRINT_SYMBOL, GameEntry, PROFILE,
};

#[derive(Debug)]
pub enum CompatError {
    ApiVersionMismatch {
        host: u32,
        extension: u32,
    },
    BevyVersionMismatch {
        host: String,
        extension: String,
    },
    ProfileMismatch {
        host: String,
        extension: String,
    },
    NullPointer {
        field: &'static str,
    },
    NonUtf8 {
        field: &'static str,
    },
    /// The cdylib does not export the `__JACKDAW_LAYOUT_FINGERPRINT`
    /// symbol. Almost always means the dylib was built against an
    /// older `jackdaw_api` that pre-dates the fingerprint export.
    /// Rebuild against the current `jackdaw_api`.
    FingerprintMissing,
    /// The cdylib's fingerprint disagrees with the editor's. The two
    /// were compiled against different layouts for at least one
    /// boundary Bevy type. The editor must restart so it picks up
    /// the same layout the cdylib was built against.
    FingerprintMismatch {
        editor: u64,
        cdylib: u64,
    },
}

impl std::fmt::Display for CompatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiVersionMismatch { host, extension } => write!(
                f,
                "jackdaw_api ABI version mismatch: host v{host}, extension v{extension}. \
                 Rebuild the extension against jackdaw_api v{host}."
            ),
            Self::BevyVersionMismatch { host, extension } => write!(
                f,
                "Bevy version mismatch: host was built against {host}, extension against {extension}. \
                 Rebuild the extension against Bevy {host}."
            ),
            Self::ProfileMismatch { host, extension } => write!(
                f,
                "build profile mismatch: host is {host}, extension is {extension}. \
                 Rebuild the extension with the same profile as the host."
            ),
            Self::NullPointer { field } => {
                write!(f, "ExtensionEntry.{field} is null")
            }
            Self::NonUtf8 { field } => {
                write!(f, "ExtensionEntry.{field} is not valid UTF-8")
            }
            Self::FingerprintMissing => write!(
                f,
                "cdylib does not export layout fingerprint; rebuild against the current jackdaw_api"
            ),
            Self::FingerprintMismatch { editor, cdylib } => write!(
                f,
                "cdylib's layout fingerprint ({cdylib:#x}) does not match editor's ({editor:#x}). \
                 This usually means bevy or jackdaw versions changed. Restart the editor."
            ),
        }
    }
}

impl std::error::Error for CompatError {}

/// Verify every embedded version tag against the host's values and
/// sanity-check that pointer fields are non-null.
pub fn verify_compat(entry: &ExtensionEntry) -> Result<(), CompatError> {
    verify_version_fields(entry.api_version, entry.bevy_version, entry.profile)
}

/// Same as [`verify_compat`] but for a [`GameEntry`]. Both envelopes
/// share the same version-field layout, so the check itself is
/// structurally identical.
pub fn verify_game_compat(entry: &GameEntry) -> Result<(), CompatError> {
    verify_version_fields(entry.api_version, entry.bevy_version, entry.profile)
}

/// Verify the cdylib's layout fingerprint matches the editor's.
///
/// Reads the `__JACKDAW_LAYOUT_FINGERPRINT` symbol the
/// [`export_game_plugin!`](jackdaw_api_internal::export_game_plugin)
/// macro emits and compares it to the editor's compile-time
/// `jackdaw_runtime::LAYOUT_FINGERPRINT`. A missing symbol or any
/// disagreement refuses the swap. Called after the API/Bevy/profile
/// version check passes so callers see the cheaper checks fire
/// first when both would diagnose the same root cause.
pub fn verify_layout_fingerprint(lib: &libloading::Library) -> Result<(), CompatError> {
    // SAFETY: `FINGERPRINT_SYMBOL` is a NUL-terminated byte literal;
    // the symbol resolution itself is fallible and reported as an
    // `Err` rather than a panic. The returned `Symbol<*const u64>`
    // outlives this call only as long as `lib` is alive, but we
    // dereference it immediately and do not store the pointer.
    let sym: libloading::Symbol<*const u64> = match unsafe { lib.get(FINGERPRINT_SYMBOL) } {
        Ok(s) => s,
        Err(_) => return Err(CompatError::FingerprintMissing),
    };
    // SAFETY: the macro emits a `pub static u64` at this symbol, so
    // the pointer is non-null and points at a valid `u64` for the
    // lifetime of the loaded library. We only read it.
    let cdylib_fp = unsafe { **sym };
    let editor_fp = jackdaw_runtime::LAYOUT_FINGERPRINT;
    if cdylib_fp != editor_fp {
        return Err(CompatError::FingerprintMismatch {
            editor: editor_fp,
            cdylib: cdylib_fp,
        });
    }
    Ok(())
}

fn verify_version_fields(
    api_version: u32,
    bevy_version: *const c_char,
    profile: *const c_char,
) -> Result<(), CompatError> {
    if api_version != API_VERSION {
        return Err(CompatError::ApiVersionMismatch {
            host: API_VERSION,
            extension: api_version,
        });
    }

    let ext_bevy = cstr_to_string(bevy_version, "bevy_version")?;
    let host_bevy = cstr_static_string(BEVY_VERSION);
    if ext_bevy != host_bevy {
        return Err(CompatError::BevyVersionMismatch {
            host: host_bevy,
            extension: ext_bevy,
        });
    }

    let ext_profile = cstr_to_string(profile, "profile")?;
    let host_profile = cstr_static_string(PROFILE);
    if ext_profile != host_profile {
        return Err(CompatError::ProfileMismatch {
            host: host_profile,
            extension: ext_profile,
        });
    }

    Ok(())
}

/// Read a dylib-provided C string into an owned `String`. Returns
/// errors tagged with `field` for readable diagnostics.
fn cstr_to_string(ptr: *const c_char, field: &'static str) -> Result<String, CompatError> {
    if ptr.is_null() {
        return Err(CompatError::NullPointer { field });
    }
    // SAFETY: caller contract: the pointer references a
    // NUL-terminated static string embedded in the dylib. The dylib
    // is kept alive for the duration of this call.
    let cstr = unsafe { CStr::from_ptr(ptr) };
    cstr.to_str()
        .map(ToOwned::to_owned)
        .map_err(|_| CompatError::NonUtf8 { field })
}

/// Read one of our own host-side constant `CStrs` into an owned
/// `String`. The `to_str` cannot fail for the hard-coded values but
/// we still return `String` to share the comparison type with the
/// extension-side lookup.
fn cstr_static_string(cstr: &'static CStr) -> String {
    cstr.to_str().unwrap_or_default().to_owned()
}
