//! Scene persistence: saving, loading, document registration, and the
//! legacy JSN read machinery.

use std::any::TypeId;
use std::collections::HashSet;

use bevy::ecs::component::ComponentId;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;

pub(crate) mod asset_fields;
pub(crate) mod asset_names;
pub mod hooks;
mod legacy;
mod load;
mod registration;
pub mod save;
pub mod save_worker;
pub mod stamp;

pub use legacy::{load_inline_assets, load_scene_from_jsn};
pub use load::SidecarImport;
pub use load::{
    LoadOutcome, LoadRefusal, RefusalCategory, declared_scene_kind, declares_ui_scene_root,
    is_ui_scene_root_type_path, load_scene_from_file, load_scene_from_file_with_outcome,
    spawn_default_lighting, spawn_open_dialog,
};
pub(crate) use load::{
    clear_scene_entities, despawn_scene_entities, forget_prefab_cache_bump, prefab_cache_epoch,
};
pub use registration::{register_entities_in_ast, register_entity_in_ast};
pub use save::{
    SaveOutcome, emit_bsn_scene_for_file, emit_bsn_scene_with_inline_assets, retarget_active_scene,
    save_layout_to_project, save_scene, save_scene_as, save_scene_with_outcome,
};
pub(crate) use save::{emit_bsn_entities_with_inline_assets, save_scene_inner};
pub use save_worker::{save_in_background, saves_in_flight, wait_for_saves};

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
    // Widget implementation detail: the marker components and generated parts
    // a feathers control builds for itself. The styling components a widget
    // definition authors are listed in `ALWAYS_SAVE_PATHS` below and override
    // this prefix.
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
    // Which graph state is playing and for how long: written every frame by
    // the evaluator, so a saved value would be stale before the file closed.
    "jackdaw_animation_runtime::graph::AnimationGraphPlayback",
    // Render-state handles are always derived in the editor (brush chunks,
    // terrain chunks, GLTF instances, reference-image quads) and rebuilt
    // from the authored components on load; serializing them would inline
    // runtime mesh/material assets into the scene.
    "bevy_mesh::components::Mesh3d",
    "bevy_pbr::mesh_material::MeshMaterial3d<bevy_pbr::pbr_material::StandardMaterial>",
    // The GLTF instance handle, derived from the authored `GltfSource` by
    // `derive_world_asset_root`. Writing it into the document would put a
    // raw asset handle in a file that other machines and the runtime read.
    "bevy_world_serialization::components::WorldAssetRoot",
    // Editor-managed routing, inserted by `route_ui_roots_to_cameras` to aim a
    // UI scene root at its view: the open 2D viewport for an authored root, the
    // 3D viewport for one a world scene imports. It names a camera entity this
    // session spawned, so a saved copy would point at nothing on reload.
    "bevy_ui::ui_node::UiTargetCamera",
    // Texture usage flags a camera requires and the renderer sets; nothing
    // authors them.
    "bevy_camera::camera::CameraMainTextureUsages",
];

/// The quad and material bevy's sprite renderer puts on every `Sprite`. A
/// save leaves them off an entity that has a `Sprite`, and nowhere else, so
/// an authored `Mesh2d` still saves.
pub(crate) fn sprite_render_component_paths() -> [&'static str; 2] {
    use bevy::reflect::TypePath;
    [
        bevy::mesh::Mesh2d::type_path(),
        bevy::sprite_render::MeshMaterial2d::<bevy::sprite_render::SpriteMeshMaterial>::type_path(),
    ]
}

