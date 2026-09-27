//! A LOD group's levels show over the distances their screen heights stand
//! for at the camera's field of view.

#![cfg(feature = "render")]

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use jackdaw_runtime::LodPlugin;
use jackdaw_scene_types::{LodGroup, LodLevel};

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
    LodGroup {
        levels: [0.5, 0.25, 0.1]
            .into_iter()
            .map(|screen_height| LodLevel { screen_height })
            .collect(),
        size,
        fade: 0.0,
    }
}

/// A group with one mesh under each of its three levels, returned in level order.
fn spawn_group(app: &mut App, group: LodGroup) -> (Entity, Vec<Entity>) {
    let root = app.world_mut().spawn((group, Transform::default())).id();
    let meshes = (0..3)
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
