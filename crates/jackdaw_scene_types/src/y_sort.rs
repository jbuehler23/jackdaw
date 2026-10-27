use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

/// Draws the entity by its world y among the other y-sorted entities of its z
/// layer: the lower its transform origin, the further in front it draws.
///
/// The sort point is the transform origin, so a character or a tree puts its
/// anchor at its feet (`sprite.set_anchor preset=bottom_center`) and sorts by
/// where it stands.
///
/// Only the drawn z moves: after transform propagation the entity's
/// `GlobalTransform` z becomes its unsorted z less [`y_sort_offset`] of its
/// world y. The authored `Transform` z is untouched and stays the layer: the
/// offset stays within [`Y_SORT_SPAN`] of it, so a y-sorted entity never
/// crosses into the next whole z step, and an explicit z layer always wins
/// over y.
///
/// Entities under a y-sorted entity move with it: they take its offset rather
/// than sorting on their own y, so the parts of a character draw as one. A
/// descendant that carries `YSort` itself sorts by its own y instead.
///
/// World y sorts within [`Y_SORT_RANGE`] units of the origin either way;
/// beyond that every position shares the edge offset. Inside the range two
/// origins about 0.05 units apart still sort apart on z layers below 16, and
/// about 0.3 units apart on z layers below 128.
#[derive(Component, Reflect, Default, Clone, Copy, Debug, PartialEq, Eq)]
#[reflect(Component, Default, @crate::EditorCategory::new("2D"))]
pub struct YSort;

/// Reflect type path for [`YSort`], for the paths that name a component by
/// its path rather than by its type.
pub const Y_SORT_TYPE_PATH: &str = "jackdaw_scene_types::y_sort::YSort";

/// World y, either side of the origin, over which [`YSort`] tells positions
/// apart.
pub const Y_SORT_RANGE: f32 = 16384.0;

/// The most a y-sorted entity's drawn z moves from its unsorted z.
pub const Y_SORT_SPAN: f32 = 0.45;

/// How far [`YSort`] pulls an entity back in z for its world `y`: linear in
/// `y` across [`Y_SORT_RANGE`], at most [`Y_SORT_SPAN`] either way.
pub fn y_sort_offset(y: f32) -> f32 {
    if y.is_nan() {
        return 0.0;
    }
    y.clamp(-Y_SORT_RANGE, Y_SORT_RANGE) * (Y_SORT_SPAN / Y_SORT_RANGE)
}

/// Where `entity`'s transforms place it in the world, with no [`YSort`]
/// offset: the product of the `Transform`s from the hierarchy's root down to
/// it. For turning a world position into a local `Transform` without the
/// drawn order leaking into it. Exact whether or not the sort has run since
/// the hierarchy last changed. `None` when `entity` has no `Transform`.
pub fn unsorted_global(world: &World, entity: Entity) -> Option<GlobalTransform> {
    world.get::<Transform>(entity)?;
    let chain: Vec<Entity> = std::iter::successors(Some(entity), |&at| {
        world.get::<ChildOf>(at).map(ChildOf::parent)
    })
    .collect();
    let global = chain
        .iter()
        .rev()
        .filter_map(|&at| world.get::<Transform>(at))
        .fold(GlobalTransform::IDENTITY, |global, transform| {
            global.mul_transform(*transform)
        });
    Some(global)
}

/// Registers [`YSort`] and keeps the drawn order of 2D meshes current: the
/// y-sort runs after transform propagation, and with the `render` feature a
/// 2D mesh whose drawn z changed is queued afresh.
pub struct DrawOrderPlugin;

impl Plugin for DrawOrderPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<YSort>()
            .add_observer(unsort_on_remove)
            .add_systems(PostUpdate, y_sort.after(TransformSystems::Propagate));
        #[cfg(feature = "render")]
        app.add_systems(
            PostUpdate,
            requeue_2d_meshes_moved_in_z
                .after(y_sort)
                .before(bevy::asset::AssetEventSystems),
        );
    }
}

