//! A scene saved while the editor core's library was still named `jackdaw`
//! loads its editor types under their `jackdaw_editor_core::` paths and
//! resaves with them, while the prefab marker keeps its pinned path.
//!
//! The scene markers such as `SceneFogVolume` are never written by a save and
//! have no reflected default, so a document naming one does not spawn it; the
//! old path still has to resolve to the type rather than to nothing.

use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw::entity_ops::SceneFogVolume;
use jackdaw::prefab::components::{PREFAB_TYPE, Prefab};
use jackdaw::reference_image::ReferenceImage;

const OLD_SCENE: &str = include_str!("../fixtures/moved_types/editor_core_lib_jackdaw.bsn");

fn make_app() -> App {
    use bevy::render::RenderPlugin;
    use bevy::render::settings::{RenderCreation, WgpuSettings};
    use bevy::winit::WinitPlugin;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
                    backends: None,
                    ..default()
                })),
                ..default()
            })
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(jackdaw_scene_types::SceneTypesPlugin::default());
    app.add_plugins(jackdaw_bsn::JackdawBsnPlugin);
    app.add_plugins(jackdaw::prefab::PrefabPlugin);
    app.register_type::<ReferenceImage>()
        .register_type::<SceneFogVolume>();
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw::scene_io::SceneFilePath>();
    app.init_resource::<jackdaw::scene_io::SceneDirtyState>();
    app.init_resource::<jackdaw::selection::Selection>();
    app
}

fn named<'w, T: Component>(world: &'w mut World, name: &str) -> Option<&'w T> {
    let entity = world
        .query::<(Entity, &Name)>()
        .iter(world)
        .find(|(_, n)| n.as_str() == name)
        .map(|(entity, _)| entity)?;
    world.get::<T>(entity)
}

#[test]
fn an_old_scene_with_a_prefab_fog_volume_and_reference_image_resaves_with_the_new_paths() {
    assert_eq!(
        ReferenceImage::type_path(),
        "jackdaw_editor_core::reference_image::ReferenceImage"
    );
    assert_eq!(
        SceneFogVolume::type_path(),
        "jackdaw_editor_core::entity_ops::SceneFogVolume"
    );
    assert_eq!(Prefab::type_path(), PREFAB_TYPE);

    let migrated = jackdaw_bsn::migrate::migrate_stamped(OLD_SCENE);
    assert!(migrated.contains(SceneFogVolume::type_path()));
    assert!(!migrated.contains("jackdaw::entity_ops::"));

    let mut app = make_app();
    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), OLD_SCENE);

    let world = app.world_mut();
    let board = named::<ReferenceImage>(world, "Board").expect("the board keeps its image");
    assert_eq!(board.path, "refs/front.png");
    assert!(board.locked);
    assert!(named::<Prefab>(world, "Tree").is_some());

    let text =
        jackdaw::scene_io::emit_bsn_scene_with_inline_assets(world, std::path::Path::new(""));
    assert!(
        text.contains(ReferenceImage::type_path()),
        "the board saves under its new path:\n{text}"
    );
    assert!(
        text.contains(PREFAB_TYPE),
        "the prefab marker keeps its path:\n{text}"
    );
    for old in [
        "jackdaw::reference_image::",
        "jackdaw::entity_ops::",
        "jackdaw::recipes::",
    ] {
        assert!(
            !text.contains(old),
            "{old} is gone from the resaved scene:\n{text}"
        );
    }
}
