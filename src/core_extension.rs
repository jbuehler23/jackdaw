use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;
use jackdaw_feathers::button::{ButtonClickEvent, CallOperator};

/// Catalog name of the Core extension. Exported so
/// [`crate::extensions_config::REQUIRED_EXTENSIONS`] and the
/// Extensions dialog can refer to it without duplicating the
/// literal string.
pub const CORE_EXTENSION_ID: &str = "jackdaw.core";

pub(super) fn plugin(app: &mut App) {
    app.register_extension::<JackdawCoreExtension>()
        .add_observer(dispatch_call_operator);
}

/// When a button carrying an [`CallOperator`] component is clicked,
/// dispatch the referenced operator. This is the single editor-wide
/// glue that makes `ButtonProps::call_operator(id)` and menu/context-menu
/// `op:`-prefixed entries (which also attach `CallOperator` via feathers)
/// actually run the operator. Without this, `CallOperator` is inert.
///
/// The feathers-level click handlers for menu/context items skip
/// firing their own `MenuAction`/`ContextMenuAction` events when they
/// see `CallOperator`, so this observer is the sole dispatch path for
/// those items and won't double-fire.
fn dispatch_call_operator(
    event: On<ButtonClickEvent>,
    call_op: Query<&CallOperator>,
    mut commands: Commands,
) {
    let Ok(CallOperator(id)) = call_op.get(event.entity) else {
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
    fn id() -> String {
        CORE_EXTENSION_ID.to_string()
    }

    fn label() -> String {
        "Jackdaw Core Functionality".to_string()
    }

    fn description() -> String {
        "Important functionality for the Jackdaw editor. This extension is always loaded and cannot be disabled.".to_string()
    }

    fn kind() -> ExtensionKind {
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

        // Spawn the three modifier-key input actions once, up front,
        // so later keybind ports can `Chord::single(ctrl)` /
        // `Chord::new([ctrl, shift])` / `Chord::single(alt)` without
        // each module having to re-register them.
        let ext = ctx.id();
        let (ctrl, shift, alt) = ctx.entity_mut().world_scope(|world| {
            let ctrl = world
                .spawn((
                    Action::<CtrlHeldAction>::new(),
                    ActionOf::<CoreExtensionInputContext>::new(ext),
                    bindings![KeyCode::ControlLeft, KeyCode::ControlRight],
                ))
                .id();
            let shift = world
                .spawn((
                    Action::<ShiftHeldAction>::new(),
                    ActionOf::<CoreExtensionInputContext>::new(ext),
                    bindings![KeyCode::ShiftLeft, KeyCode::ShiftRight],
                ))
                .id();
            let alt = world
                .spawn((
                    Action::<AltHeldAction>::new(),
                    ActionOf::<CoreExtensionInputContext>::new(ext),
                    bindings![KeyCode::AltLeft, KeyCode::AltRight],
                ))
                .id();
            (ctrl, shift, alt)
        });
        let modifiers = Modifiers { ctrl, shift, alt };

        ctx.register_operator::<CancelModalOp>();
        ctx.register_operator::<crate::asset_browser::ApplyTextureOp>();
        crate::draw_brush::add_to_extension(ctx);

        crate::scene_ops::add_to_extension(ctx, &modifiers);
        crate::history_ops::add_to_extension(ctx, &modifiers);
        crate::app_ops::add_to_extension(ctx);
        crate::view_ops::add_to_extension(ctx, &modifiers);
        crate::grid_ops::add_to_extension(ctx);
        crate::gizmo_ops::add_to_extension(ctx);
        crate::edit_mode_ops::add_to_extension(ctx);
        crate::entity_ops::add_to_extension(ctx, &modifiers);
        crate::transform_ops::add_to_extension(ctx, &modifiers);
    }

    fn register_input_context(app: &mut App) {
        app.add_input_context::<CoreExtensionInputContext>();
    }
}

#[derive(Component, Default)]
pub struct CoreExtensionInputContext;

/// BEI input action bound to Ctrl (left or right). Operator bindings
/// that need Ctrl as a modifier use this as a
/// `Chord::single(ctrl_entity)` prerequisite so e.g. `Ctrl+S` fires
/// only while Ctrl is held. The entity id is captured during
/// `register` and passed to each module via [`Modifiers`].
#[derive(Component, Debug, Default, Clone, Copy, InputAction)]
#[action_output(bool)]
pub struct CtrlHeldAction;

/// BEI input action bound to Shift (left or right). See [`CtrlHeldAction`].
#[derive(Component, Debug, Default, Clone, Copy, InputAction)]
#[action_output(bool)]
pub struct ShiftHeldAction;

/// BEI input action bound to Alt (left or right). See [`CtrlHeldAction`].
#[derive(Component, Debug, Default, Clone, Copy, InputAction)]
#[action_output(bool)]
pub struct AltHeldAction;

/// Entity ids of the Ctrl, Shift, and Alt modifier actions, passed to
/// each module's `add_to_extension` so chorded keybinds can reference
/// them.
pub struct Modifiers {
    pub ctrl: Entity,
    pub shift: Entity,
    pub alt: Entity,
}

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
