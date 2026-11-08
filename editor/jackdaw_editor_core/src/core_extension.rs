use std::borrow::Cow;

use bevy::feathers::controls::FeathersButton;
use bevy::prelude::*;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;
use jackdaw_api_internal::lifecycle::OperatorAction;
use jackdaw_feathers::{
    button::ButtonOperatorCall,
    picker::{DismissPickerEvent, Picker},
};
use jackdaw_input::{Condition, InputContext, actions, bindings};
use jackdaw_scene_types::PropertyValue;

use crate::selection::Selection;

/// Catalog name of the Core extension. Exported so
/// [`crate::extension_resolution::REQUIRED_EXTENSIONS`] and the
/// Extensions dialog can refer to it without duplicating the
/// literal string.
pub const CORE_EXTENSION_ID: &str = "jackdaw.core";

pub(super) fn plugin(app: &mut App) {
    app.register_extension::<JackdawCoreExtension>()
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
/// [`dispatch_activate_operator`] is the one caller: every editor button
/// is a `FeathersButton`, so `Activate` is the only click signal an
/// operator button dispatches from.
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

/// Dispatches when a button carrying a [`ButtonOperatorCall`] emits `Activate`.
/// Entities without one are ignored. Menu and context-menu `op:` entries come
/// through here too: their feathers click handlers skip their own events when
/// they see `ButtonOperatorCall`, so this is the sole dispatch path.
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
fn seed_operator_button_on_add(_: On<Add<ButtonOperatorCall>>, mut commands: Commands) {
    commands.trigger(RefreshOperatorButtons);
}

/// Operator availability often depends on the selection (e.g. delete,
/// duplicate, group). Selection lives in the [`Selection`] resource and
/// is mutated from many sites (some bypass the `Selected` component), so
/// resource change-detection is the one reliable signal. Freshly-spawned
/// buttons still seed off the same event via [`seed_operator_button_on_add`].
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
            actions![(
                OperatorAction(CancelModalOp::ID),
                bindings![(KeyCode::Escape, Condition::Press)]
            )],
        ));

        ctx.register_operator::<CancelModalOp>();
        ctx.register_operator::<crate::texture_files::ApplyTextureOp>();
        ctx.register_operator::<crate::WindowOpenOp>()
            .register_operator::<crate::WindowResetLayoutOp>()
            .register_operator::<crate::MenuOpenOp>()
            .register_operator::<crate::MenuHoverOp>()
            .register_operator::<crate::ProjectOpenRecentOp>()
            .register_operator::<crate::PreviewSetOp>();
        crate::draw_brush::add_to_extension(ctx);

        crate::scene_ops::add_to_extension(ctx);
        crate::scenes::operators::add_to_extension(ctx);
        crate::history_ops::add_to_extension(ctx);
        crate::app_ops::add_to_extension(ctx);
        crate::view_ops::add_to_extension(ctx);
        crate::camera_settings::add_to_extension(ctx);
        crate::viewport_settings::add_to_extension(ctx);
        crate::grid_ops::add_to_extension(ctx);
        crate::gizmo_ops::add_to_extension(ctx);
        crate::tool_ops::add_to_extension(ctx);
        crate::numeric_transform::add_to_extension(ctx);
        crate::edit_mode_ops::add_to_extension(ctx);
        crate::entity_ops::add_to_extension(ctx);
        crate::environment_ops::add_to_extension(ctx);
        crate::material_overrides::add_to_extension(ctx);
        crate::ui_align::add_to_extension(ctx);
        crate::ui_grouping::add_to_extension(ctx);
        crate::ui_layout_presets::add_to_extension(ctx);
        crate::workspace_dropdown::add_to_extension(ctx);
        crate::transform_ops::add_to_extension(ctx);
        crate::hierarchy::add_to_extension(ctx);
        crate::runtime_outliner::add_to_extension(ctx);
        crate::file_ops::add_to_extension(ctx);
        crate::material_assets::add_to_extension(ctx);
        crate::definition_assets::add_to_extension(ctx);
        crate::asset_migration::add_to_extension(ctx);
        crate::prefab::placement_migration::add_to_extension(ctx);
        crate::viewport_select::add_to_extension(ctx);
        crate::clip_ops::add_to_extension(ctx);
        crate::brush_drag_ops::add_to_extension(ctx);
        crate::brush::box_select::add_to_extension(ctx);
        crate::gizmos::add_to_extension(ctx);
        crate::pie::add_to_extension(ctx);
        crate::play_settings::add_to_extension(ctx);
        crate::project_modules::add_to_extension(ctx);
        crate::texture_files::add_to_extension(ctx);
        crate::lod_group::add_to_extension(ctx);
        crate::model_lod::add_to_extension(ctx);
        crate::lod_bar::add_to_extension(ctx);
        crate::texture_import::add_to_extension(ctx);
        crate::image_sampling::add_to_extension(ctx);
        crate::world_units::add_to_extension(ctx);
        crate::project_window::add_to_extension(ctx);
        crate::material_browser::add_to_extension(ctx);
        crate::inspector::ops::add_to_extension(ctx);
        crate::viewport::add_to_extension(ctx);
        crate::screenshot::add_to_extension(ctx);
        crate::camera_capture::add_to_extension(ctx);
        crate::probe_bake::add_to_extension(ctx);
        crate::command_palette::add_to_extension(ctx);
        crate::document_ops::add_to_extension(ctx);
        crate::dock_ops::add_to_extension(ctx);
        crate::remote_ops::add_to_extension(ctx);
        crate::tags::add_to_extension(ctx);
        crate::recipes::add_to_extension(ctx);
    }
}

#[derive(Component, Default)]
#[require(InputContext)]
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
