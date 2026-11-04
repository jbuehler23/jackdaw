//! `mesh.mirror.*` and `mesh.symmetrize` operators: manage the brush's
//! `ModifierStack` Mirror entries and bake their evaluated geometry into the
//! authored topology.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::bisect_plane::BisectKeep;
use jackdaw_geometry::{MeshMirror, Modifier, ModifierEntry, ModifierStack, evaluate_mirror};
use jackdaw_scene_types::{Brush, BrushPlane};

use jackdaw_editor_core::brush::BrushHalfedge;
use jackdaw_editor_core::brush::modifier_bake::{
    authored_geometry, bake_modifier_stack, rebuild_brush_from_eval,
};
use jackdaw_editor_core::clip_ops::bisect_brush;
use jackdaw_editor_core::draw_brush::{DrawBrushState, env_allows_brush_op};
use jackdaw_editor_core::keybind_focus::KeybindFocus;
use jackdaw_editor_core::selection::Selection;

/// Bake `mirror` into the brush's authored topology: evaluate the mirror and
/// rebuild from the result. Shared by `mesh.symmetrize` and the CSG impls.
pub(crate) fn apply_mirror_to_brush(brush: &mut Brush, mirror: &MeshMirror) {
    if mirror.axes().is_empty() || brush.topology.polygons.is_empty() {
        return;
    }
    let (vertices, face_polygons) = authored_geometry(brush);
    let eval = evaluate_mirror(&vertices, &face_polygons, mirror);
    rebuild_brush_from_eval(brush, &eval);
}

/// Whether `stack` carries at least one `Modifier::Mirror` entry.
fn stack_has_mirror(stack: Option<&ModifierStack>) -> bool {
    stack.is_some_and(|s| {
        s.modifiers
            .iter()
            .any(|e| matches!(e.modifier, Modifier::Mirror(_)))
    })
}

