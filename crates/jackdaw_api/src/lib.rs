//! Public API for Jackdaw extensions and games.
//!
//! A thin facade over [`jackdaw_api_internal`]. Only types and
//! functions intended for third-party extension and game authors are
//! re-exported here. Editor-host plumbing (the loader plugin, the
//! catalog, enable/disable helpers, internal component markers, and
//! the FFI entry structs) stays behind `jackdaw_api_internal` and is
//! used by the editor binary and by `jackdaw_loader`.
//!
//! # Static consumer
//!
//! ```toml
//! jackdaw_api = "0.4"
//! ```
//!
//! # Dylib extension
//!
//! ```toml
//! jackdaw_api = { version = "0.4", features = ["dynamic_linking"] }
//! bevy = "0.18"
//! ```
//!
//! The host binary must also enable jackdaw's `dylib` feature so the
//! editor and loaded dylibs share one compilation of the shared types.

// Links against the shared proxy dylib so the editor and every
// loaded extension share one compilation of the types that cross
// the FFI boundary. Mirrors how `bevy/dynamic_linking` pulls in
// `bevy_dylib`.
#[cfg(feature = "dynamic_linking")]
#[expect(unused_imports)]
use jackdaw_dylib as _;

// --- Extension authoring surface ---

pub use jackdaw_api_internal::{
    ExtensionContext, ExtensionPoint, HierarchyWindow, InspectorWindow, JackdawExtension,
    MenuEntryDescriptor, PanelContext, SectionBuildFn, WindowDescriptor,
};

pub use jackdaw_api_internal::lifecycle::ExtensionKind;

/// `#[operator]` attribute macro. See [`jackdaw_api_macros`] for the
/// supported keys.
pub use jackdaw_api_macros::operator;

/// Emit the FFI entry symbol a dylib extension needs.
pub use jackdaw_api_internal::export_extension;

/// Emit the FFI entry symbol a dylib game needs.
pub use jackdaw_api_internal::export_game;

// --- Sub-modules (curated) ---

/// Operator trait, dispatch API, and result types.
///
/// Used both to declare operators (via the [`Operator`](op::Operator)
/// trait, which the [`operator`](macro@crate::operator) attribute macro
/// implements) and to call them from UI code, keybinds, or other
/// operators (via [`OperatorWorldExt`](op::OperatorWorldExt) and
/// [`OperatorCommandsExt`](op::OperatorCommandsExt)).
pub mod op {
    pub use jackdaw_api_internal::operator::{
        CallOperatorError, CallOperatorSettings, ExecutionContext, Operator, OperatorCallBuilder,
        OperatorCommandsExt, OperatorParameters, OperatorResult, OperatorSystemId,
        OperatorWorldExt,
    };
}

/// Play-In-Editor state shared by the editor and loaded games.
pub mod pie {
    pub use jackdaw_api_internal::pie::PlayState;
}

/// Hot-reloadable game plugin surface. Games implement
/// [`GamePlugin`](runtime::GamePlugin) and register their systems
/// through [`GameApp`](runtime::GameApp).
pub mod runtime {
    pub use jackdaw_api_internal::runtime::{
        GameApp, GamePlugin, GameRegistered, GameRegistry, GameSystems, IntoObserverSystemBoxed,
    };
}

/// JSN primitives re-exported for operator parameter marshalling.
pub use jackdaw_jsn as jsn;

/// Convenience import for extension and operator authors.
pub mod prelude {
    pub use crate::op::{
        CallOperatorError, CallOperatorSettings, ExecutionContext, Operator,
        OperatorCommandsExt as _, OperatorParameters, OperatorResult, OperatorSystemId,
        OperatorWorldExt as _,
    };
    pub use crate::pie::PlayState;
    pub use crate::runtime::{GameApp, GamePlugin, GameRegistered, GameRegistry, GameSystems};
    pub use crate::{
        ExtensionContext, ExtensionKind, ExtensionPoint, HierarchyWindow, InspectorWindow,
        JackdawExtension, MenuEntryDescriptor, PanelContext, SectionBuildFn, WindowDescriptor,
        operator,
    };

    /// Helper [`SystemParam`](bevy::ecs::system::SystemParam) for
    /// operators that need to read or cancel the active modal.
    pub use jackdaw_api_internal::lifecycle::ActiveModalQuery;

    /// BEI types extension authors need for `actions!` / `bindings!`
    /// and observer callbacks.
    pub use bevy_enhanced_input::prelude::*;

    /// Re-exported so manual [`Operator`] impls don't need an extra
    /// bevy import.
    pub use bevy::ecs::system::SystemId;
}
