use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;
use jackdaw_feathers::button::{ButtonClickEvent, ButtonOperatorCall, ButtonProps};

/// Build a [`ButtonProps`] from an operator type, filling in the
/// label and the click dispatch in one step.
///
/// `ButtonProps::from_operator::<MyOp>()` is the preferred form for
/// editor toolbars and menus when the button text matches the
/// operator's `LABEL`. For anything custom (icon-only buttons, a
/// non-`LABEL` caption), keep using `ButtonProps::new(...).call_operator(id)`.
pub trait ButtonPropsOpExt {
    fn from_operator<Op: Operator>() -> Self;
}

impl ButtonPropsOpExt for ButtonProps {
    fn from_operator<Op: Operator>() -> Self {
        Self::new(Op::LABEL).call_operator(Op::ID)
    }
}

/// Catalog name of the Core extension. Exported so
/// [`crate::extension_resolution::REQUIRED_EXTENSIONS`] and the
/// Extensions dialog can refer to it without duplicating the
/// literal string.
pub const CORE_EXTENSION_ID: &str = "jackdaw.core";

pub(super) fn plugin(app: &mut App) {
    app.register_extension::<JackdawCoreExtension>()
        .add_observer(dispatch_button_operator_call);
}

/// When a button carrying an [`ButtonOperatorCall`] component is clicked,
/// dispatch the referenced operator. This is the single editor-wide
/// glue that makes `ButtonProps::call_operator(id)` and menu/context-menu
/// `op:`-prefixed entries (which also attach `ButtonOperatorCall` via feathers)
/// actually run the operator. Without this, `ButtonOperatorCall` is inert.
///
/// The feathers-level click handlers for menu/context items skip
/// firing their own `MenuAction`/`ContextMenuAction` events when they
/// see `ButtonOperatorCall`, so this observer is the sole dispatch path for
/// those items and won't double-fire.
fn dispatch_button_operator_call(
    event: On<ButtonClickEvent>,
    button_op: Query<&ButtonOperatorCall>,
    mut commands: Commands,
) {
    let Ok(ButtonOperatorCall(id)) = button_op.get(event.entity) else {
        return;
    };
    let id = id.clone();
    commands.queue(move |world: &mut World| {
        world
            .operator(id)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: true,
            })
            .call()
    });
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

        crate::scene_ops::add_to_extension(ctx);
        crate::history_ops::add_to_extension(ctx);
        crate::app_ops::add_to_extension(ctx);
        crate::view_ops::add_to_extension(ctx);
        crate::grid_ops::add_to_extension(ctx);
        crate::gizmo_ops::add_to_extension(ctx);
        crate::edit_mode_ops::add_to_extension(ctx);
        crate::entity_ops::add_to_extension(ctx);
        crate::transform_ops::add_to_extension(ctx);
        crate::physics_tool::add_to_extension(ctx);
        crate::hierarchy::add_to_extension(ctx);
        crate::viewport_select::add_to_extension(ctx);
        crate::clip_ops::add_to_extension(ctx);
        crate::brush_element_ops::add_to_extension(ctx);
        crate::brush_drag_ops::add_to_extension(ctx);
        crate::gizmos::add_to_extension(ctx);
        crate::terrain::sculpt::add_to_extension(ctx);
        crate::navmesh::ops::add_to_extension(ctx);
        crate::pie::add_to_extension(ctx);
        crate::terrain::ops::add_to_extension(ctx);
        crate::asset_browser::add_to_extension(ctx);
        crate::material_browser::add_to_extension(ctx);
        crate::inspector::ops::add_to_extension(ctx);
        crate::viewport::add_to_extension(ctx);
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
