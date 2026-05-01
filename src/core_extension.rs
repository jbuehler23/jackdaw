use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;
use jackdaw_feathers::{
    button::{ButtonClickEvent, ButtonOperatorCall},
    picker::{DismissPickerEvent, Picker},
};
use jackdaw_jsn::PropertyValue;

/// Catalog name of the Core extension. Exported so
/// [`crate::extension_resolution::REQUIRED_EXTENSIONS`] and the
/// Extensions dialog can refer to it without duplicating the
/// literal string.
pub const CORE_EXTENSION_ID: &str = "jackdaw.core";

pub(super) fn plugin(app: &mut App) {
    app.register_extension::<JackdawCoreExtension>()
        .add_observer(dispatch_button_operator_call);
}

/// When a button carrying a [`ButtonOperatorCall`] is clicked,
/// dispatch the referenced operator with the button's statically-declared
/// parameters. This is the single editor-wide glue that makes
/// `ButtonProps::call_operator(id)` and menu / context-menu `op:`-prefixed
/// entries (which also attach `ButtonOperatorCall` via feathers) actually
/// run the operator.
///
/// The feathers-level click handlers for menu/context items skip
/// firing their own `MenuAction`/`ContextMenuAction` events when they
/// see `ButtonOperatorCall`, so this observer is the sole dispatch path
/// for those items and won't double-fire.
fn dispatch_button_operator_call(
    event: On<ButtonClickEvent>,
    button_op: Query<&ButtonOperatorCall>,
    mut commands: Commands,
) {
    let Ok(call) = button_op.get(event.entity) else {
        return;
    };
    let id = call.id.clone().into_owned();
    let params: Vec<(String, PropertyValue)> = call
        .params
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    commands.queue(move |world: &mut World| {
        // If the target is a modal operator, cancel any in-flight
        // modal first. Lets the user switch tools (Draw Brush,
        // Measure Distance, brush-element drags, terrain sculpt, ...)
        // by clicking another toolbar button without reaching for
        // Escape, and keeps the second dispatch from failing with
        // `ModalAlreadyActive`. Extensions that wire their own
        // operators to buttons inherit this behavior for free.
        if let Ok(true) = world.operator(id.clone()).is_modal() {
            let _ = world.operator("modal.cancel").call();
        }

        let mut call = world.operator(id.clone()).settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        });
        for (k, v) in params {
            call = call.param(k, v);
        }
        if let Err(err) = call.call() {
            error!("operator dispatch failed for `{id}`: {err}");
        }
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
        ctx.register_operator::<crate::WindowOpenOp>()
            .register_operator::<crate::WindowResetLayoutOp>();
        ctx.register_operator::<crate::ClipDeleteKeyframesOp>()
            .register_operator::<crate::ClipTimelineStepLeftOp>()
            .register_operator::<crate::ClipTimelineStepRightOp>()
            .register_operator::<crate::ClipTimelineJumpPrevOp>()
            .register_operator::<crate::ClipTimelineJumpNextOp>()
            .register_operator::<crate::ClipTimelineJumpStartOp>()
            .register_operator::<crate::ClipTimelineJumpEndOp>()
            .register_operator::<crate::ClipCopyKeyframesOp>()
            .register_operator::<crate::ClipPasteKeyframesOp>()
            .register_operator::<crate::ClipPlayOp>()
            .register_operator::<crate::ClipPauseOp>()
            .register_operator::<crate::ClipStopOp>()
            .register_operator::<crate::ClipNewOp>()
            .register_operator::<crate::ClipNewBlendGraphOp>();
        let core_ext = ctx.id();
        ctx.spawn((
            Action::<crate::ClipDeleteKeyframesOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(core_ext),
            bindings![
                (KeyCode::Delete, Press::default()),
                (KeyCode::Backspace, Press::default()),
            ],
        ));
        // No `Press` on Step Left / Right: holding an arrow scrubs
        // the timeline frame-by-frame. Shift+Arrow keyframe jumps
        // below stay one-shot.
        ctx.spawn((
            Action::<crate::ClipTimelineStepLeftOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(core_ext),
            bindings![KeyCode::ArrowLeft],
        ));
        ctx.spawn((
            Action::<crate::ClipTimelineStepRightOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(core_ext),
            bindings![KeyCode::ArrowRight],
        ));
        ctx.spawn((
            Action::<crate::ClipTimelineJumpPrevOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(core_ext),
            bindings![(
                KeyCode::ArrowLeft.with_mod_keys(ModKeys::SHIFT),
                Press::default(),
            )],
        ));
        ctx.spawn((
            Action::<crate::ClipTimelineJumpNextOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(core_ext),
            bindings![(
                KeyCode::ArrowRight.with_mod_keys(ModKeys::SHIFT),
                Press::default(),
            )],
        ));
        ctx.spawn((
            Action::<crate::ClipTimelineJumpStartOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(core_ext),
            bindings![(KeyCode::Home, Press::default())],
        ));
        ctx.spawn((
            Action::<crate::ClipTimelineJumpEndOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(core_ext),
            bindings![(KeyCode::End, Press::default())],
        ));
        ctx.spawn((
            Action::<crate::ClipCopyKeyframesOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(core_ext),
            bindings![(
                KeyCode::KeyC.with_mod_keys(ModKeys::CONTROL),
                Press::default(),
            )],
        ));
        ctx.spawn((
            Action::<crate::ClipPasteKeyframesOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(core_ext),
            bindings![(
                KeyCode::KeyV.with_mod_keys(ModKeys::CONTROL),
                Press::default(),
            )],
        ));
        crate::draw_brush::add_to_extension(ctx);
        crate::measure_tool::add_to_extension(ctx);

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
        crate::command_palette::add_to_extension(ctx);
        crate::document_ops::add_to_extension(ctx);
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
fn cancel_modal(
    _: In<OperatorParameters>,
    mut active: ActiveModalQuery,
    pickers: Query<Entity, With<Picker>>,
    mut commands: Commands,
) -> OperatorResult {
    active.cancel();

    for picker in pickers {
        commands.trigger(DismissPickerEvent(picker));
    }

    OperatorResult::Finished
}

fn is_any_modal_active(active: ActiveModalQuery, pickers: Query<(), With<Picker>>) -> bool {
    active.is_modal_running() || !pickers.is_empty()
}
