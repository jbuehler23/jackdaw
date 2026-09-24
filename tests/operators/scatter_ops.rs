//! The scatter ops a caller with no pointer needs: adopting a hand-authored group
//! into the terrain's stored scatter, naming a group to act on, and taking one
//! placement back out. Until one has been run, every re-scatter doubles the
//! groups a generated scene already holds as entities.

use crate::util;

use bevy::prelude::*;
use jackdaw::boot_ops::{SELECTION_FALLBACK_OPS, run_op_clause};
use jackdaw::commands::CommandHistory;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{GltfSource, ScatterGroup};

/// Run one clause and tick the frame its queued work needs, the way the
/// boot queue's frame gap does.
#[track_caller]
fn run(app: &mut App, clause: &str) {
    let result = run_op_clause(app.world_mut(), clause)
        .unwrap_or_else(|err| panic!("{clause}: dispatch errored: {err}"));
    assert_eq!(
        result,
        OperatorResult::Finished,
        "{clause} reported {result:?}"
    );
    app.update();
}

/// A scene with one terrain that has a document to store scatter in. The sidecar
/// path is minted here: the editor's own system is gated on a state a headless
/// test never enters.
fn scene_with_a_terrain() -> (App, Entity) {
    let mut app = util::editor_test_app();
    run(&mut app, "scene.new");
    run(&mut app, "entity.add.terrain");
    let mut query = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw_scene_types::Terrain>>();
    let terrain = query
        .iter(app.world())
        .next()
        .expect("entity.add.terrain authored a terrain");

    let data_path = "scene.terrain-0.jdterrain";
    let mut document = jackdaw_terrain::RegionTerrainData::default();
    document.regions.set_height(0, 0, 1.0);
    app.world_mut()
        .resource_mut::<jackdaw::terrain::TerrainDataStore>()
        .insert(data_path, document);
    app.world_mut()
        .get_mut::<jackdaw_scene_types::Terrain>(terrain)
        .expect("a terrain")
        .data_path = data_path.to_string();
    app.update();
    (app, terrain)
}

/// A group of models placed by hand beside the terrain, the shape
/// `gen_zone` and a drag from the Project window both leave.
fn hand_authored_group(app: &mut App) -> (Entity, Entity) {
    let group = app
        .world_mut()
        .spawn((
            Name::new("Scatter_Trees"),
            Transform::from_xyz(3.0, 0.0, -2.0),
            Visibility::default(),
        ))
        .id();
    let model = app
        .world_mut()
        .spawn((
            Name::new("Tree"),
            GltfSource {
                path: "kit/Tree.gltf".to_string(),
                scene_index: 0,
            },
            Transform::from_xyz(1.0, 0.0, 1.0),
            Visibility::default(),
            ChildOf(group),
        ))
        .id();
    app.update();
    (group, model)
}

/// The stored placements of `terrain`, by group key and count.
fn stored_groups(app: &App, terrain: Entity) -> Vec<(String, usize)> {
    let data_path = app
        .world()
        .get::<jackdaw_scene_types::Terrain>(terrain)
        .expect("a terrain")
        .data_path
        .clone();
    jackdaw::terrain::scatter_data::group_counts(
        app.world().resource::<jackdaw::terrain::TerrainDataStore>(),
        &data_path,
    )
}

