//! Editor-owned BEI input contexts for modal interactions, navigation, and
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
//! after a modal ends.
//!
//! [`NavInputContext`] is always active and carries low priority (default 0).
//! It hosts scroll-bound brush-resize actions (`nav.brush_resize_up` /
//! `nav.brush_resize_down`) alongside the camera navigation actions. Brush
//! resize fires on every scroll tick that passes the `ScrollTick` condition;
//! the terrain sculpt system still gates on Shift being held. Camera zoom is
//! NOT routed through a BEI action: `populate_camera_nav_input` reads
//! `MouseWheel` events directly and normalises Line vs Pixel units before
//! writing `CameraNavInput.zoom_ticks`. Wheel zoom stays a raw axis feed:
//! unit normalization (line vs pixel) is lost through the accumulated BEI
//! path.
//!
//! [`PointerInputContext`] is always active (priority 0) and carries the
//! single `PointerPrimary` action, bound code-level to `MouseButton::Left`
//! with `Down::default()` so BEI tracks the full press/hold/release cycle.
//! The `pointer_primary_just_pressed()` edge is exposed via [`PointerInputs`]
//! via `ActionEvents::START`. LMB release checks and drag/threshold logic in
//! the invoke-trigger systems continue to read raw `ButtonInput<MouseButton>`
//! directly; only the initial press edge is routed through this action.
//! Preset-bindability of LMB itself is not meaningful until click-vs-drag
//! semantics live in the binding layer.
//!
//! # `BuiltinActions` registry
//!
//! [`BuiltinActions`] maps action names (e.g. `"modal.confirm"`) to the action
//! entities that the applier in `keymap.rs` needs in order to attach BEI
//! bindings. The Modal and Navigation arms of `apply_keymap_preset` look up
//! names here; unknown names land in `skipped_unknown_operator` (same slot
//! used for unknown operator ids, shared because the semantics are identical:
//! a preset entry named something that does not exist).
//!
//! # `ModalInputs` `SystemParam`
//!
//! [`ModalInputs`] exposes `confirm()`, `cancel()`, etc. as just-fired
//! booleans for polling modal systems. The mechanism is pull-style polling of
//! the [`ActionEvents`] component on each action entity, which BEI updates
//! every `PreUpdate` frame (see `EnhancedInputSystems::Update`).
//!
//! Specifically, `confirm()` and `cancel()` return `true` when
//! `ActionEvents::START` is set, meaning the key transitioned from not-pressed
//! to pressed on this frame. `step_up()` and `step_down()` return `true` on
//! `ActionEvents::FIRE` because `ScrollTick` fires once per wheel event,
//! making `FIRE` the correct "just happened" signal for scroll. `axis_x/y/z`
//! return `true` on `ActionEvents::START` (first press of the axis key).
//!
//! Held keys cannot leak into the modal context because all modal actions set
//! `require_reset = true`. Re-inserting `Binding` on each INACTIVE -> ACTIVE
//! transition re-arms BEI's `FirstActivation` flag, so keys held from before
//! each modal session are ignored until released.

use bevy::ecs::system::SystemParam;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy_enhanced_input::prelude::{
    Action, ActionEvents, ActionOf, ActionSettings, Binding, BindingOf, ContextActivity,
    ContextPriority, Down, InputAction, InputContextAppExt as _, ModKeys,
};
use jackdaw_api_internal::keymap_conditions::ScrollTick;

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
pub struct ModalInputContext;

/// Low-priority navigation context, always active.
///
/// Uses default priority (0) so every extension context can outrank it when
/// needed.
#[derive(Component, Default)]
pub struct NavInputContext;

/// Always-active context for viewport pointer gestures (priority 0).
///
/// Hosts `PointerPrimary` (LMB). The invoke-trigger systems read
/// `PointerInputs::pointer_primary_just_pressed()` instead of raw
/// `ButtonInput::just_pressed(MouseButton::Left)` for the gesture-start edge.
#[derive(Component, Default)]
pub struct PointerInputContext;

/// Marker on each of the seven modal action entities (confirm, cancel,
/// `axis_x`/`y`/`z`, `step_up`, `step_down`).
///
/// `update_modal_context_activity` queries for binding entities whose
/// [`BindingOf`] target carries this marker, and re-inserts their
/// [`Binding`] component on every INACTIVE -> ACTIVE transition so that
/// BEI's `FirstActivation` flag is reset for the new modal session.
#[derive(Component)]
pub struct ModalAction;