/// The UI state bevy computes every frame from `Node` and the text or image
/// beside it. None of it is authored, and a document that records it reloads
/// carrying a measurement of the session that saved it.
///
/// Spelled through [`TypePath`] rather than written out: the same list held as
/// strings went stale when `TextLayoutInfo` moved module, and a path that no
/// longer names anything skips nothing and says nothing.
///
/// [`TypePath`]: bevy::reflect::TypePath
pub fn computed_ui_component_paths() -> [&'static str; 10] {
    use bevy::reflect::TypePath;
    [
        bevy::ui::ComputedNode::type_path(),
        bevy::ui::ComputedUiTargetCamera::type_path(),
        bevy::ui::ComputedUiRenderTargetInfo::type_path(),
        bevy::ui::ComputedStackIndex::type_path(),
        bevy::ui::UiGlobalTransform::type_path(),
        bevy::ui::ContentSize::type_path(),
        bevy::text::ComputedTextBlock::type_path(),
        bevy::text::TextLayoutInfo::type_path(),
        bevy::ui::widget::TextNodeFlags::type_path(),
        bevy::ui::widget::ImageNodeSize::type_path(),
    ]
}

/// Paths that override the skip prefixes  -- these are always saved even if
/// they match a skip prefix.
const ALWAYS_SAVE_PATHS: &[&str] = &[
    "bevy_camera::visibility::Visibility",
    // The stable node id must persist so a running game can map a live
    // entity back to its authored node, and so the editor can restore
    // selection across undo and tab swaps. It is written as the
    // structural `JsnEntity::id` field rather than a component entry,
    // but this keeps any other save path from stripping it.
    jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH,
    // Prefab marker components must round-trip through save and AST
    // registration; stripping them breaks instance inheritance and
    // causes `revert_component` to lose track of the prefab source.
    "jackdaw::prefab::components::Prefab",
    "jackdaw::prefab::components::IsA",
    "jackdaw::prefab::components::PrefabEntityId",
    // Reference image boards persist with the scene; the quad mesh and
    // material are derived from this component at runtime.
    "jackdaw_editor_core::reference_image::ReferenceImage",
    // Authored feathers styling, as opposed to the derived styling the
    // `bevy_feathers::` skip above covers. These are plain `Reflect` components
    // widget creation puts on an authored node so the widget follows the theme
    // rather than a colour frozen at spawn time. Dropping them on save reloads
    // the document as flat boxes.
    "bevy_feathers::theme::ThemeBackgroundColor",
    "bevy_feathers::theme::ThemeBorderColor",
    "bevy_feathers::theme::ThemeTextColor",
    "bevy_feathers::theme::InheritableThemeTextColor",
    "bevy_feathers::theme::ThemedText",
    "bevy_feathers::controls::button::ButtonVariant",
    "bevy_feathers::focus::FocusIndicator",
    "bevy_picking::cursor::EntityCursor",
];

/// The editor's own crates, each spelled out so a game named `jackdaw_editor_*`
/// keeps its components. Theirs are editor bookkeeping and never persist unless
/// [`ALWAYS_SAVE_PATHS`] names them.
const EDITOR_TYPE_PREFIXES: &[&str] = &[
    "jackdaw::",
    "jackdaw_editor_animation::",
    "jackdaw_editor_core::",
    "jackdaw_editor_launcher::",
    "jackdaw_editor_modelling::",
    "jackdaw_editor_remote::",
    "jackdaw_editor_terrain::",
    "jackdaw_editor_tile::",
];

pub fn should_skip_component(type_path: &str) -> bool {
    // Always-save takes priority over any skip rule
    if ALWAYS_SAVE_PATHS.contains(&type_path) {
        return false;
    }
    if EDITOR_TYPE_PREFIXES
        .iter()
        .any(|prefix| type_path.starts_with(prefix))
    {
        return true;
    }
    for prefix in SKIP_COMPONENT_PREFIXES {
        if type_path.starts_with(prefix) {
            return true;
        }
    }
    SKIP_COMPONENT_PATHS.contains(&type_path)
        || computed_ui_component_paths().contains(&type_path)
        || type_path == crate::worn_material::layered_material_component()
        || type_path == crate::worn_material::foliage_material_component()
        || type_path == crate::worn_material::water_material_component()
}

