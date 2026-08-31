//! Importing a UI scene into a world scene as a prefab instance.
//!
//! A UI scene is an ordinary `.bsn` whose root carries `UiSceneRoot`, and
//! importing one is the existing `IsA` machinery pointed at that file: the
//! instance root is a member of the world
//! document, the widget tree under it is inherited rather than authored,
//! and the file on disk keeps a reference instead of a copy.
//!
//! The routing assertions are the reason this suite exists apart from
//! `prefab_lifecycle`. An imported root carries `UiSceneRoot` with no
//! `ChildOf`, which is exactly the shape every 2D-viewport query matches,
//! so nothing but a test keeps the world scene's overlay out of the panel
//! that edits UI scenes.

use crate::util;

use bevy::prelude::*;

const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";
const TRANSFORM_TYPE: &str = "bevy_transform::components::transform::Transform";
const PREFAB_ENTITY_ID_TYPE: &str = "jackdaw::prefab::components::PrefabEntityId";
const NODE_TYPE: &str = "bevy_ui::ui_node::Node";

/// A UI scene: one root carrying `UiSceneRoot`, with one named child so
/// the inherited subtree is visible to assertions.
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
    // `save_scene` re-syncs its target path from the active tab, so a save
    // is only reachable through the tab strip.
    app.init_resource::<jackdaw::scenes::Scenes>();
    app
}

/// Push a tab for `path` and return its index.
fn push_tab(app: &mut App, path: &std::path::Path) -> usize {
    let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
    let n = scenes.tabs.len() as u32 + 1;
    let mut tab = jackdaw::scenes::SceneTab::new_untitled(n);
    tab.path = Some(path.to_path_buf());
    scenes.push_tab(tab)
}

/// Write the UI scene fixture and import it into the app's live document.
fn import_ui_scene(app: &mut App, dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("hud.bsn");
    std::fs::write(&path, UI_SCENE_BSN).expect("write UI scene fixture");
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &path, Vec3::ZERO);
    path
}

/// Write a minimal world scene and open it, so the app has a document with
/// a file behind it for the round trip.
fn open_world_scene(app: &mut App, dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("level.bsn");
    std::fs::write(
        &path,
        "#World\nbevy_transform::components::transform::Transform\n",
    )
    .expect("write world scene fixture");
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &path);
    let index = push_tab(app, &path);
    app.world_mut()
        .resource_mut::<jackdaw::scenes::Scenes>()
        .active = index;
    path
}

/// The document node for the sole `IsA` instance in the live document.
fn sole_isa_node(app: &App) -> Entity {
    app.world()
        .resource::<jackdaw_bsn::SceneBsnAst>()
        .entities_with_component(ISA_TYPE)
        .first()
        .copied()
        .expect("one IsA instance node in the live document")
}

fn entity_named(app: &mut App, name: &str) -> Option<Entity> {
    let mut q = app.world_mut().query::<(Entity, &Name)>();
    q.iter(app.world())
        .find(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
}

#[test]
fn an_imported_ui_scene_root_is_a_member_of_the_world_document() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = make_app();
    import_ui_scene(&mut app, tmp.path());

    let node = sole_isa_node(&app);
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    assert!(
        ast.roots.contains(&node),
        "the instance root is a root of the world document, not a child of anything"
    );

    let root = entity_named(&mut app, "Overlay").expect("the imported root spawned");
    assert!(
        app.world()
            .get::<jackdaw_scene_types::UiSceneRoot>(root)
            .is_some(),
        "the instance root inherits UiSceneRoot, so bevy_ui lays it out"
    );
    assert!(
        app.world().get::<ChildOf>(root).is_none(),
        "a UI root must stay a real ECS root"
    );
    assert!(
        app.world().get::<jackdaw::prefab::IsA>(root).is_some(),
        "the instance root carries IsA"
    );
}

#[test]
fn an_imported_ui_root_is_not_given_a_placement_transform() {
    // A 3D instance is placed by a Transform; a UI root is placed by
    // layout, so the importer must not author one it would have to fight.
    let tmp = tempfile::tempdir().unwrap();
    let mut app = make_app();
    import_ui_scene(&mut app, tmp.path());

    let node = sole_isa_node(&app);
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    assert!(
        jackdaw_bsn::get_bsn_field(ast, node, TRANSFORM_TYPE, "").is_none(),
        "no Transform patch is authored on an imported UI scene root"
    );
}

