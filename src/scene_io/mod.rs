//! Scene persistence: saving, loading, document registration, and the
//! legacy JSN read machinery.

use std::any::TypeId;
use std::collections::HashSet;
use std::path::PathBuf;

use bevy::{
    prelude::*,
    window::{PrimaryWindow, RawHandleWrapper},
};

mod legacy;
mod load;
mod registration;
mod save;
pub mod stamp;

pub use legacy::{load_inline_assets, load_scene_from_jsn};
pub(crate) use load::{
    SidecarImport, clear_scene_entities, despawn_scene_entities, import_terrain_sidecars,
};
pub use load::{load_scene, load_scene_from_file};
pub use registration::{register_entities_in_ast, register_entity_in_ast};
pub use save::{
    SaveOutcome, emit_bsn_scene_with_inline_assets, save_layout_to_project, save_scene,
    save_scene_as, save_scene_with_outcome,
};
pub(crate) use save::{emit_bsn_entities_with_inline_assets, save_scene_inner};

use load::poll_scene_dialog;

/// Component type path prefixes that should never be saved (runtime-only / internal).
const SKIP_COMPONENT_PREFIXES: &[&str] = &[
    "bevy_render::",
    "bevy_picking::",
    "bevy_window::",
    "bevy_ecs::observer::",
    "bevy_camera::primitives::",
    "bevy_camera::visibility::",
    // AnimationPlayer / AnimationGraphHandle / AnimationTargetId / AnimatedBy
    // are installed on targets at runtime by the animation plugin.
    // They're derived from the authored clip components and must not be
    // serialized; otherwise load would restore stale player state and
    // dangling asset handles.
    "bevy_animation::",
    // Propagated/inherited values are recomputed from their source every frame
    // (`Inherited<TextColor>`, `Propagate<TextFont>`, ...).
    "bevy_app::propagate::",
    // Widget implementation detail. Feathers styling, cursors, and focus
    // treatment are re-derived by `jackdaw_ui`'s materializer from the
    // authored `Ui*` component, never authored directly.
    "bevy_feathers::",
    // Accessibility nodes are built by the widget implementation.
    "bevy_a11y::",
];

/// Specific component type paths that should never be saved.
const SKIP_COMPONENT_PATHS: &[&str] = &[
    "bevy_transform::components::transform::TransformTreeChanged",
    "bevy_light::cascade::Cascades",
    // Runtime activation state, granted and revoked by the rig systems (the
    // multiplayer gate on clients). Persisting it plants a rig that fights
    // those systems on every load.
    "jackdaw_camera_rig::ActiveCameraRig",
    // Render-state handles are always derived in the editor (brush chunks,
    // terrain chunks, GLTF instances, reference-image quads) and rebuilt
    // from the authored components on load; serializing them would inline
    // runtime mesh/material assets into the scene.
    "bevy_mesh::components::Mesh3d",
    "bevy_pbr::mesh_material::MeshMaterial3d<bevy_pbr::pbr_material::StandardMaterial>",
    // UI layout output. Bevy recomputes all of it every frame from `Node`, and
    // `ComputedUiTargetCamera` additionally holds a view-local camera entity
    // that means nothing in a saved document.
    "bevy_ui::ui_node::ComputedNode",
    "bevy_ui::ui_node::ComputedUiTargetCamera",
    "bevy_ui::ui_node::ComputedUiRenderTargetInfo",
    "bevy_ui::stack::ComputedStackIndex",
    "bevy_ui::ui_transform::UiGlobalTransform",
    "bevy_ui::measurement::ContentSize",
    "bevy_text::text::ComputedTextBlock",
    "bevy_text::text::TextLayoutInfo",
    "bevy_ui::widget::text::TextNodeFlags",
    // Marks an implementation-owned widget part; such an entity is never
    // registered in the document at all, so this is a backstop.
    "jackdaw_ui::UiGeneratedPart",
    "jackdaw_ui::UiMaterialize",
];

/// Paths that override the skip prefixes  -- these are always saved even if
/// they match a skip prefix.
const ALWAYS_SAVE_PATHS: &[&str] = &[
    "bevy_camera::visibility::Visibility",
    // Overrides the `jackdaw::` skip so `apply_ast_to_world` can
    // match selected brushes by stable id across an undo.
    "jackdaw::draw_brush::BrushStableId",
    // The stable node id must persist so a running game can map a live
    // entity back to its authored node. It is written as the structural
    // `JsnEntity::id` field rather than a component entry, but this keeps
    // any other save path from stripping it.
    jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH,
    // Prefab marker components must round-trip through save and AST
    // registration; stripping them breaks instance inheritance and
    // causes `revert_component` to lose track of the prefab source.
    "jackdaw::prefab::components::Prefab",
    "jackdaw::prefab::components::IsA",
    "jackdaw::prefab::components::PrefabEntityId",
    // Reference image boards persist with the scene; the quad mesh and
    // material are derived from this component at runtime.
    "jackdaw::reference_image::ReferenceImage",
];

pub fn should_skip_component(type_path: &str) -> bool {
    // Always-save takes priority over any skip rule
    if ALWAYS_SAVE_PATHS.contains(&type_path) {
        return false;
    }
    if type_path.starts_with("jackdaw::") {
        return true;
    }
    for prefix in SKIP_COMPONENT_PREFIXES {
        if type_path.starts_with(prefix) {
            return true;
        }
    }
    SKIP_COMPONENT_PATHS.contains(&type_path)
}