/// Adoption moves the group into the terrain's document from the selection alone:
/// the entities go and a placement stands where each model stood.
#[test]
fn an_adopt_clause_stores_the_selected_group_on_the_terrain() {
    let (mut app, terrain) = scene_with_a_terrain();
    let (group, model) = hand_authored_group(&mut app);
    let stood_at = app
        .world()
        .get::<GlobalTransform>(model)
        .copied()
        .expect("transform propagation ran")
        .translation();
    app.world_mut().resource_mut::<Selection>().entities = vec![group];

    run(&mut app, "terrain.scatter.adopt");

    assert!(app.world().get_entity(group).is_err(), "the group is gone");
    assert!(app.world().get_entity(model).is_err(), "the model is gone");
    assert_eq!(
        stored_groups(&app, terrain),
        vec![("Scatter_Trees".to_string(), 1)]
    );

    let data_path = app
        .world()
        .get::<jackdaw_scene_types::Terrain>(terrain)
        .unwrap()
        .data_path
        .clone();
    let store = app.world().resource::<jackdaw::terrain::TerrainDataStore>();
    let data = store.get(&data_path).expect("a document");
    let (coord, _, placement) = data.placements().next().expect("one placement");
    assert_eq!(
        data.scatter
            .asset(placement.asset)
            .map(|e| e.asset.as_str()),
        Some("kit/Tree.gltf")
    );
    assert!(
        data.placement_position(coord, placement)
            .abs_diff_eq(stood_at, 1e-3),
        "the placement stands where the model stood"
    );
}