#[test]
fn the_imported_widget_tree_is_inherited_not_authored() {
    // "Read-only" is the existing prefab affordance rather than a separate
    // refusal: an inherited node is one carrying `PrefabEntityId` without
    // `IsA`, which is what mutes its outliner row and puts revert dots on
    // its fields. The live document does hold expanded nodes for them, which
    // is what `resolve_scene` is for, so the invariant to pin is inheritance;
    // the save test pins that they never reach disk.
    let tmp = tempfile::tempdir().unwrap();
    let mut app = make_app();
    import_ui_scene(&mut app, tmp.path());

    let child = entity_named(&mut app, "Greeting").expect("the inherited child spawned");
    assert!(
        app.world()
            .get::<jackdaw::prefab::PrefabEntityId>(child)
            .is_some(),
        "the inherited child is tagged for override lookups"
    );
    assert!(
        app.world().get::<jackdaw::prefab::IsA>(child).is_none(),
        "only the instance root carries IsA, so the child reads as inherited"
    );

    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let node = ast
        .ast_for(child)
        .expect("the resolved child has a document node");
    assert!(
        jackdaw::prefab::overrides_bsn::is_inside_prefab_instance(ast, node),
        "the child is inside a prefab instance, which is the read-only predicate"
    );
}

#[test]
fn saving_the_world_scene_emits_a_reference_not_the_widget_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = make_app();
    let scene_path = open_world_scene(&mut app, tmp.path());
    let ui_path = import_ui_scene(&mut app, tmp.path());
    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the world scene saved"
    );

    let written = std::fs::read_to_string(&scene_path).expect("world scene written");
    let doc = jackdaw_bsn::parse_bsn_text(&written).expect("the written world scene parses");
    let instance = doc
        .entities_with_component(ISA_TYPE)
        .first()
        .copied()
        .expect("the instance is on disk as an IsA reference");
    // Named relative to the scene that references it: the two files sit in one
    // directory, so the reference is a bare file name. An absolute authoring
    // path is a path only the authoring machine has, and a scene saved with one
    // resolves nowhere in a teammate's checkout or in the shipped game.
    let expected = ui_path.file_name().expect("the source has a file name");
    assert_eq!(
        jackdaw_bsn::get_bsn_field(&doc, instance, ISA_TYPE, "source"),
        Some(jackdaw_bsn::BsnValue::String(
            expected.to_string_lossy().into_owned()
        )),
        "the reference names the source UI scene; wrote:\n{written}"
    );
    assert!(
        !written.contains("Greeting"),
        "the inherited widget tree is not flattened into the world scene; wrote:\n{written}"
    );
    // A leaked UiSceneRoot here would be read back by `declares_ui_scene_root`
    // on the next open, which classifies the whole level as a UI scene and
    // brings the 2D panel forward over it. The component is inherited from the
    // source at resolve time and has no business in the world scene's own text.
    assert!(
        !doc.component_type_paths(instance)
            .iter()
            .any(|type_path| jackdaw::scene_io::is_ui_scene_root_type_path(type_path)),
        "the instance node does not carry UiSceneRoot on disk; wrote:\n{written}"
    );
}

#[test]
fn editing_an_inherited_child_writes_an_override_and_leaves_the_source_alone() {
    // Per-instance overrides are sparse fields, as with any prefab. An edit
    // made in the world tab must not reach back into the UI scene that every
    // other world using it also reads.
    let tmp = tempfile::tempdir().unwrap();
    let mut app = make_app();
    let scene_path = open_world_scene(&mut app, tmp.path());
    let ui_path = import_ui_scene(&mut app, tmp.path());
    let source_before = std::fs::read_to_string(&ui_path).expect("source readable");

    let child = entity_named(&mut app, "Greeting").expect("the inherited child spawned");
    app.world_mut()
        .get_mut::<Node>(child)
        .expect("the inherited child is a UI node")
        .width = Val::Px(42.0);
    // Mirror the edit into the live document, as command dispatch does.
    jackdaw_bsn::sync_to_ast(app.world_mut(), child, std::any::TypeId::of::<Node>());
    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the world scene saved"
    );

    let written = std::fs::read_to_string(&scene_path).expect("world scene written");
    let doc = jackdaw_bsn::parse_bsn_text(&written).expect("the written world scene parses");
    let overridden = doc
        .entities_with_component(PREFAB_ENTITY_ID_TYPE)
        .into_iter()
        .find(|&node| {
            jackdaw_bsn::get_bsn_field(&doc, node, NODE_TYPE, "width").is_some()
                && doc.find_patch_by_type_path(node, ISA_TYPE).is_none()
        })
        .expect("an override node for the edited child; wrote:\n{written}");
    assert!(
        jackdaw_bsn::get_bsn_field(&doc, overridden, NODE_TYPE, "height").is_none(),
        "the override is sparse -- only the edited field; wrote:\n{written}"
    );
    assert_eq!(
        std::fs::read_to_string(&ui_path).expect("source readable"),
        source_before,
        "editing an instance leaves the source UI scene untouched"
    );
}

