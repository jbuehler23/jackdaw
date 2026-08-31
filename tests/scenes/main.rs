//! Scene documents: swapping, reopening, the BSN round trip, JSN
//! conversion, asset ingest and external reload.
//!
//! Each module below was its own test binary. Merged, the editor
//! links once for the theme rather than once per file.

#[path = "../util/mod.rs"]
mod util;

mod asset_ingest;
mod bsn_scene_fixpoint;
mod external_reload;
mod headless;
mod integration;
mod jsn_conversion_commit;
mod jsn_to_bsn;
mod scene_reopen;
mod scenes_swap;
