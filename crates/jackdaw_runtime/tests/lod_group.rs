//! A LOD group's levels show over the distances their screen heights stand
//! for at the camera's field of view.

#![cfg(feature = "render")]

use std::path::PathBuf;
use std::time::Duration;

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;
use jackdaw_runtime::{
    JackdawCatalogPath, JackdawPlugin, JackdawScene, JackdawSceneRoot, LiveLevelProgress,
    LiveLevelSettings, LodPart, LodPlugin, level_shows,
};
use jackdaw_scene_types::model_parts::{FlatModel, ModelPart, ModelParts};
use jackdaw_scene_types::{GltfSource, LodGroup, LodLevel};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, LodPlugin));
    app
}

fn camera(app: &mut App, fov_degrees: f32) -> Entity {
    app.world_mut()
        .spawn((
            Camera3d::default(),
            Projection::Perspective(PerspectiveProjection {
                fov: fov_degrees.to_radians(),
                ..default()
            }),
        ))
        .id()
}

fn group(size: f32) -> LodGroup {
    heights(&[0.5, 0.25, 0.1], size)
}

fn heights(screen_heights: &[f32], size: f32) -> LodGroup {
    LodGroup {
        levels: screen_heights
            .iter()
            .map(|&screen_height| LodLevel { screen_height })
            .collect(),
        size,
        fade: 0.0,
    }
}

/// A group with one mesh under each of its levels, returned in level order.
fn spawn_group(app: &mut App, group: LodGroup) -> (Entity, Vec<Entity>) {
    let count = group.levels.len();
    let root = app.world_mut().spawn((group, Transform::default())).id();
    let meshes = (0..count)
        .map(|_| {
            let level = app
                .world_mut()
                .spawn((Transform::default(), ChildOf(root)))
                .id();
            app.world_mut()
                .spawn((Mesh3d::default(), ChildOf(level)))
                .id()
        })
        .collect();
    (root, meshes)
}

fn range(app: &App, mesh: Entity) -> VisibilityRange {
    app.world()
        .get::<VisibilityRange>(mesh)
        .expect("the level's mesh has a range")
        .clone()
}

fn distance(size: f32, screen_height: f32, fov_degrees: f32) -> f32 {
    size / (2.0 * screen_height * (fov_degrees.to_radians() / 2.0).tan())
}

#[test]
fn each_level_shows_between_the_distances_its_screen_heights_stand_for() {
    let mut app = app();
    camera(&mut app, 60.0);
    let (_, meshes) = spawn_group(&mut app, group(2.0));
    app.update();
    app.update();

    let first = range(&app, meshes[0]);
    let second = range(&app, meshes[1]);
    let last = range(&app, meshes[2]);
    assert_eq!(first.start_margin, 0.0..0.0);
    assert!((first.end_margin.start - distance(2.0, 0.5, 60.0)).abs() < 1e-4);
    assert_eq!(second.start_margin, first.end_margin);
    assert_eq!(last.start_margin, second.end_margin);
    assert!((last.end_margin.start - distance(2.0, 0.1, 60.0)).abs() < 1e-3);
}

#[test]
fn a_narrower_field_of_view_pushes_every_switch_further_out() {
    let mut app = app();
    let camera = camera(&mut app, 60.0);
    let (_, meshes) = spawn_group(&mut app, group(2.0));
    app.update();
    app.update();
    let wide = range(&app, meshes[0]).end_margin.start;

    app.world_mut()
        .entity_mut(camera)
        .insert(Projection::Perspective(PerspectiveProjection {
            fov: 30f32.to_radians(),
            ..default()
        }));
    app.update();

    let narrow = range(&app, meshes[0]).end_margin.start;
    assert!((narrow - distance(2.0, 0.5, 30.0)).abs() < 1e-3);
    assert!(narrow > wide);
}

