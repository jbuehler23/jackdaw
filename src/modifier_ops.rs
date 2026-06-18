//! `modifier.*` operators managing a brush's `ModifierStack`: add, remove,
//! reorder, toggle per-entry flags, and apply (bake) a prefix into the
//! authored topology. The shared `bake_modifier_stack` folds a slice of
//! modifiers and writes the evaluated result back as authored geometry.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::{Modifier, ModifierEntry, ModifierStack, evaluate_modifier_stack};
use jackdaw_jsn::Brush;

use crate::brush::BrushHalfedge;
use crate::brush::topology_ops::mirror_ops::{authored_geometry, rebuild_brush_from_eval};
use crate::core_extension::CoreExtensionInputContext;
use crate::draw_brush::{DrawBrushState, env_allows_brush_op};
use crate::keybind_focus::KeybindFocus;
use crate::selection::Selection;

/// Fold `modifiers` over the brush's authored topology and write the result
/// back as the new authored topology. A no-op when the fold is identity.
pub(crate) fn bake_modifier_stack(brush: &mut Brush, modifiers: &[&Modifier]) {
    if brush.topology.polygons.is_empty() {
        return;
    }
    let (vertices, face_polygons) = authored_geometry(brush);
    let eval = evaluate_modifier_stack(&vertices, &face_polygons, &brush.faces, modifiers);
    rebuild_brush_from_eval(brush, &eval);
}

/// Bake `entity`'s whole modifier stack into its authored topology and remove
/// the component. After baking, the authored topology holds the evaluated
/// result, so the stack must go or the next mesh rebuild would fold it again.
/// A stale `BrushHalfedge` would overwrite the new topology on its next
/// flatten, so it is re-lifted when present (same handling as `apply_brush`).
pub(crate) fn bake_modifier_stack_entity(world: &mut World, entity: Entity) {
    let Some(stack) = world.get::<ModifierStack>(entity).cloned() else {
        return;
    };
    let relift = world.get::<BrushHalfedge>(entity).is_some();
    let Some(mut brush) = world.get_mut::<Brush>(entity) else {
        return;
    };
    let mods: Vec<&Modifier> = stack.modifiers.iter().map(|e| &e.modifier).collect();
    bake_modifier_stack(&mut brush, &mods);
    let topology = relift.then(|| brush.topology.clone());
    if let Some(topology) = topology {
        world
            .entity_mut(entity)
            .insert(BrushHalfedge::from_topology(&topology));
    }
    world.entity_mut(entity).remove::<ModifierStack>();
}

