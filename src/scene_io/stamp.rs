//! Scene version stamp. The stamp format and the type-path migration it
//! selects live in [`jackdaw_bsn::migrate`], which every BSN parse applies.

use jackdaw_bsn::migrate::{STAMP_MARKER, STAMP_SEPARATOR};
pub use jackdaw_bsn::migrate::{SceneStamp, read_stamp};
use jackdaw_project_build::{BEVY_VERSION, VERSION};

/// Prepend the current version stamp to emitted scene text. Applied only
/// at the disk-write boundary, never to the in-memory snapshots undo and
/// tab-swap take from the same emitter.
pub fn with_stamp(body: &str) -> String {
    format!("{STAMP_MARKER}{VERSION}{STAMP_SEPARATOR}{BEVY_VERSION}\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_round_trips() {
        let stamped = with_stamp("#Cube\njackdaw_scene_types::types::Brush {}");
        assert!(stamped.starts_with(STAMP_MARKER));
        let stamp = read_stamp(&stamped).expect("stamp present");
        assert_eq!(stamp.jackdaw, VERSION);
        assert_eq!(stamp.bevy, BEVY_VERSION);
    }
}