#[test]
fn a_group_with_no_size_measures_its_first_level() {
    let mut app = app();
    camera(&mut app, 60.0);
    let (_, meshes) = spawn_group(&mut app, group(0.0));
    app.world_mut()
        .entity_mut(meshes[0])
        .insert(Aabb::from_min_max(
            Vec3::new(-1.0, 0.0, -0.5),
            Vec3::new(1.0, 1.0, 0.5),
        ));
    app.update();
    app.update();

    assert!((range(&app, meshes[0]).end_margin.start - distance(2.0, 0.5, 60.0)).abs() < 1e-3);
}

#[test]
fn a_mesh_that_arrives_after_its_group_gets_its_level_range() {
    let mut app = app();
    camera(&mut app, 60.0);
    let (root, meshes) = spawn_group(&mut app, group(2.0));
    app.update();
    app.update();

    let level = app.world().get::<ChildOf>(meshes[1]).unwrap().parent();
    let late = app
        .world_mut()
        .spawn((Mesh3d::default(), ChildOf(level)))
        .id();
    app.update();
    app.update();

    assert!(app.world().get::<LodGroup>(root).is_some());
    assert!(range(&app, late) == range(&app, meshes[1]));
}

#[test]
fn fading_widens_each_switch_into_a_shared_margin() {
    let mut app = app();
    camera(&mut app, 60.0);
    let (_, meshes) = spawn_group(
        &mut app,
        LodGroup {
            fade: 0.2,
            ..group(2.0)
        },
    );
    app.update();
    app.update();

    let first = range(&app, meshes[0]);
    let second = range(&app, meshes[1]);
    let switch = distance(2.0, 0.5, 60.0);
    assert!((first.end_margin.start - switch * 0.9).abs() < 1e-3);
    assert!((first.end_margin.end - switch * 1.1).abs() < 1e-3);
    assert_eq!(second.start_margin, first.end_margin);
}

#[test]
fn a_group_with_no_size_is_measured_from_its_first_levels_model_before_anything_spawns() {
    let mut app = app();
    camera(&mut app, 60.0);
    app.world_mut().resource_mut::<ModelParts>().insert(
        "models/tree.gltf",
        FlatModel {
            parts: Vec::new(),
            bounds: Aabb::from_min_max(Vec3::new(-1.0, 0.0, -0.5), Vec3::new(1.0, 1.0, 0.5)),
            needs_instance: false,
        },
    );
    let root = app
        .world_mut()
        .spawn((group(0.0), Transform::default()))
        .id();
    app.world_mut().spawn((
        GltfSource {
            path: "models/tree.gltf".into(),
            scene_index: 0,
        },
        Transform::default(),
        ChildOf(root),
    ));
    let second = app
        .world_mut()
        .spawn((Transform::default(), ChildOf(root)))
        .id();
    let mesh = app
        .world_mut()
        .spawn((Mesh3d::default(), ChildOf(second)))
        .id();
    app.update();
    app.update();

    let range = range(&app, mesh);
    assert!((range.start_margin.start - distance(2.0, 0.5, 60.0)).abs() < 1e-3);
}

#[test]
fn a_level_as_high_as_the_one_before_it_never_shows_and_the_next_takes_over_where_that_one_ends() {
    let group = heights(&[0.5, 0.25, 0.25, 0.1], 2.0);
    assert!(level_shows(&group, 0));
    assert!(level_shows(&group, 1));
    assert!(!level_shows(&group, 2));
    assert!(level_shows(&group, 3));

    let mut app = app();
    camera(&mut app, 60.0);
    let (_, meshes) = spawn_group(&mut app, group);
    app.update();
    app.update();

    let second = range(&app, meshes[1]);
    let last = range(&app, meshes[3]);
    assert!((second.end_margin.start - distance(2.0, 0.25, 60.0)).abs() < 1e-3);
    assert_eq!(last.start_margin, second.end_margin);
    assert!((last.end_margin.start - distance(2.0, 0.1, 60.0)).abs() < 1e-3);
}

