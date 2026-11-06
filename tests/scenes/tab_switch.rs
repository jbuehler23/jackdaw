//! A tab holds its live document while inactive. Nothing is written out and
//! read back on the way, so whatever a text round trip used to settle has to
//! hold without one.

use std::path::{Path, PathBuf};

use bevy::ecs::change_detection::Tick;
use bevy::prelude::*;
use jackdaw::boot_ops::run_op_clause_as_user;
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::scene_io::{emit_bsn_scene_for_file, emit_bsn_scene_with_inline_assets};
use jackdaw::scenes::swap::swap_active_tab;
use jackdaw::scenes::{Scenes, TabContent};
use jackdaw_bsn::{BsnPatch, SceneBsnAst};
use jackdaw_scene_types::types::Brush;

use crate::util::{self, OperatorResultExt as _};

const SOIL: &str = "\
// jackdaw asset bevy_pbr::pbr_material::StandardMaterial
#soil
bevy_pbr::pbr_material::StandardMaterial {
    perceptual_roughness: 0.25,
}
";

const TREE: &str = "\
#tree
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
bevy_transform::components::transform::Transform
bevy_ecs::hierarchy::Children [
    #part
    jackdaw::prefab::components::PrefabEntityId(1)
    bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 0.0, y: 1.0, z: 0.0 } }
]
";

/// Several roots; a brush whose compact faces name a file material, one that
/// does not exist yet and one the scene embeds; a prefab instance with a sparse
/// override; and the embedded asset itself.
const SCENE: &str = "\
bevy_ecs::hierarchy::Children [
    #Block
    bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 1.5, y: 0.0, z: 0.0 } }
    jackdaw_scene_types::types::Brush {
        faces: [
            jackdaw_geometry::BrushFaceData {
                plane: jackdaw_geometry::BrushPlane { normal: glam::Vec3 { x: 0.0, y: 1.0, z: 0.0 }, distance: 1.5 },
                material: \"materials/soil.bsn\",
            },
            jackdaw_geometry::BrushFaceData {
                plane: jackdaw_geometry::BrushPlane { normal: glam::Vec3 { x: 1.0, y: 0.0, z: 0.0 }, distance: 0.1 },
                material: \"materials/waiting.bsn\",
            },
            jackdaw_geometry::BrushFaceData {
                plane: jackdaw_geometry::BrushPlane { normal: glam::Vec3 { x: 0.0, y: 0.0, z: 1.0 }, distance: 0.7 },
                material: \"#Inline\",
            },
        ],
    }
    --
    #Holder
    jackdaw::prefab::components::IsA { source: \"tree.bsn\", deleted: [] }
    jackdaw::prefab::components::PrefabEntityId(0)
    bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 5.0, y: 0.0, z: 0.0 } }
    --
    #Loose
    bevy_transform::components::transform::Transform
    --
    #Inline
    bevy_pbr::pbr_material::StandardMaterial {
        perceptual_roughness: 0.75,
    }
]
";

struct Project {
    app: App,
    _tmp: tempfile::TempDir,
    assets: PathBuf,
    scene: PathBuf,
}

/// The fixture scene in front, with a second scene open behind it.
fn project() -> Project {
    let tmp = tempfile::tempdir().expect("tempdir");
    let assets = dunce::canonicalize(tmp.path())
        .expect("the folder has a real path")
        .join("assets");
    std::fs::create_dir_all(assets.join("materials")).expect("the folders are made");
    std::fs::write(assets.join("materials/soil.bsn"), SOIL).expect("the material is written");
    std::fs::write(assets.join("tree.bsn"), TREE).expect("the prefab is written");
    let scene = assets.join("scene.bsn");
    std::fs::write(&scene, SCENE).expect("the scene is written");
    let other = assets.join("other.bsn");
    std::fs::write(&other, "#Elsewhere\n").expect("the other scene is written");

    let mut app = util::editor_test_app_reading(&assets);
    app.insert_resource(ProjectRoot::new(
        assets.parent().expect("a parent").to_path_buf(),
        ProjectConfig::default(),
    ));
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &other);
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    app.update();
    Project {
        app,
        _tmp: tmp,
        assets,
        scene,
    }
}

