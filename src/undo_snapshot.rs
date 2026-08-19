//! BSN-document-backed implementation of the snapshotter traits.
//!
//! The snapshot captures both the scene document and a set of editor-state
//! resources (edit mode, gizmo mode/space, grid, view overlays, physics
//! overlay). That way Ctrl+Z also reverts "I toggled wireframe" or "I
//! switched to Face mode", matching user expectations. Entity-ref
//! resources (`Selection`, `BrushSelection`) are deliberately excluded
//! because entity ids are re-minted by the snapshot respawn and would
//! dangle.

use std::any::Any;

use bevy::prelude::*;
use jackdaw_api_internal::snapshot::{ActiveSnapshotter, SceneSnapshot, SceneSnapshotter};
use jackdaw_avian_integration::PhysicsOverlayConfig;

use crate::active_tool::ActiveTool;
use crate::brush::EditMode;
use crate::gizmos::GizmoSpace;
use crate::snapping::SnapSettings;
use crate::view_modes::ViewModeSettings;
use crate::viewport_overlays::OverlaySettings;

pub(super) fn plugin(app: &mut App) {
    app.insert_resource(ActiveSnapshotter(Box::new(BsnDocumentSnapshotter)));
}

/// Snapshot of the editor-state resources that should round-trip
/// through undo/redo alongside the scene document.
#[derive(Clone, PartialEq)]
struct EditorStateSnapshot {
    edit_mode: EditMode,
    active_tool: ActiveTool,
    gizmo_space: GizmoSpace,
    snap_settings: SnapSettings,
    view_mode: ViewModeSettings,
    overlays: OverlaySettings,
    physics_overlays: PhysicsOverlayConfig,
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
    }
}

/// BSN-document-backed snapshotter, the [`ActiveSnapshotter`].
///
/// The document is the source of truth, so a snapshot is its emitted text:
/// capture is one emit (no world walk), equality is string equality, and
/// apply reloads the text through the scene loader.
pub struct BsnDocumentSnapshotter;

