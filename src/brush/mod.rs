pub(crate) mod box_select;
mod csg;
pub mod edit_mode_systems;
mod geometry;
mod gizmo_overlay;
mod hull;
pub(crate) mod interaction;
pub(crate) mod knife_mode;
pub(crate) mod mesh;
pub mod preview;
pub mod topology_migration;
pub mod topology_ops;

use bevy::prelude::*;

use crate::commands::EditorCommand;

pub use self::csg::{
    brush_planes_to_world, brushes_intersect, clean_degenerate_faces, subtract_brush,
};
pub use self::geometry::{compute_brush_geometry_from_planes, compute_face_tangent_axes};
pub use self::hull::HullFace;
pub(crate) use self::hull::{merge_hull_triangles, rebuild_brush_from_vertices};
pub(crate) use self::interaction::{
    BrushDragCapture, BrushDragState, ClipMode, ClipState, EdgeDragState, VertexDragState,
};
pub use edit_mode_systems::BrushHalfedge;
pub use jackdaw_jsn::{Brush, BrushFaceData, BrushPlane};
pub use knife_mode::{KnifeMode, KnifePathPoint, KnifeSnapKind, KnifeSnapTarget};
pub use preview::{ActivePreview, PreviewMesh, PreviewState};
pub use topology_ops::edge_bevel::EdgeBevelModalState;
pub use topology_ops::edge_slide_modal::EdgeSlideModalState;
pub use topology_ops::extrude::ExtrudeModalState;
pub use topology_ops::inset::InsetModalState;
pub use topology_ops::loop_cut::{LoopCutModalState, LoopCutPreviewLines};
pub use topology_ops::vertex_bevel::VertexBevelModalState;
pub use topology_ops::vertex_slide_modal::VertexSlideModalState;

/// Cached computed geometry (NOT serialized, rebuilt from Brush).
#[derive(Component)]
pub struct BrushMeshCache {
    pub vertices: Vec<Vec3>,
    /// Per-face: ordered vertex indices into `vertices`.
    pub face_polygons: Vec<Vec<usize>>,
    pub face_entities: Vec<Entity>,
}

impl BrushMeshCache {
    /// Unique undirected edges as normalized `(min, max)` vertex-index pairs,
    /// derived from the face polygons. Order follows first appearance.
    pub fn unique_edges(&self) -> Vec<(usize, usize)> {
        let mut unique_edges: Vec<(usize, usize)> = Vec::new();
        for polygon in &self.face_polygons {
            if polygon.len() < 2 {
                continue;
            }
            for i in 0..polygon.len() {
                let a = polygon[i];
                let b = polygon[(i + 1) % polygon.len()];
                let edge = (a.min(b), a.max(b));
                if !unique_edges.contains(&edge) {
                    unique_edges.push(edge);
                }
            }
        }
        unique_edges
    }
}

/// Marker on child entities that render individual brush faces.
/// Brush faces are derived from the parent brush's `Brush` data,
/// so they're always hidden from the outliner and excluded from
/// the saved scene.
#[derive(Component)]
#[require(crate::EditorHidden, crate::NonSerializable)]
pub struct BrushFaceEntity {
    pub brush_entity: Entity,
    pub face_index: usize,
}

/// Marker: brush is being actively modified and should render with transparent preview materials.
#[derive(Component)]
pub struct BrushPreview;

/// Edit mode: Object (default), brush editing, or the Hammer-style physics
/// placement tool.
#[derive(Resource, Default, PartialEq, Eq, Clone, Copy, Debug, Reflect)]
pub enum EditMode {
    #[default]
    Object,
    BrushEdit(BrushEditMode),
    Physics,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug, Reflect)]
pub enum BrushEditMode {
    Face,
    Vertex,
    Edge,
    Clip,
    Knife,
}

/// Per-brush sub-element selection (faces, vertices, edges).
#[derive(Default, Clone)]
pub struct BrushSubSelection {
    pub faces: Vec<usize>,
    pub vertices: Vec<usize>,
    /// Selected edges as normalized (min, max) vertex index pairs.
    pub edges: Vec<(usize, usize)>,
}

impl BrushSubSelection {
    /// Clear all selected faces, vertices, and edges.
    pub fn clear(&mut self) {
        self.faces.clear();
        self.vertices.clear();
        self.edges.clear();
    }
}

