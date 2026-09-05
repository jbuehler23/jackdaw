//! Focused authoring surface for native Jackdaw editor extensions.
//!
//! Game runtime and editor-host plumbing are intentionally excluded.

pub use jackdaw_api::{
    ExtensionInputContext, ExtensionKind, ExtensionPoint, ExtensionRegistrar, JackdawExtension,
    MenuEntryDescriptor, PanelContext, TopLevelMenu, WidgetDefinition, WidgetInstantiateContext,
    WidgetPreviewState, WidgetProperty, WidgetPropertyKind, WidgetRegistry, WidgetSlot,
    WindowDescriptor, keymap, op, scene, ui,
};
pub use jackdaw_api_macros::extension_operator as operator;

#[doc(hidden)]
pub mod __private {
    pub use bevy_enhanced_input::prelude::InputAction;
}

pub mod prelude {
    pub use jackdaw_api::keymap::{PresetInput, PresetPhase};
    pub use jackdaw_api::op::{OperatorCommandsExt as _, OperatorWorldExt as _};
    pub use jackdaw_api::prelude::{
        ButtonProps, CallOperatorError, CallOperatorSettings, ExecutionContext,
        ExtensionInputContext, ExtensionKind, ExtensionPoint, ExtensionRegistrar, Icon,
        JackdawExtension, MenuEntryDescriptor, Operator, OperatorParameters, OperatorResult,
        OperatorSignature, OperatorSystemId, PanelContext, ParamSpec, RefreshOperatorButtons,
        TopLevelMenu, WidgetDefinition, WidgetInstantiateContext, WidgetPreviewState,
        WidgetProperty, WidgetPropertyKind, WidgetRegistry, WidgetSlot, WindowDescriptor, button,
    };
    pub use jackdaw_api::ui::ButtonPropsOpExt as _;
    pub use jackdaw_api_macros::extension_operator as operator;
}
