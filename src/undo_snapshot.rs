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

use crate::active_tool::ActiveTool;
use crate::brush::EditMode;
use crate::gizmos::GizmoSpace;
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
    active_tool: ActiveTool,
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
            active_tool: *world.resource::<ActiveTool>(),
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
        *world.resource_mut::<ActiveTool>() = self.active_tool;
        *world.resource_mut::<GizmoSpace>() = self.gizmo_space;
        *world.resource_mut::<SnapSettings>() = self.snap_settings.clone();
        *world.resource_mut::<ViewModeSettings>() = self.view_mode.clone();
        *world.resource_mut::<OverlaySettings>() = self.overlays.clone();
        *world.resource_mut::<PhysicsOverlayConfig>() = self.physics_overlays.clone();
        let valid_group = self.active_group.filter(|e| world.get_entity(*e).is_ok());
        world.resource_mut::<GroupEditState>().active_group = valid_group;
    }
}

/// BSN-document-backed snapshotter. Becomes the [`ActiveSnapshotter`] when
/// the live editor document switches to [`jackdaw_bsn::SceneBsnAst`]; until
/// then it exists alongside the JSN one.
///
/// The document is the source of truth, so a snapshot is its emitted text:
/// capture is one emit (no world walk), equality is string equality, and
/// apply reloads the text through the scene loader.
pub struct BsnDocumentSnapshotter;

impl SceneSnapshotter for BsnDocumentSnapshotter {
    fn capture(&self, world: &mut World) -> Box<dyn SceneSnapshot> {
        let text = world
            .get_resource::<jackdaw_bsn::SceneBsnAst>()
            .map(jackdaw_bsn::emit_scene)
            .unwrap_or_default();
        Box::new(BsnDocumentSnapshot {
            text,
            editor_state: EditorStateSnapshot::capture(world),
        })
    }
}

pub struct BsnDocumentSnapshot {
    text: String,
    editor_state: EditorStateSnapshot,
}

impl SceneSnapshot for BsnDocumentSnapshot {
    fn apply(&self, world: &mut World) {
        // Mirror the JSN apply sequence: preserve undo history (despawn
        // directly, never through `clear_scene_entities`), drop stale
        // selection and tree rows first, and restore selection by stable
        // id after the respawn re-mints entities.
        let selected_stable_ids: Vec<crate::draw_brush::BrushStableId> = world
            .get_resource::<crate::selection::Selection>()
            .map(|selection| {
                selection
                    .entities
                    .iter()
                    .filter_map(|&e| world.get::<crate::draw_brush::BrushStableId>(e).copied())
                    .collect()
            })
            .unwrap_or_default();
        if let Some(mut selection) = world.get_resource_mut::<crate::selection::Selection>() {
            selection.entities.clear();
        }
        if let Err(err) = world.run_system_cached(crate::hierarchy::clear_all_tree_rows) {
            error!("Failed to clear tree rows: {err}");
        }

        if let Err(err) = crate::scene_io::despawn_scene_entities(world) {
            error!("undo snapshot: despawn_scene_entities failed: {err}");
        }
        if let Err(err) = jackdaw_bsn::load_bsn_scene(world, &self.text) {
            error!("undo snapshot failed to reload: {err}");
        }

        if !selected_stable_ids.is_empty()
            && let Some(_) = world.get_resource::<crate::selection::Selection>()
        {
            let restored: Vec<Entity> = {
                let mut query = world.query::<(Entity, &crate::draw_brush::BrushStableId)>();
                query
                    .iter(world)
                    .filter(|(_, id)| selected_stable_ids.contains(id))
                    .map(|(e, _)| e)
                    .collect()
            };
            world
                .resource_mut::<crate::selection::Selection>()
                .entities
                .extend(restored);
        }

        self.editor_state.apply(world);
    }

    fn equals(&self, other: &dyn SceneSnapshot) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self.text == o.text && self.editor_state == o.editor_state)
    }

    fn clone_box(&self) -> Box<dyn SceneSnapshot> {
        Box::new(Self {
            text: self.text.clone(),
            editor_state: self.editor_state.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::TaskPoolPlugin;
    use bevy::asset::AssetPlugin;

    fn snapshot_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins.build().disable::<TaskPoolPlugin>(),
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
        ));
        app.init_resource::<jackdaw_bsn::SceneBsnAst>();
        app.init_resource::<crate::selection::Selection>();
        app.init_resource::<EditMode>();
        app.init_resource::<ActiveTool>();
        app.init_resource::<GizmoSpace>();
        app.init_resource::<SnapSettings>();
        app.init_resource::<ViewModeSettings>();
        app.init_resource::<OverlaySettings>();
        app.init_resource::<PhysicsOverlayConfig>();
        app.init_resource::<GroupEditState>();
        app
    }

    fn doc(x: f32) -> String {
        format!(
            "#Node\nbevy_transform::components::transform::Transform {{\n    translation: glam::Vec3 {{ x: {x:?}, y: 0.0, z: 0.0 }},\n}}\n"
        )
    }

    #[test]
    fn bsn_snapshot_round_trips_document_and_editor_state() {
        let mut app = snapshot_app();
        jackdaw_bsn::load_bsn_scene(app.world_mut(), &doc(1.0)).expect("initial load");

        let snap1 = BsnDocumentSnapshotter.capture(app.world_mut());

        // Replace the scene with a different document.
        crate::scene_io::despawn_scene_entities(app.world_mut()).expect("despawn");
        jackdaw_bsn::load_bsn_scene(app.world_mut(), &doc(2.0)).expect("second load");
        let snap2 = BsnDocumentSnapshotter.capture(app.world_mut());
        assert!(!snap1.equals(&*snap2), "different documents must differ");
        assert!(
            snap1.equals(&*snap1.clone_box()),
            "clone compares equal to itself"
        );

        // Undo back to the first document.
        snap1.apply(app.world_mut());
        let mut query = app.world_mut().query::<(&Name, &Transform)>();
        let (name, transform) = query
            .single(app.world())
            .expect("exactly one scene entity after apply");
        assert_eq!(name.as_str(), "Node");
        assert!((transform.translation.x - 1.0).abs() < 1e-6);

        // Re-capturing after apply matches the original snapshot.
        let snap3 = BsnDocumentSnapshotter.capture(app.world_mut());
        assert!(snap1.equals(&*snap3), "apply then capture is a fixpoint");
    }
}
