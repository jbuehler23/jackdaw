//! Disk persistence for the active keymap preset and the user's own
//! per-operator bindings.

use std::path::PathBuf;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::types::{ActiveKeymapPreset, PresetBinding};

fn keymap_preset_path() -> Option<PathBuf> {
    jackdaw_env::paths::config_dir().map(|d| d.join("keymap_preset.json"))
}

/// Load the active keymap preset from disk. Returns the default ("classic")
/// silently if the file is absent, or with a `warn!` if the file is present
/// but cannot be parsed.
pub fn load_active_keymap_preset() -> ActiveKeymapPreset {
    let Some(path) = keymap_preset_path() else {
        return ActiveKeymapPreset::default();
    };
    if !path.is_file() {
        return ActiveKeymapPreset::default();
    }
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to read keymap preset file {}: {e}", path.display());
            return ActiveKeymapPreset::default();
        }
    };
    match serde_json::from_str::<ActiveKeymapPreset>(&data) {
        Ok(preset) => preset,
        Err(e) => {
            warn!(
                "Corrupt keymap preset file {}; falling back to default: {e}",
                path.display()
            );
            ActiveKeymapPreset::default()
        }
    }
}

/// Persist the active keymap preset to disk.
pub fn save_active_keymap_preset(preset: &ActiveKeymapPreset) {
    let Some(path) = keymap_preset_path() else {
        warn!("Could not determine config directory for keymap preset");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(preset) {
        Ok(data) => {
            if let Err(e) = std::fs::write(&path, data) {
                warn!("Failed to write keymap preset file: {e}");
            }
        }
        Err(e) => {
            warn!("Failed to serialize keymap preset: {e}");
        }
    }
}

/// The user's own bindings, layered over the shipped defaults by
/// [`super::resolve_keymap`]. Only rebound operators appear here, which is what
/// makes a reset a deletion rather than a copy of the defaults.
///
/// Persisted as JSON at [`jackdaw_env::paths::keymap_path`], written only from
/// the settings dialog's own diff, so a row naming an operator this session
/// cannot resolve still survives.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserKeymap {
    #[serde(default)]
    pub bindings: Vec<PresetBinding>,
}

/// What was wrong with the keymap file on disk, if anything.
///
/// Loading falls back to an empty keymap, which is quiet enough to look like
/// nothing was ever saved, so the dialog says it out loud from here.
#[derive(Resource, Clone, Debug, PartialEq, Eq, Default)]
pub struct KeymapLoadProblem {
    /// Empty when the file loaded, or was not there.
    pub message: String,
}

impl KeymapLoadProblem {
    pub fn is_some(&self) -> bool {
        !self.message.is_empty()
    }
}

/// Where a keymap that would not parse is put, beside the file it came from, so
/// it can still be rescued by hand.
///
/// `keymap.json.invalid`, then `.invalid.2` and on, so a second corruption does
/// not destroy the rescue of the first.
fn rescue_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".invalid");
    let first = path.with_file_name(name);
    if !first.exists() {
        return first;
    }
    for counter in 2..1000u32 {
        let mut name = first.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".{counter}"));
        let candidate = first.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

/// Moves the unreadable file at `path` out of the way, so the next save writes
/// a new file rather than over one nobody has read.
///
/// A rename, falling back to a copy and an unlink across filesystems. If even
/// that fails the original is left exactly as it was, and
/// [`save_user_keymap`] then refuses to write over it.
///
/// Returns where the file was kept, or why it could not be kept.
fn rescue_unreadable(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let kept = rescue_path(path);
    match std::fs::rename(path, &kept) {
        Ok(()) => Ok(kept),
        Err(rename) => copy_aside(path, &kept, &rename),
    }
}

/// The rename's fallback: copy the bytes to `kept` and take `path` away.
///
/// Removing rather than emptying, since an emptied file would be rescued again
/// every launch and climb the `.invalid.N` counter until the first rescue was
/// overwritten. Emptying is the last resort, for a directory that allows a
/// write but not an unlink.
fn copy_aside(
    path: &std::path::Path,
    kept: &std::path::Path,
    rename: &std::io::Error,
) -> Result<std::path::PathBuf, String> {
    if let Err(copy) = std::fs::copy(path, kept) {
        return Err(format!("{rename}; and copying it aside failed too: {copy}"));
    }
    if std::fs::remove_file(path).is_ok() {
        return Ok(kept.to_path_buf());
    }
    match std::fs::write(path, "") {
        Ok(()) => Ok(kept.to_path_buf()),
        Err(empty) => Err(format!(
            "{rename}; the copy was kept but the original could not be cleared: {empty}"
        )),
    }
}