const GROUP_WITH_A_MODEL_PAST_ITS_LEVELS: &str = "#Tree\n\
     jackdaw_scene_types::types::LodGroup { levels: [\
     jackdaw_scene_types::types::LodLevel { screen_height: 0.5 },\
     jackdaw_scene_types::types::LodLevel { screen_height: 0.1 },\
     ] }\n\
     bevy_ecs::hierarchy::Children [\n\
     #LOD0\n\
     jackdaw_scene_types::types::GltfSource { path: \"tree.gltf\", scene_index: 0 }\n\
     ,\n\
     #LOD1\n\
     jackdaw_scene_types::types::GltfSource { path: \"tree_LOD1.gltf\", scene_index: 0 }\n\
     ,\n\
     #Nest\n\
     jackdaw_scene_types::types::GltfSource { path: \"nest.gltf\", scene_index: 0 }\n\
     ]\n";

#[test]
fn a_lod_level_is_kept_live_rather_than_spawned_as_a_model_and_a_child_past_the_levels_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = runtime_app(dir.path());
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(
            GROUP_WITH_A_MODEL_PAST_ITS_LEVELS.to_owned(),
            PathBuf::new(),
        ));
    app.world_mut().spawn(JackdawSceneRoot(scene));
    app.update();
    app.update();

    assert_eq!(placed_models(app.world_mut()), ["nest.gltf"]);
}

#[test]
fn the_levels_of_a_group_that_goes_are_given_their_models() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = runtime_app(dir.path());
    let root = app.world_mut().spawn(heights(&[0.5, 0.1], 0.0)).id();
    for path in ["tree.gltf", "tree_LOD1.gltf"] {
        app.world_mut().spawn((
            GltfSource {
                path: path.to_owned(),
                scene_index: 0,
            },
            ChildOf(root),
        ));
    }
    app.update();
    assert!(placed_models(app.world_mut()).is_empty());

    app.world_mut().entity_mut(root).remove::<LodGroup>();
    app.update();

    assert_eq!(
        placed_models(app.world_mut()),
        ["tree.gltf", "tree_LOD1.gltf"]
    );
}

/// The glTF files given to the world-asset spawner, in path order.
fn placed_models(world: &mut World) -> Vec<String> {
    let mut placed = world.query::<(&GltfSource, &WorldAssetRoot)>();
    let mut paths: Vec<String> = placed
        .iter(world)
        .map(|(source, _)| source.path.clone())
        .collect();
    paths.sort();
    paths
}

fn runtime_app(assets_root: &std::path::Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.insert_resource(JackdawCatalogPath(assets_root.join("catalog.bsn")));
    app.add_plugins(JackdawPlugin);
    app
}

/// A model of `count` parts, two units across.
fn flat_model(count: usize, needs_instance: bool) -> FlatModel {
    FlatModel {
        parts: (0..count)
            .map(|index| ModelPart {
                mesh: Handle::default(),
                material: Handle::default(),
                material_name: Some(format!("Part{index}")),
                local: Transform::from_xyz(0.0, index as f32, 0.0),
            })
            .collect(),
        bounds: Aabb::from_min_max(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 2.0, 1.0)),
        needs_instance,
    }
}

const TREE_LEVELS: [&str; 3] = ["tree.gltf", "tree_LOD1.gltf", "tree_LOD2.gltf"];

/// An app keeping live levels of models it already holds, placing everything
/// queued each frame unless `budget` says otherwise.
fn live_app(budget: Duration) -> App {
    let mut app = app();
    app.insert_resource(LiveLevelSettings {
        budget,
        opening_budget: budget,
        stand_ins: true,
    });
    let mut models = ModelParts::default();
    for path in TREE_LEVELS {
        models.insert(path, flat_model(1, false));
    }
    app.insert_resource(models);
    app
}

