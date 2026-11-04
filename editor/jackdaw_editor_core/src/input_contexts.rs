//! Editor-owned input contexts for modal interactions, navigation, and
//! viewport pointer gestures.
//!
//! # Contexts
//!
//! [`ModalInputContext`] is active only while any modal interaction is running
//! (drag op, modal transform, or a `tick_modal_operator` modal). It carries
//! high `ContextPriority` so its bindings are evaluated before extension
//! contexts. All modal actions use `consume_input = true` and
//! `require_reset = true` so that held keys from the interaction that
//! triggered the modal cannot leak into the modal context and so keys
//! pressed inside the modal do not bleed into extension contexts below.
//!
//! `update_modal_context_activity` runs in `PostUpdate` so it sees the
//! frame's final modal state; the context flips before the next `PreUpdate`
//! evaluation, leaving no frame where modal keys leak to lower contexts
//! after a modal ends. Each activation ignores keys already held, because
//! every modal action requires a reset.
//!
//! [`NavInputContext`] is always active and carries low priority (default 0).
//! It hosts scroll-bound brush-resize actions (`nav.brush_resize_up` /
//! `nav.brush_resize_down`) alongside the camera navigation actions. Brush
//! resize fires on every frame that scrolls in its direction;
//! the terrain sculpt system still gates on Shift being held. Camera zoom is
//! not an action: `populate_camera_nav_input` reads the normalized
//! [`ScrollTicks`] into `CameraNavInput.zoom_ticks`.
//!
//! [`PointerInputContext`] is always active (priority 0) and carries the
//! single `PointerPrimary` action, bound code-level to `MouseButton::Left`
//! with [`Condition::Down`] so the action tracks the full press/hold/release
//! cycle. The `pointer_primary_just_pressed()` edge is exposed via
//! `PointerInputs` via `ActionEvents::START`. LMB release checks and drag/threshold logic in
//! the invoke-trigger systems continue to read raw `ButtonInput<MouseButton>`
//! directly; only the initial press edge is routed through this action.
//! Preset-bindability of LMB itself is not meaningful until click-vs-drag
//! semantics live in the binding layer.
//!
//! # `BuiltinActions` registry
//!
//! [`BuiltinActions`] maps action names (e.g. `"modal.confirm"`) to the action
//! entities that the applier in `keymap.rs` needs to attach
//! bindings. The Modal and Navigation arms of `apply_keymap_preset` look up
//! names here; unknown names land in `skipped_unknown_operator` (same slot
//! used for unknown operator ids, shared because the semantics are identical:
//! a preset entry named something that does not exist).
//!
//! The action types and their polling `SystemParam`s (`ModalInputs`,
//! `NavScrollInputs`, `PointerInputs`) live in [`crate::modal_inputs`].

use bevy::prelude::*;
use jackdaw_input::{
    ActionOf, ActionSettings, ActionSystems, ActionValue, Binding, BindingOf, Condition,
    ContextActivity, ContextPriority, InputContext, ScrollTicks,
};

use crate::modal_inputs::{
    ModalAction, ModalAxisX, ModalAxisY, ModalAxisZ, ModalCancel, ModalConfirm, ModalStepDown,
    ModalStepUp, NavBrushResizeDown, NavBrushResizeUp, NavFly, NavLook, PointerPrimary,
};

use crate::brush::{BrushDragState, EdgeDragState, VertexDragState};
use crate::modal_transform::ModalTransformState;
use jackdaw_api_internal::keymap::{
    BuiltinActions, DefaultKeymap, PresetBinding, PresetContext, PresetInput, PresetPhase,
};
use jackdaw_api_internal::lifecycle::ActiveModalQuery;
use jackdaw_camera::CameraNavInput;
use jackdaw_commands::keybinds::{EditorAction, KeybindRegistry};

// ---- Context marker components ----

/// High-priority input context active only during modal interactions.
///
/// Priority 10 ensures modal bindings (Escape, Enter, X/Y/Z) are evaluated
/// before any extension context regardless of spawn order. All actions use
/// `consume_input = true, require_reset = true` to prevent key bleed.
#[derive(Component, Default)]
#[require(InputContext)]
pub struct ModalInputContext;

/// Low-priority navigation context, always active.
///
/// Uses default priority (0) so every extension context can outrank it when
/// needed.
#[derive(Component, Default)]
#[require(InputContext)]
pub struct NavInputContext;

