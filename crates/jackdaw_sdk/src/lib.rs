//! SDK facade dylib shipped with Jackdaw.
//!
//! Extensions are built via `cargo rustc` with:
//!
//! ```text
//! -C prefer-dynamic
//! --extern bevy=<jackdaw>/target/debug/libjackdaw_sdk.so
//! --extern jackdaw_api=<jackdaw>/target/debug/libjackdaw_sdk.so
//! -L dependency=<jackdaw>/target/debug/deps
//! ```
//!
//! The `--extern` aliases rename this proxy as `bevy` and
//! `jackdaw_api` during compilation of the extension, so extension
//! code writes plain `use bevy::prelude::*;` and
//! `use jackdaw_api::prelude::*;`. Both resolve to this crate's
//! re-exports, which ultimately point at the shared `bevy_dylib` and
//! `jackdaw_dylib` runtimes loaded by the editor.
//!
//! Re-exports mirror `jackdaw_api`'s public surface. Editor-host
//! plumbing (loader plugin, catalog, enable/disable helpers) lives
//! behind `jackdaw_api_internal` and is deliberately not proxied.

/// Merged prelude serving both aliased names.
///
/// `use bevy::prelude::*` (aliased to `jackdaw_sdk::prelude`) and
/// `use jackdaw_api::prelude::*` (also aliased to
/// `jackdaw_sdk::prelude`) both land here. `bevy::prelude` and
/// `jackdaw_api::prelude` define a few same-named items (`Press`,
/// `Release` from `bevy_input` vs. `bevy_enhanced_input`). Extensions
/// referencing those unqualified will need to disambiguate; globbing
/// both is still the best UX since authors rarely touch the overlap.
pub mod prelude {
    // using the bevy-defined exports over the BEI-defined ones.
    pub use bevy::prelude::{Cancel, Press, Release, *};
    pub use jackdaw_api::prelude::*;
}

pub use jackdaw_api::operator;

pub use jackdaw_api::{
    ExtensionContext, ExtensionKind, ExtensionPoint, HierarchyWindow, InspectorWindow,
    JackdawExtension, MenuEntryDescriptor, PanelContext, WindowDescriptor, op, pie, runtime, scene,
};

/// Bevy root surface for extension code walking bevy paths beyond
/// the prelude. Safe to glob: none of the explicit `jackdaw_api`
/// re-exports above are items bevy defines at its root.
pub use bevy::*;