/// Append a modifier of the named kind to every selected brush's stack,
/// inserting an empty `ModifierStack` first when one is missing. Available
/// with any brush selected.
#[operator(
    id = "modifier.add",
    label = "Add Modifier",
    is_available = can_edit_any_modifier_brush,
    allows_undo = true,
    params(
        kind(String, default = "mirror", doc = "Modifier kind to add, e.g. \"mirror\"."),
    ),
)]
pub(crate) fn modifier_add(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<Option<&mut ModifierStack>, With<Brush>>,
    mut commands: Commands,
) -> OperatorResult {
    let kind = params.as_str("kind").unwrap_or("mirror");
    let Some(modifier) = Modifier::from_kind(kind) else {
        warn!("modifier.add: unrecognized kind \"{kind}\"");
        return OperatorResult::Cancelled;
    };

    let mut added = false;
    for &entity in &selection.entities {
        let Ok(stack) = brushes.get_mut(entity) else {
            continue;
        };
        match stack {
            Some(mut stack) => stack.modifiers.push(ModifierEntry::new(modifier.clone())),
            None => {
                commands.entity(entity).insert(ModifierStack {
                    modifiers: vec![ModifierEntry::new(modifier.clone())],
                });
            }
        }
        added = true;
    }
    if added {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Remove the entry at `index` from every selected brush's stack. Drops the
/// `ModifierStack` component when its last entry goes. Available with a
/// selected brush carrying a non-empty stack.
#[operator(
    id = "modifier.remove",
    label = "Remove Modifier",
    is_available = can_edit_modifier_stack,
    allows_undo = true,
    params(
        index(i64, doc = "Index of the stack entry to remove."),
    ),
)]
pub(crate) fn modifier_remove(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<&mut ModifierStack, With<Brush>>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(index) = params.as_int("index") else {
        return OperatorResult::Cancelled;
    };
    let index = index as usize;

    let mut removed = false;
    for &entity in &selection.entities {
        let Ok(mut stack) = brushes.get_mut(entity) else {
            continue;
        };
        if index >= stack.modifiers.len() {
            continue;
        }
        stack.modifiers.remove(index);
        if stack.modifiers.is_empty() {
            commands.entity(entity).remove::<ModifierStack>();
        }
        removed = true;
    }
    if removed {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Swap the entry at `index` with the one above it on every selected brush.
/// No-op at the top of the stack. Available with two or more modifiers.
#[operator(
    id = "modifier.move_up",
    label = "Move Modifier Up",
    is_available = can_reorder_modifier_stack,
    allows_undo = true,
    params(
        index(i64, doc = "Index of the stack entry to move up."),
    ),
)]
pub(crate) fn modifier_move_up(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<&mut ModifierStack, With<Brush>>,
) -> OperatorResult {
    let Some(index) = params.as_int("index") else {
        return OperatorResult::Cancelled;
    };
    let index = index as usize;

    let mut moved = false;
    for &entity in &selection.entities {
        let Ok(mut stack) = brushes.get_mut(entity) else {
            continue;
        };
        if index == 0 || index >= stack.modifiers.len() {
            continue;
        }
        stack.modifiers.swap(index, index - 1);
        moved = true;
    }
    if moved {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Swap the entry at `index` with the one below it on every selected brush.
/// No-op at the bottom of the stack. Available with two or more modifiers.
#[operator(
    id = "modifier.move_down",
    label = "Move Modifier Down",
    is_available = can_reorder_modifier_stack,
    allows_undo = true,
    params(
        index(i64, doc = "Index of the stack entry to move down."),
    ),
)]
pub(crate) fn modifier_move_down(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<&mut ModifierStack, With<Brush>>,
) -> OperatorResult {
    let Some(index) = params.as_int("index") else {
        return OperatorResult::Cancelled;
    };
    let index = index as usize;

    let mut moved = false;
    for &entity in &selection.entities {
        let Ok(mut stack) = brushes.get_mut(entity) else {
            continue;
        };
        // `saturating_sub` keeps a wrapped (negative-cast) index from
        // overflowing; the last movable index is `len - 2`.
        if index >= stack.modifiers.len().saturating_sub(1) {
            continue;
        }
        stack.modifiers.swap(index, index + 1);
        moved = true;
    }
    if moved {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Flip one per-entry flag (`enabled` / `in_game` / `on_mesh`) on the entry
/// at `index` for every selected brush. Available with a selected brush
/// carrying a non-empty stack.
#[operator(
    id = "modifier.toggle",
    label = "Toggle Modifier Flag",
    is_available = can_edit_modifier_stack,
    allows_undo = true,
    params(
        index(i64, doc = "Index of the stack entry to toggle."),
        flag(String, default = "enabled", doc = "Flag to flip: \"enabled\", \"in_game\", or \"on_mesh\"."),
    ),
)]
pub(crate) fn modifier_toggle(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<&mut ModifierStack, With<Brush>>,
) -> OperatorResult {
    let Some(index) = params.as_int("index") else {
        return OperatorResult::Cancelled;
    };
    let index = index as usize;
    let flag = params.as_str("flag").unwrap_or("enabled");
    if !matches!(flag, "enabled" | "in_game" | "on_mesh") {
        warn!("modifier.toggle: unrecognized flag \"{flag}\"");
        return OperatorResult::Cancelled;
    }

    let mut toggled = false;
    for &entity in &selection.entities {
        let Ok(mut stack) = brushes.get_mut(entity) else {
            continue;
        };
        let Some(entry) = stack.modifiers.get_mut(index) else {
            continue;
        };
        match flag {
            "in_game" => entry.in_game = !entry.in_game,
            "on_mesh" => entry.on_mesh = !entry.on_mesh,
            _ => entry.enabled = !entry.enabled,
        }
        toggled = true;
    }
    if toggled {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Bake the prefix `[0..=index]` of every selected brush's stack into its
/// authored topology, drop the applied entries, and remove the component when
/// the stack empties. Available with a selected brush carrying a non-empty
/// stack.
#[operator(
    id = "modifier.apply",
    label = "Apply Modifier",
    is_available = can_edit_modifier_stack,
    allows_undo = true,
    params(
        index(i64, doc = "Apply the stack prefix through this entry index."),
    ),
)]
pub(crate) fn modifier_apply(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<(&mut Brush, &mut ModifierStack)>,
    halfedges: Query<(), With<BrushHalfedge>>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(index) = params.as_int("index") else {
        return OperatorResult::Cancelled;
    };
    let index = index as usize;

    let mut applied = false;
    for &entity in &selection.entities {
        let Ok((mut brush, mut stack)) = brushes.get_mut(entity) else {
            continue;
        };
        if index >= stack.modifiers.len() {
            continue;
        }
        let prefix: Vec<&Modifier> = stack.modifiers[0..=index]
            .iter()
            .map(|e| &e.modifier)
            .collect();
        bake_modifier_stack(&mut brush, &prefix);
        stack.modifiers.drain(0..=index);
        if halfedges.contains(entity) {
            commands
                .entity(entity)
                .insert(BrushHalfedge::from_topology(&brush.topology));
        }
        if stack.modifiers.is_empty() {
            commands.entity(entity).remove::<ModifierStack>();
        }
        applied = true;
    }
    if applied {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Any selected brush, the gate for `modifier.add`.
pub(crate) fn can_edit_any_modifier_brush(
    keybind_focus: KeybindFocus,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    candidates: Query<(), With<Brush>>,
) -> bool {
    env_allows_brush_op(&keybind_focus, &modal, &draw_state)
        && selection.entities.iter().any(|&e| candidates.contains(e))
}

/// A selected brush with a non-empty stack, the gate for remove / toggle /
/// apply.
pub(crate) fn can_edit_modifier_stack(
    keybind_focus: KeybindFocus,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    candidates: Query<&ModifierStack, With<Brush>>,
) -> bool {
    env_allows_brush_op(&keybind_focus, &modal, &draw_state)
        && selection.entities.iter().any(|&e| {
            candidates
                .get(e)
                .is_ok_and(|stack| !stack.modifiers.is_empty())
        })
}

/// A selected brush with two or more modifiers, the gate for reordering.
pub(crate) fn can_reorder_modifier_stack(
    keybind_focus: KeybindFocus,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    candidates: Query<&ModifierStack, With<Brush>>,
) -> bool {
    env_allows_brush_op(&keybind_focus, &modal, &draw_state)
        && selection.entities.iter().any(|&e| {
            candidates
                .get(e)
                .is_ok_and(|stack| stack.modifiers.len() >= 2)
        })
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ModifierAddOp>()
        .register_operator::<ModifierRemoveOp>()
        .register_operator::<ModifierMoveUpOp>()
        .register_operator::<ModifierMoveDownOp>()
        .register_operator::<ModifierToggleOp>()
        .register_operator::<ModifierApplyOp>();
    // No default keybinds; the unbound action entities keep the ops
    // bindable by presets and user rebinds.
    ctx.action_for::<CoreExtensionInputContext, ModifierAddOp>();
    ctx.action_for::<CoreExtensionInputContext, ModifierRemoveOp>();
    ctx.action_for::<CoreExtensionInputContext, ModifierMoveUpOp>();
    ctx.action_for::<CoreExtensionInputContext, ModifierMoveDownOp>();
    ctx.action_for::<CoreExtensionInputContext, ModifierToggleOp>();
    ctx.action_for::<CoreExtensionInputContext, ModifierApplyOp>();
}