/// Always-active context for viewport pointer gestures (priority 0).
///
/// Hosts `PointerPrimary` (LMB). The invoke-trigger systems read
/// `PointerInputs::pointer_primary_just_pressed()` instead of raw
/// `ButtonInput::just_pressed(MouseButton::Left)` for the gesture-start edge.
#[derive(Component, Default)]
#[require(InputContext)]
pub struct PointerInputContext;

// ---- Entities that carry the contexts ----

/// Resource that holds the entity owning the modal context.
#[derive(Resource)]
pub struct ModalContextEntity(pub Entity);

/// Resource that holds the entity owning the nav context.
#[derive(Resource)]
pub struct NavContextEntity(pub Entity);

/// Resource that holds the entity owning the pointer context.
#[derive(Resource)]
pub struct PointerContextEntity(pub Entity);

// ---- ActionSettings for modal actions ----

/// Returns `ActionSettings` with `consume_input = true, require_reset = true`
/// used for every modal action.
fn modal_action_settings() -> ActionSettings {
    ActionSettings {
        consume_input: true,
        require_reset: true,
    }
}

// ---- Plugin ----

pub(super) struct InputContextsPlugin;

impl Plugin for InputContextsPlugin {
    fn build(&self, app: &mut App) {
        // `spawn_contexts` is NOT added here because
        // `extension_lifecycle::plugin` chains it explicitly between
        // `apply_enabled_extensions_startup` and `apply_active_keymap`.
        //
        // `populate_camera_nav_input` runs in `PreUpdate` after
        // `ActionSystems` so every action value for this frame is already
        // evaluated. The camera system runs in `Update` (always after
        // `PreUpdate`), so it sees the freshly written `CameraNavInput`.
        app.add_systems(PostUpdate, update_modal_context_activity)
            .add_systems(PreUpdate, populate_camera_nav_input.after(ActionSystems));
    }
}

/// Spawn the modal and nav context entities, attach actions, register names in
/// `BuiltinActions`, and record defaults into `DefaultKeymap`.
///
/// This system is NOT registered by `InputContextsPlugin::build`. Instead,
/// `extension_lifecycle::plugin` chains it between
/// `apply_enabled_extensions_startup` and `apply_active_keymap`:
///
/// ```text
/// (apply_enabled_extensions_startup, spawn_contexts, apply_active_keymap).chain()
/// ```
///
/// This guarantees that modal/nav action entities and their `DefaultKeymap`
/// entries exist before `apply_active_keymap` iterates the preset bindings.
/// Spawn one modal-action entity (marker `A` + settings + `ModalAction`)
/// parented to the modal context.
fn spawn_modal_action<A: Component + Default>(
    commands: &mut Commands,
    modal_entity: Entity,
) -> Entity {
    commands
        .spawn((
            A::default(),
            modal_action_settings(),
            ActionOf(modal_entity),
            ModalAction,
            ChildOf(modal_entity),
        ))
        .id()
}