/// A group of the three tree levels standing at `at`, sized two units across,
/// with the nodes of its levels in order.
fn live_group(app: &mut App, group: LodGroup, at: Vec3) -> (Entity, Vec<Entity>) {
    let root = app
        .world_mut()
        .spawn((group, Transform::from_translation(at)))
        .id();
    let levels = TREE_LEVELS
        .iter()
        .map(|path| {
            app.world_mut()
                .spawn((
                    GltfSource {
                        path: path.to_string(),
                        scene_index: 0,
                    },
                    Transform::default(),
                    ChildOf(root),
                ))
                .id()
        })
        .collect();
    (root, levels)
}

fn camera_at(app: &mut App, distance: f32) -> Entity {
    let camera = camera(app, 60.0);
    app.world_mut()
        .entity_mut(camera)
        .insert(Transform::from_xyz(0.0, 0.0, distance));
    camera
}

fn move_camera(app: &mut App, camera: Entity, distance: f32) {
    app.world_mut()
        .entity_mut(camera)
        .insert(Transform::from_xyz(0.0, 0.0, distance));
}

/// How many parts each level has in the world.
fn live_parts(app: &mut App, levels: &[Entity]) -> Vec<usize> {
    let mut parts = app.world_mut().query_filtered::<&ChildOf, With<LodPart>>();
    let parents: Vec<Entity> = parts.iter(app.world()).map(ChildOf::parent).collect();
    levels
        .iter()
        .map(|level| parents.iter().filter(|parent| *parent == level).count())
        .collect()
}

/// Whether some live part's range takes in `distance`.
fn something_draws_at(app: &mut App, distance: f32) -> bool {
    let mut parts = app
        .world_mut()
        .query_filtered::<&VisibilityRange, With<LodPart>>();
    parts
        .iter(app.world())
        .any(|range| range.start_margin.start <= distance && distance < range.end_margin.end)
}

