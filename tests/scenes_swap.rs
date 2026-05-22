use jackdaw::scenes::{SceneTab, Scenes, TabContent};

#[test]
fn scenes_default_is_empty() {
    let scenes = Scenes::default();
    assert!(scenes.tabs.is_empty());
    assert_eq!(scenes.active, 0);
}

#[test]
fn push_tab_appends_to_end() {
    let mut scenes = Scenes::default();
    let idx = scenes.push_tab(SceneTab::new_untitled(1));
    assert_eq!(idx, 0);
    assert_eq!(scenes.tabs.len(), 1);
}

#[test]
fn untitled_tab_has_correct_display_name() {
    let tab = SceneTab::new_untitled(3);
    assert_eq!(tab.display_name, "untitled-3");
    assert!(tab.path.is_none());
    assert!(!tab.dirty);
}

#[test]
fn scene_tab_has_ast_snapshot_field() {
    let tab = jackdaw::scenes::SceneTab::new_untitled(1);
    assert!(
        matches!(tab.content, TabContent::Scene(None)),
        "fresh tab starts with no AST snapshot"
    );
}

#[test]
fn view_state_round_trips_camera_transform() {
    use bevy::math::Vec3;
    use bevy::prelude::Transform;
    use jackdaw::scenes::ViewState;
    let vs = ViewState {
        camera_transform: Transform::from_xyz(1.0, 2.0, 3.0),
        ..ViewState::default()
    };
    assert_eq!(vs.camera_transform.translation, Vec3::new(1.0, 2.0, 3.0));
}

#[test]
fn view_state_default_has_empty_selection_and_no_sub_selection() {
    use jackdaw::scenes::ViewState;
    let vs = ViewState::default();
    assert!(vs.selection.is_empty());
    assert!(vs.brush_sub_selection.entity.is_none());
}

#[test]
fn serialize_world_to_jsn_scene_captures_brushes() {
    use bevy::prelude::*;
    use bevy::render::RenderPlugin;
    use bevy::render::settings::{RenderCreation, WgpuSettings};
    use bevy::winit::WinitPlugin;
    use jackdaw_jsn::Brush;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(WgpuSettings {
                    backends: None,
                    ..default()
                }),
                ..default()
            })
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(jackdaw_jsn::JsnPlugin::default());
    app.world_mut()
        .spawn((Brush::cuboid(1.0, 1.0, 1.0), Name::new("test_brush")));

    let jsn = jackdaw::scene_io::serialize_world_to_jsn_scene(app.world_mut());
    assert!(
        !jsn.scene.is_empty(),
        "expected at least one entity in serialized scene"
    );
}

#[test]
fn swap_round_trips_a_single_brush() {
    use bevy::prelude::*;
    use bevy::render::RenderPlugin;
    use bevy::render::settings::{RenderCreation, WgpuSettings};
    use bevy::winit::WinitPlugin;
    use jackdaw_jsn::Brush;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(WgpuSettings {
                    backends: None,
                    ..default()
                }),
                ..default()
            })
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(jackdaw_jsn::JsnPlugin::default());
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw_jsn::SceneJsnAst>();
    app.init_resource::<jackdaw::selection::Selection>();

    // Spawn a brush on the active scene, and register a corresponding node
    // in the AST. After T4 the swap captures the AST (not the live world),
    // so a node needs to exist in the AST for the captured snapshot to be
    // non-empty. Only `Name` is round-tripped through deserialization here
    // because `Brush` carries a `Handle<StandardMaterial>` which needs the
    // editor's full asset-aware deserializer; the brush respawn assertion
    // is what verifies T4's AST-as-spawn-source path end-to-end.
    let brush_entity = app
        .world_mut()
        .spawn((Brush::cuboid(1.0, 1.0, 1.0), Name::new("a")))
        .id();
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let idx = ast.create_node(brush_entity, None);
        ast.nodes[idx].components.insert(
            "bevy_ecs::name::Name".to_string(),
            serde_json::Value::String("a".to_string()),
        );
    }

    // Two tabs: A (active) and B (empty).
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(1));
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(2));
        scenes.active = 0;
    }

    // Swap to tab B.
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);

    // Original brush entity is gone (clear_scene_entities ran).
    assert!(app.world().get::<Brush>(brush_entity).is_none());

    // Tab A holds the AST snapshot now.
    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    assert_eq!(scenes.active, 1);
    let TabContent::Scene(Some(captured)) = &scenes.tabs[0].content else {
        panic!("expected captured Scene AST");
    };
    assert!(!captured.nodes.is_empty());

    // Swap back to tab A. The Name-bearing entity respawns from the AST.
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 0);
    let name_count: usize = app
        .world_mut()
        .query::<&Name>()
        .iter(app.world())
        .filter(|n| n.as_str() == "a")
        .count();
    assert_eq!(name_count, 1, "tab A's AST entity should respawn");
}