pub(crate) fn spawn_contexts(
    mut commands: Commands,
    mut defaults: ResMut<DefaultKeymap>,
    mut builtin: ResMut<BuiltinActions>,
) {
    // ---- Modal context entity ----
    // Start INACTIVE; the activation system enables it each frame as needed.
    let modal_entity = commands
        .spawn((
            ModalInputContext,
            ContextPriority(10),
            ContextActivity::INACTIVE,
            Name::new("ModalInputContext"),
            crate::EditorEntity,
        ))
        .id();

    // Spawn each modal action as a child of the modal entity.
    // All seven carry ModalAction, which marks them as the modal's own.
    let confirm = spawn_modal_action::<ModalConfirm>(&mut commands, modal_entity);
    let cancel = spawn_modal_action::<ModalCancel>(&mut commands, modal_entity);
    let axis_x = spawn_modal_action::<ModalAxisX>(&mut commands, modal_entity);
    let axis_y = spawn_modal_action::<ModalAxisY>(&mut commands, modal_entity);
    let axis_z = spawn_modal_action::<ModalAxisZ>(&mut commands, modal_entity);
    let step_up = spawn_modal_action::<ModalStepUp>(&mut commands, modal_entity);
    let step_down = spawn_modal_action::<ModalStepDown>(&mut commands, modal_entity);

    // ---- Nav context entity ----
    let nav_entity = commands
        .spawn((
            NavInputContext,
            // Default priority = 0 (low).
            Name::new("NavInputContext"),
            crate::EditorEntity,
        ))
        .id();

    // NavFly: code-level binding with the `Down` condition so the action value
    // is `true` every frame RMB is held (not just on the rising edge).
    // `Down` is the correct condition for hold semantics; `Press` would fire
    // only on the first frame and leave fly_active false on all subsequent
    // held frames. This action is NOT registered in `BuiltinActions` and has
    // no `DefaultKeymap` entry: it is not preset-bindable this pass.
    let nav_fly_entity = commands
        .spawn((NavFly, ActionOf(nav_entity), ChildOf(nav_entity)))
        .id();
    commands.spawn((
        Binding::from(MouseButton::Right),
        Condition::Down,
        BindingOf(nav_fly_entity),
        ChildOf(nav_fly_entity),
    ));

    // NavLook: mouse motion delta. Bound code-level; axis feeds are not
    // preset-bindable (the activation chord is the rebindable part).
    let nav_look_entity = commands
        .spawn((NavLook, ActionOf(nav_entity), ChildOf(nav_entity)))
        .id();
    commands.spawn((
        Binding::mouse_motion(),
        BindingOf(nav_look_entity),
        ChildOf(nav_look_entity),
    ));

    // NavBrushResizeUp / NavBrushResizeDown: one scroll tick upward/downward.
    // Bound to the mouse wheel with a scroll tick, so each scrolling frame
    // fires once. Registered in BuiltinActions and DefaultKeymap
    // (PresetContext::Navigation) so keymap presets can rebind the resize
    // chord. Camera zoom reads `ScrollTicks` in populate_camera_nav_input
    // rather than an action; the Shift guard in the camera system keeps zoom
    // and resize from conflicting.
    let nav_resize_up_entity = commands
        .spawn((NavBrushResizeUp, ActionOf(nav_entity), ChildOf(nav_entity)))
        .id();
    commands.spawn((
        Binding::mouse_wheel(),
        Condition::ScrollTick { up: true },
        BindingOf(nav_resize_up_entity),
        ChildOf(nav_resize_up_entity),
    ));

    let nav_resize_down_entity = commands
        .spawn((
            NavBrushResizeDown,
            ActionOf(nav_entity),
            ChildOf(nav_entity),
        ))
        .id();
    commands.spawn((
        Binding::mouse_wheel(),
        Condition::ScrollTick { up: false },
        BindingOf(nav_resize_down_entity),
        ChildOf(nav_resize_down_entity),
    ));

    // ---- Pointer context entity ----
    let pointer_entity = commands
        .spawn((
            PointerInputContext,
            Name::new("PointerInputContext"),
            crate::EditorEntity,
        ))
        .id();

    // PointerPrimary: LMB press/hold. Bound code-level with Down so the action
    // is true every frame LMB is held; ActionEvents::START fires only on the
    // first held frame. Not registered in BuiltinActions: LMB preset-bindability
    // is deferred until click-vs-drag semantics live in the binding layer.
    let pointer_primary_entity = commands
        .spawn((
            PointerPrimary,
            ActionOf(pointer_entity),
            ChildOf(pointer_entity),
        ))
        .id();
    commands.spawn((
        Binding::from(MouseButton::Left),
        Condition::Down,
        BindingOf(pointer_primary_entity),
        ChildOf(pointer_primary_entity),
    ));

    // ---- BuiltinActions registry ----
    // nav.fly / nav.look / nav.zoom are code-level and do not need name
    // resolution by the preset applier, so they are not registered here.
    builtin.register("modal.confirm", confirm);
    builtin.register("modal.cancel", cancel);
    builtin.register("modal.axis_x", axis_x);
    builtin.register("modal.axis_y", axis_y);
    builtin.register("modal.axis_z", axis_z);
    builtin.register("modal.step_up", step_up);
    builtin.register("modal.step_down", step_down);
    builtin.register("nav.brush_resize_up", nav_resize_up_entity);
    builtin.register("nav.brush_resize_down", nav_resize_down_entity);

    // ---- DefaultKeymap entries ----
    // modal.confirm / modal.step_up / modal.step_down are registered in
    // BuiltinActions (above) so future consumers can bind them, but their
    // classic-preset entries are deliberately omitted here.
    // Recorded into the keymap once a modal consumer exists; an unbound
    // consume_input action consumes nothing.
    let modal_bindings: &[(&str, PresetInput, PresetPhase)] = &[
        (
            "modal.cancel",
            PresetInput::key("Escape"),
            PresetPhase::Press,
        ),
        ("modal.axis_x", PresetInput::key("KeyX"), PresetPhase::Press),
        ("modal.axis_y", PresetInput::key("KeyY"), PresetPhase::Press),
        ("modal.axis_z", PresetInput::key("KeyZ"), PresetPhase::Press),
    ];
    for (name, input, phase) in modal_bindings {
        defaults.bindings.push(PresetBinding {
            operator: name.to_string(),
            input: input.clone(),
            phase: *phase,
            context: PresetContext::Modal,
        });
    }
    // nav.fly has no DefaultKeymap entry: its binding is code-level.

    // nav.brush_resize_up / nav.brush_resize_down: scroll-bound, Navigation context.
    let nav_resize_bindings: &[(&str, bool)] = &[
        ("nav.brush_resize_up", true),
        ("nav.brush_resize_down", false),
    ];
    for (name, positive) in nav_resize_bindings {
        defaults.bindings.push(PresetBinding {
            operator: name.to_string(),
            input: PresetInput::scroll(*positive),
            phase: PresetPhase::Press,
            context: PresetContext::Navigation,
        });
    }

    // ---- Store entity handles ----
    commands.insert_resource(ModalContextEntity(modal_entity));
    commands.insert_resource(NavContextEntity(nav_entity));
    commands.insert_resource(PointerContextEntity(pointer_entity));
}