#[test]
fn reloading_the_world_scene_reconstructs_the_imported_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = make_app();
    let scene_path = open_world_scene(&mut app, tmp.path());
    import_ui_scene(&mut app, tmp.path());
    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the world scene saved"
    );

    let mut reloaded = make_app();
    jackdaw::scene_io::load_scene_from_file(reloaded.world_mut(), &scene_path);

    let root = entity_named(&mut reloaded, "Overlay").expect("the instance root came back");
    assert!(
        reloaded
            .world()
            .get::<jackdaw_scene_types::UiSceneRoot>(root)
            .is_some(),
        "the reconstructed root is still a UI root"
    );
    assert!(
        entity_named(&mut reloaded, "Greeting").is_some(),
        "the inherited widget tree is rebuilt from the source on load"
    );
}

#[test]
fn the_2d_panel_router_leaves_an_imported_instance_to_the_world_view() {
    // The hard edge: an imported root is `With<UiSceneRoot>,
    // Without<ChildOf>`, the same shape the 2D panel's own scene has. The
    // panel must claim the authored root and only the authored root.
    let mut app = make_app();
    app.add_systems(Update, jackdaw::viewport_2d::route_ui_roots_to_cameras);

    let world_camera = app
        .world_mut()
        .spawn((jackdaw::viewport::MainViewportCamera, Camera2d))
        .id();
    let authored = app
        .world_mut()
        .spawn(jackdaw_scene_types::UiSceneRoot::default())
        .id();
    let imported = app
        .world_mut()
        .spawn((
            jackdaw_scene_types::UiSceneRoot::default(),
            jackdaw::prefab::IsA {
                source: "hud.bsn".into(),
                deleted: Vec::new(),
            },
        ))
        .id();

    app.update();

    let parking = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::viewport_2d::UiSceneParkingCamera>>()
        .iter(app.world())
        .next()
        .expect("no panel is open, so the authored root parks");
    assert_eq!(
        app.world()
            .get::<bevy::ui::UiTargetCamera>(authored)
            .map(bevy::ui::UiTargetCamera::entity),
        Some(parking),
        "the authored UI scene keeps the 2D panel's routing"
    );
    assert_eq!(
        app.world()
            .get::<bevy::ui::UiTargetCamera>(imported)
            .map(bevy::ui::UiTargetCamera::entity),
        Some(world_camera),
        "an imported instance renders in the world view, not on the 2D stage"
    );
}

#[test]
fn an_imported_instance_parks_when_there_is_no_world_view() {
    // The "always routed somewhere" invariant holds for imports too: an
    // unrouted root falls back to `DefaultUiCamera`, which is the editor's
    // own window camera, and would draw the overlay over the editor chrome.
    let mut app = make_app();
    app.add_systems(Update, jackdaw::viewport_2d::route_ui_roots_to_cameras);

    let imported = app
        .world_mut()
        .spawn((
            jackdaw_scene_types::UiSceneRoot::default(),
            jackdaw::prefab::IsA {
                source: "hud.bsn".into(),
                deleted: Vec::new(),
            },
        ))
        .id();

    app.update();

    let parking = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::viewport_2d::UiSceneParkingCamera>>()
        .iter(app.world())
        .next()
        .expect("with no 3D viewport to render into, the import parks");
    assert_eq!(
        app.world()
            .get::<bevy::ui::UiTargetCamera>(imported)
            .map(bevy::ui::UiTargetCamera::entity),
        Some(parking),
        "the import is parked rather than left to DefaultUiCamera"
    );
}