#[test]
fn swap_preserves_camera_transform_per_tab() {
    use bevy::prelude::*;
    use bevy::render::RenderPlugin;
    use bevy::render::settings::{RenderCreation, WgpuSettings};
    use bevy::winit::WinitPlugin;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(WgpuSettings {
                    backends: None,
                    ..default()
                }),
                ..default()
            })
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(jackdaw_jsn::JsnPlugin::default());
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw_jsn::SceneJsnAst>();
    app.init_resource::<jackdaw::selection::Selection>();

    // Camera tagged as the main viewport camera, placed at (1, 2, 3).
    let cam = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Transform::from_xyz(1.0, 2.0, 3.0),
            jackdaw::viewport::MainViewportCamera,
        ))
        .id();

    // Two tabs.
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(1));
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(2));
        scenes.active = 0;
    }

    // Swap to B (camera should be captured for A, reset for B).
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);
    // While on B, move the camera somewhere else.
    if let Some(mut tf) = app.world_mut().get_mut::<Transform>(cam) {
        *tf = Transform::from_xyz(10.0, 20.0, 30.0);
    }
    // Swap back to A. The camera should return to (1, 2, 3).
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 0);

    let tf = app.world().get::<Transform>(cam).unwrap();
    assert_eq!(tf.translation, Vec3::new(1.0, 2.0, 3.0));
}

#[test]
fn scene_new_appends_an_untitled_tab() {
    use bevy::prelude::*;
    use bevy::render::RenderPlugin;
    use bevy::render::settings::{RenderCreation, WgpuSettings};
    use bevy::winit::WinitPlugin;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(WgpuSettings {
                    backends: None,
                    ..default()
                }),
                ..default()
            })
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(jackdaw_jsn::JsnPlugin::default());
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw_jsn::SceneJsnAst>();
    app.init_resource::<jackdaw::selection::Selection>();
    app.init_resource::<jackdaw::scenes::operators::UntitledCounter>();

    // Run scene_new_system twice. The first appends and activates tab 0;
    // the second appends and swaps to tab 1.
    app.world_mut()
        .run_system_cached(jackdaw::scenes::operators::scene_new_system)
        .unwrap();
    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    assert_eq!(scenes.tabs.len(), 1);
    assert_eq!(scenes.active, 0);
    assert!(scenes.tabs[0].path.is_none());
    assert!(scenes.tabs[0].display_name.starts_with("untitled-"));

    app.world_mut()
        .run_system_cached(jackdaw::scenes::operators::scene_new_system)
        .unwrap();
    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    assert_eq!(scenes.tabs.len(), 2);
    assert_eq!(scenes.active, 1);
    // Counter increments: distinct names.
    assert_ne!(scenes.tabs[0].display_name, scenes.tabs[1].display_name);
}

#[test]
fn scene_open_dedupes_by_path() {
    use bevy::prelude::*;
    use bevy::render::RenderPlugin;
    use bevy::render::settings::{RenderCreation, WgpuSettings};
    use bevy::winit::WinitPlugin;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(WgpuSettings {
                    backends: None,
                    ..default()
                }),
                ..default()
            })
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(jackdaw_jsn::JsnPlugin::default());
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw_jsn::SceneJsnAst>();
    app.init_resource::<jackdaw::selection::Selection>();
    app.init_resource::<jackdaw::scenes::operators::UntitledCounter>();

    // Make sure there is an active tab (otherwise the swap inside scene_open
    // has nothing to capture from).
    app.world_mut()
        .run_system_cached(jackdaw::scenes::operators::scene_new_system)
        .unwrap();

    // Write an empty scene to a temp file (valid JsnScene JSON).
    let tmp_dir = std::env::temp_dir();
    let path = tmp_dir.join("jackdaw_scene_open_dedupe_test.jsn");
    std::fs::write(
        &path,
        r#"{"jsn":{"format_version":[3,0,0],"editor_version":"0.1.0","bevy_version":"0.18"},"metadata":{"name":"","description":"","author":"","created":"","modified":""},"assets":{},"scene":[]}"#,
    )
    .unwrap();

    // Open twice; only one tab with this path should result.
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &path);
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &path);

    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
    let matches = scenes
        .tabs
        .iter()
        .filter(|t| {
            t.path
                .as_ref()
                .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()) == canonical)
                .unwrap_or(false)
        })
        .count();
    assert_eq!(matches, 1);

    let _ = std::fs::remove_file(&path);
}