/// Writes `CameraNavInput` from action values and the scroll each
/// `PreUpdate` frame, after `ActionSystems` has evaluated actions for this
/// frame.
///
/// Ordering: runs in `PreUpdate` after `ActionSystems`; the camera system
/// runs in `Update`. This guarantees a single write per frame with values
/// that are stable for the entire `Update` phase.
///
/// Zoom reads [`ScrollTicks`], where a line counts one tick and a hundred
/// pixels count one.
///
/// WASD/QE movement keys are read from raw keyboard this pass. These keys
/// are registered in `KeybindRegistry` as RMB-chords, but the RMB gate is
/// already provided by `fly_active` (the `NavFly` action value). Using
/// `key_pressed` (key only, no mouse check) guarded by `fly_active` is
/// exactly equivalent to the previous `key_chord_pressed` call guarded by
/// `right_held`.
pub(crate) fn populate_camera_nav_input(
    fly_q: Query<&ActionValue, With<NavFly>>,
    look_q: Query<&ActionValue, With<NavLook>>,
    scroll: Res<ScrollTicks>,
    keyboard: Res<ButtonInput<KeyCode>>,
    keybinds: Res<KeybindRegistry>,
    mut nav: ResMut<CameraNavInput>,
) {
    let fly_active = fly_q.iter().any(|a| a.as_bool());
    let look_delta = look_q.iter().fold(Vec2::ZERO, |acc, a| acc + a.as_axis2d());
    let zoom_ticks = scroll.y;

    // WASD/QE: keyboard only, guarded by fly_active. Raw keyboard stays
    // because the camera movement keys live in `KeybindRegistry` (the
    // legacy binding system) and are not actions.
    let mut move_axes = Vec3::ZERO;
    if fly_active {
        if keybinds.key_pressed(EditorAction::CameraForward, &keyboard) {
            move_axes.z += 1.0;
        }
        if keybinds.key_pressed(EditorAction::CameraBackward, &keyboard) {
            move_axes.z -= 1.0;
        }
        if keybinds.key_pressed(EditorAction::CameraLeft, &keyboard) {
            move_axes.x -= 1.0;
        }
        if keybinds.key_pressed(EditorAction::CameraRight, &keyboard) {
            move_axes.x += 1.0;
        }
        if keybinds.key_pressed(EditorAction::CameraDown, &keyboard) {
            move_axes.y -= 1.0;
        }
        if keybinds.key_pressed(EditorAction::CameraUp, &keyboard) {
            move_axes.y += 1.0;
        }
    }

    *nav = CameraNavInput {
        fly_active,
        look_delta,
        zoom_ticks,
        move_axes,
    };
}