/// One placement comes back out as an ordinary model entity, which is how
/// a hand edits one instance of a stored group.
#[test]
fn a_promote_clause_turns_one_placement_back_into_an_entity() {
    let (mut app, terrain) = scene_with_a_terrain();
    let (group, _) = hand_authored_group(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    run(&mut app, "terrain.scatter.adopt");

    run(
        &mut app,
        "terrain.scatter.promote key=Scatter_Trees index=0",
    );

    assert!(stored_groups(&app, terrain).is_empty());
    let mut query = app.world_mut().query::<&GltfSource>();
    assert_eq!(
        query
            .iter(app.world())
            .filter(|source| source.path == "kit/Tree.gltf")
            .count(),
        1,
        "the promoted placement is a model entity again"
    );
}

/// A clear names a stored group and empties it.
#[test]
fn a_clear_clause_empties_a_stored_group() {
    let (mut app, terrain) = scene_with_a_terrain();
    let (group, _) = hand_authored_group(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    run(&mut app, "terrain.scatter.adopt");
    assert_eq!(stored_groups(&app, terrain).len(), 1);

    run(&mut app, "terrain.scatter.clear group=Scatter_Trees");

    assert!(stored_groups(&app, terrain).is_empty());
}

/// The whole adoption is one history entry: the user asked for one thing,
/// so one undo puts the scene back and empties the document again.
#[test]
fn an_adoption_is_one_undo_entry() {
    let (mut app, terrain) = scene_with_a_terrain();
    let (group, _) = hand_authored_group(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];

    let before = app.world().resource::<CommandHistory>().undo_stack.len();
    run(&mut app, "terrain.scatter.adopt");
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1,
        "adoption must land as a single entry"
    );

    run(&mut app, "history.undo");

    assert!(
        stored_groups(&app, terrain).is_empty(),
        "undo left placements behind in the document"
    );
    let mut query = app.world_mut().query::<&GltfSource>();
    assert_eq!(
        query.iter(app.world()).count(),
        1,
        "undo did not put the model entity back"
    );
}

/// A group a previous build stamped as entities is adopted under the key it
/// already carries, so its groups do not change name.
#[test]
fn adopting_a_group_that_is_already_stamped_keeps_its_key() {
    let (mut app, terrain) = scene_with_a_terrain();
    let group = app
        .world_mut()
        .spawn((
            Name::new("Renamed"),
            ScatterGroup {
                generator: "terrain.scatter".to_string(),
                key: "Trees".to_string(),
            },
            Transform::default(),
            Visibility::default(),
            ChildOf(terrain),
        ))
        .id();
    app.world_mut().spawn((
        Name::new("Tree"),
        GltfSource {
            path: "kit/Tree.gltf".to_string(),
            scene_index: 0,
        },
        Transform::from_xyz(1.0, 0.0, 1.0),
        Visibility::default(),
        ChildOf(group),
    ));
    app.update();
    app.world_mut().resource_mut::<Selection>().entities = vec![group];

    run(&mut app, "terrain.scatter.adopt");

    assert_eq!(stored_groups(&app, terrain), vec![("Trees".to_string(), 1)]);
    assert!(app.world().get_entity(group).is_err());
}

/// Naming a group selects it, which is what the panel's buttons and a
/// script both need before acting on one.
#[test]
fn a_group_select_clause_selects_the_group_with_that_key() {
    let (mut app, terrain) = scene_with_a_terrain();
    let group = app
        .world_mut()
        .spawn((
            Name::new("Trees"),
            ScatterGroup {
                generator: "terrain.scatter".to_string(),
                key: "Trees".to_string(),
            },
            Transform::default(),
            Visibility::default(),
            ChildOf(terrain),
        ))
        .id();
    app.update();

    run(&mut app, "terrain.scatter.group.select key=Trees");

    assert_eq!(app.world().resource::<Selection>().primary(), Some(group));
}

/// The clause form of adoption acts on the selection, so the operator has to be
/// in the fallback list or every scripted run is short a target.
#[test]
fn adopt_takes_its_target_from_the_selection() {
    assert!(SELECTION_FALLBACK_OPS.contains(&"terrain.scatter.adopt"));
}

/// A scripted clear names its group and has no selection to fall back on;
/// finishing silently there reads as "the group is gone".
#[test]
fn clearing_with_no_terrain_says_so_rather_than_finishing_quietly() {
    let mut app = util::editor_test_app();
    run(&mut app, "scene.new");
    app.world_mut()
        .get_resource_or_init::<jackdaw_api_internal::operator::OperatorWarnings>()
        .0
        .clear();

    run(&mut app, "terrain.scatter.clear group=Undergrowth");

    let warnings = app
        .world()
        .resource::<jackdaw_api_internal::operator::OperatorWarnings>()
        .0
        .clone();
    assert!(
        warnings.iter().any(|w| w.contains("no terrain resolved")),
        "the caller has to hear that nothing was cleared: {warnings:?}"
    );
    let report = app
        .world()
        .resource::<jackdaw::terrain::scatter::TerrainScatterReport>();
    assert!(
        report.message.contains("no terrain resolved"),
        "the panel says the same thing: {:?}",
        report.message
    );
}

/// A scatter mask's channels and palette entries carry names, and the numbers
/// behind them are the one thing a caller outside the editor can see least of.
/// A name that is dropped rather than resolved leaves no mask at all, which
/// scatters over the whole terrain instead of the painted part of it.
#[test]
fn scatter_takes_a_mask_and_a_palette_value_by_name() {
    let (mut app, terrain) = scene_with_a_terrain();
    app.world_mut().resource_mut::<Selection>().entities = vec![terrain];
    app.update();
    run(&mut app, "terrain.channel.add");
    paint_a_corner_of_the_mask(&mut app, terrain, 1);

    run(
        &mut app,
        "terrain.scatter group=Everywhere assets=kit/Tree.gltf density=0.05 spacing=0.0",
    );
    run(
        &mut app,
        "terrain.scatter group=Painted channel=channel-0 accept=value-1 \
         assets=kit/Tree.gltf density=0.05 spacing=0.0",
    );

    let everywhere = placed_in(&app, terrain, "Everywhere");
    let painted = placed_in(&app, terrain, "Painted");
    assert!(painted > 0, "the painted corner took no instances");
    assert!(
        painted < everywhere,
        "the palette name left no mask: {painted} of {everywhere} placed"
    );
}

/// A name no palette carries is a caller's mistake, and the answer has to say
/// which names it could have used.
#[test]
fn scatter_answers_an_unknown_palette_name_with_the_ones_the_mask_has() {
    let (mut app, terrain) = scene_with_a_terrain();
    app.world_mut().resource_mut::<Selection>().entities = vec![terrain];
    app.update();
    run(&mut app, "terrain.channel.add");
    paint_a_corner_of_the_mask(&mut app, terrain, 1);

    app.world_mut()
        .get_resource_or_init::<jackdaw_api_internal::operator::OperatorReports>()
        .0
        .clear();
    run(
        &mut app,
        "terrain.scatter group=Wrong channel=channel-0 accept=meadow \
         assets=kit/Tree.gltf density=0.05 spacing=0.0",
    );

    assert_eq!(placed_in(&app, terrain, "Wrong"), 0);
    let reports = app
        .world_mut()
        .get_resource_or_init::<jackdaw_api_internal::operator::OperatorReports>()
        .0
        .clone();
    assert!(
        reports.iter().any(
            |report| report.contains("no palette value 'meadow'") && report.contains("value-1")
        ),
        "the answer did not say what the mask's palette holds: {reports:?}"
    );
}

/// How many instances one scatter group holds on this terrain.
fn placed_in(app: &App, terrain: Entity, group: &str) -> usize {
    stored_groups(app, terrain)
        .into_iter()
        .filter(|(key, _)| key == group)
        .map(|(_, count)| count)
        .sum()
}

/// Ground under the whole terrain, with one value painted into a corner of
/// its first mask.
fn paint_a_corner_of_the_mask(app: &mut App, terrain: Entity, value: u16) {
    const SIDE: i32 = 64;
    const CORNER: i32 = 16;

    let data_path = app
        .world()
        .get::<jackdaw_scene_types::Terrain>(terrain)
        .expect("a terrain")
        .data_path
        .clone();
    let mut document = jackdaw_terrain::RegionTerrainData {
        channels: vec![jackdaw_terrain::ChannelDescriptor::new(
            "channel-0",
            jackdaw_terrain::ChannelElement::U8,
        )],
        ..default()
    };
    document.regions.set_channel_count(1);
    for z in 0..SIDE {
        for x in 0..SIDE {
            document.regions.set_height(x, z, 1.0);
            if x < CORNER && z < CORNER {
                document.regions.set_channel(0, x, z, value);
            }
        }
    }
    app.world_mut()
        .resource_mut::<jackdaw::terrain::TerrainDataStore>()
        .insert(&data_path, document);
    app.update();
}

fn stored_materials(app: &App, terrain: Entity) -> std::collections::BTreeMap<String, String> {
    let data_path = app
        .world()
        .get::<jackdaw_scene_types::Terrain>(terrain)
        .expect("a terrain")
        .data_path
        .clone();
    let store = app.world().resource::<jackdaw::terrain::TerrainDataStore>();
    let data = store.get(&data_path).expect("a document");
    data.scatter
        .assets
        .iter()
        .find(|entry| entry.asset == "kit/Tree.gltf")
        .map(|entry| entry.materials.clone())
        .unwrap_or_default()
}

fn a_known_material(app: &mut App, path: &str) {
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let mut references = app
        .world_mut()
        .remove_resource::<jackdaw_bsn::BsnProjectAssets>()
        .unwrap_or_else(|| jackdaw_bsn::BsnProjectAssets(Default::default()));
    references.0.insert(path.to_string(), handle.untyped());
    app.world_mut().insert_resource(references);
}

#[test]
fn a_models_material_overrides_follow_it_into_stored_scatter_and_back_out() {
    let (mut app, terrain) = scene_with_a_terrain();
    let (group, model) = hand_authored_group(&mut app);
    let overrides = jackdaw_scene_types::MaterialOverrides {
        materials: [("Leaves".to_string(), "materials/pine.bsn".to_string())]
            .into_iter()
            .collect(),
    };
    app.world_mut().entity_mut(model).insert(overrides.clone());
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    run(&mut app, "terrain.scatter.adopt");

    assert_eq!(stored_materials(&app, terrain), overrides.materials);

    run(
        &mut app,
        "terrain.scatter.promote key=Scatter_Trees index=0",
    );
    let mut query = app
        .world_mut()
        .query::<(&GltfSource, &jackdaw_scene_types::MaterialOverrides)>();
    let (_, promoted) = query
        .iter(app.world())
        .find(|(source, _)| source.path == "kit/Tree.gltf")
        .expect("the promoted model carries the overrides");
    assert_eq!(promoted, &overrides);
}

#[test]
fn a_stored_models_material_override_is_one_undo_entry() {
    let (mut app, terrain) = scene_with_a_terrain();
    let (group, _) = hand_authored_group(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    run(&mut app, "terrain.scatter.adopt");
    a_known_material(&mut app, "materials/bark.bsn");

    let before = app.world().resource::<CommandHistory>().undo_stack.len();
    run(
        &mut app,
        "terrain.scatter.palette.material asset=kit/Tree.gltf name=Bark material=materials/bark.bsn",
    );
    assert_eq!(
        stored_materials(&app, terrain)
            .get("Bark")
            .map(String::as_str),
        Some("materials/bark.bsn")
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1
    );

    run(&mut app, "history.undo");
    assert!(stored_materials(&app, terrain).is_empty());
}

#[test]
fn a_stored_material_override_naming_no_material_is_refused() {
    let (mut app, terrain) = scene_with_a_terrain();
    let (group, _) = hand_authored_group(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    run(&mut app, "terrain.scatter.adopt");

    let result = run_op_clause(
        app.world_mut(),
        "terrain.scatter.palette.material asset=kit/Tree.gltf name=Bark material=materials/none.bsn",
    )
    .expect("the clause dispatches");
    assert_eq!(result, OperatorResult::Cancelled);
    assert!(stored_materials(&app, terrain).is_empty());
}

fn stored_placements(app: &App, terrain: Entity) -> Vec<(String, Vec3, f32, f32)> {
    let data_path = app
        .world()
        .get::<jackdaw_scene_types::Terrain>(terrain)
        .expect("a terrain")
        .data_path
        .clone();
    let store = app.world().resource::<jackdaw::terrain::TerrainDataStore>();
    let data = store.get(&data_path).expect("a document");
    data.placements()
        .map(|(coord, _, placement)| {
            (
                data.scatter
                    .asset(placement.asset)
                    .map(|entry| entry.asset.clone())
                    .unwrap_or_default(),
                data.placement_position(coord, placement),
                placement.yaw,
                placement.scale,
            )
        })
        .collect()
}

#[test]
fn an_imported_layout_lands_exactly_where_it_says_as_one_undo_entry() {
    let (mut app, terrain) = scene_with_a_terrain();
    let project = tempfile::tempdir().expect("tempdir");
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot::new(
            project.path().to_path_buf(),
            jackdaw::project::ProjectConfig::default(),
        ));
    std::fs::write(
        project.path().join("layout.json"),
        r#"[
            {"asset": "kit/Tree.gltf", "x": 1.5, "y": 0.5, "z": 2.5, "yaw": 90.0, "scale": 1.2,
             "materials": {"Leaves": "materials/pine.bsn"}},
            {"asset": "kit/Bush.gltf", "x": 3.0, "y": 0.25, "z": 1.0},
            {"asset": "kit/Tree.gltf", "x": 2.0, "y": 0.5, "z": 3.0, "yaw": -45.0, "scale": 0.8}
        ]"#,
    )
    .expect("the layout is written");

    let before = app.world().resource::<CommandHistory>().undo_stack.len();
    run(
        &mut app,
        "terrain.scatter.import path=layout.json group=woods",
    );

    let placed = stored_placements(&app, terrain);
    assert_eq!(placed.len(), 3);
    let tree = placed
        .iter()
        .find(|(asset, at, ..)| {
            asset == "kit/Tree.gltf" && at.abs_diff_eq(Vec3::new(1.5, 0.5, 2.5), 1e-3)
        })
        .expect("the first tree stands where the file put it");
    assert!((tree.2 - 90.0_f32.to_radians()).abs() < 1e-4);
    assert!((tree.3 - 1.2).abs() < 1e-4);
    assert_eq!(stored_groups(&app, terrain), vec![("woods".to_string(), 3)]);
    assert_eq!(
        stored_materials(&app, terrain)
            .get("Leaves")
            .map(String::as_str),
        Some("materials/pine.bsn"),
        "the file's overrides reach the palette entry"
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1
    );

    run(&mut app, "history.undo");
    assert!(stored_placements(&app, terrain).is_empty());
}

#[test]
fn a_layout_file_outside_the_project_is_refused() {
    let (mut app, _terrain) = scene_with_a_terrain();
    let project = tempfile::tempdir().expect("tempdir");
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot::new(
            project.path().to_path_buf(),
            jackdaw::project::ProjectConfig::default(),
        ));
    let result = run_op_clause(
        app.world_mut(),
        "terrain.scatter.import path=../layout.json",
    )
    .expect("the clause dispatches");
    assert_eq!(result, OperatorResult::Cancelled);
}

