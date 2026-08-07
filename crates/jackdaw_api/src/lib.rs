//! Public API for Jackdaw extensions and games.
//!
//! A thin facade over [`jackdaw_api_internal`]. Only types and
//! functions intended for third-party extension and game authors are
//! re-exported here. Editor-host plumbing (the loader plugin, the
//! catalog, enable/disable helpers, and internal component markers)
//! stays behind `jackdaw_api_internal` and is used by the editor
//! binary and by `jackdaw_loader`.
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

// Force the Jackdaw proxy dylib into every dynamic host and extension.
// This mirrors Bevy's own `bevy/dynamic_linking` facade: public API remains
// available through this crate while process-wide state lives in one shared
// runtime library.
#[cfg(feature = "dynamic_linking")]
#[expect(
    unused_imports,
    clippy::single_component_path_imports,
    reason = "this forces the shared Jackdaw runtime to be linked"
)]
use jackdaw_dylib;

// --- Extension authoring surface ---

pub use jackdaw_api_internal::{
    DefaultArea, ExtensionContext, ExtensionInputContext, ExtensionPoint, ExtensionRegistrar,
    HierarchyWindow, InspectorWindow, JackdawExtension, MenuEntryDescriptor, PanelContext,
    ToAnchorId as _, TopLevelMenu, WidgetDefinition, WidgetInstantiateContext, WidgetPreviewState,
    WidgetProperty, WidgetPropertyKind, WidgetRegistry, WidgetSlot, WindowDescriptor,
};

pub use jackdaw_api_internal::lifecycle::ExtensionKind;

/// Maps component type paths to the icon the outliner shows for entities
/// carrying them. Extensions seed it through
/// [`ExtensionContext::register_entity_icon`].
pub mod entity_icons {
    pub use jackdaw_api_internal::entity_icons::{EntityIconRegistry, registered_icon};
}

/// Inspector category registry: route components to category tabs and register
/// new tabs. The six built-in categories are pre-registered.
pub mod inspector {
    pub use jackdaw_api_internal::inspector::{InspectorCategory, InspectorRegistry};
}

/// `#[operator]` attribute macro. See [`jackdaw_api_macros`] for the
/// supported keys.
pub use jackdaw_api_macros::operator;

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
        OperatorCommandsExt, OperatorParameters, OperatorResult, OperatorSignature,
        OperatorSystemId, OperatorWorldExt, ParamSpec, RefreshOperatorButtons,
    };
}