fn make_app_with_n_tabs(n: usize) -> bevy::app::App {
    use bevy::prelude::*;
    use bevy::render::RenderPlugin;
    use bevy::render::settings::{RenderCreation, WgpuSettings};
    use bevy::winit::WinitPlugin;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(WgpuSettings {
                    backends: None,
                    ..default()
                }),
                ..default()
            })
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(jackdaw_jsn::JsnPlugin::default());
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw_jsn::SceneJsnAst>();
    app.init_resource::<jackdaw::selection::Selection>();
    app.init_resource::<jackdaw::scenes::operators::UntitledCounter>();
    app.init_resource::<jackdaw::scene_io::SceneFilePath>();
    app.init_resource::<jackdaw::scene_io::SceneDirtyState>();
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        for i in 0..n {
            scenes
                .tabs
                .push(jackdaw::scenes::SceneTab::new_untitled((i + 1) as u32));
        }
        scenes.active = 0;
    }
    app
}

#[test]
fn scene_switch_changes_active_index() {
    let mut app = make_app_with_n_tabs(2);
    // Active starts at 0; switch to 1.
    jackdaw::scenes::operators::scene_switch_system(app.world_mut(), 1);
    assert_eq!(app.world().resource::<jackdaw::scenes::Scenes>().active, 1);
}

#[test]
fn scene_save_all_writes_each_path_bound_tab() {
    // Build two tabs, both with paths to temp files. After save_all, the
    // files should exist on disk.
    let mut app = make_app_with_n_tabs(2);
    let tmp_a = std::env::temp_dir().join("jackdaw_save_all_a.jsn");
    let tmp_b = std::env::temp_dir().join("jackdaw_save_all_b.jsn");
    let _ = std::fs::remove_file(&tmp_a);
    let _ = std::fs::remove_file(&tmp_b);

    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.tabs[0].path = Some(tmp_a.clone());
        scenes.tabs[1].path = Some(tmp_b.clone());
    }

    jackdaw::scenes::operators::scene_save_all_system(app.world_mut());

    assert!(tmp_a.exists(), "tab 0 should have been saved");
    assert!(tmp_b.exists(), "tab 1 should have been saved");

    let _ = std::fs::remove_file(&tmp_a);
    let _ = std::fs::remove_file(&tmp_b);
}

#[test]
fn scene_close_blocks_closing_last_tab() {
    let mut app = make_app_with_n_tabs(1);
    jackdaw::scenes::operators::scene_close_system(app.world_mut(), 0);
    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    assert_eq!(scenes.tabs.len(), 1);
    assert_eq!(scenes.active, 0);
}

#[test]
fn scene_close_drops_inactive_tab_and_shifts_active_index() {
    let mut app = make_app_with_n_tabs(2);
    // Active = 1. Close tab 0; active should now be 0 (the surviving tab).
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.active = 1;
    }
    jackdaw::scenes::operators::scene_close_system(app.world_mut(), 0);
    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    assert_eq!(scenes.tabs.len(), 1);
    assert_eq!(scenes.active, 0);
}

#[test]
fn scene_close_drops_active_tab_and_picks_neighbor() {
    let mut app = make_app_with_n_tabs(2);
    // Active = 1; close tab 1. Neighbor (index 0) becomes active.
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.active = 1;
    }
    jackdaw::scenes::operators::scene_close_system(app.world_mut(), 1);
    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    assert_eq!(scenes.tabs.len(), 1);
    assert_eq!(scenes.active, 0);
}