/// A subtree that stops sorting gets its unsorted placement back from the
/// next transform propagation.
fn unsort_on_remove(removed: On<Remove<YSort>>, mut transforms: Query<&mut Transform>) -> Result {
    if let Ok(mut transform) = transforms.get_mut(removed.entity) {
        transform.set_changed();
    }
    Ok(())
}

/// The z a 2D mesh was last seen drawn at.
#[cfg(feature = "render")]
#[derive(Component)]
struct DrawnZ(f32);

/// The renderer orders a 2D mesh by the z it had when it was queued, and
/// queues it again only when its mesh or material changes, so a mesh that
/// only moved in z would keep its old place in the order. Flagging the mesh
/// when its drawn z changes queues it again at that z in the same frame, as
/// the renderer reads the flag after asset events.
#[cfg(feature = "render")]
fn requeue_2d_meshes_moved_in_z(
    mut moved: Query<
        (
            Entity,
            &GlobalTransform,
            &mut bevy::mesh::Mesh2d,
            Option<&mut DrawnZ>,
        ),
        Changed<GlobalTransform>,
    >,
    mut commands: Commands,
) {
    for (entity, global, mut mesh, drawn) in &mut moved {
        let z = global.translation().z;
        match drawn {
            Some(mut drawn) => {
                if drawn.0 != z {
                    drawn.0 = z;
                    mesh.set_changed();
                }
            }
            None => {
                commands.entity(entity).try_insert(DrawnZ(z));
            }
        }
    }
}