/// The editor's component skip policy as a [`jackdaw_bsn::BsnWriterConfig`]
/// for the world-to-text BSN writer. Mirrors [`should_skip_component`]
/// (prefixes, exact paths, the `jackdaw::` internals prefix, and the
/// always-save overrides) plus the structural components the engine rebuilds
/// on spawn.
pub fn editor_writer_config() -> jackdaw_bsn::BsnWriterConfig {
    use bevy::reflect::TypePath;

    let mut config = jackdaw_bsn::BsnWriterConfig::include_all();
    config.skip_prefixes.push("jackdaw::".to_string());
    for prefix in SKIP_COMPONENT_PREFIXES {
        config.skip_prefixes.push((*prefix).to_string());
    }
    for path in SKIP_COMPONENT_PATHS {
        config.skip_paths.push((*path).to_string());
    }
    for path in ALWAYS_SAVE_PATHS {
        config.always_save_paths.push((*path).to_string());
    }
    config
        .skip_path(GlobalTransform::type_path())
        .skip_path(InheritedVisibility::type_path())
        .skip_path(ViewVisibility::type_path())
}

/// Component types that never persist to the scene document: derived
/// structural state the engine rebuilds every frame or on spawn (transform
/// propagation, visibility resolution, hierarchy links).
pub(crate) fn structural_skip_type_ids() -> HashSet<TypeId> {
    HashSet::from([
        TypeId::of::<GlobalTransform>(),
        TypeId::of::<InheritedVisibility>(),
        TypeId::of::<ViewVisibility>(),
        TypeId::of::<ChildOf>(),
        TypeId::of::<Children>(),
    ])
}

/// [`structural_skip_type_ids`] plus the document's own bookkeeping
/// components and `Name`, which persists as a `#name` reference patch
/// rather than a component patch.
pub(crate) fn doc_skip_type_ids() -> HashSet<TypeId> {
    let mut ids = structural_skip_type_ids();
    ids.insert(TypeId::of::<Name>());
    ids.insert(TypeId::of::<jackdaw_bsn::AstNodeRef>());
    ids.insert(TypeId::of::<jackdaw_bsn::AstDirty>());
    ids
}

pub struct SceneIoPlugin;

impl Plugin for SceneIoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SceneFilePath>()
            .init_resource::<SceneDirtyState>()
            .add_systems(
                Update,
                poll_scene_dialog.run_if(in_state(crate::AppState::Editor)),
            )
            .add_systems(PostUpdate, deactivate_document_cameras);
    }
}

/// Keeps cameras authored in the scene document from rendering in the
/// editor. `Camera3d` on a document entity pulls in a required `Camera`
/// whose defaults target the primary window at order 0, the same window
/// the editor UI camera composites into, so an authored game camera
/// drew the scene on top of the docks. The components stay on the
/// entity for inspection and save; only rendering is suppressed.
fn deactivate_document_cameras(
    mut cameras: Query<
        &mut bevy::camera::Camera,
        (With<jackdaw_bsn::AstNodeRef>, Without<crate::EditorEntity>),
    >,
) {
    for mut camera in &mut cameras {
        if camera.is_active {
            camera.is_active = false;
        }
    }
}

/// Tracks whether the scene has unsaved changes by comparing the current
/// undo stack length against the length at the time of last save/load/new.
#[derive(Resource, Default)]
pub struct SceneDirtyState {
    pub undo_len_at_save: usize,
}

/// Returns `true` when the scene has unsaved changes.
pub fn is_scene_dirty(world: &World) -> bool {
    let history = world.resource::<jackdaw_commands::CommandHistory>();
    let dirty_state = world.resource::<SceneDirtyState>();
    history.undo_stack.len() != dirty_state.undo_len_at_save
}

/// Stores the currently active scene file path and metadata.
#[derive(Resource, Default)]
pub struct SceneFilePath {
    pub path: Option<String>,
    pub metadata: SceneMetadata,
    pub last_directory: Option<PathBuf>,
}

/// Human-readable metadata for the active scene, tracked live on
/// [`SceneFilePath`]. Mirrors the fields the legacy JSN scene metadata
/// carried, decoupled from `jackdaw_jsn` so only the import boundary
/// (the `From` conversion below) touches that crate.
#[derive(Clone, Debug, Default)]
pub struct SceneMetadata {
    pub name: String,
    pub description: String,
    pub author: String,
    pub created: String,
    pub modified: String,
}

impl From<jackdaw_jsn::format::JsnMetadata> for SceneMetadata {
    fn from(metadata: jackdaw_jsn::format::JsnMetadata) -> Self {
        Self {
            name: metadata.name,
            description: metadata.description,
            author: metadata.author,
            created: metadata.created,
            modified: metadata.modified,
        }
    }
}

fn get_window_handle(world: &mut World) -> Option<RawHandleWrapper> {
    world
        .query_filtered::<&RawHandleWrapper, With<PrimaryWindow>>()
        .single(world)
        .ok()
        .cloned()
}

#[cfg(test)]
mod camera_tests {
    use super::*;

    #[test]
    fn document_cameras_are_deactivated_and_editor_cameras_kept() {
        let mut world = World::new();
        let ast_node = world.spawn_empty().id();
        let authored = world
            .spawn((
                bevy::camera::Camera::default(),
                jackdaw_bsn::AstNodeRef {
                    patches_entity: ast_node,
                },
            ))
            .id();
        let editor = world.spawn(bevy::camera::Camera::default()).id();

        world
            .run_system_cached(deactivate_document_cameras)
            .expect("run the deactivation system");

        let is_active = |world: &World, e| world.get::<bevy::camera::Camera>(e).unwrap().is_active;
        assert!(
            !is_active(&world, authored),
            "a document camera must not render in the editor"
        );
        assert!(
            is_active(&world, editor),
            "non-document cameras keep rendering"
        );
    }
}