// ---- Action types ----

/// Confirm the active modal interaction (default: Enter).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalConfirm;

/// Cancel the active modal interaction (default: Escape).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalCancel;

/// Constrain modal to the X axis (default: X key).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalAxisX;

/// Constrain modal to the Y axis (default: Y key).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalAxisY;

/// Constrain modal to the Z axis (default: Z key).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalAxisZ;

/// Step the active modal value upward (default: scroll up).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalStepUp;

/// Step the active modal value downward (default: scroll down).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalStepDown;

/// Hold to enter fly-navigation mode (right mouse button, `Down` condition).
///
/// Bound code-level with `Binding::MouseButton { Right }` + `Down` so fly is
/// active every frame RMB is held. This action is NOT registered in
/// `BuiltinActions` and carries no `DefaultKeymap` entry: the binding is
/// hard-wired here, not preset-bindable this pass. Preset-bindability of fly
/// will arrive with the settings UI once `PresetPhase::Hold` (or equivalent)
/// is added to the preset format.
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct NavFly;

/// Mouse motion delta while navigating.
///
/// Bound code-level with `Binding::mouse_motion()`. Not preset-bindable:
/// axis feeds drive a continuous value; only the activation chord (RMB) is
/// the user-rebindable part, and that is gated by `NavFly.fly_active` in the
/// populate system.
#[derive(InputAction, Default)]
#[action_output(Vec2)]
pub struct NavLook;

/// One scroll-up tick while in the nav context.
///
/// Bound code-level with `Binding::mouse_wheel()` + `ScrollTick::new(true)`.
/// Registered as `"nav.brush_resize_up"` in `BuiltinActions` and
/// `DefaultKeymap` so keymap presets can rebind the resize chord. The
/// terrain sculpt system gates on Shift held in keyboard, matching the
/// pre-BEI behavior where scroll-resize and camera zoom coexist via the
/// Shift guard in `jackdaw_camera` (the camera skips zoom when Shift is
/// down; the resize system only fires when Shift is down).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct NavBrushResizeUp;

/// One scroll-down tick while in the nav context.
///
/// Mirror of [`NavBrushResizeUp`] for the downward direction.
/// Registered as `"nav.brush_resize_down"`.
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct NavBrushResizeDown;

/// LMB press/hold while in the pointer context.
///
/// Bound code-level with `Binding::MouseButton { Left }` + `Down::default()`
/// so BEI tracks the full press/hold cycle. The `Down` condition means the
/// action value is `true` every frame LMB is held; `ActionEvents::START` fires
/// only on the first held frame (same-frame as `ButtonInput::just_pressed`).
/// [`PointerInputs`] exposes the start edge via `pointer_primary_just_pressed()`.
///
/// LMB release checks and drag/threshold logic in invoke-trigger systems
/// keep reading raw `ButtonInput<MouseButton>` directly; only the initial
/// press edge is routed here. Preset-bindability of LMB itself is not
/// meaningful until click-vs-drag semantics live in the binding layer.
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct PointerPrimary;

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
        ..default()
    }
}

// ---- Plugin ----

pub(super) struct InputContextsPlugin;