#[test]
fn a_ui_scene_variant_is_edited_on_the_stage_not_routed_to_the_world_view() {
    // `save_as_variant` stamps the variant root `Prefab + PrefabEntityId(0) +
    // IsA(original)` and copies the resolved component patches, so a variant
    // saved off an imported UI overlay is a UI scene file whose root carries
    // both `UiSceneRoot` and `IsA`. Opening it is editing it, so `IsA` alone
    // cannot be the discriminator: `Prefab` says the document owns this root.
    let mut app = make_app();
    app.add_systems(Update, jackdaw::viewport_2d::route_ui_roots_to_cameras);

    let world_camera = app
        .world_mut()
        .spawn((jackdaw::viewport::MainViewportCamera, Camera2d))
        .id();
    let variant = app
        .world_mut()
        .spawn((
            jackdaw_scene_types::UiSceneRoot::default(),
            jackdaw::prefab::Prefab,
            jackdaw::prefab::PrefabEntityId(0),
            jackdaw::prefab::IsA {
                source: "hud.bsn".into(),
                deleted: Vec::new(),
            },
        ))
        .id();
    // The root of the tab being edited is in that tab's document, which is
    // how the palette tells it from a root another tab left in the world.
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), variant);

    app.update();

    let parking = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::viewport_2d::UiSceneParkingCamera>>()
        .iter(app.world())
        .next()
        .expect("an edited variant parks like any other authored root");
    assert_ne!(
        parking, world_camera,
        "the two cameras the routing chooses between are distinct"
    );
    assert_eq!(
        app.world()
            .get::<bevy::ui::UiTargetCamera>(variant)
            .map(bevy::ui::UiTargetCamera::entity),
        Some(parking),
        "a variant being edited routes as an authored root, not as a world scene's overlay"
    );
    assert_eq!(
        jackdaw::ui_palette::resolve_widget_parent(app.world_mut()),
        Some(variant),
        "the palette can add widgets to the variant, so it has a parent to add to"
    );
}

#[test]
fn opening_an_instances_source_activates_that_scenes_tab() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = make_app();
    app.init_resource::<jackdaw::scenes::Scenes>();
    let ui_path = import_ui_scene(&mut app, tmp.path());

    push_tab(&mut app, &ui_path);

    let root = entity_named(&mut app, "Overlay").expect("the imported root spawned");
    assert!(
        jackdaw::prefab::operators::open_instance_source(app.world_mut(), root),
        "the instance's source is openable"
    );

    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    assert_eq!(
        scenes.tabs[scenes.active].path.as_deref(),
        Some(ui_path.as_path()),
        "the source UI scene's tab is the active one"
    );
}

/// Click an outliner row the way the tree view does: the event carries the
/// scene entity the row stands for.
fn click_row(app: &mut App, source: Entity) {
    app.world_mut()
        .trigger(jackdaw_widgets::tree_view::TreeRowClicked {
            entity: source,
            source_entity: source,
        });
    app.update();
}

/// Double-clicking an instance row is the only gesture that opens an
/// imported UI scene for editing, and the order inside the observer is what
/// makes it usable: the pair is resolved BEFORE the ordinary click handling
/// below it, so the second click opens the source instead of re-selecting
/// the row. A consumed pair resets, so a third click is an ordinary click
/// again, which keeps the row selected and opens nothing.
#[test]
fn a_double_click_on_an_instance_row_opens_its_source_and_keeps_the_row_selected() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = util::editor_test_app();
    let ui_path = import_ui_scene(&mut app, tmp.path());
    push_tab(&mut app, &ui_path);
    app.update();

    let root = entity_named(&mut app, "Overlay").expect("the imported root spawned");
    click_row(&mut app, root);
    assert_eq!(
        app.world()
            .resource::<jackdaw::selection::Selection>()
            .entities,
        vec![root],
        "the first click of the pair selects the row it lands on"
    );

    click_row(&mut app, root);

    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    assert_eq!(
        scenes.tabs[scenes.active].path.as_deref(),
        Some(ui_path.as_path()),
        "the pair opened the scene the instance inherits from"
    );
    assert_eq!(
        app.world()
            .resource::<jackdaw::selection::Selection>()
            .entities,
        vec![root],
        "and left the row selected, rather than deselecting it on the way"
    );

    let tabs_before = app.world().resource::<jackdaw::scenes::Scenes>().tabs.len();
    click_row(&mut app, root);
    assert_eq!(
        app.world()
            .resource::<jackdaw::selection::Selection>()
            .entities,
        vec![root],
        "a consumed pair resets: the third click is an ordinary click, which keeps the row"
    );
    assert_eq!(
        app.world().resource::<jackdaw::scenes::Scenes>().tabs.len(),
        tabs_before,
        "and opens nothing, so it was not read as a second pair"
    );
}