/// Write the drawn z of every y-sorted subtree that moved, was reparented or
/// was just tagged. Propagation overwrites a moved entity's z, which is what
/// marks its subtree for sorting again; the z this pass writes does not. Each
/// z is worked out afresh from the transforms, never from the z this wrote
/// before, so it holds still while nothing moves.
fn y_sort(
    mut access: ParamSet<(
        Query<Entity, Or<(Changed<GlobalTransform>, Changed<ChildOf>, Added<YSort>)>>,
        Query<&mut GlobalTransform>,
    )>,
    marked: Query<(), With<YSort>>,
    parents: Query<&ChildOf>,
    nodes: Query<(&Transform, Option<&Children>)>,
    mut roots: Local<EntityHashSet>,
    mut stack: Local<Vec<(Entity, GlobalTransform, f32)>>,
) {
    roots.clear();
    for entity in &access.p0() {
        let topmost = std::iter::once(entity)
            .chain(parents.iter_ancestors(entity))
            .filter(|&at| marked.contains(at))
            .last();
        if let Some(root) = topmost {
            roots.insert(root);
        }
    }
    let mut globals = access.p1();
    for &root in roots.iter() {
        let Ok((transform, _)) = nodes.get(root) else {
            continue;
        };
        let parent = parents
            .get(root)
            .ok()
            .and_then(|child_of| globals.get(child_of.parent()).ok())
            .copied()
            .unwrap_or_default();
        stack.push((root, parent.mul_transform(*transform), 0.0));
        while let Some((at, unsorted, inherited)) = stack.pop() {
            let offset = if marked.contains(at) {
                y_sort_offset(unsorted.translation().y)
            } else {
                inherited
            };
            if let Ok(mut global) = globals.get_mut(at) {
                let z = unsorted.translation().z - offset;
                if global.translation().z != z {
                    let mut affine = global.affine();
                    affine.translation.z = z;
                    *global = affine.into();
                }
            }
            let Ok((_, Some(children))) = nodes.get(at) else {
                continue;
            };
            for child in children {
                if let Ok((transform, _)) = nodes.get(*child) {
                    stack.push((*child, unsorted.mul_transform(*transform), offset));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::TypePath;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            DrawOrderPlugin,
        ));
        app
    }

    fn drawn_z(app: &App, entity: Entity) -> f32 {
        app.world()
            .get::<GlobalTransform>(entity)
            .expect("the entity has a global transform")
            .translation()
            .z
    }

    #[test]
    fn the_lower_of_two_y_sorted_entities_draws_in_front_and_holds_still() {
        let mut app = app();
        let root = app.world_mut().spawn(Transform::default()).id();
        let high = app
            .world_mut()
            .spawn((YSort, Transform::from_xyz(0.0, 10.0, 2.0), ChildOf(root)))
            .id();
        let low = app
            .world_mut()
            .spawn((YSort, Transform::from_xyz(5.0, -10.0, 2.0), ChildOf(root)))
            .id();
        app.update();
        let (first_high, first_low) = (drawn_z(&app, high), drawn_z(&app, low));
        assert!(first_low > first_high, "{first_low} vs {first_high}");
        assert_eq!(first_high, 2.0 - y_sort_offset(10.0));
        assert_eq!(first_low, 2.0 - y_sort_offset(-10.0));

        for _ in 0..5 {
            app.update();
        }
        assert_eq!(drawn_z(&app, high), first_high, "no drift over frames");
        assert_eq!(drawn_z(&app, low), first_low, "no drift over frames");
        assert_eq!(
            app.world().get::<Transform>(high).map(|t| t.translation.z),
            Some(2.0),
            "the authored z is untouched"
        );

        app.world_mut()
            .get_mut::<Transform>(high)
            .expect("transform")
            .translation
            .y = -20.0;
        app.update();
        assert!(
            drawn_z(&app, high) > drawn_z(&app, low),
            "moving below flips the order"
        );
    }

    #[test]
    fn the_offset_never_reaches_the_next_z_step() {
        let edge = Y_SORT_RANGE;
        for y in [
            -f32::INFINITY,
            -edge * 4.0,
            -edge,
            -1.0,
            0.0,
            1.0,
            edge,
            edge * 4.0,
            f32::INFINITY,
            f32::NAN,
        ] {
            assert!(y_sort_offset(y).abs() < 0.5, "{y}");
        }
        for layer in [-128.0_f32, 0.0, 1.0, 127.0] {
            assert!(layer - y_sort_offset(-edge) < layer + 1.0 - y_sort_offset(edge));
            assert!(layer - y_sort_offset(-edge) < layer + 0.5);
            assert!(layer - y_sort_offset(edge) > layer - 0.5);
        }
    }

    #[test]
    fn positions_inside_the_range_still_sort_apart() {
        for (layer, apart) in [(15.0_f32, 0.05_f32), (127.0, 0.3)] {
            for y in [-Y_SORT_RANGE + 1.0, -1.0, 0.0, 1.0, Y_SORT_RANGE - 1.0] {
                assert!(
                    layer - y_sort_offset(y) > layer - y_sort_offset(y + apart),
                    "layer {layer}, y {y}"
                );
            }
        }
    }

    #[test]
    fn a_child_moves_with_its_y_sorted_parent() {
        let mut app = app();
        let parent = app
            .world_mut()
            .spawn((YSort, Transform::from_xyz(0.0, -10.0, 0.0)))
            .id();
        let child = app
            .world_mut()
            .spawn((Transform::from_xyz(0.0, 30.0, 1.0), ChildOf(parent)))
            .id();
        app.update();
        let expected = 1.0 - y_sort_offset(-10.0);
        assert_eq!(drawn_z(&app, child), expected);

        app.world_mut()
            .get_mut::<Transform>(child)
            .expect("transform")
            .translation
            .x = 4.0;
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(drawn_z(&app, child), expected, "no drift after a move");
    }

    #[test]
    fn unsorted_global_is_exact_before_and_after_the_sort() {
        let mut app = app();
        let parent = app
            .world_mut()
            .spawn((YSort, Transform::from_xyz(0.0, 100.0, 3.0)))
            .id();
        let child = app
            .world_mut()
            .spawn((Transform::from_xyz(2.0, 0.0, 1.0), ChildOf(parent)))
            .id();
        for _ in 0..2 {
            for (entity, at) in [
                (parent, Vec3::new(0.0, 100.0, 3.0)),
                (child, Vec3::new(2.0, 100.0, 4.0)),
            ] {
                let unsorted = unsorted_global(app.world(), entity).expect("a transform");
                assert_eq!(unsorted.translation(), at, "{entity}");
            }
            app.update();
        }
    }

    #[test]
    fn taking_the_tag_off_restores_the_unsorted_z() {
        let mut app = app();
        let parent = app
            .world_mut()
            .spawn((YSort, Transform::from_xyz(0.0, 100.0, 3.0)))
            .id();
        let child = app
            .world_mut()
            .spawn((Transform::from_xyz(0.0, 0.0, 1.0), ChildOf(parent)))
            .id();
        app.update();
        assert_ne!(drawn_z(&app, parent), 3.0);
        app.world_mut().entity_mut(parent).remove::<YSort>();
        app.update();
        assert_eq!(drawn_z(&app, parent), 3.0);
        assert_eq!(drawn_z(&app, child), 4.0);
    }

    #[derive(Resource, Default)]
    struct Seen(Vec<Entity>);

    fn watch_globals(mut seen: ResMut<Seen>, moved: Query<Entity, Changed<GlobalTransform>>) {
        seen.0 = moved.iter().collect();
    }

    #[test]
    fn a_still_scene_is_left_alone_and_a_move_sorts_in_the_same_frame() {
        let mut app = app();
        app.init_resource::<Seen>().add_systems(Last, watch_globals);
        let sorted = app
            .world_mut()
            .spawn((YSort, Transform::from_xyz(0.0, 40.0, 0.0)))
            .id();
        for _ in 0..3 {
            app.update();
        }
        assert!(
            app.world().resource::<Seen>().0.is_empty(),
            "no writes while still"
        );

        app.world_mut()
            .get_mut::<Transform>(sorted)
            .expect("transform")
            .translation
            .x = 7.0;
        app.update();
        assert_eq!(drawn_z(&app, sorted), -y_sort_offset(40.0));
    }

    #[cfg(feature = "render")]
    #[test]
    fn a_2d_mesh_is_queued_again_only_when_its_drawn_z_changes() {
        #[derive(Resource, Default)]
        struct Requeued(Vec<Entity>);

        fn watch(
            mut requeued: ResMut<Requeued>,
            meshes: Query<Entity, Changed<bevy::mesh::Mesh2d>>,
        ) {
            requeued.0 = meshes.iter().collect();
        }

        let mut app = app();
        app.init_resource::<Requeued>().add_systems(Last, watch);
        let plain = app
            .world_mut()
            .spawn((bevy::mesh::Mesh2d::default(), Transform::default()))
            .id();
        let sorted = app
            .world_mut()
            .spawn((YSort, bevy::mesh::Mesh2d::default(), Transform::default()))
            .id();
        for _ in 0..3 {
            app.update();
        }
        let step = |app: &mut App, entity: Entity, by: Vec3| {
            app.world_mut()
                .get_mut::<Transform>(entity)
                .expect("transform")
                .translation += by;
            app.update();
            app.world().resource::<Requeued>().0.clone()
        };
        assert!(
            step(&mut app, plain, Vec3::X).is_empty(),
            "x alone keeps the order"
        );
        assert!(
            step(&mut app, plain, Vec3::Y).is_empty(),
            "y alone keeps the order"
        );
        assert_eq!(step(&mut app, plain, Vec3::Z), vec![plain]);
        assert_eq!(
            step(&mut app, sorted, Vec3::Y),
            vec![sorted],
            "a y-sort flip"
        );
    }

    #[test]
    fn y_sort_type_path_const_matches_the_reflected_path() {
        assert_eq!(YSort::type_path(), Y_SORT_TYPE_PATH);
    }
}
