//! Shading view modes: each draws the scene's meshes with a material made from
//! their own, and Lit puts their own back.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::view_modes::ShadedBy;

fn run(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .unwrap_or_else(|err| panic!("{clause}: {err}"))
        .assert_finished();
    for _ in 0..3 {
        app.update();
    }
}

fn drawn_with(app: &App, mesh: Entity) -> (Handle<StandardMaterial>, StandardMaterial) {
    let handle = app
        .world()
        .get::<MeshMaterial3d<StandardMaterial>>(mesh)
        .expect("the mesh keeps a material")
        .0
        .clone();
    let material = app
        .world()
        .resource::<Assets<StandardMaterial>>()
        .get(&handle)
        .expect("the material exists")
        .clone();
    (handle, material)
}

#[test]
fn each_shading_mode_draws_a_derived_material_and_lit_restores_the_own() {
    let mut app = util::editor_test_app();
    let red = Color::srgb(0.9, 0.1, 0.1);
    let own = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: red,
            ..default()
        });
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::default());
    let cube = app
        .world_mut()
        .spawn((Mesh3d(mesh), MeshMaterial3d(own.clone())))
        .id();
    app.update();

    run(&mut app, "view.mode mode=unlit");
    let (handle, material) = drawn_with(&app, cube);
    assert_ne!(handle, own);
    assert!(material.unlit && material.base_color == red);

    run(&mut app, "view.mode mode=lighting_only");
    let (_, material) = drawn_with(&app, cube);
    assert!(!material.unlit && material.base_color == Color::WHITE);

    run(&mut app, "view.mode mode=wireframe");
    let (_, material) = drawn_with(&app, cube);
    assert!(material.unlit && material.base_color != red);
    assert!(
        app.world()
            .resource::<bevy::pbr::wireframe::WireframeConfig>()
            .global
    );

    run(&mut app, "view.mode mode=lit");
    assert_eq!(drawn_with(&app, cube).0, own);
    assert!(app.world().get::<ShadedBy>(cube).is_none());
    assert!(
        !app.world()
            .resource::<bevy::pbr::wireframe::WireframeConfig>()
            .global
    );
}

#[test]
fn lit_with_wireframe_keeps_the_own_material_and_draws_edges() {
    let mut app = util::editor_test_app();
    let own = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::default());
    let cube = app
        .world_mut()
        .spawn((Mesh3d(mesh), MeshMaterial3d(own.clone())))
        .id();
    run(&mut app, "view.mode mode=lit_wireframe");
    assert_eq!(drawn_with(&app, cube).0, own);
    assert!(
        app.world()
            .resource::<bevy::pbr::wireframe::WireframeConfig>()
            .global
    );
}

#[test]
fn a_mesh_wearing_an_extended_material_is_shaded_from_its_base_and_given_it_back() {
    use bevy::pbr::ExtendedMaterial;
    use jackdaw_surface::{Foliage, FoliageMaterial};

    let mut app = util::editor_test_app();
    let leaves = Color::srgb(0.2, 0.7, 0.2);
    let own = app
        .world_mut()
        .resource_mut::<Assets<FoliageMaterial>>()
        .add(ExtendedMaterial {
            base: StandardMaterial {
                base_color: leaves,
                ..default()
            },
            extension: Foliage::default(),
        });
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::default());
    let bush = app
        .world_mut()
        .spawn((Mesh3d(mesh), MeshMaterial3d(own.clone())))
        .id();
    app.update();

    run(&mut app, "view.mode mode=unlit");
    assert!(
        app.world()
            .get::<MeshMaterial3d<FoliageMaterial>>(bush)
            .is_none()
    );
    let (_, material) = drawn_with(&app, bush);
    assert!(material.unlit && material.base_color == leaves);

    run(&mut app, "view.mode mode=lighting_only");
    let (_, material) = drawn_with(&app, bush);
    assert_eq!(material.base_color, Color::WHITE, "shaded once, not twice");

    run(&mut app, "view.mode mode=lit");
    assert!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(bush)
            .is_none()
    );
    assert_eq!(
        app.world()
            .get::<MeshMaterial3d<FoliageMaterial>>(bush)
            .map(|material| material.0.clone()),
        Some(own)
    );
}
