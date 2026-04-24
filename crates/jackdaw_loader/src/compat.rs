//! Host-vs-dylib compatibility checking.
//!
//! Every dylib (extension or game) embeds an API version, a Bevy
//! version string, and a build profile (debug/release). The loader
//! refuses to bring one in unless all three match the host's
//! constants exactly. That catches the three most common reasons a
//! trait-object call across the dylib boundary would go sideways.

use core::ffi::CStr;
use std::ffi::c_char;

use jackdaw_api_internal::ffi::{API_VERSION, BEVY_VERSION, ExtensionEntry, GameEntry, PROFILE};

#[derive(Debug)]
pub enum CompatError {
    ApiVersionMismatch { host: u32, extension: u32 },
    BevyVersionMismatch { host: String, extension: String },
    ProfileMismatch { host: String, extension: String },
    NullPointer { field: &'static str },
    NonUtf8 { field: &'static str },
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