/// The Scatter panel's Import Placements presses the operator with no
/// arguments, reading the file and group the panel holds; that has to be the
/// same import, and the same undo entry, as a script naming both.
#[test]
fn the_panels_import_placements_is_the_same_undo_entry_as_the_arguments() {
    let layout = r#"[
        {"asset": "kit/Tree.gltf", "x": 1.5, "y": 0.5, "z": 2.5, "yaw": 90.0, "scale": 1.2},
        {"asset": "kit/Bush.gltf", "x": 3.0, "y": 0.25, "z": 1.0}
    ]"#;
    let import = |panel: bool| {
        let (mut app, terrain) = scene_with_a_terrain();
        let project = tempfile::tempdir().expect("tempdir");
        app.world_mut()
            .insert_resource(jackdaw::project::ProjectRoot::new(
                project.path().to_path_buf(),
                jackdaw::project::ProjectConfig::default(),
            ));
        std::fs::create_dir_all(project.path().join("layouts")).expect("a layouts folder");
        std::fs::write(project.path().join("layouts/woods.json"), layout)
            .expect("the layout is written");
        let before = app.world().resource::<CommandHistory>().undo_stack.len();
        if panel {
            let mut state = app
                .world_mut()
                .resource_mut::<jackdaw::terrain::scatter::TerrainScatterState>();
            state.import_path = "layouts/woods.json".to_string();
            state.import_group = "woods".to_string();
            run(&mut app, "terrain.scatter.import");
        } else {
            run(
                &mut app,
                "terrain.scatter.import path=layouts/woods.json group=woods",
            );
        }
        let history = app.world().resource::<CommandHistory>();
        assert_eq!(history.undo_stack.len(), before + 1, "one undo entry");
        let entry = history
            .undo_stack
            .last()
            .map(|entry| (entry.description().to_string(), entry.heap_bytes()))
            .expect("an entry");
        (
            entry,
            stored_placements(&app, terrain),
            stored_groups(&app, terrain),
        )
    };

    let (panel_entry, panel_placed, panel_groups) = import(true);
    let (script_entry, script_placed, script_groups) = import(false);
    assert_eq!(panel_entry, script_entry);
    assert_eq!(panel_placed, script_placed);
    assert_eq!(panel_groups, vec![("woods".to_string(), 2)]);
    assert_eq!(panel_groups, script_groups);
}

