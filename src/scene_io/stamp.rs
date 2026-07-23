//! Scene version stamp plus type-path migration on load.
//!
//! Saved scenes carry a leading comment recording the jackdaw and Bevy
//! versions that wrote them:
//!
//! ```text
//! // jackdaw 0.19.0 | bevy 0.19
//! ```
//!
//! The BSN lexer treats it as a comment, so it never reaches the parsed
//! document; the save regenerates it and the load scans the raw text for
//! it. Its purpose is provenance: when a future jackdaw built against a
//! newer Bevy opens an older scene, the recorded Bevy version selects the
//! type-path renames to apply, so a component whose reflect path moved in
//! a later Bevy still resolves. Bevy renames land on minor bumps, and the
//! stamp records the Bevy minor, so that is the granularity migration
//! keys on.

use std::borrow::Cow;

use jackdaw_project_build::{BEVY_VERSION, VERSION};

const MARKER: &str = "// jackdaw ";
const SEPARATOR: &str = " | bevy ";

/// The versions recorded in a scene's stamp.
pub struct SceneStamp {
    /// The jackdaw version that wrote the scene, e.g. `0.19.0`.
    pub jackdaw: String,
    /// The Bevy minor the scene targeted, e.g. `0.19`.
    pub bevy: String,
}

/// Prepend the current version stamp to emitted scene text. Applied only
/// at the disk-write boundary, never to the in-memory snapshots undo and
/// tab-swap take from the same emitter.
pub fn with_stamp(body: &str) -> String {
    format!("{MARKER}{VERSION}{SEPARATOR}{BEVY_VERSION}\n{body}")
}

/// Read the stamp from the head of a scene file, if present. Absent for
/// hand-authored scenes and anything written before stamping existed;
/// those are treated as the current version (no migration).
pub fn read_stamp(text: &str) -> Option<SceneStamp> {
    let line = text.lines().next()?.trim();
    let rest = line.strip_prefix(MARKER)?;
    let (jackdaw, bevy) = rest.split_once(SEPARATOR)?;
    Some(SceneStamp {
        jackdaw: jackdaw.trim().to_string(),
        bevy: bevy.trim().to_string(),
    })
}

/// Apply type-path migrations to raw scene text for a scene written under
/// `from_bevy`. Each Bevy minor's renames apply only to scenes older than
/// that minor, so opening an old scene rewrites every rename between its
/// version and the current one. Currently a no-op: 0.19 is the baseline,
/// nothing has been renamed yet. When a later Bevy renames a reflected
/// type, add its old -> new path to `RENAMES` under that minor.
pub fn migrate_type_paths<'a>(text: &'a str, from_bevy: &str) -> Cow<'a, str> {
    let mut out = Cow::Borrowed(text);
    for (introduced_in, renames) in RENAMES {
        if !bevy_minor_lt(from_bevy, introduced_in) {
            continue;
        }
        for (old, new) in *renames {
            if out.contains(old) {
                out = Cow::Owned(out.replace(old, new));
            }
        }
    }
    out
}

/// Type-path renames grouped by the Bevy minor that introduced them. Each
/// entry rewrites a fully-qualified reflect path so an older scene's
/// components resolve against the newer registry.
///
/// Example, once Bevy 0.20 lands and moves a type:
/// ```ignore
/// ("0.20", &[("bevy_pbr::StandardMaterial", "bevy_pbr::material::StandardMaterial")]),
/// ```
const RENAMES: &[(&str, &[(&str, &str)])] = &[];

/// Is `major.minor` string `a` an older Bevy minor than `b`?
fn bevy_minor_lt(a: &str, b: &str) -> bool {
    parse_minor(a) < parse_minor(b)
}

fn parse_minor(v: &str) -> (u32, u32) {
    let mut it = v.split('.');
    let major = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_round_trips() {
        let stamped = with_stamp("#Cube\njackdaw_scene_types::types::Brush {}");
        assert!(stamped.starts_with(MARKER));
        let stamp = read_stamp(&stamped).expect("stamp present");
        assert_eq!(stamp.jackdaw, VERSION);
        assert_eq!(stamp.bevy, BEVY_VERSION);
    }

    #[test]
    fn hand_authored_scene_has_no_stamp() {
        assert!(read_stamp("#Cube\njackdaw_scene_types::types::Brush {}").is_none());
    }

    #[test]
    fn baseline_migration_is_a_noop_and_does_not_allocate() {
        let text = "bevy_pbr::StandardMaterial {}";
        let out = migrate_type_paths(text, "0.19");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, text);
    }

    #[test]
    fn minor_ordering() {
        assert!(bevy_minor_lt("0.19", "0.20"));
        assert!(!bevy_minor_lt("0.20", "0.19"));
        assert!(!bevy_minor_lt("0.19", "0.19"));
    }
}
