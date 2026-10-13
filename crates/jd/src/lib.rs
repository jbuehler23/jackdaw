//! The library behind `jd`, the Jackdaw command-line tool.
//!
//! It holds what the editor deliberately does not link: https downloads
//! (and so TLS) and publisher signing keys. The editor reaches this
//! functionality by running the `jd` installed beside it, so a download or
//! an install that replaces editor files runs in its own process.
//!
//! It also installs and removes editors: see [`editors`].

pub mod archive;
pub mod editor_cli;
pub mod editors;
pub mod fetch;
pub mod self_update;
pub mod sign;