/// Tracks selected sub-elements within brush edit mode.
///
/// Multiple brushes can be in edit mode simultaneously (their handles shown).
/// `active_brush` is the single brush that single-brush consumers (clip mode,
/// inspector, material apply) act on; it is set to the last entered or clicked brush.
#[derive(Resource, Default, Clone)]
pub struct BrushSelection {
    /// Edit brushes (whose handles are shown) and their per-brush selected sub-elements.
    pub brushes: std::collections::HashMap<Entity, BrushSubSelection>,
    /// The brush single-brush consumers (clip mode, inspector, material apply) act on.
    /// Set to the last entered / clicked brush.
    pub active_brush: Option<Entity>,
    /// Remembered face from the last time face mode was exited (for extend-to-brush fallback).
    pub last_face_entity: Option<Entity>,
    pub last_face_index: Option<usize>,
}

impl BrushSelection {
    /// Clear the active selection (edit brushes + `active_brush`).
    /// Leaves `last_face_*` untouched so the extend-to-brush fallback
    /// still works after deselecting.
    pub fn clear(&mut self) {
        self.brushes.clear();
        self.active_brush = None;
    }

    /// Empty every edit brush's sub-selection while staying in edit mode
    /// (distinct from `clear`, which also drops the edit-brush set).
    pub fn clear_sub_selections(&mut self) {
        for sub in self.brushes.values_mut() {
            sub.clear();
        }
    }

    /// Sub-selection for one brush, if it is an edit brush.
    pub fn sub(&self, e: Entity) -> Option<&BrushSubSelection> {
        self.brushes.get(&e)
    }

    /// Mutable sub-selection for a brush, inserting an empty one if absent.
    pub fn sub_mut(&mut self, e: Entity) -> &mut BrushSubSelection {
        self.brushes.entry(e).or_default()
    }

    /// The brushes currently in edit mode (handles shown).
    pub fn edit_brushes(&self) -> impl Iterator<Item = Entity> + '_ {
        self.brushes.keys().copied()
    }

    /// Sub-selection of the active brush.
    pub fn active_sub(&self) -> Option<&BrushSubSelection> {
        self.active_brush.and_then(|e| self.brushes.get(&e))
    }
}

/// Intent for face hover highlight color.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum HoverIntent {
    #[default]
    PushPull,
    Extend,
}

/// Tracks which brush sub-element the cursor is hovering over.
///
/// `entity` is the brush entity; `face_index`, `vertex_index`, and `edge`
/// are set to `Some` for the element type that is currently highlighted.
/// At most one of the three will be `Some` in any given frame.
#[derive(Resource, Default)]
pub struct BrushFaceHover {
    pub entity: Option<Entity>,
    pub face_index: Option<usize>,
    pub vertex_index: Option<usize>,
    pub edge: Option<(usize, usize)>,
    pub intent: HoverIntent,
}

/// Material palette for brush faces.
#[derive(Resource, Default)]
pub struct BrushMaterialPalette {
    pub materials: Vec<Handle<StandardMaterial>>,
    pub preview_materials: Vec<Handle<StandardMaterial>>,
    /// Grid-textured default material at low alpha.
    pub default_material: Handle<StandardMaterial>,
    /// Grid-textured default material at high alpha.
    pub default_selected_material: Handle<StandardMaterial>,
}

/// Remembers the last material applied via the texture/material browser, so new brushes inherit it.
#[derive(Resource, Default)]
pub struct LastUsedMaterial {
    pub material: Option<Handle<StandardMaterial>>,
}

pub struct SetBrush {
    pub entity: Entity,
    pub old: Brush,
    pub new: Brush,
    pub label: String,
}

impl EditorCommand for SetBrush {
    fn execute(&mut self, world: &mut World) {
        apply_brush(world, self.entity, &self.new);
    }

    fn undo(&mut self, world: &mut World) {
        apply_brush(world, self.entity, &self.old);
    }

    fn description(&self) -> &str {
        &self.label
    }

    fn sync_after_external_execute(&self, world: &mut World) {
        // Brush element drags (face / edge / vertex push, knife cut,
        // bridge, etc.) mutate the ECS `Brush` directly during the
        // operation. By the time the command reaches the history, the
        // ECS already holds `self.new`; the AST still needs syncing
        // so a later reload doesn't restore the pre-drag state.
        sync_brush_to_ast(world, self.entity, &self.new);
    }
}