/// The editor's component skip policy as a [`jackdaw_bsn::BsnWriterConfig`]
/// for the world-to-text BSN writer. Mirrors [`should_skip_component`]
/// (prefixes, exact paths, the editor crates' prefixes, and the
/// always-save overrides) plus the structural components the engine rebuilds
/// on spawn.
pub fn editor_writer_config() -> jackdaw_bsn::BsnWriterConfig {
    use bevy::reflect::TypePath;

    let mut config = jackdaw_bsn::BsnWriterConfig::include_all();
    for prefix in EDITOR_TYPE_PREFIXES {
        config.skip_prefixes.push((*prefix).to_string());
    }
    for prefix in SKIP_COMPONENT_PREFIXES {
        config.skip_prefixes.push((*prefix).to_string());
    }
    for path in SKIP_COMPONENT_PATHS {
        config.skip_paths.push((*path).to_string());
    }
    for path in computed_ui_component_paths() {
        config.skip_paths.push(path.to_string());
    }
    config
        .skip_paths
        .push(crate::worn_material::layered_material_component().to_string());
    config
        .skip_paths
        .push(crate::worn_material::foliage_material_component().to_string());
    config
        .skip_paths
        .push(crate::worn_material::water_material_component().to_string());
    for path in ALWAYS_SAVE_PATHS {
        config.always_save_paths.push((*path).to_string());
    }
    config
        .skip_path(GlobalTransform::type_path())
        .skip_path(InheritedVisibility::type_path())
        .skip_path(ViewVisibility::type_path())
}

/// Component types that never persist as document component patches:
/// engine-derived pose/hierarchy the engine rebuilds, `Name` (a `#name`
/// reference patch), and the document's own bookkeeping.
pub(crate) fn doc_skip_type_ids() -> HashSet<TypeId> {
    HashSet::from([
        TypeId::of::<GlobalTransform>(),
        TypeId::of::<InheritedVisibility>(),
        TypeId::of::<ViewVisibility>(),
        TypeId::of::<ChildOf>(),
        TypeId::of::<Children>(),
        TypeId::of::<Name>(),
        TypeId::of::<jackdaw_bsn::AstNodeRef>(),
        TypeId::of::<jackdaw_bsn::AstDirty>(),
    ])
}

/// Components [`resync_entity_from_ast`] must leave on the entity: document
/// bookkeeping, identity, selection, outliner rows, and prefab override
/// baselines. Everything else is torn down and rebuilt from the AST, including
/// unreflected `#[require]` companions.
fn resync_keep_type_ids() -> HashSet<TypeId> {
    let mut ids = doc_skip_type_ids();
    ids.insert(TypeId::of::<jackdaw_scene_types::SceneNodeId>());
    ids.insert(TypeId::of::<jackdaw_scene_types::PrefabBaseline>());
    ids.insert(TypeId::of::<crate::selection::Selected>());
    ids.insert(TypeId::of::<jackdaw_widgets::tree_view::TreeNodeSource>());
    ids
}

/// Rebuild an entity's scene-derived ECS components from its document node.
///
/// Tears down every component that is not keep-listed or skip-listed, then
/// applies the live AST onto the same entity so `#[require]` companions follow
/// the document rather than lingering. Hierarchy, computed transform/visibility,
/// skip-listed editor/runtime components, and document identity stay, so
/// selection and children survive.
pub(crate) fn resync_entity_from_ast(world: &mut World, entity: Entity) {
    if world.get::<jackdaw_bsn::AstNodeRef>(entity).is_none() {
        return;
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let skip_ids = resync_keep_type_ids();
    let component_ids: Vec<ComponentId> = {
        let Ok(entity_ref) = world.get_entity(entity) else {
            return;
        };
        entity_ref.archetype().iter_components().collect()
    };
    let to_remove: Vec<ComponentId> = {
        let reg = registry.read();
        component_ids
            .into_iter()
            .filter(|&component_id| {
                let Some(info) = world.components().get_info(component_id) else {
                    return false;
                };
                let Some(type_id) = info.type_id() else {
                    return false;
                };
                if skip_ids.contains(&type_id) {
                    return false;
                }
                let fallback_name = info.name();
                let type_path = match reg.get(type_id) {
                    Some(registration) => registration.type_info().type_path_table().path(),
                    None => &*fallback_name,
                };
                !should_skip_component(type_path)
            })
            .collect()
    };
    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
        for component_id in to_remove {
            entity_mut.remove_by_id(component_id);
        }
    }
    jackdaw_bsn::apply_ast_to_ecs(world, entity);
}