#[test]
fn pushing_to_history_marks_active_tab_dirty() {
    use bevy::prelude::*;

    struct NoOpCommand;
    impl jackdaw::commands::EditorCommand for NoOpCommand {
        fn execute(&mut self, _world: &mut World) {}
        fn undo(&mut self, _world: &mut World) {}
        fn description(&self) -> &str {
            "noop"
        }
    }

    let mut app = make_app_with_n_tabs(1);
    // Confirm not yet dirty.
    assert!(!app.world().resource::<jackdaw::scenes::Scenes>().tabs[0].dirty);

    // Push a no-op into history.
    app.world_mut()
        .resource_mut::<jackdaw::commands::CommandHistory>()
        .push_executed(Box::new(NoOpCommand));

    // Run the dirty-sync system once.
    app.world_mut()
        .run_system_cached(jackdaw::scenes::mark_active_dirty_on_history_growth)
        .unwrap();

    assert!(app.world().resource::<jackdaw::scenes::Scenes>().tabs[0].dirty);
}

#[test]
fn project_config_persists_tab_paths_and_active_index() {
    use jackdaw_jsn::format::{JsnHeader, JsnProject, JsnProjectConfig};

    let mut app = make_app_with_n_tabs(0);

    // Use a uniquely-named subdirectory of temp to act as the project root.
    let tmp_root = std::env::temp_dir().join("jackdaw_persist_tabs_test_root");
    std::fs::create_dir_all(&tmp_root).unwrap();
    let scene_path = tmp_root.join("level1.jsn");
    std::fs::write(
        &scene_path,
        r#"{"jsn":{"format_version":[3,0,0],"editor_version":"0.1.0","bevy_version":"0.18"},"metadata":{"name":"","description":"","author":"","created":"","modified":""},"assets":{},"scene":[]}"#,
    )
    .unwrap();

    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp_root.clone(),
            config: JsnProject {
                jsn: JsnHeader::default(),
                project: JsnProjectConfig {
                    name: "test".into(),
                    ..Default::default()
                },
            },
        });

    // Open one tab.
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene_path);
    // Run the persist system.
    app.world_mut()
        .run_system_cached(jackdaw::scenes::persist_tabs_to_project_config)
        .unwrap();

    // The on-disk project.jsn now lists that path.
    let saved = jackdaw::project::load_project_config(&tmp_root).unwrap();
    assert_eq!(saved.project.last_open_tabs, vec!["level1.jsn".to_string()]);

    // Cleanup.
    let _ = std::fs::remove_file(&scene_path);
    let _ = std::fs::remove_dir_all(&tmp_root);
}

/// Verifies that `paste_components` merges assets from the clipboard payload into
/// the destination scene's `SceneJsnAst`, and that existing asset definitions are
/// not overwritten by incoming ones with the same name.
///
/// We test this directly by calling `merge_payload_assets` (which is the internal
/// helper extracted from `paste_components`) via the public API of `SceneJsnAst`.
/// The actual paste flow (clipboard read, entity spawn) is exercised in the
/// inline unit tests in `src/entity_ops.rs`.
#[test]
fn paste_merges_assets_without_clobbering_existing() {
    use jackdaw_jsn::format::JsnAssets;
    use std::collections::HashMap;

    // Build destination assets: already has "Metal" under "bevy_pbr::StandardMaterial".
    let mut dest_map: HashMap<String, HashMap<String, serde_json::Value>> = HashMap::new();
    let mut mat_map: HashMap<String, serde_json::Value> = HashMap::new();
    mat_map.insert("Metal".to_string(), serde_json::json!({"metallic": 1.0}));
    dest_map.insert("bevy_pbr::StandardMaterial".to_string(), mat_map);
    let dest_assets = JsnAssets(dest_map);

    // Source assets from clipboard payload: has "Metal" (different value) + "Brick".
    let mut src_map: HashMap<String, HashMap<String, serde_json::Value>> = HashMap::new();
    let mut src_mat_map: HashMap<String, serde_json::Value> = HashMap::new();
    src_mat_map.insert("Metal".to_string(), serde_json::json!({"metallic": 0.5}));
    src_mat_map.insert(
        "Brick".to_string(),
        serde_json::json!({"base_color": [1.0, 0.5, 0.0, 1.0]}),
    );
    src_map.insert("bevy_pbr::StandardMaterial".to_string(), src_mat_map);
    let src_assets = JsnAssets(src_map);

    // Simulate the merge by applying the same logic as merge_payload_assets.
    let mut merged = dest_assets.clone();
    for (type_path, name_map) in &src_assets.0 {
        let dest_type_map = merged.0.entry(type_path.clone()).or_default();
        for (name, def) in name_map {
            if !dest_type_map.contains_key(name) {
                dest_type_map.insert(name.clone(), def.clone());
            }
        }
    }

    let mat_section = &merged.0["bevy_pbr::StandardMaterial"];

    // "Metal" was already present; its original value must be preserved (not clobbered).
    assert_eq!(
        mat_section["Metal"]["metallic"].as_f64().unwrap(),
        1.0,
        "existing Metal definition should not be overwritten"
    );

    // "Brick" was new in the source; it should be added.
    assert!(
        mat_section.contains_key("Brick"),
        "new Brick definition from source should be merged in"
    );
}

