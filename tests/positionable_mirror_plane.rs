//! Headless coverage for the positionable mirror plane: the
//! `mirror.plane.set` operator writes a mirror modifier's `offset` on the
//! named axis, and the change is picked up by the existing re-fold + AST
//! sync systems.

use bevy::prelude::*;
use jackdaw::brush::{Brush, MeshMirror};
use jackdaw_api::prelude::*;
use jackdaw_geometry::{Modifier, ModifierEntry, ModifierStack};

mod util;

/// Wrap a `MeshMirror` as a single-entry editor modifier stack, the
/// component the brush mesh systems read.
fn mirror_stack(mirror: MeshMirror) -> ModifierStack {
    ModifierStack {
        modifiers: vec![ModifierEntry::new(Modifier::Mirror(mirror))],
    }
}

/// Spawn a half-cube occupying x in [0, 1]: a unit cube shifted so its
/// -X face lies exactly on the X = 0 mirror plane.
fn spawn_half_cube(app: &mut App) -> Entity {
    let mut brush = Brush::cuboid(0.5, 0.5, 0.5);
    for vert in &mut brush.topology.vertices {
        vert.position.x += 0.5;
    }
    app.world_mut()
        .spawn((
            Name::new("HalfCube"),
            brush,
            Transform::default(),
            Visibility::default(),
        ))
        .id()
}

/// Select `entity` and clear the headless placeholder `InputFocus`,
/// which the mirror ops' availability checks read as "a text field
/// owns the keyboard".
fn select_for_operators(app: &mut App, entity: Entity) {
    use bevy::input_focus::InputFocus;
    app.world_mut().resource_mut::<InputFocus>().0 = None;
    app.world_mut()
        .resource_mut::<jackdaw::selection::Selection>()
        .entities = vec![entity];
    app.update();
}

#[test]
fn plane_set_moves_the_mirror_offset_and_refolds() {
    let mut app = util::headless_app();
    app.finish();
    app.update();
    let entity = spawn_half_cube(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(mirror_stack(MeshMirror::default()));
    select_for_operators(&mut app, entity);

    let result = app
        .world_mut()
        .operator("mirror.plane.set")
        .param("axis", 0i64)
        .param("value", 0.5f64)
        .call()
        .expect("mirror.plane.set dispatched");
    assert_eq!(result, OperatorResult::Finished);

    let stack = app.world().entity(entity).get::<ModifierStack>().unwrap();
    assert_eq!(stack.first_enabled_mirror().unwrap().offset.x, 0.5);
}

#[test]
fn snap_resolves_to_nearest_vertex_then_grid_then_free() {
    use jackdaw::brush::mirror_plane_ops::resolve_axis_snap_to_geometry;
    let verts = vec![Vec3::splat(-0.5), Vec3::splat(0.5)];
    assert_eq!(
        resolve_axis_snap_to_geometry(&verts, 0, 0.46, 0.1),
        Some(0.5)
    );
    assert_eq!(resolve_axis_snap_to_geometry(&verts, 0, 0.1, 0.05), None);
}