impl SceneSnapshotter for BsnDocumentSnapshotter {
    fn capture(&self, world: &mut World) -> Box<dyn SceneSnapshot> {
        // Emit through the inline-asset pass so runtime materials and other
        // pathless asset handles on kept components survive undo/redo. The
        // parent path only affects file-backed handles; project root (falling
        // back to the working directory) matches the JSN snapshot path.
        let parent_path = world
            .get_resource::<crate::project::ProjectRoot>()
            .map(|r| r.root.clone())
            .unwrap_or_else(|| std::path::PathBuf::from(""));
        let text = crate::scene_io::emit_bsn_scene_with_inline_assets(world, &parent_path);
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
        // selection and tree rows first, and restore selection by node
        // id after the respawn re-mints entities.
        let selected_node_ids: Vec<jackdaw_scene_types::SceneNodeId> = world
            .get_resource::<crate::selection::Selection>()
            .map(|selection| {
                selection
                    .entities
                    .iter()
                    .filter_map(|&e| world.get::<jackdaw_scene_types::SceneNodeId>(e).copied())
                    .collect()
            })
            .unwrap_or_default();
        if let Some(mut selection) = world.get_resource_mut::<crate::selection::Selection>() {
            selection.entities.clear();
        }
        if let Err(err) = world.run_system_cached(crate::hierarchy::clear_all_tree_rows) {
            error!("Failed to clear tree rows: {err}");
        }

        // Resolve prefab `IsA` references before spawning. The captured text
        // stores inherited descendants sparsely (`PrefabEntityId` plus only
        // diverged fields); the resolver materializes the inherited subtrees
        // back so the respawn produces complete entities. Resolve the cache
        // borrow before the spawn borrow.
        let resolved_text = match world.get_resource::<crate::prefab::PrefabAstCache>() {
            Some(_) => match jackdaw_bsn::parse_bsn_text(&self.text) {
                Ok(authored) => {
                    let cache = world.resource::<crate::prefab::PrefabAstCache>();
                    let get_prefab = |p: &std::path::Path| cache.get(p);
                    match crate::prefab::resolver_bsn::resolve_scene(&authored, &get_prefab) {
                        Ok(resolved) => jackdaw_bsn::emit_scene(&resolved),
                        Err(e) => {
                            warn!("undo snapshot: resolver failed: {e}; spawning unresolved");
                            self.text.clone()
                        }
                    }
                }
                Err(e) => {
                    warn!("undo snapshot: parse failed: {e}; spawning raw text");
                    self.text.clone()
                }
            },
            None => self.text.clone(),
        };

        if let Err(err) = crate::scene_io::despawn_scene_entities(world) {
            error!("undo snapshot: despawn_scene_entities failed: {err}");
        }
        if let Err(err) = jackdaw_bsn::load_bsn_scene(world, &resolved_text) {
            error!("undo snapshot failed to reload: {err}");
        }

        if !selected_node_ids.is_empty()
            && let Some(_) = world.get_resource::<crate::selection::Selection>()
        {
            let restored: Vec<Entity> = {
                let mut query = world.query::<(Entity, &jackdaw_scene_types::SceneNodeId)>();
                query
                    .iter(world)
                    .filter(|(_, id)| selected_node_ids.contains(id))
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
        app
    }

    // App that registers the brush and material types plus asset stores, so a
    // brush face can reference a runtime `StandardMaterial` handle and the
    // captured document can round-trip through the BSN loader.
    fn material_snapshot_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins.build().disable::<TaskPoolPlugin>(),
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
        ));
        app.add_plugins(jackdaw_scene_types::SceneTypesPlugin {
            runtime_mesh_rebuild: false,
        });
        app.add_plugins(jackdaw_bsn::JackdawBsnPlugin);
        app.init_resource::<jackdaw_bsn::SceneBsnAst>();
        app.init_resource::<crate::selection::Selection>();
        app.init_resource::<EditMode>();
        app.init_resource::<ActiveTool>();
        app.init_resource::<GizmoSpace>();
        app.init_resource::<SnapSettings>();
        app.init_resource::<ViewModeSettings>();
        app.init_resource::<OverlaySettings>();
        app.init_resource::<PhysicsOverlayConfig>();
        app
    }

    // A runtime material handle assigned to a brush face must survive a capture:
    // the incremental document records the `Brush` patch without an asset
    // context, so a bare emit would drop the handle. The capture-time inline
    // asset pass embeds the material and rewrites the reference.
    #[test]
    fn bsn_snapshot_embeds_runtime_face_material() {
        use bevy::pbr::StandardMaterial;
        use jackdaw_scene_types::Brush;

        let mut app = material_snapshot_app();

        // Ad-hoc runtime material (no filesystem path) with a non-default color.
        let color = Color::srgb(0.9, 0.1, 0.2);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: color,
                ..Default::default()
            });

        // A brush whose first face references that runtime material.
        let mut brush = Brush::cuboid(1.0, 1.0, 1.0);
        brush.faces[0].material = handle.clone();

        let entity = app
            .world_mut()
            .spawn((Name::new("Cube"), brush.clone()))
            .id();

        // Register the entity in the live document and sync its Brush patch the
        // same way an editor edit does (this is the path that drops the handle).
        jackdaw_bsn::create_entity_in_ast(app.world_mut(), entity, None);
        jackdaw_bsn::sync_to_ast(
            app.world_mut(),
            entity,
            std::any::TypeId::of::<jackdaw_scene_types::Brush>(),
        );

        // Capture the document (the code path undo/redo and save both use).
        let text = crate::scene_io::emit_bsn_scene_with_inline_assets(
            app.world_mut(),
            std::path::Path::new(""),
        );

        // The captured text embeds the material and references it by name, not
        // as an empty string.
        assert!(
            text.contains("StandardMaterial"),
            "captured document must embed the inline material:\n{text}"
        );
        assert!(
            text.contains("\"#StandardMaterial0\""),
            "the runtime face material must emit its inline reference name:\n{text}"
        );

        // Emission is read-only and idempotent: a second capture matches.
        let text2 = crate::scene_io::emit_bsn_scene_with_inline_assets(
            app.world_mut(),
            std::path::Path::new(""),
        );
        assert_eq!(text, text2, "capture must be idempotent");

        // Reload the captured text into a fresh world; the face material must
        // resolve to a real asset with the original base color.
        let mut fresh = material_snapshot_app();
        jackdaw_bsn::load_bsn_scene(fresh.world_mut(), &text).expect("reload captured scene");

        let reloaded_brush = {
            let mut query = fresh.world_mut().query::<&Brush>();
            query
                .iter(fresh.world())
                .next()
                .cloned()
                .expect("brush entity reloaded")
        };
        let face_handle = reloaded_brush.faces[0].material.clone();
        let material = fresh
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&face_handle)
            .expect("face material asset survived the round trip");

        let want = color.to_linear();
        let got = material.base_color.to_linear();
        assert!(
            (want.red - got.red).abs() < 1e-4
                && (want.green - got.green).abs() < 1e-4
                && (want.blue - got.blue).abs() < 1e-4,
            "base color must survive: want {want:?}, got {got:?}"
        );
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
