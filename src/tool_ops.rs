//! Tool switch operators: Select, Translate, Rotate, Scale.
//!
//! Bound to Q/W/E/R via `bevy_enhanced_input`. Each action is guarded
//! by a `BlockBy` referencing an RMB-held sentinel so the keys do not
//! fire while the user is flying the camera.

use bevy::prelude::*;
use bevy_enhanced_input::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;

use crate::active_tool::ActiveTool;
use crate::core_extension::CoreExtensionInputContext;
use crate::keybind_focus::KeybindFocus;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ToolSelectOp>()
        .register_operator::<ToolTranslateOp>()
        .register_operator::<ToolRotateOp>()
        .register_operator::<ToolScaleOp>();

    // Defaults recorded into the keymap; bindings themselves come from the preset applier.
    ctx.bind_operator::<CoreExtensionInputContext, ToolSelectOp>([PresetInput::key("KeyQ")]);
    ctx.bind_operator::<CoreExtensionInputContext, ToolTranslateOp>([PresetInput::key("KeyW")]);
    ctx.bind_operator::<CoreExtensionInputContext, ToolRotateOp>([PresetInput::key("KeyE")]);
    ctx.bind_operator::<CoreExtensionInputContext, ToolScaleOp>([PresetInput::key("KeyR")]);

    // Spawn the RMB-held guard action (mouse binding: left unchanged) and attach
    // BlockBy to every tool action entity so the tool keys stay silent while the
    // camera fly is active. Must run after bind_operator so the Action<Tool*>
    // entities already exist.
    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        let rmb_guard = world
            .spawn((
                Action::<RmbHeldGuard>::new(),
                ActionOf::<CoreExtensionInputContext>::new(ext),
                bindings![MouseButton::Right],
            ))
            .id();

        let block = BlockBy::single(rmb_guard);
        let mut q = world.query_filtered::<Entity, With<Action<ToolSelectOp>>>();
        for e in q.iter(world).collect::<Vec<_>>() {
            world.entity_mut(e).insert(block.clone());
        }
        let mut q = world.query_filtered::<Entity, With<Action<ToolTranslateOp>>>();
        for e in q.iter(world).collect::<Vec<_>>() {
            world.entity_mut(e).insert(block.clone());
        }
        let mut q = world.query_filtered::<Entity, With<Action<ToolRotateOp>>>();
        for e in q.iter(world).collect::<Vec<_>>() {
            world.entity_mut(e).insert(block.clone());
        }
        let mut q = world.query_filtered::<Entity, With<Action<ToolScaleOp>>>();
        for e in q.iter(world).collect::<Vec<_>>() {
            world.entity_mut(e).insert(block.clone());
        }
    });
}

/// Tool switching is allowed in any edit mode (Object / Vertex / Edge / Face).
/// In-flight brush drags are blocked by `is_modal_running` since drag systems
/// install an `ActiveModalOperator` for their duration.
fn can_change_tool(keybind_focus: KeybindFocus, active: ActiveModalQuery) -> bool {
    !keybind_focus.is_typing() && !active.is_modal_running()
}

/// The Rotate tool shares `KeyE` with brush face Extrude. To keep both from
/// firing on the same press, the Rotate tool is unavailable in Face edit mode
/// (Extrude's home), so E means Extrude there; it stays available (keybind and
/// palette) in object / vertex / edge modes. Until the planned keymap-preset
/// switch lands, this is the context-aware split.
fn can_rotate_tool(
    keybind_focus: KeybindFocus,
    active: ActiveModalQuery,
    edit_mode: Res<crate::brush::EditMode>,
) -> bool {
    can_change_tool(keybind_focus, active)
        && *edit_mode != crate::brush::EditMode::BrushEdit(crate::brush::BrushEditMode::Face)
}

pub(crate) fn tool_select_impl(world: &mut World) {
    world.insert_resource(ActiveTool::Select);
    crate::edit_mode_ops::set_edit_mode_object(world);
}

pub(crate) fn tool_translate_impl(world: &mut World) {
    world.insert_resource(ActiveTool::Translate);
}

pub(crate) fn tool_rotate_impl(world: &mut World) {
    world.insert_resource(ActiveTool::Rotate);
}

pub(crate) fn tool_scale_impl(world: &mut World) {
    world.insert_resource(ActiveTool::Scale);
}

#[operator(id = "tool.select", label = "Select Tool", is_available = can_change_tool)]
pub fn tool_select(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    tool_select_impl(world);
    OperatorResult::Finished
}

#[operator(
    id = "tool.translate",
    label = "Translate Tool",
    is_available = can_change_tool
)]
pub fn tool_translate(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    tool_translate_impl(world);
    OperatorResult::Finished
}

#[operator(id = "tool.rotate", label = "Rotate Tool", is_available = can_rotate_tool)]
pub fn tool_rotate(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    tool_rotate_impl(world);
    OperatorResult::Finished
}

#[operator(id = "tool.scale", label = "Scale Tool", is_available = can_change_tool)]
pub fn tool_scale(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    tool_scale_impl(world);
    OperatorResult::Finished
}

/// Sentinel action: fires while RMB is held. Used as a `BlockBy`
/// guard so Q/W/E/R do not fire during camera fly.
#[derive(InputAction)]
#[action_output(bool)]
struct RmbHeldGuard;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::{BrushEditMode, EditMode};
    use bevy::app::App;
    use bevy::prelude::World;

    fn world_with_resources() -> World {
        let mut app = App::new();
        app.init_resource::<ActiveTool>()
            .init_resource::<EditMode>();
        app.init_resource::<crate::brush::BrushSelection>();
        app.init_resource::<crate::draw_brush::DrawBrushState>();
        std::mem::take(app.world_mut())
    }

    #[test]
    fn tool_select_sets_select_and_resets_edit_mode() {
        let mut world = world_with_resources();
        world.insert_resource(ActiveTool::Translate);
        world.insert_resource(EditMode::BrushEdit(BrushEditMode::Vertex));

        super::tool_select_impl(&mut world);

        assert_eq!(*world.resource::<ActiveTool>(), ActiveTool::Select);
        assert_eq!(*world.resource::<EditMode>(), EditMode::Object);
    }

    #[test]
    fn tool_translate_only_sets_tool() {
        let mut world = world_with_resources();
        world.insert_resource(EditMode::BrushEdit(BrushEditMode::Vertex));

        super::tool_translate_impl(&mut world);

        assert_eq!(*world.resource::<ActiveTool>(), ActiveTool::Translate);
        assert_eq!(
            *world.resource::<EditMode>(),
            EditMode::BrushEdit(BrushEditMode::Vertex)
        );
    }
}
