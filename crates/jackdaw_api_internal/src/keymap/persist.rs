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
/// [`super::resolve_keymap`]. Only operators the user has actually
/// rebound appear here: an operator with no row keeps every default
/// row it was registered with, which is what makes a "reset" a
/// deletion rather than a copy of the defaults.
///
/// Persisted as JSON at [`jackdaw_env::paths::keymap_path`]. The file
/// is written only from the settings dialog's own diff, never from an
/// applied keymap, so an entry naming an operator that is not loaded
/// right now survives the sessions that cannot resolve it.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserKeymap {
    #[serde(default)]
    pub bindings: Vec<PresetBinding>,
}

/// What was wrong with the keymap file on disk, if anything.
///
/// Loading falls back to an empty keymap so a bad file costs the user
/// their overrides and not the editor's input. That is quiet enough to
/// look like the overrides were never saved, so what happened is recorded
/// here for the dialog to say out loud.
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

/// Where a keymap that would not parse is put, beside the file it came
/// from, so the overrides in it can still be read and rescued by hand.
///
/// `keymap.json.invalid` when nothing is there, and `.invalid.2`,
/// `.invalid.3` and so on when something is: a second corruption must not
/// destroy the rescue of the first, which is the one thing this whole path
/// exists to prevent.
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

/// Move the unreadable file at `path` out of the way, so the next Save
/// writes a new file rather than over the one nobody has read yet.
///
/// A rename is the whole of it when it works. When it does not -- a
/// read-only directory, or a rescue path on another filesystem -- the
/// bytes are copied and the original is emptied instead, which leaves the
/// same two facts on disk: a copy that can still be rescued by hand, and
/// nothing at `path` worth losing. If even that fails the original is left
/// exactly as it was and the failure is reported, because a half-moved
/// file is worse than an unmoved one; [`save_user_keymap`] then refuses to
/// write over it.
///
/// Returns where the file was kept, or why it could not be kept.
fn rescue_unreadable(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let kept = rescue_path(path);
    let rename = match std::fs::rename(path, &kept) {
        Ok(()) => return Ok(kept),
        Err(error) => error,
    };
    match std::fs::copy(path, &kept).and_then(|_| std::fs::write(path, "")) {
        Ok(()) => Ok(kept),
        Err(copy) => Err(format!("{rename}; and copying it aside failed too: {copy}")),
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

/// Read the user keymap from disk. An absent file is an empty keymap;
/// an unreadable or corrupt one warns and is treated as empty.
///
/// Either way the file is moved aside first. The next Save writes the
/// whole file, so leaving one that nobody could read in place would
/// silently destroy whatever was in it -- and a file that cannot be read
/// at all is no safer than one that will not parse.
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

/// Write the user keymap to disk.
///
/// Refuses while a file nobody could read is still sitting there: loading
/// moves such a file aside, so one that is still in place is one the
/// rescue could not move, and writing the whole file over it is exactly
/// the loss the rescue exists to prevent.
///
/// `Ok` once the file holds the keymap, and otherwise why it does not, in a
/// sentence the caller can put in front of the user: a refusal here means the
/// rebinds live only until the process ends, which nobody finds out about by
/// reading the log.
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

/// Whether what is at `path` is a file this build could not read and the
/// rescue could not move.
fn unrescued(path: &std::path::Path) -> bool {
    match std::fs::read_to_string(path) {
        Ok(data) => !data.is_empty() && serde_json::from_str::<UserKeymap>(&data).is_err(),
        // An absent file is nothing to lose; an unreadable one is.
        Err(_) => path.is_file(),
    }
}