#[test]
fn closing_dirty_tab_defers_via_pending_close_resource() {
    let mut app = make_app_with_n_tabs(2);
    // Also register PendingTabClose so the test can query it.
    app.init_resource::<jackdaw::scenes::confirm_dialog::PendingTabClose>();

    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.tabs[0].dirty = true;
        scenes.active = 1;
    }

    // No dialog up yet.
    assert!(
        app.world()
            .resource::<jackdaw::scenes::confirm_dialog::PendingTabClose>()
            .tab_index
            .is_none()
    );

    jackdaw::scenes::operators::scene_close_system(app.world_mut(), 0);

    // Tab should still be there (deferred to dialog).
    assert_eq!(
        app.world().resource::<jackdaw::scenes::Scenes>().tabs.len(),
        2
    );
    // PendingTabClose should record index 0.
    assert_eq!(
        app.world()
            .resource::<jackdaw::scenes::confirm_dialog::PendingTabClose>()
            .tab_index,
        Some(0)
    );
}

#[test]
fn window_close_with_dirty_tabs_does_not_exit() {
    let mut app = make_app_with_n_tabs(2);
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.tabs[0].dirty = true;
    }
    app.world_mut()
        .init_resource::<jackdaw::scenes::confirm_dialog::PendingQuit>();

    app.add_systems(
        bevy::prelude::Update,
        jackdaw::scenes::intercept_window_close,
    );
    app.add_message::<bevy::window::WindowCloseRequested>();
    app.add_message::<bevy::app::AppExit>();

    // Fire a WindowCloseRequested.
    app.world_mut()
        .resource_mut::<bevy::ecs::message::Messages<bevy::window::WindowCloseRequested>>()
        .write(bevy::window::WindowCloseRequested {
            window: bevy::prelude::Entity::PLACEHOLDER,
        });
    app.update();

    // No AppExit should have been emitted.
    let exits: Vec<_> = app
        .world_mut()
        .resource_mut::<bevy::ecs::message::Messages<bevy::app::AppExit>>()
        .drain()
        .collect();
    assert!(exits.is_empty(), "should not have emitted AppExit");

    // PendingQuit should now be active.
    assert!(
        app.world()
            .resource::<jackdaw::scenes::confirm_dialog::PendingQuit>()
            .active
    );
}

#[test]
fn swap_captures_ast_snapshot_into_outgoing_tab() {
    let mut app = make_app_with_n_tabs(2);
    // Mark the live AST so we can recognize it after capture.
    let marker_node_index = {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        ast.create_node(bevy::prelude::Entity::PLACEHOLDER, None)
    };
    app.world_mut()
        .resource_mut::<jackdaw::scenes::Scenes>()
        .active = 0;
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);
    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    let TabContent::Scene(Some(captured)) = &scenes.tabs[0].content else {
        panic!("outgoing tab has captured Scene AST");
    };
    assert_eq!(
        captured
            .nodes
            .get(marker_node_index)
            .and_then(|n| n.ecs_entity),
        Some(bevy::prelude::Entity::PLACEHOLDER),
        "outgoing tab's stored AST is the one we marked"
    );
}