/// Add a default mirror entry (X axis, clipped) to every selected brush that
/// doesn't already carry one. Available with a mirror-less brush selected.
#[operator(
    id = "mesh.mirror.add",
    label = "Add Mirror",
    is_available = can_add_mirror,
    allows_undo = true
)]
pub(crate) fn mesh_mirror_add(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    candidates: Query<(Entity, Option<&ModifierStack>), With<Brush>>,
    mut commands: Commands,
) -> OperatorResult {
    let mut added = false;
    for &entity in &selection.entities {
        let Ok((_, stack)) = candidates.get(entity) else {
            continue;
        };
        if stack_has_mirror(stack) {
            continue;
        }
        let mut stack = stack.cloned().unwrap_or_default();
        stack
            .modifiers
            .push(ModifierEntry::new(Modifier::Mirror(MeshMirror::default())));
        commands.entity(entity).insert(stack);
        added = true;
    }
    if added {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Bake the modifier stack of every selected mirrored brush into its authored
/// topology and remove the component. Available with a mirrored brush
/// selected.
#[operator(
    id = "mesh.mirror.apply",
    label = "Apply Mirror",
    is_available = can_apply_mirror,
    allows_undo = true
)]
pub(crate) fn mesh_mirror_apply(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<(&mut Brush, &ModifierStack)>,
    halfedges: Query<(), With<BrushHalfedge>>,
    mut commands: Commands,
) -> OperatorResult {
    let mut applied = false;
    for &entity in &selection.entities {
        let Ok((mut brush, stack)) = brushes.get_mut(entity) else {
            continue;
        };
        let mods: Vec<&Modifier> = stack.modifiers.iter().map(|e| &e.modifier).collect();
        bake_modifier_stack(&mut brush, &mods);
        if halfedges.contains(entity) {
            commands
                .entity(entity)
                .insert(BrushHalfedge::from_topology(&brush.topology));
        }
        commands.entity(entity).remove::<ModifierStack>();
        applied = true;
    }
    if applied {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Cut every selected mirror-less brush at the default mirror plane (brush
/// local X through zero), keep the positive side, and add a default mirror
/// entry so the discarded half re-appears as the mirrored copy. Available
/// with a mirror-less brush selected.
#[operator(
    id = "mesh.mirror.bisect",
    label = "Bisect and Mirror",
    is_available = can_add_mirror,
    allows_undo = true
)]
pub(crate) fn mesh_mirror_bisect(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<(&mut Brush, Option<&ModifierStack>)>,
    halfedges: Query<(), With<BrushHalfedge>>,
    mut commands: Commands,
) -> OperatorResult {
    let plane = BrushPlane {
        normal: Vec3::X,
        distance: 0.0,
    };
    let mut cut = false;
    for &entity in &selection.entities {
        let Ok((mut brush, stack)) = brushes.get_mut(entity) else {
            continue;
        };
        if stack_has_mirror(stack) {
            continue;
        }
        let Some(half) = bisect_brush(&brush, &plane, BisectKeep::Front) else {
            continue;
        };
        *brush = half;
        if halfedges.contains(entity) {
            commands
                .entity(entity)
                .insert(BrushHalfedge::from_topology(&brush.topology));
        }
        let mut stack = stack.cloned().unwrap_or_default();
        stack
            .modifiers
            .push(ModifierEntry::new(Modifier::Mirror(MeshMirror::default())));
        commands.entity(entity).insert(stack);
        cut = true;
    }
    if cut {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Make every selected brush symmetric across a brush-local axis plane in
/// one step: any live mirror is first baked into the authored topology, then
/// the result is cut at the plane keeping the positive side, mirror-evaluated,
/// and written back as authored topology. No live mirror is left behind; any
/// existing one is removed since the authored topology now holds both halves.
/// Available with a brush selected.
#[operator(
    id = "mesh.symmetrize",
    label = "Symmetrize",
    is_available = can_symmetrize,
    allows_undo = true,
    params(
        axis(String, default = "x", doc = "Brush-local mirror axis: \"x\", \"y\", or \"z\"."),
    ),
)]
pub(crate) fn mesh_symmetrize(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<(&mut Brush, Option<&ModifierStack>)>,
    halfedges: Query<(), With<BrushHalfedge>>,
    mut commands: Commands,
) -> OperatorResult {
    let axis_str = params.as_str("axis").unwrap_or("x");
    let axis = match axis_str {
        "y" => 1,
        "z" => 2,
        "x" => 0,
        other => {
            warn!("mesh.symmetrize: unrecognized axis \"{other}\", falling back to \"x\"");
            0
        }
    };
    let mut normal = Vec3::ZERO;
    normal[axis] = 1.0;
    let plane = BrushPlane {
        normal,
        distance: 0.0,
    };
    let mirror = MeshMirror {
        mirror_x: axis == 0,
        mirror_y: axis == 1,
        mirror_z: axis == 2,
        ..MeshMirror::default()
    };

    let mut symmetrized = false;
    for &entity in &selection.entities {
        let Ok((mut brush, existing_stack)) = brushes.get_mut(entity) else {
            continue;
        };
        let had_stack = existing_stack.is_some();
        if let Some(existing) = existing_stack {
            let mods: Vec<&Modifier> = existing.modifiers.iter().map(|e| &e.modifier).collect();
            bake_modifier_stack(&mut brush, &mods);
        }
        let Some(half) = bisect_brush(&brush, &plane, BisectKeep::Front) else {
            continue;
        };
        *brush = half;
        apply_mirror_to_brush(&mut brush, &mirror);
        if halfedges.contains(entity) {
            commands
                .entity(entity)
                .insert(BrushHalfedge::from_topology(&brush.topology));
        }
        if had_stack {
            commands.entity(entity).remove::<ModifierStack>();
        }
        symmetrized = true;
    }
    if symmetrized {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// `mesh.mirror.add` / `mesh.mirror.bisect` need a selected brush without
/// a Mirror entry in its stack.
pub(crate) fn can_add_mirror(
    keybind_focus: KeybindFocus,
    modal: Res<jackdaw_editor_core::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    candidates: Query<(Entity, Option<&ModifierStack>), With<Brush>>,
) -> bool {
    env_allows_brush_op(&keybind_focus, &modal, &draw_state)
        && selection.entities.iter().any(|&e| {
            candidates
                .get(e)
                .is_ok_and(|(_, stack)| !stack_has_mirror(stack))
        })
}

/// `mesh.mirror.apply` needs a selected brush carrying a Mirror entry.
pub(crate) fn can_apply_mirror(
    keybind_focus: KeybindFocus,
    modal: Res<jackdaw_editor_core::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    candidates: Query<(Entity, Option<&ModifierStack>), With<Brush>>,
) -> bool {
    env_allows_brush_op(&keybind_focus, &modal, &draw_state)
        && selection.entities.iter().any(|&e| {
            candidates
                .get(e)
                .is_ok_and(|(_, stack)| stack_has_mirror(stack))
        })
}

/// `mesh.symmetrize` needs any selected brush.
pub(crate) fn can_symmetrize(
    keybind_focus: KeybindFocus,
    modal: Res<jackdaw_editor_core::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    candidates: Query<(), With<Brush>>,
) -> bool {
    env_allows_brush_op(&keybind_focus, &modal, &draw_state)
        && selection.entities.iter().any(|&e| candidates.contains(e))
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<MeshMirrorAddOp>()
        .register_operator::<MeshMirrorApplyOp>()
        .register_operator::<MeshMirrorBisectOp>()
        .register_operator::<MeshSymmetrizeOp>();
    // No default keybinds; the unbound action entities keep the ops
    // bindable by presets and user rebinds.
    ctx.action_for::<ExtensionInputContext, MeshMirrorAddOp>();
    ctx.action_for::<ExtensionInputContext, MeshMirrorApplyOp>();
    ctx.action_for::<ExtensionInputContext, MeshMirrorBisectOp>();
    ctx.action_for::<ExtensionInputContext, MeshSymmetrizeOp>();
}
