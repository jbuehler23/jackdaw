//! Where the SDK is found and what a project resolves against: the
//! resolution matrix, the feature closure, template versions and dylib
//! loading.
//!
//! Each module below was its own test binary. Merged, the editor
//! links once for the theme rather than once per file.

#[path = "../util/mod.rs"]
mod util;

mod dylib_loading;
mod sdk_feature_closure;
mod sdk_resolution_matrix;
mod template_versions;