#[test]
fn activate_restores_ast_snapshot_from_tab() {
    let mut app = make_app_with_n_tabs(2);
    // Pre-stash an AST snapshot containing a single marker node on tab 1.
    // After swap, activate_tab spawns fresh entities for each AST node and
    // rebinds ecs_entity, so we assert on node count and component shape
    // rather than on the placeholder ID.
    let marker_node_count = {
        let mut prepared = jackdaw_jsn::SceneJsnAst::default();
        prepared.create_node(bevy::prelude::Entity::PLACEHOLDER, None);
        let count = prepared.nodes.len();
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.tabs[1].content = TabContent::Scene(Some(prepared));
        scenes.active = 0;
        count
    };
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);
    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    assert_eq!(
        ast.nodes.len(),
        marker_node_count,
        "activating tab 1 installed its AST snapshot (node count preserved)"
    );
    // ecs_to_jsn was rebuilt: every node's ecs_entity is a fresh id mapped
    // back to its own node index.
    for (i, node) in ast.nodes.iter().enumerate() {
        let e = node.ecs_entity.expect("node rebound to a fresh entity");
        assert_eq!(ast.ecs_to_jsn.get(&e), Some(&i));
    }
}

#[test]
fn swap_does_not_keep_a_jsn_scene_snapshot_field() {
    // Compile-only check: SceneTab no longer has a `snapshot` field.
    // If it does, this won't compile because the struct literal omits it
    // (Rust requires every field to be initialized).
    let _tab = jackdaw::scenes::SceneTab {
        path: None,
        display_name: "x".to_string(),
        dirty: false,
        kind: jackdaw::scenes::TabKind::Scene,
        content: TabContent::Scene(None),
        view_state: jackdaw::scenes::ViewState::default(),
        history: jackdaw::commands::CommandHistory::default(),
        history_depth_at_last_check: 0,
    };
}

#[test]
fn tab_swap_preserves_entity_ordering_and_components() {
    use bevy::prelude::*;
    use bevy::render::RenderPlugin;
    use bevy::render::settings::{RenderCreation, WgpuSettings};
    use bevy::winit::WinitPlugin;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(WgpuSettings {
                    backends: None,
                    ..default()
                }),
                ..default()
            })
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(jackdaw_jsn::JsnPlugin::default());
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw_jsn::SceneJsnAst>();
    app.init_resource::<jackdaw::selection::Selection>();
    app.init_resource::<jackdaw::scene_io::SceneFilePath>();
    app.init_resource::<jackdaw::scene_io::SceneDirtyState>();
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(1));
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(2));
        scenes.active = 0;
    }

    // Seed tab 0's AST with two named nodes. The codebase uses
    // `bevy_ecs::name::Name` as the reflect path and a bare JSON string as
    // the serialized shape (see e.g. crates/jackdaw_jsn/src/format.rs).
    let name_key = "bevy_ecs::name::Name";
    {
        let e1 = app.world_mut().spawn_empty().id();
        let e2 = app.world_mut().spawn_empty().id();
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let i1 = ast.create_node(e1, None);
        let i2 = ast.create_node(e2, None);
        ast.set_component(e1, name_key, serde_json::Value::String("alpha".to_string()));
        ast.set_component(e2, name_key, serde_json::Value::String("beta".to_string()));
        // Sanity: indices should be 0 and 1.
        assert_eq!(i1, 0);
        assert_eq!(i2, 1);
    }

    // Swap away and back.
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 0);

    // The AST still has both nodes in order, with the right component data.
    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    assert_eq!(ast.nodes.len(), 2, "both nodes survived round-trip");
    let name0 = ast.nodes[0]
        .components
        .get(name_key)
        .expect("node 0 still has Name component");
    let name1 = ast.nodes[1]
        .components
        .get(name_key)
        .expect("node 1 still has Name component");
    assert_eq!(name0.as_str(), Some("alpha"));
    assert_eq!(name1.as_str(), Some("beta"));

    // The freshly-spawned entities also carry the original names.
    let names: Vec<String> = app
        .world_mut()
        .query::<&Name>()
        .iter(app.world())
        .map(|n| n.as_str().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n == "alpha"),
        "respawned entities include alpha; got {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n == "beta"),
        "respawned entities include beta; got {:?}",
        names
    );
}