fn settle(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

const PLENTY: Duration = Duration::from_secs(1);

#[test]
fn only_the_level_whose_range_holds_the_camera_has_parts() {
    let mut app = live_app(PLENTY);
    camera_at(&mut app, 5.0);
    let (_, levels) = live_group(&mut app, group(2.0), Vec3::ZERO);
    settle(&mut app, 8);

    assert_eq!(live_parts(&mut app, &levels), [0, 1, 0]);
    assert!(app.world().resource::<LiveLevelProgress>().is_refined());
}

#[test]
fn a_group_that_draws_nothing_gets_its_coarsest_level_first() {
    let mut app = live_app(PLENTY);
    camera_at(&mut app, 2.0);
    let (_, levels) = live_group(&mut app, group(2.0), Vec3::ZERO);
    let mut first = None;
    for _ in 0..8 {
        app.update();
        let parts = live_parts(&mut app, &levels);
        if first.is_none() && parts.iter().any(|count| *count > 0) {
            first = Some(parts);
        }
    }

    assert_eq!(
        first,
        Some(vec![0, 0, 1]),
        "the coarsest level stands in first"
    );
    assert_eq!(live_parts(&mut app, &levels), [1, 0, 0]);
}

#[test]
fn without_stand_ins_the_wanted_level_comes_first() {
    let mut app = live_app(PLENTY);
    app.world_mut()
        .resource_mut::<LiveLevelSettings>()
        .stand_ins = false;
    camera_at(&mut app, 2.0);
    let (_, levels) = live_group(&mut app, group(2.0), Vec3::ZERO);
    for _ in 0..8 {
        app.update();
        let parts = live_parts(&mut app, &levels);
        assert_eq!(parts[2], 0, "the coarsest level never stands in");
    }
    assert_eq!(live_parts(&mut app, &levels), [1, 0, 0]);
}

#[test]
fn crossing_a_switch_places_the_next_level_before_the_last_one_goes() {
    let mut app = live_app(PLENTY);
    let camera = camera_at(&mut app, 5.0);
    let (_, levels) = live_group(&mut app, group(2.0), Vec3::ZERO);
    settle(&mut app, 8);
    assert_eq!(live_parts(&mut app, &levels), [0, 1, 0]);

    let far = 10.0;
    move_camera(&mut app, camera, far);
    for frame in 0..8 {
        app.update();
        assert!(
            something_draws_at(&mut app, far),
            "nothing draws on frame {frame}"
        );
    }
    assert_eq!(live_parts(&mut app, &levels), [0, 0, 1]);
}

#[test]
fn wobbling_the_camera_around_a_switch_places_nothing_new() {
    let mut app = live_app(PLENTY);
    let switch = distance(2.0, 0.25, 60.0);
    let camera = camera_at(&mut app, switch);
    live_group(&mut app, group(2.0), Vec3::ZERO);
    settle(&mut app, 8);
    let placed: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<LodPart>>()
        .iter(app.world())
        .collect();

    for step in 0..12 {
        let wobble = if step % 2 == 0 { 1.01 } else { 0.99 };
        move_camera(&mut app, camera, switch * wobble);
        app.update();
    }

    let now: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<LodPart>>()
        .iter(app.world())
        .collect();
    assert_eq!(now, placed);
}

#[test]
fn a_fading_switch_keeps_both_levels_inside_its_margin() {
    let mut app = live_app(PLENTY);
    camera_at(&mut app, distance(2.0, 0.25, 60.0));
    let (_, levels) = live_group(
        &mut app,
        LodGroup {
            fade: 0.2,
            ..group(2.0)
        },
        Vec3::ZERO,
    );
    settle(&mut app, 8);

    assert_eq!(live_parts(&mut app, &levels), [0, 1, 1]);
}

#[test]
fn a_level_whose_model_is_still_loading_leaves_the_level_standing_in_drawn() {
    let mut app = live_app(PLENTY);
    let mut models = ModelParts::default();
    for path in &TREE_LEVELS[1..] {
        models.insert(path, flat_model(1, false));
    }
    app.insert_resource(models);
    camera_at(&mut app, 2.0);
    let (_, levels) = live_group(&mut app, group(2.0), Vec3::ZERO);
    settle(&mut app, 8);

    assert_eq!(live_parts(&mut app, &levels), [0, 0, 1]);
    assert!(something_draws_at(&mut app, 2.0));
    assert!(!app.world().resource::<LiveLevelProgress>().is_refined());
}

#[test]
fn many_groups_switching_at_once_come_in_over_several_frames() {
    let mut app = live_app(Duration::from_micros(1));
    camera_at(&mut app, 2.0);
    let groups: Vec<Vec<Entity>> = (0..40)
        .map(|index| {
            live_group(
                &mut app,
                group(2.0),
                Vec3::new(index as f32 * 0.01, 0.0, 0.0),
            )
            .1
        })
        .collect();
    settle(&mut app, 3);
    let early: usize = groups
        .iter()
        .map(|levels| live_parts(&mut app, levels).iter().sum::<usize>())
        .sum();
    assert!(early < 40, "{early} levels came in on the first frames");

    settle(&mut app, 200);
    for levels in &groups {
        assert_eq!(live_parts(&mut app, levels), [1, 0, 0]);
    }
}

#[test]
fn a_despawned_group_leaves_no_parts_behind() {
    let mut app = live_app(PLENTY);
    camera_at(&mut app, 5.0);
    let (root, _) = live_group(&mut app, group(2.0), Vec3::ZERO);
    settle(&mut app, 8);

    app.world_mut().entity_mut(root).despawn();
    app.update();

    let left = app
        .world_mut()
        .query_filtered::<Entity, With<LodPart>>()
        .iter(app.world())
        .count();
    assert_eq!(left, 0);
}

#[test]
fn a_level_whose_model_moves_falls_back_to_a_spawned_instance() {
    let mut app = live_app(PLENTY);
    app.add_plugins((
        AssetPlugin::default(),
        bevy::world_serialization::WorldSerializationPlugin,
    ));
    app.world_mut()
        .resource_mut::<ModelParts>()
        .insert("tree_LOD1.gltf", flat_model(1, true));
    camera_at(&mut app, 5.0);
    let (_, levels) = live_group(&mut app, group(2.0), Vec3::ZERO);
    settle(&mut app, 8);

    assert_eq!(live_parts(&mut app, &levels), [0, 1, 0]);
    let instance = app
        .world_mut()
        .query_filtered::<&ChildOf, (With<LodPart>, With<WorldAssetRoot>)>()
        .iter(app.world())
        .map(ChildOf::parent)
        .collect::<Vec<_>>();
    assert_eq!(
        instance,
        [levels[1]],
        "the level's one part is a spawned instance"
    );
}

#[test]
fn a_second_camera_keeps_the_levels_it_needs_live() {
    let mut app = live_app(PLENTY);
    camera_at(&mut app, 5.0);
    let far = camera(&mut app, 60.0);
    app.world_mut().entity_mut(far).insert((
        Transform::from_xyz(0.0, 0.0, 10.0),
        Camera {
            order: -1,
            ..default()
        },
    ));
    let (_, levels) = live_group(&mut app, group(2.0), Vec3::ZERO);
    settle(&mut app, 8);

    assert_eq!(live_parts(&mut app, &levels), [0, 1, 1]);
}

#[test]
fn a_level_part_wears_its_override_from_the_frame_it_appears() {
    let mut app = live_app(PLENTY);
    app.add_plugins(AssetPlugin::default())
        .init_asset::<StandardMaterial>()
        .add_plugins(jackdaw_runtime::MaterialOverridesPlugin);
    let impostor = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let mut references = bevy::platform::collections::HashMap::default();
    references.insert(
        "materials/impostor.bsn".to_string(),
        impostor.clone().untyped(),
    );
    app.insert_resource(jackdaw_bsn::BsnProjectAssets(references));
    camera_at(&mut app, 10.0);
    let (root, _) = live_group(&mut app, group(2.0), Vec3::ZERO);
    app.world_mut()
        .entity_mut(root)
        .insert(jackdaw_scene_types::MaterialOverrides {
            materials: [("Part0".to_string(), "materials/impostor.bsn".to_string())].into(),
        });

    for _ in 0..8 {
        app.update();
        let mut parts = app
            .world_mut()
            .query_filtered::<&MeshMaterial3d<StandardMaterial>, With<LodPart>>();
        for material in parts.iter(app.world()) {
            assert_eq!(material.0, impostor, "a part showed in its own material");
        }
    }
}

#[test]
fn a_group_that_names_its_model_itself_keeps_the_same_levels_live() {
    let mut app = live_app(PLENTY);
    camera_at(&mut app, 5.0);
    let root = app
        .world_mut()
        .spawn((
            group(2.0),
            GltfSource {
                path: "tree.gltf".into(),
                scene_index: 0,
            },
            Transform::default(),
        ))
        .id();
    settle(&mut app, 8);

    let live = app
        .world()
        .get::<jackdaw_runtime::LiveLevels>(root)
        .expect("the group keeps its levels live");
    let placed: Vec<usize> = (0..3).map(|level| live.parts(level).len()).collect();
    assert_eq!(placed, [0, 1, 0]);
    assert_eq!(live.model_path(1), Some("tree_LOD1.gltf"));
    assert!(app.world().get::<WorldAssetRoot>(root).is_none());
}

#[test]
fn forcing_a_level_shows_it_at_every_distance() {
    let mut app = live_app(PLENTY);
    app.insert_resource(jackdaw_runtime::ForcedLod(Some(0)));
    camera_at(&mut app, 500.0);
    let (_, levels) = live_group(&mut app, group(2.0), Vec3::ZERO);
    settle(&mut app, 8);

    assert_eq!(live_parts(&mut app, &levels), [1, 0, 0]);
    assert!(something_draws_at(&mut app, 500.0));
    assert!(something_draws_at(&mut app, 0.5));

    app.insert_resource(jackdaw_runtime::ForcedLod(None));
    settle(&mut app, 8);
    assert_eq!(
        live_parts(&mut app, &levels),
        [0, 0, 0],
        "past the last level nothing draws"
    );
}