/// A prefab drawing `kit/Tree.gltf` with its leaves in `leaves`.
fn tree_prefab(leaves: &str) -> String {
    format!(
        r#"jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
#Tree
bevy_transform::components::transform::Transform
bevy_ecs::hierarchy::Children [
    #Tree
    bevy_transform::components::transform::Transform
    jackdaw_scene_types::types::GltfSource {{ path: "kit/Tree.gltf", scene_index: 0 }}
    jackdaw_scene_types::types::MaterialOverrides {{ materials: map[("Leaves", "{leaves}")] }}
    jackdaw::prefab::components::PrefabEntityId(1)
]
"#
    )
}

/// What the terrain's document draws, read from the store by its path since a
/// prefab reaching the cache respawns the scene around it.
fn stored_assets(app: &App) -> Vec<String> {
    let data_path = "scene.terrain-0.jdterrain";
    let store = app.world().resource::<jackdaw::terrain::TerrainDataStore>();
    let data = store.get(data_path).expect("a document");
    data.scatter
        .assets
        .iter()
        .map(|entry| entry.asset.clone())
        .collect()
}

/// Hand the stored scatter to the renderer and let it ask for what it draws,
/// as the editor's own sync does once a project is open.
fn draw_stored_scatter(app: &mut App) {
    app.world_mut()
        .run_system_cached(jackdaw::terrain::scatter_data::sync_terrain_scatter)
        .expect("the sync runs");
    for _ in 0..4 {
        app.update();
    }
}