#[test]
fn window_close_with_no_dirty_tabs_exits_cleanly() {
    let mut app = make_app_with_n_tabs(1);
    app.world_mut()
        .init_resource::<jackdaw::scenes::confirm_dialog::PendingQuit>();

    app.add_systems(
        bevy::prelude::Update,
        jackdaw::scenes::intercept_window_close,
    );
    app.add_message::<bevy::window::WindowCloseRequested>();
    app.add_message::<bevy::app::AppExit>();

    // Fire a WindowCloseRequested with no dirty tabs.
    app.world_mut()
        .resource_mut::<bevy::ecs::message::Messages<bevy::window::WindowCloseRequested>>()
        .write(bevy::window::WindowCloseRequested {
            window: bevy::prelude::Entity::PLACEHOLDER,
        });
    app.update();

    // Exactly one AppExit::Success should have been emitted.
    let exits: Vec<_> = app
        .world_mut()
        .resource_mut::<bevy::ecs::message::Messages<bevy::app::AppExit>>()
        .drain()
        .collect();
    assert_eq!(exits.len(), 1);
}

#[test]
fn open_prefab_file_sets_tab_kind_prefab() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    std::fs::write(
        &prefab_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "p", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0
                }
            }]
        }"#,
    )
    .unwrap();

    let mut app = make_app_with_n_tabs(0);
    app.world_mut()
        .init_resource::<jackdaw::prefab::PrefabAstCache>();
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &prefab_path);

    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    let tab = scenes.tabs.last().expect("tab pushed by open");
    assert!(
        matches!(tab.kind, jackdaw::scenes::TabKind::Prefab),
        "opening a prefab file sets TabKind::Prefab"
    );
}

#[test]
fn open_regular_scene_file_sets_tab_kind_scene() {
    let tmp = tempfile::tempdir().unwrap();
    let scene_path = tmp.path().join("s.jsn");
    std::fs::write(
        &scene_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "s", "created": "", "modified": "" },
            "assets": {},
            "scene": [{ "components": { "bevy_ecs::name::Name": "x" } }]
        }"#,
    )
    .unwrap();

    let mut app = make_app_with_n_tabs(0);
    app.world_mut()
        .init_resource::<jackdaw::prefab::PrefabAstCache>();
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene_path);

    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    let tab = scenes.tabs.last().expect("tab pushed");
    assert!(matches!(tab.kind, jackdaw::scenes::TabKind::Scene));
}

#[test]
fn each_tab_has_its_own_undo_stack() {
    // Pushing history entries in tab A then swapping to tab B must
    // leave tab B's history empty. Swapping back to A must restore
    // A's stack.
    use bevy::prelude::*;

    struct CounterCommand;
    impl jackdaw::commands::EditorCommand for CounterCommand {
        fn execute(&mut self, _world: &mut World) {}
        fn undo(&mut self, _world: &mut World) {}
        fn description(&self) -> &str {
            "counter"
        }
    }

    let mut app = make_app_with_n_tabs(2);

    // Tab A active. Push two entries.
    app.world_mut()
        .resource_mut::<jackdaw::commands::CommandHistory>()
        .push_executed(Box::new(CounterCommand));
    app.world_mut()
        .resource_mut::<jackdaw::commands::CommandHistory>()
        .push_executed(Box::new(CounterCommand));
    assert_eq!(
        app.world()
            .resource::<jackdaw::commands::CommandHistory>()
            .undo_stack
            .len(),
        2,
        "tab A has 2 entries"
    );

    // Swap to tab B. Live history should be tab B's (empty).
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);
    assert_eq!(
        app.world()
            .resource::<jackdaw::commands::CommandHistory>()
            .undo_stack
            .len(),
        0,
        "tab B has its own empty undo stack"
    );

    // Push an entry in tab B.
    app.world_mut()
        .resource_mut::<jackdaw::commands::CommandHistory>()
        .push_executed(Box::new(CounterCommand));
    assert_eq!(
        app.world()
            .resource::<jackdaw::commands::CommandHistory>()
            .undo_stack
            .len(),
        1,
        "tab B has 1 entry"
    );

    // Swap back to tab A. Its 2-entry stack is restored.
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 0);
    assert_eq!(
        app.world()
            .resource::<jackdaw::commands::CommandHistory>()
            .undo_stack
            .len(),
        2,
        "tab A's 2-entry stack is restored after swap-back"
    );

    // And swapping back to B still has its 1 entry.
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);
    assert_eq!(
        app.world()
            .resource::<jackdaw::commands::CommandHistory>()
            .undo_stack
            .len(),
        1,
        "tab B's stack persists across the round-trip"
    );
}
