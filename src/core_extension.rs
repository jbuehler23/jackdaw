use std::borrow::Cow;

use bevy::feathers::controls::FeathersButton;
use bevy::prelude::*;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;
use jackdaw_feathers::{
    button::{ButtonClickEvent, ButtonOperatorCall},
    picker::{DismissPickerEvent, Picker},
};
use jackdaw_jsn::PropertyValue;

use crate::selection::Selection;

/// Catalog name of the Core extension. Exported so
/// [`crate::extension_resolution::REQUIRED_EXTENSIONS`] and the
/// Extensions dialog can refer to it without duplicating the
/// literal string.
pub const CORE_EXTENSION_ID: &str = "jackdaw.core";

pub(super) fn plugin(app: &mut App) {
    app.register_extension::<JackdawCoreExtension>()
        .add_observer(dispatch_button_operator_call)
        .add_observer(dispatch_activate_operator)
        .add_observer(update_operator_button_availability)
        .add_observer(seed_operator_button_on_add)
        .add_systems(Update, refresh_buttons_on_selection_change);
}

/// Queues the work that makes a [`ButtonOperatorCall`] run its operator.
/// Cancels any active modal first so a toolbar button is a peer of
/// whatever tool currently owns the modal slot, then dispatches the
/// referenced operator with the button's statically-declared parameters.
///
/// Both dispatch entry points call this: [`dispatch_button_operator_call`]
/// for the `ButtonClickEvent` from `feathers::button`, and
/// [`dispatch_activate_operator`] for the `Activate` from a
/// `FeathersButton`.
fn queue_operator_dispatch(commands: &mut Commands, call: &ButtonOperatorCall) {
    let id = call.id.clone().into_owned();
    let params: Vec<(String, PropertyValue)> = call
        .params
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    commands.queue(move |world: &mut World| {
        // Treat every toolbar button as a peer of any active tool: cancel
        // whatever modal is running first, then dispatch. Without this,
        // clicking Object Mode (or any other non-modal mode button) while
        // Draw Brush / Measure / etc. owns the modal slot is silently
        // blocked by their `is_available` checks.
        let _ = world.operator("modal.cancel").call();

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

/// Dispatches when a `feathers::button` carrying a [`ButtonOperatorCall`]
/// is clicked and fires `ButtonClickEvent`. This also covers menu and
/// context-menu `op:`-prefixed entries, which attach `ButtonOperatorCall`
/// via feathers. The feathers-level click handlers skip firing their own
/// `MenuAction`/`ContextMenuAction` events when they see
/// `ButtonOperatorCall`, so this observer is the sole dispatch path for
/// those items and won't double-fire.
fn dispatch_button_operator_call(
    event: On<ButtonClickEvent>,
    button_op: Query<&ButtonOperatorCall>,
    mut commands: Commands,
) {
    if let Ok(call) = button_op.get(event.entity) {
        queue_operator_dispatch(&mut commands, call);
    }
}

/// Dispatches when a `bevy_feathers` button authored via
/// `jackdaw_feathers::button::operator_button` emits `Activate` on click
/// or keyboard activation. Entities without a `ButtonOperatorCall` are
/// ignored, so other `Activate` sources such as a button with its own
/// `on(Activate)` observer don't double-fire.
fn dispatch_activate_operator(
    activate: On<Activate>,
    button_op: Query<&ButtonOperatorCall>,
    mut commands: Commands,
) {
    if let Ok(call) = button_op.get(activate.entity) {
        queue_operator_dispatch(&mut commands, call);
    }
}

/// Drive `InteractionDisabled` on every `FeathersButton` operator button
/// from its operator's live availability. `bevy_feathers` reads
/// `InteractionDisabled` to grey the button out and suppress its
/// `Activate`, so this is the only place such a button's enabled state
/// lives.
///
/// This is an [`On<RefreshOperatorButtons>`] observer. Availability turns
/// on the editor state every operator mutates, announced through dispatch,
/// and on the current selection. [`refresh_buttons_on_selection_change`]
/// re-fires the event when `Selection` changes, and freshly-spawned
/// buttons seed off the same event via [`seed_operator_button_on_add`].
/// Reading `is_available` needs `&mut World`, so the recompute is queued.
///
/// Scoped to `FeathersButton` so the `feathers::button` path, which gates
/// clicks via its own `ButtonVariant::Disabled`, is left untouched.
/// `is_available` returning `Err` keeps the button enabled. That covers an
/// unknown id or a modal op while a modal runs; dispatch cancels the
/// active modal first, so modal-op buttons must stay clickable.
fn update_operator_button_availability(
    _: On<RefreshOperatorButtons>,
    buttons: Query<(Entity, &ButtonOperatorCall), With<FeathersButton>>,
    mut commands: Commands,
) {
    let calls: Vec<(Entity, Cow<'static, str>)> =
        buttons.iter().map(|(e, c)| (e, c.id.clone())).collect();
    if calls.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        for (entity, id) in calls {
            let available = world.operator(id).is_available().unwrap_or(true);
            let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                continue;
            };
            let disabled = entity_mut.contains::<InteractionDisabled>();
            if available && disabled {
                entity_mut.remove::<InteractionDisabled>();
            } else if !available && !disabled {
                entity_mut.insert(InteractionDisabled);
            }
        }
    });
}

/// Seed a button's variant and disabled state the moment its
/// [`ButtonOperatorCall`] is added. The variant highlighters and the
/// availability driver are [`On<RefreshOperatorButtons>`] observers, so
/// they only run on a state change. Without this, a freshly-opened editor
/// or a just-spawned contextual toolbar would show stale defaults until
/// the first operator ran. Re-firing the event recomputes every button, so
/// the new one settles alongside the rest. The trigger is queued, so it
/// runs after the spawn flushes and the button's `ButtonVariant` is in
/// place.
fn seed_operator_button_on_add(_: On<Add, ButtonOperatorCall>, mut commands: Commands) {
    commands.trigger(RefreshOperatorButtons);
}

/// Operator availability often depends on the selection (e.g. delete,
/// duplicate, group). Selection lives in the [`Selection`] resource and
/// is mutated from many sites (some bypass the `Selected` component), so
/// resource change-detection is the one reliable signal. This bridges it
/// to the refresh event; it recomputes nothing itself and short-circuits
/// on frames where the selection is unchanged.
fn refresh_buttons_on_selection_change(selection: Res<Selection>, mut commands: Commands) {
    if selection.is_changed() {
        commands.trigger(RefreshOperatorButtons);
    }
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
        ctx.bind_operator::<CoreExtensionInputContext, crate::ClipDeleteKeyframesOp>([
            PresetInput::key("Delete"),
            PresetInput::key("Backspace"),
        ]);
        // No Press on Step Left / Right: deferred (hold-to-repeat, not bare Press::default()).
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
        ctx.bind_operator::<CoreExtensionInputContext, crate::ClipTimelineJumpPrevOp>([
            PresetInput::key("ArrowLeft").shift(),
        ]);
        ctx.bind_operator::<CoreExtensionInputContext, crate::ClipTimelineJumpNextOp>([
            PresetInput::key("ArrowRight").shift(),
        ]);
        ctx.bind_operator::<CoreExtensionInputContext, crate::ClipTimelineJumpStartOp>([
            PresetInput::key("Home"),
        ]);
        ctx.bind_operator::<CoreExtensionInputContext, crate::ClipTimelineJumpEndOp>([
            PresetInput::key("End"),
        ]);
        ctx.bind_operator::<CoreExtensionInputContext, crate::ClipCopyKeyframesOp>([
            PresetInput::key("KeyC").ctrl(),
        ]);
        ctx.bind_operator::<CoreExtensionInputContext, crate::ClipPasteKeyframesOp>([
            PresetInput::key("KeyV").ctrl(),
        ]);
        crate::draw_brush::add_to_extension(ctx);
        crate::measure_tool::add_to_extension(ctx);

        crate::scene_ops::add_to_extension(ctx);
        crate::scenes::operators::add_to_extension(ctx);
        crate::history_ops::add_to_extension(ctx);
        crate::app_ops::add_to_extension(ctx);
        crate::view_ops::add_to_extension(ctx);
        crate::grid_ops::add_to_extension(ctx);
        crate::gizmo_ops::add_to_extension(ctx);
        crate::tool_ops::add_to_extension(ctx);
        crate::numeric_transform::add_to_extension(ctx);
        crate::edit_mode_ops::add_to_extension(ctx);
        crate::entity_ops::add_to_extension(ctx);
        crate::transform_ops::add_to_extension(ctx);
        crate::physics_tool::add_to_extension(ctx);
        crate::hierarchy::add_to_extension(ctx);
        crate::file_ops::add_to_extension(ctx);
        crate::viewport_select::add_to_extension(ctx);
        crate::clip_ops::add_to_extension(ctx);
        crate::brush_element_ops::add_to_extension(ctx);
        crate::brush_drag_ops::add_to_extension(ctx);
        crate::brush::box_select::add_to_extension(ctx);
        crate::brush::topology_ops::bridge_edge_loops::add_to_extension(ctx);
        crate::brush::topology_ops::connect_verts::add_to_extension(ctx);
        crate::brush::topology_ops::dissolve_edges::add_to_extension(ctx);
        crate::brush::topology_ops::dissolve_faces::add_to_extension(ctx);
        crate::brush::topology_ops::dissolve_verts::add_to_extension(ctx);
        crate::brush::topology_ops::edge_bevel::add_to_extension(ctx);
        crate::brush::topology_ops::vertex_bevel::add_to_extension(ctx);
        crate::brush::topology_ops::edge_slide::add_to_extension(ctx);
        crate::brush::topology_ops::edge_slide_modal::add_to_extension(ctx);
        crate::brush::topology_ops::select_invert::add_to_extension(ctx);
        crate::brush::topology_ops::select_less::add_to_extension(ctx);
        crate::brush::topology_ops::select_linked::add_to_extension(ctx);
        crate::brush::topology_ops::select_loop::add_to_extension(ctx);
        crate::brush::topology_ops::select_more::add_to_extension(ctx);
        crate::brush::topology_ops::select_ring::add_to_extension(ctx);
        crate::brush::topology_ops::extrude::add_to_extension(ctx);
        crate::brush::topology_ops::inset::add_to_extension(ctx);
        crate::brush::topology_ops::loop_cut::add_to_extension(ctx);
        crate::brush::topology_ops::make_edge_face::add_to_extension(ctx);
        crate::brush::topology_ops::merge_by_distance::add_to_extension(ctx);
        crate::brush::topology_ops::mirror_ops::add_to_extension(ctx);
        crate::brush::mirror_plane_ops::add_to_extension(ctx);
        crate::modifier_ops::add_to_extension(ctx);
        crate::brush::topology_ops::subdivide::add_to_extension(ctx);
        crate::brush::topology_ops::vertex_slide::add_to_extension(ctx);
        crate::brush::topology_ops::vertex_slide_modal::add_to_extension(ctx);
        crate::brush::topology_ops::weld_selected::add_to_extension(ctx);
        crate::brush::topology_ops::uv_reset_axes::add_to_extension(ctx);
        crate::brush::topology_ops::uv_world_aligned::add_to_extension(ctx);
        crate::brush::topology_ops::uv_rotate_90::add_to_extension(ctx);
        crate::brush::topology_ops::uv_fit_to_face::add_to_extension(ctx);
        crate::brush::topology_ops::uv_texel_density::add_to_extension(ctx);
        crate::brush::topology_ops::uv_align_to_edge::add_to_extension(ctx);
        crate::brush::topology_ops::reconvexify::add_to_extension(ctx);
        crate::brush::knife_mode::add_to_extension(ctx);
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
        crate::dock_ops::add_to_extension(ctx);
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
