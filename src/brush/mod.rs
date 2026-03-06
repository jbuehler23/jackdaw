mod csg;
mod geometry;
mod gizmo_overlay;
mod hull;
mod interaction;
mod mesh;

use std::collections::HashMap;

use bevy::prelude::*;

use crate::commands::EditorCommand;

pub use self::csg::{
    brush_planes_to_world, brushes_intersect, clean_degenerate_faces, subtract_brush,
};
pub use self::geometry::{compute_brush_geometry, compute_face_tangent_axes};
pub use self::hull::HullFace;
pub(crate) use self::hull::merge_hull_triangles;
pub(crate) use self::interaction::{
    BrushDragState, ClipState, EdgeDragState, VertexDragConstraint, VertexDragState,
};
pub use jackdaw_jsn::{Brush, BrushFaceData, BrushPlane};

/// Cached computed geometry (NOT serialized, rebuilt from Brush).
#[derive(Component)]
pub struct BrushMeshCache {
    pub vertices: Vec<Vec3>,
    /// Per-face: ordered vertex indices into `vertices`.
    pub face_polygons: Vec<Vec<usize>>,
    pub face_entities: Vec<Entity>,
}

/// Marker on child entities that render individual brush faces.
#[derive(Component)]
pub struct BrushFaceEntity {
    pub brush_entity: Entity,
    pub face_index: usize,
}

/// Marker: brush is being actively modified and should render with transparent preview materials.
#[derive(Component)]
pub struct BrushPreview;

/// Edit mode: Object (default) or brush editing.
#[derive(Resource, Default, PartialEq, Eq, Clone, Copy, Debug, Reflect)]
pub enum EditMode {
    #[default]
    Object,
    BrushEdit(BrushEditMode),
}

#[derive(PartialEq, Eq, Clone, Copy, Debug, Reflect)]
pub enum BrushEditMode {
    Face,
    Vertex,
    Edge,
    Clip,
}

/// Tracks selected sub-elements within brush edit mode.
#[derive(Resource, Default)]
pub struct BrushSelection {
    pub entity: Option<Entity>,
    pub faces: Vec<usize>,
    pub vertices: Vec<usize>,
    /// Selected edges as normalized (min, max) vertex index pairs.
    pub edges: Vec<(usize, usize)>,
    /// True when face mode was entered via shift+click (temporary peek).
    pub temporary_mode: bool,
}

/// Material palette for brush faces.
#[derive(Resource, Default)]
pub struct BrushMaterialPalette {
    pub materials: Vec<Handle<StandardMaterial>>,
    pub preview_materials: Vec<Handle<StandardMaterial>>,
}

/// Remembers the last texture applied via the texture browser, so new brushes inherit it.
#[derive(Resource, Default)]
pub struct LastUsedTexture {
    pub texture_path: Option<String>,
}

/// Cached texture materials, keyed by asset-relative path.
#[derive(Resource, Default)]
pub struct TextureMaterialCache {
    pub entries: HashMap<String, TextureCacheEntry>,
}

pub struct TextureCacheEntry {
    pub image: Handle<Image>,
    pub material: Handle<StandardMaterial>,
}

pub struct SetBrush {
    pub entity: Entity,
    pub old: Brush,
    pub new: Brush,
    pub label: String,
}

impl EditorCommand for SetBrush {
    fn execute(&self, world: &mut World) {
        if let Some(mut brush) = world.get_mut::<Brush>(self.entity) {
            *brush = self.new.clone();
        }
    }

    fn undo(&self, world: &mut World) {
        if let Some(mut brush) = world.get_mut::<Brush>(self.entity) {
            *brush = self.old.clone();
        }
    }

    fn description(&self) -> &str {
        &self.label
    }
}

pub struct BrushPlugin;

impl Plugin for BrushPlugin {
    fn build(&self, app: &mut App) {
        // Note: Brush, BrushFaceData, BrushPlane type registration is handled by JsnPlugin
        app.register_type::<EditMode>()
            .register_type::<BrushEditMode>()
            .init_resource::<EditMode>()
            .init_resource::<BrushSelection>()
            .init_resource::<BrushMaterialPalette>()
            .init_resource::<TextureMaterialCache>()
            .init_resource::<BrushDragState>()
            .init_resource::<VertexDragState>()
            .init_resource::<EdgeDragState>()
            .init_resource::<ClipState>()
            .init_resource::<LastUsedTexture>()
            .init_resource::<crate::material_definition::MaterialDefinitionCache>()
            .init_resource::<crate::material_definition::MaterialLibrary>()
            .add_systems(
                OnEnter(crate::AppState::Editor),
                mesh::setup_default_materials,
            )
            .add_systems(
                Update,
                (
                    interaction::handle_edit_mode_keys,
                    mesh::ensure_texture_materials,
                    mesh::ensure_material_definitions,
                    mesh::set_texture_repeat_mode,
                    mesh::set_material_def_repeat_mode,
                    mesh::sync_brush_preview,
                    mesh::regenerate_brush_meshes,
                    mesh::apply_brush_preview_materials,
                    interaction::brush_face_interact,
                    interaction::brush_vertex_interact,
                    interaction::brush_edge_interact,
                    interaction::handle_brush_delete,
                    interaction::handle_clip_mode,
                    gizmo_overlay::draw_brush_edit_gizmos,
                )
                    .chain()
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}