/// Replace `entity`'s `Brush` with `target` and keep dependent components in
/// sync. The renderer reads `BrushHalfedge` (the live half-edge mesh) while
/// the user is in vertex / edge / face / knife mode, so reverting only the
/// `Brush` component leaves the visible mesh stuck at its pre-revert state.
/// We re-lift `BrushHalfedge` from `target.topology` here so undo / redo
/// produce the expected visual result, and flag the inspector for rebuild.
fn apply_brush(world: &mut World, entity: Entity, target: &Brush) {
    if let Some(mut brush) = world.get_mut::<Brush>(entity) {
        *brush = target.clone();
    }
    sync_brush_to_ast(world, entity, target);
    if world.get::<BrushHalfedge>(entity).is_some() && !target.topology.polygons.is_empty() {
        let halfedge = BrushHalfedge::from_topology(&target.topology);
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.insert(halfedge);
        }
    }
    if let Ok(mut ec) = world.get_entity_mut(entity) {
        ec.insert(crate::inspector::InspectorDirty);
    }
}

/// Serialize a Brush component to JSON and store it in the AST.
pub fn sync_brush_to_ast(world: &mut World, entity: Entity, brush: &Brush) {
    // `jackdaw_jsn::types::Brush`; the canonical reflected type
    // path (Brush is defined directly in `jackdaw_jsn::types`, not a
    // `types::brush` submodule; historically this string was wrong
    // and the AST ended up with a `types::brush::Brush` key that
    // `load_scene_from_jsn` then skipped with an `Unknown type`
    // warning and silently lost the Brush on every scene reload).
    crate::commands::sync_component_to_ast(world, entity, "jackdaw_jsn::types::Brush", brush);
}

/// Watch for any `Changed<Brush>` and mirror the new state into the
/// scene AST. This lets callers that mutate `Brush` directly (and
/// push `SetBrush` to history as already-executed via
/// `push_executed`) skip a manual `sync_brush_to_ast` call; without
/// this system, the modal draw-brush operator's `before_snapshot`
/// would capture the pre-mutation AST and an undo across the draw
/// would wipe the prior Brush edit (e.g. undoing a new brush would
/// also strip a material that had been applied beforehand).
///
/// Cloning the Brush per change is cheap (a small `Vec<BrushFaceData>`),
/// and in practice `Changed<Brush>` is near-empty every frame.
fn sync_changed_brushes_to_ast(
    changed: Query<(Entity, &Brush), Changed<Brush>>,
    mut commands: Commands,
) {
    let entries: Vec<(Entity, Brush)> = changed.iter().map(|(e, b)| (e, b.clone())).collect();
    if entries.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        for (entity, brush) in entries {
            sync_brush_to_ast(world, entity, &brush);
        }
    });
}

// `impl EditorMeta for Brush` lives in `jackdaw_jsn` so the orphan
// rule is satisfied (trait and type share a crate); the category
// is "Brush", same as before.

pub struct BrushPlugin;