fn active(app: &App) -> usize {
    app.world().resource::<Scenes>().active
}

fn tab_named(app: &App, path: &Path) -> usize {
    app.world()
        .resource::<Scenes>()
        .tabs
        .iter()
        .position(|tab| tab.path.as_deref() == Some(path))
        .expect("the tab is open")
}

fn file_text(project: &mut Project) -> String {
    emit_bsn_scene_for_file(project.app.world_mut(), &project.assets)
        .expect("the document names no absolute path")
}

fn held_text(project: &mut Project) -> String {
    emit_bsn_scene_with_inline_assets(project.app.world_mut(), &project.assets)
}

/// Away to the other tab and back.
fn switch_and_back(project: &mut Project) {
    let front = active(&project.app);
    let away = usize::from(front == 0);
    assert!(swap_active_tab(project.app.world_mut(), away));
    project.app.update();
    assert!(swap_active_tab(project.app.world_mut(), front));
    project.app.update();
}

/// What the world holds of the scene: every named entity with its parent and
/// position, and each brush face's material as the store holds it.
fn spawned(app: &mut App) -> (Vec<(String, Option<String>, Vec3)>, Vec<Option<f32>>) {
    let mut named = app
        .world_mut()
        .query_filtered::<(&Name, Option<&ChildOf>, &Transform), With<jackdaw_bsn::AstNodeRef>>();
    let mut rows: Vec<(String, Option<String>, Vec3)> = named
        .iter(app.world())
        .map(|(name, parent, transform)| {
            let parent = parent
                .and_then(|parent| app.world().get::<Name>(parent.parent()))
                .map(ToString::to_string);
            (name.to_string(), parent, transform.translation)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    let mut brushes = app.world_mut().query::<&Brush>();
    let store = app.world().resource::<Assets<StandardMaterial>>();
    let faces = brushes
        .iter(app.world())
        .flat_map(|brush| &brush.faces)
        .map(|face| store.get(&face.material).map(|m| m.perceptual_roughness))
        .collect();
    (rows, faces)
}

#[test]
fn the_document_after_a_switch_and_back_emits_the_same_text() {
    let mut project = project();
    let file = file_text(&mut project);
    let held = held_text(&mut project);
    let world_before = spawned(&mut project.app);
    assert!(file.contains("materials/waiting.bsn"), "{file}");
    assert!(file.contains("#Inline"), "{file}");
    assert!(file.contains("tree.bsn"), "{file}");
    assert!(
        !file.contains("#part"),
        "the instance stays sparse in text: {file}"
    );

    for round in 0..3 {
        switch_and_back(&mut project);
        assert_eq!(file_text(&mut project), file, "file text, round {round}");
        assert_eq!(held_text(&mut project), held, "held text, round {round}");
        assert_eq!(
            spawned(&mut project.app),
            world_before,
            "the world, round {round}"
        );
    }
}

#[test]
fn an_inactive_tab_holds_the_text_it_left_with_and_no_links() {
    let mut project = project();
    let held = held_text(&mut project);
    let tab = active(&project.app);
    assert!(swap_active_tab(project.app.world_mut(), 1 - tab));

    let scenes = project.app.world().resource::<Scenes>();
    let TabContent::Scene(Some(document)) = &scenes.tabs[tab].content else {
        panic!("the tab holds its document");
    };
    assert_eq!(jackdaw_bsn::emit_scene(document), held);
    assert!(
        document.ast_to_ecs.is_empty() && document.ecs_to_ast.is_empty(),
        "the entities it was linked to are gone"
    );
}

#[test]
fn a_tab_keeps_the_live_document_rather_than_a_copy_of_it() {
    let mut project = project();
    let tab = active(&project.app);
    // The live document came out of the resolver, so go round once to have one
    // that spawned from what the tab held.
    let live = project.app.world().resource::<SceneBsnAst>().world.id();
    assert!(swap_active_tab(project.app.world_mut(), 1 - tab));

    let scenes = project.app.world().resource::<Scenes>();
    let TabContent::Scene(Some(document)) = &scenes.tabs[tab].content else {
        panic!("the tab holds its document");
    };
    assert_eq!(document.world.id(), live, "the same document, not a parse");
}

#[test]
fn links_left_stale_in_the_live_document_do_not_come_back() {
    let mut project = project();
    let file = file_text(&mut project);

    // A node of the embedded asset spawns no entity, so nothing overwrites a
    // link it holds; an entity id handed out again would read as its own.
    let gone = project.app.world_mut().spawn_empty().id();
    project.app.world_mut().despawn(gone);
    {
        let mut live = project.app.world_mut().resource_mut::<SceneBsnAst>();
        let root = *live.roots.last().expect("a root");
        live.link(gone, root);
    }

    switch_and_back(&mut project);

    assert_eq!(file_text(&mut project), file);
    let live = project.app.world().resource::<SceneBsnAst>();
    assert!(!live.ecs_to_ast.contains_key(&gone));
    for (&node, &entity) in &live.ast_to_ecs {
        let held = project
            .app
            .world()
            .get::<jackdaw_bsn::AstNodeRef>(entity)
            .unwrap_or_else(|| panic!("{entity} is no spawned node"));
        assert_eq!(held.patches_entity, node);
    }
}

#[test]
fn a_no_edit_save_after_a_switch_writes_the_same_bytes() {
    let mut project = project();
    assert!(jackdaw::scene_io::save_scene(project.app.world_mut()));
    let saved = std::fs::read(&project.scene).expect("the file");

    switch_and_back(&mut project);

    assert!(jackdaw::scene_io::save_scene(project.app.world_mut()));
    assert_eq!(
        String::from_utf8_lossy(&std::fs::read(&project.scene).expect("the file")),
        String::from_utf8_lossy(&saved),
    );
}

#[test]
fn undo_dirty_and_selection_cross_a_switch() {
    let mut project = project();
    let tab = tab_named(&project.app, &project.scene);
    let clean = file_text(&mut project);
    run_op_clause_as_user(
        project.app.world_mut(),
        "entity.add.group name=Props parent=Loose",
    )
    .expect("the group dispatched")
    .assert_finished();
    project.app.update();
    let edited = file_text(&mut project);
    assert_ne!(edited, clean);

    let block = project
        .app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(project.app.world())
        .find_map(|(entity, name)| (name.as_str() == "Block").then_some(entity))
        .expect("the block spawned");
    project
        .app
        .world_mut()
        .resource_mut::<jackdaw::selection::Selection>()
        .entities = vec![block];
    project
        .app
        .world_mut()
        .entity_mut(block)
        .insert(jackdaw::selection::Selected);

    let selected = |project: &mut Project| {
        let mut names: Vec<String> = project
            .app
            .world_mut()
            .query_filtered::<&Name, With<jackdaw::selection::Selected>>()
            .iter(project.app.world())
            .map(ToString::to_string)
            .collect();
        names.sort();
        names
    };
    let selected_before = selected(&mut project);
    assert!(selected_before.iter().any(|name| name == "Block"));

    switch_and_back(&mut project);

    assert_eq!(file_text(&mut project), edited);
    assert!(project.app.world().resource::<Scenes>().tabs[tab].dirty);
    assert_eq!(selected(&mut project), selected_before);

    run_op_clause_as_user(project.app.world_mut(), "history.undo")
        .expect("the undo dispatched")
        .assert_finished();
    project.app.update();
    project.app.update();
    assert_eq!(
        file_text(&mut project),
        clean,
        "undo reaches over the switch"
    );

    run_op_clause_as_user(project.app.world_mut(), "history.redo")
        .expect("the redo dispatched")
        .assert_finished();
    project.app.update();
    project.app.update();
    assert_eq!(file_text(&mut project), edited, "and redo comes back");
}

#[test]
fn a_prefab_tab_comes_back_as_it_left() {
    let mut project = project();
    let prefab = project.assets.join("tree.bsn");
    jackdaw::scenes::operators::scene_open_system(project.app.world_mut(), &prefab);
    project.app.update();
    let tab = tab_named(&project.app, &prefab);
    assert!(matches!(
        project.app.world().resource::<Scenes>().tabs[tab].content,
        TabContent::Prefab(_)
    ));
    let held = held_text(&mut project);
    let names = spawned(&mut project.app);

    switch_and_back(&mut project);

    assert_eq!(held_text(&mut project), held);
    assert_eq!(spawned(&mut project.app), names);

    assert!(swap_active_tab(project.app.world_mut(), 1 - tab.min(1)));
    let cache = project
        .app
        .world()
        .resource::<jackdaw::prefab::PrefabAstCache>();
    let entry = cache.get(&prefab).expect("the cache holds the prefab");
    assert_eq!(jackdaw_bsn::emit_scene(entry), held);
    assert!(entry.ast_to_ecs.is_empty(), "the cache holds no links");
}

#[test]
fn a_scene_instancing_a_prefab_keeps_its_instance_after_the_prefab_tab_was_visited() {
    let mut project = project();
    let scene_tab = tab_named(&project.app, &project.scene);
    let file = file_text(&mut project);
    let world = spawned(&mut project.app);
    let prefab = project.assets.join("tree.bsn");
    jackdaw::scenes::operators::scene_open_system(project.app.world_mut(), &prefab);
    project.app.update();

    assert!(swap_active_tab(project.app.world_mut(), scene_tab));
    project.app.update();

    assert_eq!(file_text(&mut project), file);
    assert_eq!(spawned(&mut project.app), world);
}

/// A click at the point in the frame the pointer delivers one, before the frame's
/// `Update`.
#[derive(Resource)]
struct ClickTab(usize);

fn deliver_click(world: &mut World) {
    if let Some(ClickTab(target)) = world.remove_resource::<ClickTab>() {
        jackdaw::scenes::ui::request_tab_switch(world, target);
    }
}

#[test]
fn a_clicked_tab_comes_forward_after_a_frame_that_shows_it_selected() {
    let mut project = project();
    project.app.add_systems(PreUpdate, deliver_click);
    let front = active(&project.app);
    let target = 1 - front.min(1);
    let tab_colors = |app: &mut App| {
        let mut tabs = app
            .world_mut()
            .query::<(&jackdaw::scenes::ui::SceneTabIndex, &BackgroundColor)>();
        let mut color = |index: usize| {
            tabs.iter(app.world())
                .find(|(tab, _)| tab.0 == index)
                .map(|(_, color)| color.0)
        };
        (color(front), color(target))
    };
    for index in [front, target] {
        project.app.world_mut().spawn((
            jackdaw::scenes::ui::SceneTabIndex(index),
            BackgroundColor::default(),
        ));
    }
    project
        .app
        .world_mut()
        .resource_mut::<Scenes>()
        .set_changed();
    project.app.update();
    let (selected, unselected) = tab_colors(&mut project.app);
    assert_ne!(selected, unselected);

    project.app.insert_resource(ClickTab(target));
    project.app.update();
    assert_eq!(active(&project.app), front, "the frame drawing the tab");
    assert_eq!(
        tab_colors(&mut project.app),
        (unselected, selected),
        "it draws the clicked tab as selected"
    );
    assert!(
        project
            .app
            .world()
            .contains_resource::<jackdaw::scenes::ui::PendingTabSwitch>()
    );

    project.app.update();
    assert_eq!(active(&project.app), target);
    assert!(
        !project
            .app
            .world()
            .contains_resource::<jackdaw::scenes::ui::PendingTabSwitch>()
    );
}

fn open_scene(project: &mut Project, name: &str) -> PathBuf {
    let path = project.assets.join(name);
    std::fs::write(&path, format!("#{}\n", name.replace(".bsn", "")))
        .expect("the scene is written");
    jackdaw::scenes::operators::scene_open_system(project.app.world_mut(), &path);
    project.app.update();
    path
}

fn front_path(project: &Project) -> Option<PathBuf> {
    let scenes = project.app.world().resource::<Scenes>();
    scenes.tabs[scenes.active].path.clone()
}

#[test]
fn a_pending_tab_is_dropped_when_a_close_moves_the_indices() {
    let mut project = project();
    open_scene(&mut project, "third.bsn");
    let front = open_scene(&mut project, "fourth.bsn");
    assert_eq!(project.app.world().resource::<Scenes>().tabs.len(), 4);
    assert_eq!(active(&project.app), 3);

    jackdaw::scenes::ui::request_tab_switch(project.app.world_mut(), 1);
    jackdaw::scenes::operators::scene_close_system(project.app.world_mut(), 0);
    project.app.update();
    project.app.update();

    assert_eq!(project.app.world().resource::<Scenes>().tabs.len(), 3);
    assert_eq!(front_path(&project).as_ref(), Some(&front));
}

#[test]
fn a_pending_tab_is_dropped_when_a_close_and_an_open_keep_the_count() {
    let mut project = project();
    let third = open_scene(&mut project, "third.bsn");
    open_scene(&mut project, "fourth.bsn");
    let fifth = project.assets.join("fifth.bsn");
    std::fs::write(&fifth, "#fifth\n").expect("the scene is written");

    jackdaw::scenes::ui::request_tab_switch(project.app.world_mut(), 1);
    jackdaw::scenes::operators::scene_close_system(project.app.world_mut(), 0);
    jackdaw::scenes::operators::scene_open_system(project.app.world_mut(), &fifth);
    assert_eq!(project.app.world().resource::<Scenes>().tabs.len(), 4);
    project.app.update();
    project.app.update();

    assert_eq!(front_path(&project).as_ref(), Some(&fifth));
    assert_ne!(front_path(&project).as_ref(), Some(&third));
}

/// The fixture project with the editor state running, where the sync systems do.
fn running_project() -> Project {
    let mut project = project();
    project
        .app
        .world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    project.app.update();
    project.app.update();
    project
}

fn named_entity(app: &mut App, wanted: &str) -> Entity {
    app.world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find_map(|(entity, name)| (name.as_str() == wanted).then_some(entity))
        .unwrap_or_else(|| panic!("{wanted} spawned"))
}

/// A write late in the frame, after the sync systems have read brushes and stacks.
#[derive(Resource)]
struct WriteLate(Entity);

fn write_late(world: &mut World) {
    let Some(WriteLate(entity)) = world.remove_resource::<WriteLate>() else {
        return;
    };
    if let Some(mut brush) = world.get_mut::<Brush>(entity) {
        brush.faces[0].plane.distance = 9.25;
    }
    world
        .entity_mut(entity)
        .insert(jackdaw_geometry::ModifierStack::default());
}

/// Two brushes every material of which resolves: one naming a file, one naming
/// nothing.
const RESOLVED_BRUSHES: &str = "\
bevy_ecs::hierarchy::Children [
    #Plain
    bevy_transform::components::transform::Transform
    jackdaw_scene_types::types::Brush {
        faces: [
            jackdaw_geometry::BrushFaceData {
                plane: jackdaw_geometry::BrushPlane { normal: glam::Vec3 { x: 0.0, y: 1.0, z: 0.0 }, distance: 1.5 },
            },
        ],
    }
    --
    #Soiled
    bevy_transform::components::transform::Transform
    jackdaw_scene_types::types::Brush {
        faces: [
            jackdaw_geometry::BrushFaceData {
                plane: jackdaw_geometry::BrushPlane { normal: glam::Vec3 { x: 0.0, y: 1.0, z: 0.0 }, distance: 1.5 },
                material: \"materials/soil.bsn\",
            },
        ],
    }
]
";

/// [`running_project`] with [`RESOLVED_BRUSHES`] in front, returning its tab.
fn resolved_brushes_in_front(project: &mut Project) -> usize {
    let path = project.assets.join("resolved.bsn");
    std::fs::write(&path, RESOLVED_BRUSHES).expect("the scene is written");
    jackdaw::scenes::operators::scene_open_system(project.app.world_mut(), &path);
    project.app.update();
    tab_named(&project.app, &path)
}

#[test]
fn a_write_in_the_frame_of_a_switch_reaches_the_held_document() {
    let mut project = running_project();
    let tab = resolved_brushes_in_front(&mut project);
    project
        .app
        .add_systems(PreUpdate, deliver_click)
        .add_systems(PostUpdate, write_late);
    let brush = named_entity(&mut project.app, "Plain");

    project.app.insert_resource(ClickTab(0));
    project.app.insert_resource(WriteLate(brush));
    project.app.update();
    assert_eq!(active(&project.app), tab, "the switch has not run yet");
    project.app.update();
    assert_eq!(active(&project.app), 0);

    let scenes = project.app.world().resource::<Scenes>();
    let TabContent::Scene(Some(document)) = &scenes.tabs[tab].content else {
        panic!("the tab holds its document");
    };
    let text = jackdaw_bsn::emit_scene(document);
    assert!(text.contains("9.25"), "{text}");
    assert!(text.contains("ModifierStack"), "{text}");
}

/// Every brush patch in the live document, with when it last changed.
fn brush_patch_ticks(app: &mut App) -> Vec<(Entity, Tick)> {
    let brushes: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .collect();
    let live = app.world().resource::<SceneBsnAst>();
    let mut ticks = Vec::new();
    for brush in brushes {
        let node = live.ast_for(brush).expect("a document node");
        for &patch in &live.get_patches(node).expect("patches").0 {
            let patch_ref = live
                .world
                .get_entity(patch)
                .expect("a patch entity")
                .get_ref::<BsnPatch>()
                .expect("a patch");
            if matches!(&*patch_ref, BsnPatch::Struct(data) if data.type_path.ends_with("::Brush"))
            {
                ticks.push((patch, patch_ref.last_changed()));
            }
        }
    }
    ticks
}

#[test]
fn an_activated_document_keeps_its_brush_patches_untouched() {
    let mut project = running_project();
    let tab = resolved_brushes_in_front(&mut project);
    assert!(swap_active_tab(project.app.world_mut(), 0));
    project.app.update();

    assert!(swap_active_tab(project.app.world_mut(), tab));
    let after_switch = brush_patch_ticks(&mut project.app);
    assert_eq!(after_switch.len(), 2);
    // A write from here on stamps a later tick of the document's own world.
    project
        .app
        .world_mut()
        .resource_mut::<SceneBsnAst>()
        .bypass_change_detection()
        .world
        .increment_change_tick();
    for _ in 0..4 {
        project.app.update();
    }

    assert_eq!(brush_patch_ticks(&mut project.app), after_switch);
}

fn status_text(app: &App) -> String {
    app.world()
        .resource::<jackdaw::status_bar::StatusNotice>()
        .text()
        .to_string()
}

/// A struct patch with no type path and a field named `0`, which emits text the
/// parser rejects.
fn unreadable_patch() -> BsnPatch {
    BsnPatch::Struct(jackdaw_bsn::BsnStructData {
        type_path: String::new(),
        fields: jackdaw_bsn::BsnStructFields(vec![jackdaw_bsn::BsnField {
            name: "0".to_string(),
            value: jackdaw_bsn::BsnValue::Int(1),
        }]),
    })
}

fn mark_front_dirty(app: &mut App) {
    let mut scenes = app.world_mut().resource_mut::<Scenes>();
    let front = scenes.active;
    scenes.tabs[front].dirty = true;
}

fn finish_read_back_checks(app: &mut App) {
    util::update_until(
        app,
        "the read-back checks to finish",
        std::time::Duration::from_secs(30),
        |app| {
            !app.world()
                .contains_resource::<jackdaw::scenes::swap::ReadBackChecks>()
        },
    );
}

#[test]
fn an_edited_document_that_would_not_read_back_is_reported_and_stays_editable() {
    let mut project = project();
    project
        .app
        .init_resource::<jackdaw::status_bar::StatusNotice>();
    let tab = tab_named(&project.app, &project.scene);
    mark_front_dirty(&mut project.app);
    {
        let mut live = project.app.world_mut().resource_mut::<SceneBsnAst>();
        let odd = live.create_entity_node(vec![unreadable_patch()]);
        live.add_to_roots(odd);
    }

    assert!(swap_active_tab(project.app.world_mut(), 1 - tab));
    finish_read_back_checks(&mut project.app);

    let notice = status_text(&project.app);
    assert!(notice.starts_with("scene will not save: "), "{notice}");
    assert!(notice.len() <= 48, "fits a narrow status bar: {notice}");
    assert!(swap_active_tab(project.app.world_mut(), tab));
    assert!(
        project.app.world().resource::<Scenes>().tabs[tab]
            .refusal
            .is_none()
    );
}

#[test]
fn an_edited_document_that_reads_back_is_not_reported() {
    let mut project = project();
    project
        .app
        .init_resource::<jackdaw::status_bar::StatusNotice>();
    let tab = tab_named(&project.app, &project.scene);
    mark_front_dirty(&mut project.app);

    assert!(swap_active_tab(project.app.world_mut(), 1 - tab));
    finish_read_back_checks(&mut project.app);

    assert_eq!(status_text(&project.app), "");
}

#[test]
fn a_clean_tab_is_not_checked() {
    let mut project = project();
    let tab = tab_named(&project.app, &project.scene);
    {
        let mut live = project.app.world_mut().resource_mut::<SceneBsnAst>();
        let odd = live.create_entity_node(vec![unreadable_patch()]);
        live.add_to_roots(odd);
    }

    assert!(swap_active_tab(project.app.world_mut(), 1 - tab));

    assert!(
        !project
            .app
            .world()
            .contains_resource::<jackdaw::scenes::swap::ReadBackChecks>()
    );
}

#[test]
fn a_clean_scene_keeps_its_text_when_a_prefab_it_uses_turned_unreadable() {
    let mut project = project();
    project
        .app
        .init_resource::<jackdaw::status_bar::StatusNotice>();
    let scene_tab = tab_named(&project.app, &project.scene);
    assert!(jackdaw::scene_io::save_scene(project.app.world_mut()));
    let saved = std::fs::read_to_string(&project.scene).expect("the file");
    let prefab = project.assets.join("tree.bsn");
    jackdaw::scenes::operators::scene_open_system(project.app.world_mut(), &prefab);
    project.app.update();

    mark_front_dirty(&mut project.app);
    {
        let mut live = project.app.world_mut().resource_mut::<SceneBsnAst>();
        let root = live.roots[0];
        let child = live.descendants_of(root)[0];
        for node in [root, child] {
            let patch = live.world.spawn(unreadable_patch()).id();
            live.get_patches_mut(node).expect("patches").0.push(patch);
        }
    }

    assert!(swap_active_tab(project.app.world_mut(), scene_tab));
    project.app.update();
    finish_read_back_checks(&mut project.app);
    let notice = status_text(&project.app);
    assert!(notice.starts_with("tree will not save: "), "{notice}");

    assert!(jackdaw::scene_io::save_scene(project.app.world_mut()));
    assert_eq!(
        std::fs::read_to_string(&project.scene).expect("the file"),
        saved
    );
}
