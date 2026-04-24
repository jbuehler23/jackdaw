use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;

/// Catalog name of the Core extension. Exported so
/// [`crate::extensions_config::REQUIRED_EXTENSIONS`] and the
/// Extensions dialog can refer to it without duplicating the
/// literal string.
pub const CORE_EXTENSION_ID: &str = "jackdaw.core";

pub(super) fn plugin(app: &mut App) {
    app.register_extension::<JackdawCoreExtension>();
}

#[derive(Default)]
pub struct JackdawCoreExtension;

impl JackdawExtension for JackdawCoreExtension {
    fn id(&self) -> String {
        CORE_EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Jackdaw Core Functionality".to_string()
    }

    fn description(&self) -> String {
        "Important functionality for the Jackdaw editor. This extension is always loaded and cannot be disabled.".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.entity_mut().insert((
            CoreExtensionInputContext,
            actions!(
                CoreExtensionInputContext[(
                    Action::<CancelModalOp>::new(),
                    bindings!((KeyCode::Escape, Press::default()))
                )]
            ),
        ));
        ctx.register_operator::<CancelModalOp>();
        ctx.register_operator::<crate::asset_browser::ApplyTextureOp>();
        crate::draw_brush::add_to_extension(ctx);
    }

    fn register_input_context(&self, app: &mut App) {
        app.add_input_context::<CoreExtensionInputContext>();
    }
}

#[derive(Component, Default)]
pub struct CoreExtensionInputContext;

#[operator(
    id = "modal.cancel",
    label = "Cancel Tool",
    description = "Cancels the currently active tool",
    allows_undo = false,
    is_available = is_any_modal_active
)]
fn cancel_modal(_: In<OperatorParameters>, mut active: ActiveModalQuery) -> OperatorResult {
    active.cancel();
    OperatorResult::Finished
}

fn is_any_modal_active(active: ActiveModalQuery) -> bool {
    active.is_modal_running()
}