/// What the user is told once the file has been dealt with.
fn rescue_message(path: &std::path::Path, kept: Result<std::path::PathBuf, String>) -> String {
    match kept {
        Ok(kept) => format!(
            "{} could not be read and was kept as {}; the shipped chords are in use.",
            path.display(),
            kept.display()
        ),
        Err(why) => format!(
            "{} could not be read and could not be moved aside ({why}); \
             the shipped chords are in use, and saving will not write over it.",
            path.display()
        ),
    }
}

/// Reads the user keymap from disk. An absent file is an empty keymap; an
/// unreadable or corrupt one warns, is moved aside, and is treated as empty.
pub fn load_user_keymap() -> UserKeymap {
    load_user_keymap_reporting().0
}

/// [`load_user_keymap`] with what went wrong, for the dialog.
pub fn load_user_keymap_reporting() -> (UserKeymap, KeymapLoadProblem) {
    let none = KeymapLoadProblem::default();
    let Some(path) = jackdaw_env::paths::keymap_path() else {
        return (UserKeymap::default(), none);
    };
    if !path.is_file() {
        return (UserKeymap::default(), none);
    }
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to read keymap file {}: {e}", path.display());
            let kept = rescue_unreadable(&path);
            return (
                UserKeymap::default(),
                KeymapLoadProblem {
                    message: rescue_message(&path, kept),
                },
            );
        }
    };
    // An empty file is a keymap with nothing in it, not a corrupt one; reading
    // it as a parse failure would rescue it again every launch.
    if data.trim().is_empty() {
        return (UserKeymap::default(), none);
    }
    match serde_json::from_str::<UserKeymap>(&data) {
        Ok(keymap) => (keymap, none),
        Err(e) => {
            warn!(
                "Corrupt keymap file {}; ignoring user bindings: {e}",
                path.display()
            );
            let kept = rescue_unreadable(&path);
            (
                UserKeymap::default(),
                KeymapLoadProblem {
                    message: rescue_message(&path, kept),
                },
            )
        }
    }
}

/// Writes the user keymap to disk.
///
/// Refuses while a file nobody could read is still sitting there, since the
/// rescue could not move it and a whole-file write would destroy it. The error
/// is a sentence the caller can put in front of the user: a refusal means the
/// rebinds live only until the process ends.
#[must_use = "a refused save leaves the rebinds unwritten, and the user has to be told"]
pub fn save_user_keymap(keymap: &UserKeymap) -> Result<(), String> {
    let Some(path) = jackdaw_env::paths::keymap_path() else {
        return Err(fail("there is no config directory to write the keymap to"));
    };
    if unrescued(&path) {
        return Err(fail(format!(
            "{} could not be read and could not be moved aside",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let data = match serde_json::to_string_pretty(keymap) {
        Ok(data) => data,
        Err(e) => return Err(fail(format!("the keymap could not be serialized: {e}"))),
    };
    match std::fs::write(&path, data) {
        Ok(()) => Ok(()),
        Err(e) => Err(fail(format!(
            "{} could not be written: {e}",
            path.display()
        ))),
    }
}

/// Log the reason a save refused and hand it back for the caller to show.
fn fail(reason: impl Into<String>) -> String {
    let reason = reason.into();
    warn!("Not writing the keymap: {reason}");
    reason
}

/// Whether what is at `path` is a file this build could not read and the rescue
/// could not move.
fn unrescued(path: &std::path::Path) -> bool {
    match std::fs::read_to_string(path) {
        Ok(data) => !data.trim().is_empty() && serde_json::from_str::<UserKeymap>(&data).is_err(),
        // An absent file is nothing to lose; an unreadable one is.
        Err(_) => path.is_file(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "jackdaw_keymap_rescue_{}_{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory for the rescue");
        dir
    }

    /// The fallback a failed rename takes: emptying the original instead of
    /// removing it would have it rescued again every launch.
    #[test]
    fn the_copy_fallback_leaves_nothing_behind_to_be_rescued_again() {
        let dir = temp_dir("fallback");
        let path = dir.join("keymap.json");
        let kept = dir.join("keymap.json.invalid");
        std::fs::write(&path, "{ this is not json").expect("a corrupt keymap");
        let rename = std::io::Error::other("rename refused");

        let out = copy_aside(&path, &kept, &rename).expect("the bytes were copied aside");

        assert_eq!(out, kept);
        assert_eq!(
            std::fs::read_to_string(&kept).expect("the copy is there"),
            "{ this is not json",
            "the bytes were kept byte for byte",
        );
        assert!(
            !path.exists(),
            "and the original is gone, not sitting there empty",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file with nothing in it is a keymap with no overrides, not a corrupt
    /// one.
    #[test]
    fn an_empty_file_is_not_a_file_worth_rescuing() {
        let dir = temp_dir("empty");
        let path = dir.join("keymap.json");
        std::fs::write(&path, "").expect("an empty keymap");
        assert!(!unrescued(&path));
        std::fs::write(&path, "\n  \n").expect("a blank keymap");
        assert!(!unrescued(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