fn prefab_leaves(app: &App) -> Option<String> {
    app.world()
        .resource::<jackdaw_terrain::render::ScatterPrefabs>()
        .get("prefabs/tree.bsn")
        .and_then(|prefab| prefab.materials.get("Leaves").cloned())
}

#[test]
fn stored_placements_pointed_at_a_prefab_wear_what_it_wears_and_follow_its_changes() {
    let (mut app, _) = scene_with_a_terrain();
    let dir = tempfile::tempdir().expect("tempdir");
    let prefab = dir.path().join("assets/prefabs/tree.bsn");
    std::fs::create_dir_all(prefab.parent().expect("parent")).expect("a prefabs folder");
    std::fs::write(&prefab, tree_prefab("materials/pine.bsn")).expect("write the prefab");
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot::new(
            dir.path().to_path_buf(),
            jackdaw::project::ProjectConfig::default(),
        ));
    let (group, _) = hand_authored_group(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    run(&mut app, "terrain.scatter.adopt");

    let before = app.world().resource::<CommandHistory>().undo_stack.len();
    run(
        &mut app,
        "terrain.scatter.palette.asset asset=kit/Tree.gltf to=prefabs/tree.bsn",
    );
    assert_eq!(stored_assets(&app), vec!["prefabs/tree.bsn"]);
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1,
        "pointing an entry elsewhere is one undo entry"
    );
    draw_stored_scatter(&mut app);
    assert_eq!(
        prefab_leaves(&app).as_deref(),
        Some("materials/pine.bsn"),
        "the entry wears the prefab's leaves"
    );

    std::fs::write(&prefab, tree_prefab("materials/larch.bsn")).expect("rewrite the prefab");
    let assets_root = jackdaw::prefab::save_load::source_root_of(app.world(), &prefab);
    let ast = jackdaw::prefab::save_load::read_prefab_ast(&prefab, &assets_root)
        .expect("the new prefab parses");
    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .insert(&prefab, ast);
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(
        prefab_leaves(&app).as_deref(),
        Some("materials/larch.bsn"),
        "a prefab dressed anew redresses every placement drawing it"
    );

    run(&mut app, "history.undo");
    assert_eq!(stored_assets(&app), vec!["kit/Tree.gltf"]);
}

#[test]
fn an_entry_is_not_pointed_at_a_file_that_draws_nothing() {
    let (mut app, _) = scene_with_a_terrain();
    let (group, _) = hand_authored_group(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    run(&mut app, "terrain.scatter.adopt");

    let result = run_op_clause(
        app.world_mut(),
        "terrain.scatter.palette.asset asset=kit/Tree.gltf to=kit/Tree.png",
    )
    .expect("the clause dispatches");
    assert_eq!(result, OperatorResult::Cancelled, "a texture draws nothing");
    assert_eq!(stored_assets(&app), vec!["kit/Tree.gltf"]);
}
