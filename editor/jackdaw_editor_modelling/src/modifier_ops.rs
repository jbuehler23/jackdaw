//! `modifier.*` operators managing a brush's `ModifierStack`: add, remove,
//! reorder, toggle per-entry flags, and apply (bake) a prefix into the
//! authored topology. The shared `bake_modifier_stack` folds a slice of
//! modifiers and writes the evaluated result back as authored geometry.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::{Modifier, ModifierEntry, ModifierStack};
use jackdaw_scene_types::Brush;

use jackdaw_editor_core::brush::BrushHalfedge;
use jackdaw_editor_core::brush::modifier_bake::bake_modifier_stack;
use jackdaw_editor_core::draw_brush::{DrawBrushState, env_allows_brush_op};
use jackdaw_editor_core::keybind_focus::KeybindFocus;
use jackdaw_editor_core::selection::Selection;

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
    modal: Res<jackdaw_editor_core::modal_transform::ModalTransformState>,
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
    modal: Res<jackdaw_editor_core::modal_transform::ModalTransformState>,
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
    modal: Res<jackdaw_editor_core::modal_transform::ModalTransformState>,
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
    ctx.action_for::<ExtensionInputContext, ModifierAddOp>();
    ctx.action_for::<ExtensionInputContext, ModifierRemoveOp>();
    ctx.action_for::<ExtensionInputContext, ModifierMoveUpOp>();
    ctx.action_for::<ExtensionInputContext, ModifierMoveDownOp>();
    ctx.action_for::<ExtensionInputContext, ModifierToggleOp>();
    ctx.action_for::<ExtensionInputContext, ModifierApplyOp>();
}
