//! Re-export of the build pipeline's SDK path resolution
//! ([`jackdaw_project_build::sdk_paths`]) at the editor's historical
//! module path, so existing `crate::sdk_paths::*` call sites keep working
//! after the pipeline moved to its own crate.

pub use jackdaw_project_build::sdk_paths::*;
