//! How a saved scene spells the prefab it instances.
//!
//! The editor holds an instance's source as the absolute file it opened, which
//! resolves on exactly one machine, so the disk boundary rewrites the source
//! relative to the scene file and opening rewrites it back. `prefab_ui_import`
//! covers the same-directory case; here the saved reference has to climb.

use bevy::prelude::*;

const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";

const UI_SCENE_BSN: &str = r#"#Overlay
jackdaw_scene_types::UiSceneRoot { reference_size: glam::UVec2 { x: 800, y: 600 } }
bevy_ui::ui_node::Node
Children [
    #Greeting
    bevy_ui::ui_node::Node
]
"#;

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
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw::scene_io::SceneFilePath>();
    app.init_resource::<jackdaw::scene_io::SceneDirtyState>();
    app.init_resource::<jackdaw::selection::Selection>();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app
}

/// A project laid out the way one is: the scene under `zones/`, the prefab
/// under `ui/`, so the reference between them is not a bare file name.
fn project(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    std::fs::create_dir_all(dir.join("zones")).unwrap();
    std::fs::create_dir_all(dir.join("ui")).unwrap();
    let scene = dir.join("zones/level.bsn");
    std::fs::write(
        &scene,
        "#World\nbevy_transform::components::transform::Transform\n",
    )
    .unwrap();
    let prefab = dir.join("ui/hud.bsn");
    std::fs::write(&prefab, UI_SCENE_BSN).unwrap();
    (scene, prefab)
}

fn open(app: &mut App, scene: &std::path::Path) {
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), scene);
    let index = {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        let n = scenes.tabs.len() as u32 + 1;
        let mut tab = jackdaw::scenes::SceneTab::new_untitled(n);
        tab.path = Some(scene.to_path_buf());
        scenes.push_tab(tab)
    };
    app.world_mut()
        .resource_mut::<jackdaw::scenes::Scenes>()
        .active = index;
}

fn written_source(scene: &std::path::Path) -> String {
    let text = std::fs::read_to_string(scene).expect("scene written");
    let doc = jackdaw_bsn::parse_bsn_text(&text).expect("the written scene parses");
    let instance = doc
        .entities_with_component(ISA_TYPE)
        .first()
        .copied()
        .unwrap_or_else(|| panic!("no IsA reference on disk; wrote:\n{text}"));
    match jackdaw_bsn::get_bsn_field(&doc, instance, ISA_TYPE, "source") {
        Some(jackdaw_bsn::BsnValue::String(s)) => s,
        other => panic!("expected a source string, got {other:?}; wrote:\n{text}"),
    }
}

fn live_source(app: &App) -> String {
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let instance = ast
        .entities_with_component(ISA_TYPE)
        .first()
        .copied()
        .expect("one IsA instance node in the live document");
    match jackdaw_bsn::get_bsn_field(ast, instance, ISA_TYPE, "source") {
        Some(jackdaw_bsn::BsnValue::String(s)) => s,
        other => panic!("expected a source string, got {other:?}"),
    }
}

fn entity_named(app: &mut App, name: &str) -> Option<Entity> {
    let mut q = app.world_mut().query::<(Entity, &Name)>();
    q.iter(app.world())
        .find(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
}

#[test]
fn a_saved_scene_names_its_prefab_relative_to_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let (scene, prefab) = project(tmp.path());
    let mut app = make_app();
    open(&mut app, &scene);
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &prefab, Vec3::ZERO);

    assert_eq!(
        live_source(&app),
        prefab.to_string_lossy(),
        "in memory the source is the file the editor opened"
    );
    assert!(jackdaw::scene_io::save_scene(app.world_mut()), "saved");
    assert_eq!(
        written_source(&scene),
        "../ui/hud.bsn",
        "on disk it is the way from the scene to the prefab"
    );
}

#[test]
fn reopening_the_saved_scene_rebuilds_the_inherited_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let (scene, prefab) = project(tmp.path());
    let mut app = make_app();
    open(&mut app, &scene);
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &prefab, Vec3::ZERO);
    assert!(jackdaw::scene_io::save_scene(app.world_mut()), "saved");

    // A fresh editor, so nothing but the file carries the reference.
    let mut reopened = make_app();
    open(&mut reopened, &scene);

    assert!(
        entity_named(&mut reopened, "Greeting").is_some(),
        "the relative reference resolved and the inherited child spawned"
    );
    assert_eq!(
        live_source(&reopened),
        prefab.to_string_lossy(),
        "opening puts the file back, so the cache and the inspector can be asked"
    );
}

#[test]
fn saving_twice_writes_the_same_reference() {
    let tmp = tempfile::tempdir().unwrap();
    let (scene, prefab) = project(tmp.path());
    let mut app = make_app();
    open(&mut app, &scene);
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &prefab, Vec3::ZERO);
    assert!(jackdaw::scene_io::save_scene(app.world_mut()), "saved");

    let mut reopened = make_app();
    open(&mut reopened, &scene);
    assert!(
        jackdaw::scene_io::save_scene(reopened.world_mut()),
        "saved again"
    );

    assert_eq!(
        written_source(&scene),
        "../ui/hud.bsn",
        "open and save leave the reference where they found it"
    );
}

/// A variant is a prefab whose own root carries `IsA`, written by a different
/// function than a scene save, and that function owes the same relative rewrite.
#[test]
fn a_saved_variant_names_its_base_relative_to_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let (scene, prefab) = project(tmp.path());
    let mut app = make_app();
    open(&mut app, &scene);
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &prefab, Vec3::ZERO);

    let instance = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::prefab::IsA>>()
        .iter(app.world())
        .next()
        .expect("the instance spawned");

    // Cut the variant into the scene's directory, so the way back to the base
    // has to climb.
    let variant = tmp.path().join("zones/hud_variant.bsn");
    jackdaw::prefab::operators::save_as_variant(app.world_mut(), instance, &variant);

    assert_eq!(
        written_source(&variant),
        "../ui/hud.bsn",
        "the variant names its base the way anything but this machine can follow"
    );
}

/// A hand-authored scene can name a prefab anywhere on the machine, and the
/// person opening it authored it. The game refuses the same reference; see
/// `jackdaw_runtime`'s resolution tests.
#[test]
fn the_editor_still_follows_a_source_outside_the_project() {
    let tmp = tempfile::tempdir().unwrap();
    let (scene, _) = project(tmp.path());
    let outside = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    let prefab = outside.join("hud.bsn");
    std::fs::write(&prefab, UI_SCENE_BSN).unwrap();

    std::fs::write(
        &scene,
        format!(
            "jackdaw::prefab::components::IsA {{ source: \"{}\", deleted: [] }}\n\
             jackdaw::prefab::components::PrefabEntityId(0)\n",
            prefab.display()
        ),
    )
    .unwrap();

    let mut app = make_app();
    open(&mut app, &scene);

    assert!(
        entity_named(&mut app, "Greeting").is_some(),
        "an absolute source is still followed on the authoring machine"
    );
}