/// Runs in `PostUpdate`: ORs all modal-active conditions and sets
/// `ContextActivity` on the modal context entity accordingly.
///
/// Modal is active when any of:
/// - `ActiveModalQuery::is_modal_running()` (a `tick_modal_operator` loop is live)
/// - `BrushDragState::active`
/// - `VertexDragState::active`
/// - `EdgeDragState::active`
/// - `ModalTransformState::active.is_some()`
///
/// Writes `ContextActivity` only on change.
fn update_modal_context_activity(
    modal_query: ActiveModalQuery,
    brush_drag: Res<BrushDragState>,
    vertex_drag: Res<VertexDragState>,
    edge_drag: Res<EdgeDragState>,
    modal_transform: Res<ModalTransformState>,
    modal_ctx: Res<ModalContextEntity>,
    mut activity: Query<&mut ContextActivity>,
) {
    let should_be_active = modal_query.is_modal_running()
        || brush_drag.active
        || vertex_drag.active
        || edge_drag.active
        || modal_transform.active.is_some();

    if let Ok(mut activity) = activity.get_mut(modal_ctx.0) {
        activity.set_if_neq(ContextActivity(should_be_active));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modal_transform::{ActiveModal, ModalConstraint, ModalOp};

    // Build a minimal App that can run `update_modal_context_activity` in PostUpdate.
    // The modal context entity is spawned INACTIVE and stored in ModalContextEntity.
    fn make_test_app() -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<BrushDragState>()
            .init_resource::<VertexDragState>()
            .init_resource::<EdgeDragState>()
            .init_resource::<ModalTransformState>();
        let modal_entity = app.world_mut().spawn(ContextActivity::INACTIVE).id();
        app.world_mut()
            .insert_resource(ModalContextEntity(modal_entity));
        app.add_systems(PostUpdate, update_modal_context_activity);
        (app, modal_entity)
    }

    fn is_active(app: &App, entity: Entity) -> bool {
        app.world()
            .entity(entity)
            .get::<ContextActivity>()
            .map(|a| **a)
            .unwrap_or(false)
    }

    #[test]
    fn all_sources_inactive_context_stays_inactive() {
        let (mut app, modal_entity) = make_test_app();
        app.update();
        assert!(
            !is_active(&app, modal_entity),
            "all sources off => INACTIVE"
        );
    }

    #[test]
    fn brush_drag_activates_context() {
        let (mut app, modal_entity) = make_test_app();
        app.world_mut().resource_mut::<BrushDragState>().active = true;
        app.update();
        assert!(
            is_active(&app, modal_entity),
            "BrushDragState.active => ACTIVE"
        );
        app.world_mut().resource_mut::<BrushDragState>().active = false;
        app.update();
        assert!(
            !is_active(&app, modal_entity),
            "BrushDragState cleared => INACTIVE"
        );
    }

    #[test]
    fn vertex_drag_activates_context() {
        let (mut app, modal_entity) = make_test_app();
        app.world_mut().resource_mut::<VertexDragState>().active = true;
        app.update();
        assert!(
            is_active(&app, modal_entity),
            "VertexDragState.active => ACTIVE"
        );
        app.world_mut().resource_mut::<VertexDragState>().active = false;
        app.update();
        assert!(
            !is_active(&app, modal_entity),
            "VertexDragState cleared => INACTIVE"
        );
    }

    #[test]
    fn edge_drag_activates_context() {
        let (mut app, modal_entity) = make_test_app();
        app.world_mut().resource_mut::<EdgeDragState>().active = true;
        app.update();
        assert!(
            is_active(&app, modal_entity),
            "EdgeDragState.active => ACTIVE"
        );
        app.world_mut().resource_mut::<EdgeDragState>().active = false;
        app.update();
        assert!(
            !is_active(&app, modal_entity),
            "EdgeDragState cleared => INACTIVE"
        );
    }

    #[test]
    fn modal_transform_activates_context() {
        let (mut app, modal_entity) = make_test_app();
        app.world_mut().resource_mut::<ModalTransformState>().active = Some(ActiveModal {
            op: ModalOp::Grab,
            entity: Entity::PLACEHOLDER,
            start_transform: Transform::default(),
            constraint: ModalConstraint::Free,
            start_cursor: Vec2::ZERO,
        });
        app.update();
        assert!(
            is_active(&app, modal_entity),
            "ModalTransformState.active.is_some() => ACTIVE"
        );
        app.world_mut().resource_mut::<ModalTransformState>().active = None;
        app.update();
        assert!(
            !is_active(&app, modal_entity),
            "ModalTransformState cleared => INACTIVE"
        );
    }
}