/// Data-driven keymap presets for operator bindings.
pub mod keymap {
    pub use jackdaw_api_internal::keymap::{
        DefaultKeymap, KeymapApplyReport, KeymapPreset, PresetBinding, PresetInput, PresetPhase,
        PresetSpawnedBinding, apply_keymap_preset, key_code_from_name, key_code_name,
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

/// Format-independent scene primitives re-exported for operator parameter
/// marshalling: `PropertyValue`, `Brush`, and the other `jackdaw_scene_types`
/// types, exposed here so extension authors have one import path.
pub mod scene {
    pub use jackdaw_scene_types::*;
}

/// UI primitives an extension needs to spawn editor-style widgets:
/// `button(ButtonProps)` plus the radial quick-menu below. Kept
/// deliberately small.
pub mod ui {
    pub use jackdaw_feathers::button::{
        ButtonProps, button, operator_button, operator_button_variant,
    };
    pub use jackdaw_feathers::icons::Icon;

    /// Radial (pie) quick-menu widget. Open a ring of [`RadialMenuItem`]s at
    /// a screen anchor with [`open_radial_menu`], let the highlight follow the
    /// cursor, then [`confirm_radial_menu`] the highlighted wedge (which fires
    /// a [`RadialMenuSelect`] observer event the extension reacts to) or
    /// [`cancel_radial_menu`] to dismiss. `action` on each item is an opaque
    /// string the extension routes to its own behavior (e.g. an operator id).
    /// Add [`RadialMenuPlugin`] once if the host has not already; the editor
    /// registers it for its own mesh quick-menu.
    pub use jackdaw_widgets::{
        RadialMenuItem, RadialMenuPlugin, RadialMenuSelect, cancel_radial_menu,
        confirm_radial_menu, open_radial_menu,
    };

    /// Build inspector cards matching the editor's look (header bar + bordered body)
    /// and standard field rows.
    pub use jackdaw_feathers::inspector_card::{
        InspectorCardEntities, InspectorCardOpts, InspectorCardRemoveButton, spawn_inspector_card,
        spawn_inspector_field_row,
    };

    use crate::op::Operator;
    use std::borrow::Cow;

    /// Add a typed `ButtonProps::from_operator::<Op>()` constructor.
    /// Lives as an extension trait because [`ButtonProps`] is defined
    /// in `jackdaw_feathers`, which deliberately has no dependency on
    /// the operator API.
    pub trait ButtonPropsOpExt {
        /// Build a button bound to operator `Op`. Sets the label to
        /// `Op::LABEL` and wires the click observer to dispatch
        /// `Op::ID`.
        fn from_operator<Op: Operator>() -> Self;
        /// Set the button's icon. Shorthand for
        /// `ButtonProps::with_left_icon` when only one icon is set.
        fn icon(self, icon: Icon) -> Self;
    }

    impl ButtonPropsOpExt for ButtonProps {
        fn from_operator<Op: Operator>() -> Self {
            Self::new(Op::LABEL).call_operator(Cow::Borrowed(Op::ID))
        }
        fn icon(self, icon: Icon) -> Self {
            self.with_left_icon(icon)
        }
    }
}

/// Convenience import for extension and operator authors.
pub mod prelude {
    pub use crate::op::{
        CallOperatorError, CallOperatorSettings, ExecutionContext, Operator,
        OperatorCommandsExt as _, OperatorParameters, OperatorResult, OperatorSignature,
        OperatorSystemId, OperatorWorldExt as _, ParamSpec, RefreshOperatorButtons,
    };
    pub use crate::pie::PlayState;
    pub use crate::runtime::{GameApp, GamePlugin, GameRegistered, GameRegistry, GameSystems};
    pub use crate::{
        DefaultArea, ExtensionContext, ExtensionInputContext, ExtensionKind, ExtensionPoint,
        ExtensionRegistrar, HierarchyWindow, InspectorWindow, JackdawExtension,
        MenuEntryDescriptor, PanelContext, TopLevelMenu, WidgetDefinition,
        WidgetInstantiateContext, WidgetPreviewState, WidgetProperty, WidgetPropertyKind,
        WidgetRegistry, WidgetSlot, WindowDescriptor, operator,
    };

    /// Helper [`SystemParam`](bevy::ecs::system::SystemParam) for
    /// operators that need to read or cancel the active modal.
    pub use jackdaw_api_internal::lifecycle::ActiveModalQuery;

    /// Editor button-construction surface. The trait is in scope so
    /// `ButtonProps::from_operator::<MyOp>()` works without a manual
    /// `use jackdaw_api::ui::ButtonPropsOpExt`.
    pub use crate::ui::{ButtonProps, ButtonPropsOpExt as _, Icon, button};

    /// Radial quick-menu primitives so an extension can open its own pie
    /// menu and react to selections without an explicit `ui` import.
    pub use crate::ui::{
        RadialMenuItem, RadialMenuSelect, cancel_radial_menu, confirm_radial_menu, open_radial_menu,
    };

    /// BEI types extension authors need for `actions!` / `bindings!`
    /// and observer callbacks.
    pub use bevy_enhanced_input::prelude::*;

    /// Re-exported so manual [`Operator`] impls don't need an extra
    /// bevy import.
    pub use bevy::ecs::system::SystemId;
}