/// Forget the references left unresolved, which name assets of the project being
/// left.
pub fn forget_unresolved_references(world: &World) {
    if let Some(unresolved) = world.get_resource::<jackdaw_bsn::UnresolvedReferences>() {
        unresolved.clear();
    }
}

pub struct SceneIoPlugin;

impl Plugin for SceneIoPlugin {
    fn build(&self, app: &mut App) {
        save_worker::plugin(app);
        app.init_resource::<SceneFilePath>()
            .init_resource::<jackdaw_bsn::UnresolvedReferences>()
            .init_resource::<asset_names::EmbeddedNames>()
            .add_systems(
                Update,
                asset_names::relink_arrived_references
                    .run_if(resource_exists_and_changed::<jackdaw_bsn::BsnSceneAssets>),
            )
            .init_resource::<hooks::DocumentIoHooks>()
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
/// entity for inspection and save; only rendering is suppressed. A
/// `SceneCamera` the document has not registered yet is covered too.
fn deactivate_document_cameras(
    mut cameras: Query<
        &mut bevy::camera::Camera,
        (
            Or<(
                With<jackdaw_bsn::AstNodeRef>,
                With<crate::entity_ops::SceneCamera>,
            )>,
            Without<crate::EditorEntity>,
        ),
    >,
) {
    for mut camera in &mut cameras {
        if camera.is_active {
            camera.is_active = false;
        }
    }
}

/// Returns `true` when the scene in front has unsaved changes.
pub fn is_scene_dirty(world: &World) -> bool {
    let has_tab = world
        .get_resource::<crate::scenes::Scenes>()
        .is_some_and(|scenes| scenes.tabs.get(scenes.active).is_some());
    if has_tab {
        return crate::scenes::operators::active_tab_modified(world);
    }
    world
        .get_resource::<jackdaw_commands::CommandHistory>()
        .is_some_and(|history| !history.is_saved())
}

pub use jackdaw_api_internal::document::{SceneFilePath, SceneMetadata};

/// The metadata a legacy JSN scene carried.
pub(crate) fn jsn_scene_metadata(metadata: jackdaw_jsn::format::JsnMetadata) -> SceneMetadata {
    SceneMetadata {
        name: metadata.name,
        description: metadata.description,
        author: metadata.author,
        created: metadata.created,
        modified: metadata.modified,
    }
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
        let unregistered = world
            .spawn((
                bevy::camera::Camera::default(),
                crate::entity_ops::SceneCamera,
            ))
            .id();

        world
            .run_system_cached(deactivate_document_cameras)
            .expect("run the deactivation system");

        let is_active = |world: &World, e| world.get::<bevy::camera::Camera>(e).unwrap().is_active;
        assert!(
            !is_active(&world, authored),
            "a document camera must not render in the editor"
        );
        assert!(
            !is_active(&world, unregistered),
            "nor a scene camera the document has not registered"
        );
        assert!(
            is_active(&world, editor),
            "non-document cameras keep rendering"
        );
    }
}

#[cfg(test)]
mod editor_type_tests {
    #[test]
    fn a_game_named_like_an_editor_crate_keeps_its_components() {
        assert!(!super::should_skip_component(
            "jackdaw_editor_arena::Health"
        ));
        assert!(super::should_skip_component(
            "jackdaw_editor_core::SomeBookkeeping"
        ));
    }

    #[test]
    fn every_editor_crate_is_listed_as_editor_code() {
        let editor_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the crate sits in the editor folder");
        for entry in std::fs::read_dir(editor_dir).expect("the editor folder reads") {
            let name = entry.expect("an entry").file_name();
            let prefix = format!("{}::", name.to_string_lossy());
            assert!(
                super::EDITOR_TYPE_PREFIXES.contains(&prefix.as_str()),
                "{prefix} is missing from EDITOR_TYPE_PREFIXES"
            );
        }
    }
}
