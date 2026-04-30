//! `SceneJsnAst`-backed implementation of the snapshotter traits.
//!
//! Swapped out for a BSN-backed implementation on BSN migration day.
//!
//! The snapshot captures both the scene AST and a set of editor-state
//! resources (edit mode, gizmo mode/space, grid, view overlays, physics
//! overlay). That way Ctrl+Z also reverts "I toggled wireframe" or "I
//! switched to Face mode", matching user expectations. Entity-ref
//! resources (`Selection`, `BrushSelection`) are deliberately excluded
//! because entity ids are re-minted by `apply_ast_to_world` and would
//! dangle.

use std::any::Any;

use bevy::prelude::*;
use jackdaw_api_internal::snapshot::{ActiveSnapshotter, SceneSnapshot, SceneSnapshotter};
use jackdaw_avian_integration::PhysicsOverlayConfig;
use jackdaw_jsn::SceneJsnAst;

use crate::brush::EditMode;
use crate::gizmos::{GizmoMode, GizmoSpace};
use crate::snapping::SnapSettings;
use crate::view_modes::ViewModeSettings;
use crate::viewport_overlays::OverlaySettings;
use crate::viewport_select::GroupEditState;

pub(super) fn plugin(app: &mut App) {
    app.insert_resource(ActiveSnapshotter(Box::new(JsnAstSnapshotter)));
}

pub struct JsnAstSnapshotter;

impl SceneSnapshotter for JsnAstSnapshotter {
    fn capture(&self, world: &mut World) -> Box<dyn SceneSnapshot> {
        // Re-run the full scene serialization (same pass as
        // `save_scene_inner`) rather than cloning the live AST.
        // `sync_component_to_ast` / `register_entity_in_ast` use the
        // stateless `AstSerializerProcessor` which emits runtime
        // asset handles (ad-hoc materials from `materials.add(...)`)
        // as `null`; cloning that would lose them on every undo.
        // `build_snapshot_ast` uses the inline-asset-aware pipeline,
        // so runtime handles are captured under `#Name` references
        // alongside their serialized data.
        Box::new(JsnAstSnapshot {
            ast: crate::scene_io::build_snapshot_ast(world),
            editor_state: EditorStateSnapshot::capture(world),
        })
    }
}

pub struct JsnAstSnapshot {
    ast: SceneJsnAst,
    editor_state: EditorStateSnapshot,
}

impl SceneSnapshot for JsnAstSnapshot {
    fn apply(&self, world: &mut World) {
        crate::scene_io::apply_ast_to_world(world, &self.ast);
        self.editor_state.apply(world);
    }

    fn equals(&self, other: &dyn SceneSnapshot) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self.ast == o.ast && self.editor_state == o.editor_state)
    }

    fn clone_box(&self) -> Box<dyn SceneSnapshot> {
        Box::new(Self {
            ast: self.ast.clone(),
            editor_state: self.editor_state.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Snapshot of the editor-state resources that should round-trip
/// through undo/redo alongside the scene AST.
#[derive(Clone, PartialEq)]
struct EditorStateSnapshot {
    edit_mode: EditMode,
    gizmo_mode: GizmoMode,
    gizmo_space: GizmoSpace,
    snap_settings: SnapSettings,
    view_mode: ViewModeSettings,
    overlays: OverlaySettings,
    physics_overlays: PhysicsOverlayConfig,
    /// Active `BrushGroup` for group-edit mode. The entity id is
    /// validated against the live world on `apply` because
    /// `apply_ast_to_world` re-mints scene entities; stale ids are
    /// dropped to `None`.
    active_group: Option<Entity>,
}

impl EditorStateSnapshot {
    fn capture(world: &World) -> Self {
        Self {
            edit_mode: *world.resource::<EditMode>(),
            gizmo_mode: *world.resource::<GizmoMode>(),
            gizmo_space: *world.resource::<GizmoSpace>(),
            snap_settings: world.resource::<SnapSettings>().clone(),
            view_mode: world.resource::<ViewModeSettings>().clone(),
            overlays: world.resource::<OverlaySettings>().clone(),
            physics_overlays: world.resource::<PhysicsOverlayConfig>().clone(),
            active_group: world.resource::<GroupEditState>().active_group,
        }
    }

    fn apply(&self, world: &mut World) {
        *world.resource_mut::<EditMode>() = self.edit_mode;
        *world.resource_mut::<GizmoMode>() = self.gizmo_mode;
        *world.resource_mut::<GizmoSpace>() = self.gizmo_space;
        *world.resource_mut::<SnapSettings>() = self.snap_settings.clone();
        *world.resource_mut::<ViewModeSettings>() = self.view_mode.clone();
        *world.resource_mut::<OverlaySettings>() = self.overlays.clone();
        *world.resource_mut::<PhysicsOverlayConfig>() = self.physics_overlays.clone();
        let valid_group = self.active_group.filter(|e| world.get_entity(*e).is_ok());
        world.resource_mut::<GroupEditState>().active_group = valid_group;
    }
}