impl Plugin for BrushPlugin {
    fn build(&self, app: &mut App) {
        // `Brush`/`BrushFaceData`/`BrushPlane` register through
        // `JsnPlugin`. Picker category lives on `Brush` via
        // `#[reflect(@EditorCategory("Brush"))]`.
        app.register_type::<EditMode>()
            .register_type::<BrushEditMode>()
            .init_resource::<EditMode>()
            .init_resource::<BrushSelection>()
            .init_resource::<BrushMaterialPalette>()
            .init_resource::<BrushFaceHover>()
            .init_resource::<BrushDragState>()
            .init_resource::<VertexDragState>()
            .init_resource::<EdgeDragState>()
            .init_resource::<box_select::BrushBoxSelectState>()
            .init_resource::<ClipState>()
            .init_resource::<InsetModalState>()
            .init_resource::<LoopCutModalState>()
            .init_resource::<LoopCutPreviewLines>()
            .init_resource::<ExtrudeModalState>()
            .init_resource::<EdgeSlideModalState>()
            .init_resource::<VertexSlideModalState>()
            .init_resource::<EdgeBevelModalState>()
            .init_resource::<VertexBevelModalState>()
            .init_resource::<KnifeMode>()
            .init_resource::<LastUsedMaterial>()
            .add_plugins(mesh::MeshPlugin)
            .add_plugins(preview::PreviewPlugin)
            .add_systems(
                OnEnter(crate::AppState::Editor),
                mesh::setup_default_materials,
            )
            .add_systems(
                Update,
                (
                    interaction::drop_brush_edit_on_deselect,
                    interaction::brush_face_hover,
                    interaction::brush_vertex_edge_hover,
                    crate::brush_drag_ops::face_drag_invoke_trigger,
                    crate::brush_drag_ops::vertex_drag_invoke_trigger,
                    crate::brush_drag_ops::edge_drag_invoke_trigger,
                    box_select::brush_box_select_promote,
                    crate::clip_ops::place_point_invoke_trigger,
                    interaction::handle_clip_mode,
                    knife_mode::handle_knife_mode,
                )
                    .chain()
                    .in_set(crate::EditorInteractionSystems),
            )
            .add_systems(
                Update,
                (
                    mesh::sync_brush_preview,
                    ApplyDeferred,
                    mesh::recenter_brush_origins,
                    ApplyDeferred,
                    mesh::regenerate_brush_meshes,
                    ApplyDeferred,
                    mesh::ensure_brush_face_materials,
                    gizmo_overlay::draw_brush_edit_gizmos,
                    gizmo_overlay::draw_loop_cut_preview,
                    knife_mode::draw_knife_overlay,
                    box_select::update_brush_box_select_overlay,
                )
                    .chain()
                    .after(crate::EditorInteractionSystems)
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_systems(
                Update,
                sync_changed_brushes_to_ast.run_if(in_state(crate::AppState::Editor)),
            )
            .add_systems(
                Update,
                edit_mode_systems::sync_brush_halfedge_on_edit_mode
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_systems(
                Update,
                topology_migration::migrate_legacy_brush_topology
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

/// Edit brushes for a selection: every selected brush, plus the child brushes
/// of any selected entity that is not itself a brush (e.g. a `BrushGroup`).
/// `is_brush` reports whether an entity has a `Brush`; `children_of` yields an
/// entity's direct children. Order follows the selection; duplicates removed.
pub fn shown_edit_brushes(
    selected: &[Entity],
    is_brush: impl Fn(Entity) -> bool,
    children_of: impl Fn(Entity) -> Vec<Entity>,
) -> Vec<Entity> {
    let mut out = Vec::new();
    for &e in selected {
        if is_brush(e) {
            if !out.contains(&e) {
                out.push(e);
            }
        } else {
            for child in children_of(e) {
                if is_brush(child) && !out.contains(&child) {
                    out.push(child);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod shown_edit_brushes_tests {
    use super::shown_edit_brushes;
    use bevy::prelude::Entity;

    #[test]
    fn brush_and_group_expand_correctly() {
        let brush1 = Entity::from_raw_u32(1).unwrap();
        let group = Entity::from_raw_u32(2).unwrap();
        let gb1 = Entity::from_raw_u32(3).unwrap();
        let gb2 = Entity::from_raw_u32(4).unwrap();
        let unselected = Entity::from_raw_u32(5).unwrap();

        let brushes = [brush1, gb1, gb2, unselected];

        let result = shown_edit_brushes(
            &[brush1, group],
            |e| brushes.contains(&e),
            |e| {
                if e == group {
                    vec![gb1, gb2]
                } else {
                    vec![]
                }
            },
        );

        assert_eq!(result.len(), 3);
        assert!(result.contains(&brush1));
        assert!(result.contains(&gb1));
        assert!(result.contains(&gb2));
        assert!(!result.contains(&unselected));
    }

    #[test]
    fn unselected_brush_excluded() {
        let brush1 = Entity::from_raw_u32(1).unwrap();
        let unselected = Entity::from_raw_u32(2).unwrap();

        let brushes = [brush1, unselected];

        let result = shown_edit_brushes(
            &[brush1],
            |e| brushes.contains(&e),
            |_| vec![],
        );

        assert_eq!(result.len(), 1);
        assert!(result.contains(&brush1));
        assert!(!result.contains(&unselected));
    }

    #[test]
    fn no_duplicates_when_group_child_also_selected() {
        let brush1 = Entity::from_raw_u32(1).unwrap();
        let group = Entity::from_raw_u32(2).unwrap();

        // brush1 is both directly selected and a child of the group
        let result = shown_edit_brushes(
            &[brush1, group],
            |e| e == brush1,
            |e| {
                if e == group {
                    vec![brush1]
                } else {
                    vec![]
                }
            },
        );

        assert_eq!(result.len(), 1);
        assert!(result.contains(&brush1));
    }

    #[test]
    fn empty_selection_yields_empty() {
        let result = shown_edit_brushes(&[], |_| true, |_| vec![]);
        assert!(result.is_empty());
    }
}