impl Plugin for InputContextsPlugin {
    fn build(&self, app: &mut App) {
        // Register the two BEI contexts. `spawn_contexts` is NOT added here
        // because `extension_lifecycle::plugin` chains it explicitly between
        // `apply_enabled_extensions_startup` and `apply_active_keymap`.
        //
        // `populate_camera_nav_input` runs in `PreUpdate` after
        // `EnhancedInputSystems::Update` so BEI has already evaluated all
        // action values for this frame before we read them. The camera system
        // runs in `Update` (always after `PreUpdate`), so it sees the freshly
        // written `CameraNavInput` values.
        use bevy_enhanced_input::prelude::EnhancedInputSystems;
        app.add_input_context::<ModalInputContext>()
            .add_input_context::<NavInputContext>()
            .add_input_context::<PointerInputContext>()
            .add_systems(PostUpdate, update_modal_context_activity)
            .add_systems(
                PreUpdate,
                populate_camera_nav_input.after(EnhancedInputSystems::Update),
            );
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
            ContextPriority::<ModalInputContext>::new(10),
            ContextActivity::<ModalInputContext>::INACTIVE,
            Name::new("ModalInputContext"),
            crate::EditorEntity,
        ))
        .id();

    // Spawn each modal action as a child of the modal entity.
    // All seven carry ModalAction so the activation system can find their
    // binding entities when re-arming FirstActivation on each session start.
    let confirm = commands
        .spawn((
            Action::<ModalConfirm>::new(),
            modal_action_settings(),
            ActionOf::<ModalInputContext>::new(modal_entity),
            ModalAction,
            ChildOf(modal_entity),
        ))
        .id();
    let cancel = commands
        .spawn((
            Action::<ModalCancel>::new(),
            modal_action_settings(),
            ActionOf::<ModalInputContext>::new(modal_entity),
            ModalAction,
            ChildOf(modal_entity),
        ))
        .id();
    let axis_x = commands
        .spawn((
            Action::<ModalAxisX>::new(),
            modal_action_settings(),
            ActionOf::<ModalInputContext>::new(modal_entity),
            ModalAction,
            ChildOf(modal_entity),
        ))
        .id();
    let axis_y = commands
        .spawn((
            Action::<ModalAxisY>::new(),
            modal_action_settings(),
            ActionOf::<ModalInputContext>::new(modal_entity),
            ModalAction,
            ChildOf(modal_entity),
        ))
        .id();
    let axis_z = commands
        .spawn((
            Action::<ModalAxisZ>::new(),
            modal_action_settings(),
            ActionOf::<ModalInputContext>::new(modal_entity),
            ModalAction,
            ChildOf(modal_entity),
        ))
        .id();
    let step_up = commands
        .spawn((
            Action::<ModalStepUp>::new(),
            modal_action_settings(),
            ActionOf::<ModalInputContext>::new(modal_entity),
            ModalAction,
            ChildOf(modal_entity),
        ))
        .id();
    let step_down = commands
        .spawn((
            Action::<ModalStepDown>::new(),
            modal_action_settings(),
            ActionOf::<ModalInputContext>::new(modal_entity),
            ModalAction,
            ChildOf(modal_entity),
        ))
        .id();

    // ---- Nav context entity ----
    let nav_entity = commands
        .spawn((
            NavInputContext,
            // Default priority = 0 (low).
            Name::new("NavInputContext"),
            crate::EditorEntity,
        ))
        .id();

    // NavFly: code-level binding with `Down` condition so the action value is
    // `true` every frame RMB is held (not just on the rising edge).
    // `Down` is the correct condition for hold semantics; `Press` would fire
    // only on the first frame and leave fly_active false on all subsequent
    // held frames. This action is NOT registered in `BuiltinActions` and has
    // no `DefaultKeymap` entry: it is not preset-bindable this pass.
    let nav_fly_entity = commands
        .spawn((
            Action::<NavFly>::new(),
            ActionOf::<NavInputContext>::new(nav_entity),
            ChildOf(nav_entity),
        ))
        .id();
    commands.spawn((
        Binding::MouseButton {
            button: MouseButton::Right,
            mod_keys: ModKeys::empty(),
        },
        Down::default(),
        BindingOf(nav_fly_entity),
        ChildOf(nav_fly_entity),
    ));

    // NavLook: mouse motion delta. Bound code-level; axis feeds are not
    // preset-bindable (the activation chord is the rebindable part).
    let nav_look_entity = commands
        .spawn((
            Action::<NavLook>::new(),
            ActionOf::<NavInputContext>::new(nav_entity),
            ChildOf(nav_entity),
        ))
        .id();
    commands.spawn((
        Binding::mouse_motion(),
        BindingOf(nav_look_entity),
        ChildOf(nav_look_entity),
    ));

    // NavBrushResizeUp / NavBrushResizeDown: one scroll tick upward/downward.
    // Bound to mouse wheel with ScrollTick so each tick fires FIRE once.
    // Registered in BuiltinActions and DefaultKeymap (PresetContext::Navigation)
    // so keymap presets can rebind the resize chord.
    // Camera zoom is read raw via MouseWheel in populate_camera_nav_input, not
    // through a BEI action; the Shift guard in the camera system keeps zoom and
    // resize from conflicting.
    let nav_resize_up_entity = commands
        .spawn((
            Action::<NavBrushResizeUp>::new(),
            ActionOf::<NavInputContext>::new(nav_entity),
            ChildOf(nav_entity),
        ))
        .id();
    commands.spawn((
        Binding::mouse_wheel(),
        ScrollTick::new(true),
        BindingOf(nav_resize_up_entity),
        ChildOf(nav_resize_up_entity),
    ));

    let nav_resize_down_entity = commands
        .spawn((
            Action::<NavBrushResizeDown>::new(),
            ActionOf::<NavInputContext>::new(nav_entity),
            ChildOf(nav_entity),
        ))
        .id();
    commands.spawn((
        Binding::mouse_wheel(),
        ScrollTick::new(false),
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
            Action::<PointerPrimary>::new(),
            ActionOf::<PointerInputContext>::new(pointer_entity),
            ChildOf(pointer_entity),
        ))
        .id();
    commands.spawn((
        Binding::MouseButton {
            button: MouseButton::Left,
            mod_keys: ModKeys::empty(),
        },
        Down::default(),
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

/// Writes `CameraNavInput` from BEI action values and raw wheel events each
/// `PreUpdate` frame, after `EnhancedInputSystems::Update` has evaluated
/// actions for this frame.
///
/// Ordering: runs in `PreUpdate` after `EnhancedInputSystems::Update`; the
/// camera system runs in `Update`. This guarantees a single write per frame
/// with values that are stable for the entire `Update` phase.
///
/// Zoom is read from `MouseWheel` events directly rather than through a BEI
/// action so that `MouseScrollUnit` normalization is preserved.
/// `MouseScrollUnit::Line` events count as 1.0 tick each; `Pixel` events are
/// scaled by 0.01 to produce an equivalent magnitude. Wheel zoom stays a raw
/// axis feed: unit normalization (line vs pixel) is lost through the
/// accumulated BEI path.
///
/// WASD/QE movement keys are read from raw keyboard this pass. These keys
/// are registered in `KeybindRegistry` as RMB-chords, but the RMB gate is
/// already provided by `fly_active` (the `NavFly` action value). Using
/// `key_pressed` (key only, no mouse check) guarded by `fly_active` is
/// exactly equivalent to the previous `key_chord_pressed` call guarded by
/// `right_held`.
pub(crate) fn populate_camera_nav_input(
    fly_q: Query<&Action<NavFly>>,
    look_q: Query<&Action<NavLook>>,
    mut wheel_events: MessageReader<MouseWheel>,
    keyboard: Res<ButtonInput<KeyCode>>,
    keybinds: Res<KeybindRegistry>,
    mut nav: ResMut<CameraNavInput>,
) {
    let fly_active = fly_q.iter().any(|a| **a);
    let look_delta = look_q.iter().fold(Vec2::ZERO, |acc, a| acc + **a);
    let zoom_ticks: f32 = wheel_events
        .read()
        .map(|ev| match ev.unit {
            MouseScrollUnit::Line => ev.y,
            MouseScrollUnit::Pixel => ev.y * 0.01,
        })
        .sum();

    // WASD/QE: keyboard only, guarded by fly_active. Raw keyboard stays this
    // pass because the camera movement keys live in `KeybindRegistry` (the
    // legacy binding system) and are not yet ported to BEI actions.
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
/// On the INACTIVE -> ACTIVE transition, re-inserts the `Binding` component on
/// every binding entity whose action carries [`ModalAction`]. Re-inserting
/// `Binding` re-arms BEI's `FirstActivation` flag, so keys held from before
/// each modal session are ignored until released.
///
/// Writes `ContextActivity` via `commands.entity().insert()` only on change.
fn update_modal_context_activity(
    modal_query: ActiveModalQuery,
    brush_drag: Res<BrushDragState>,
    vertex_drag: Res<VertexDragState>,
    edge_drag: Res<EdgeDragState>,
    modal_transform: Res<ModalTransformState>,
    modal_ctx: Res<ModalContextEntity>,
    activity: Query<&ContextActivity<ModalInputContext>>,
    bindings_q: Query<(Entity, &Binding, &BindingOf)>,
    modal_actions: Query<(), With<ModalAction>>,
    mut commands: Commands,
) {
    let should_be_active = modal_query.is_modal_running()
        || brush_drag.active
        || vertex_drag.active
        || edge_drag.active
        || modal_transform.active.is_some();

    let current = activity.get(modal_ctx.0).map(|a| **a).unwrap_or(false);

    if current != should_be_active {
        commands
            .entity(modal_ctx.0)
            .insert(ContextActivity::<ModalInputContext>::new(should_be_active));

        if should_be_active {
            // Rising edge: re-arm FirstActivation on all modal binding entities
            // so that any key already held is ignored for this session.
            for (binding_entity, binding, binding_of) in &bindings_q {
                if modal_actions.contains(**binding_of) {
                    commands.entity(binding_entity).insert(*binding);
                }
            }
        }
    }
}

// ---- ModalInputs SystemParam ----

/// System parameter exposing just-fired signals from the modal actions.
///
/// The mechanism is pull-style polling of the [`ActionEvents`] component on
/// each action entity. BEI updates `ActionEvents` every `PreUpdate` in
/// `EnhancedInputSystems::Update`. Systems that run in `Update` (after
/// `PreUpdate`) see a stable value for the whole frame.
///
/// - `confirm`, `cancel`, `axis_x/y/z`: return `true` on `ActionEvents::START`
///   (key transition from not-pressed to pressed; correct for one-shot actions).
/// - `step_up`, `step_down`: return `true` on `ActionEvents::FIRE` (each
///   `ScrollTick` fires once, so FIRE is the right per-tick signal).
#[derive(SystemParam)]
pub struct ModalInputs<'w, 's> {
    confirm_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalConfirm>>>,
    cancel_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalCancel>>>,
    axis_x_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalAxisX>>>,
    axis_y_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalAxisY>>>,
    axis_z_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalAxisZ>>>,
    step_up_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalStepUp>>>,
    step_down_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalStepDown>>>,
}

impl<'w, 's> ModalInputs<'w, 's> {
    fn any_start(q: &Query<'_, '_, &ActionEvents, impl bevy::ecs::query::QueryFilter>) -> bool {
        q.iter().any(|e| e.contains(ActionEvents::START))
    }

    fn any_fire(q: &Query<'_, '_, &ActionEvents, impl bevy::ecs::query::QueryFilter>) -> bool {
        q.iter().any(|e| e.contains(ActionEvents::FIRE))
    }

    /// Returns `true` on the first frame the confirm key (Enter) is pressed.
    pub fn confirm(&self) -> bool {
        Self::any_start(&self.confirm_q)
    }

    /// Returns `true` on the first frame the cancel key (Escape) is pressed.
    pub fn cancel(&self) -> bool {
        Self::any_start(&self.cancel_q)
    }

    /// Returns `true` on the first frame the X-axis key is pressed.
    pub fn axis_x(&self) -> bool {
        Self::any_start(&self.axis_x_q)
    }

    /// Returns `true` on the first frame the Y-axis key is pressed.
    pub fn axis_y(&self) -> bool {
        Self::any_start(&self.axis_y_q)
    }

    /// Returns `true` on the first frame the Z-axis key is pressed.
    pub fn axis_z(&self) -> bool {
        Self::any_start(&self.axis_z_q)
    }

    /// Returns `true` each frame a scroll-up tick fires.
    pub fn step_up(&self) -> bool {
        Self::any_fire(&self.step_up_q)
    }

    /// Returns `true` each frame a scroll-down tick fires.
    pub fn step_down(&self) -> bool {
        Self::any_fire(&self.step_down_q)
    }
}

// ---- NavScrollInputs SystemParam ----

/// System parameter exposing per-tick brush-resize scroll signals from the
/// nav context.
///
/// Uses the same pull-style `ActionEvents::FIRE` mechanism as `ModalInputs`
/// step signals: `ScrollTick` fires once per wheel event, so `FIRE` is the
/// correct per-tick signal. Systems run in `Update` after BEI's `PreUpdate`
/// evaluation; `ActionEvents` is stable for the whole `Update` phase.
#[derive(SystemParam)]
pub struct NavScrollInputs<'w, 's> {
    resize_up_q: Query<'w, 's, &'static ActionEvents, With<Action<NavBrushResizeUp>>>,
    resize_down_q: Query<'w, 's, &'static ActionEvents, With<Action<NavBrushResizeDown>>>,
}

impl<'w, 's> NavScrollInputs<'w, 's> {
    /// Returns `true` each frame a scroll-up tick fires.
    pub fn resize_up(&self) -> bool {
        self.resize_up_q
            .iter()
            .any(|e| e.contains(ActionEvents::FIRE))
    }

    /// Returns `true` each frame a scroll-down tick fires.
    pub fn resize_down(&self) -> bool {
        self.resize_down_q
            .iter()
            .any(|e| e.contains(ActionEvents::FIRE))
    }
}

// ---- PointerInputs SystemParam ----

/// System parameter exposing the LMB press edge from the pointer context.
///
/// `pointer_primary_just_pressed()` returns `true` on the first frame LMB is
/// pressed, matching the semantics of `ButtonInput::just_pressed(MouseButton::Left)`.
///
/// The edge is detected via `ActionEvents::START`: BEI sets START on the frame
/// the `Down` condition transitions from not-met to met, which is the same
/// frame `ButtonInput` reports `just_pressed`. Both are updated before `Update`
/// runs (`ButtonInput` in `First`, BEI in `PreUpdate` via `EnhancedInputSystems::Update`);
/// systems reading either signal in `Update` observe the same press on the same frame.
///
/// `ActionEvents::START` is already a single-frame flag (set only when the action
/// state transitions to active this frame); no previous-frame mirror is needed.
#[derive(SystemParam)]
pub struct PointerInputs<'w, 's> {
    primary_q: Query<'w, 's, &'static ActionEvents, With<Action<PointerPrimary>>>,
}

impl<'w, 's> PointerInputs<'w, 's> {
    /// Returns `true` on the first frame LMB is pressed (rising edge).
    pub fn pointer_primary_just_pressed(&self) -> bool {
        self.primary_q
            .iter()
            .any(|e| e.contains(ActionEvents::START))
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
        let modal_entity = app
            .world_mut()
            .spawn(ContextActivity::<ModalInputContext>::INACTIVE)
            .id();
        app.world_mut()
            .insert_resource(ModalContextEntity(modal_entity));
        app.add_systems(PostUpdate, update_modal_context_activity);
        (app, modal_entity)
    }

    fn is_active(app: &App, entity: Entity) -> bool {
        app.world()
            .entity(entity)
            .get::<ContextActivity<ModalInputContext>>()
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

    // Verifies that the INACTIVE -> ACTIVE rising edge re-inserts Binding on
    // each modal binding entity. BEI's `FirstActivation` is `pub(crate)` so
    // we verify the re-arm indirectly: the component's `changed` tick must
    // advance on the rising edge (Bevy's `column::replace` updates `changed`
    // but not `added` for an existing component), proving the on_insert hook
    // re-ran and FirstActivation was reset.
    #[test]
    fn rearm_re_inserts_binding_on_rising_edge() {
        let (mut app, _modal_entity) = make_test_app();

        // Spawn a ModalAction action entity and a Binding entity pointing at it.
        let action_entity = app.world_mut().spawn(ModalAction).id();
        let test_binding = Binding::Keyboard {
            key: KeyCode::KeyX,
            mod_keys: bevy_enhanced_input::prelude::ModKeys::empty(),
        };
        let binding_entity = app
            .world_mut()
            .spawn((test_binding, BindingOf(action_entity)))
            .id();

        // Run once with everything INACTIVE to establish the baseline tick.
        app.update();
        let tick_before = app
            .world()
            .entity(binding_entity)
            .get_change_ticks::<Binding>()
            .expect("Binding component must exist")
            .changed;

        // Trigger the rising edge.
        app.world_mut().resource_mut::<BrushDragState>().active = true;
        app.update();

        let tick_after = app
            .world()
            .entity(binding_entity)
            .get_change_ticks::<Binding>()
            .expect("Binding component must exist")
            .changed;

        assert!(
            tick_after.get() > tick_before.get(),
            "Binding.changed tick must advance on the INACTIVE->ACTIVE rising edge \
             (re-insert re-arms FirstActivation); before={}, after={}",
            tick_before.get(),
            tick_after.get(),
        );

        // Second activation from an already-ACTIVE state must NOT re-insert
        // (no change, no re-arm).
        app.update();
        let tick_stable = app
            .world()
            .entity(binding_entity)
            .get_change_ticks::<Binding>()
            .expect("Binding component must exist")
            .changed;
        assert_eq!(
            tick_after.get(),
            tick_stable.get(),
            "Binding.changed tick must not advance when context is already ACTIVE"
        );
    }
}
