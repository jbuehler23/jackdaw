//! Packing groups into prefab files from parameters.
//!
//! `prefab.pack` writes the file and leaves an instance of it standing
//! where the group stood; `prefab.pack_matching` does that once and then
//! replaces the group's copies elsewhere in the scene with instances of the
//! same file. Both are one undo entry, and neither takes the file back.

use crate::util;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::entity_ops::GltfSource;
use jackdaw::prefab::{IsA, PrefabEntityId};
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::PropertyValue;

/// An editor app whose project root is a fresh directory, so an
/// assets-relative path has somewhere to land.
fn app_in_project() -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    (app, dir)
}

/// A top-level group at `at`, with one glTF-backed child per entry in
/// `children`, registered in the live document.
fn spawn_group(app: &mut App, name: &str, at: Vec3, children: &[(&str, Vec3)]) -> Entity {
    let root = app
        .world_mut()
        .spawn((Name::new(name.to_string()), Transform::from_translation(at)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), root);
    for (source, offset) in children {
        let child = app
            .world_mut()
            .spawn((
                Name::new(format!("{name}.{source}")),
                Transform::from_translation(*offset),
                GltfSource {
                    path: (*source).to_string(),
                    scene_index: 0,
                },
                ChildOf(root),
            ))
            .id();
        jackdaw::scene_io::register_entity_in_ast(app.world_mut(), child);
    }
    app.update();
    root
}

/// A glTF-backed child of `parent`, for a group that is more than one
/// level deep.
fn spawn_child(app: &mut App, parent: Entity, name: &str, source: &str, at: Vec3) -> Entity {
    let child = app
        .world_mut()
        .spawn((
            Name::new(name.to_string()),
            Transform::from_translation(at),
            GltfSource {
                path: source.to_string(),
                scene_index: 0,
            },
            ChildOf(parent),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), child);
    app.update();
    child
}

/// The stock steading: two pieces, the same way up in every copy.
const PIECES: &[(&str, Vec3)] = &[
    ("walls.gltf", Vec3::new(0.0, 0.0, 0.0)),
    ("roof.gltf", Vec3::new(0.0, 2.0, 0.0)),
];

/// Dispatch `id` the way a menu item does, so it lands on the undo stack,
/// then tick the frame its queued work needs.
#[track_caller]
fn call(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call
        .call()
        .unwrap_or_else(|err| panic!("{id}: dispatch errored: {err}"));
    assert_eq!(result, OperatorResult::Finished, "{id} reported {result:?}");
    app.update();
}

fn instance_roots(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<IsA>>()
        .iter(app.world())
        .collect()
}

fn entity_named(app: &mut App, name: &str) -> Option<Entity> {
    let mut query = app.world_mut().query::<(Entity, &Name)>();
    query
        .iter(app.world())
        .find(|(_, found)| found.as_str() == name)
        .map(|(entity, _)| entity)
}

#[test]
fn pack_writes_the_file_and_leaves_an_instance_where_the_group_was() {
    let (mut app, dir) = app_in_project();
    let group = spawn_group(&mut app, "Steading", Vec3::new(5.0, 0.0, -2.0), PIECES);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];

    call(
        &mut app,
        "prefab.pack",
        &[("path", PropertyValue::from("prefabs/steading.bsn"))],
    );

    let written = dir.path().join("assets/prefabs/steading.bsn");
    assert!(
        written.is_file(),
        "prefab.pack should write {}",
        written.display()
    );

    let instances = instance_roots(&mut app);
    assert_eq!(
        instances.len(),
        1,
        "the packed group should leave exactly one instance behind"
    );
    let source = app
        .world()
        .get::<IsA>(instances[0])
        .map(|isa| isa.source.clone())
        .expect("the instance carries IsA");
    assert_eq!(
        jackdaw::prefab::canonical_prefab_path(&source),
        jackdaw::prefab::canonical_prefab_path(&written),
        "the instance inherits from the file that was written"
    );
    assert_eq!(
        app.world()
            .get::<Transform>(instances[0])
            .map(|at| at.translation),
        Some(Vec3::new(5.0, 0.0, -2.0)),
        "the instance stands where the group stood"
    );

    let packed = entity_named(&mut app, "Steading").expect("the group respawned as prefab content");
    assert!(
        app.world().get::<PrefabEntityId>(packed).is_some(),
        "the group is now inherited from the prefab rather than authored"
    );
}

#[test]
fn undo_of_pack_puts_the_group_back_and_leaves_the_file() {
    let (mut app, dir) = app_in_project();
    let group = spawn_group(&mut app, "Steading", Vec3::new(5.0, 0.0, -2.0), PIECES);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    let depth = app.world().resource::<CommandHistory>().undo_stack.len();

    call(
        &mut app,
        "prefab.pack",
        &[("path", PropertyValue::from("prefabs/steading.bsn"))],
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        depth + 1,
        "packing is one undo entry"
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();

    assert!(
        instance_roots(&mut app).is_empty(),
        "undo removes the instance"
    );
    let restored = entity_named(&mut app, "Steading").expect("undo puts the group back");
    assert!(
        app.world().get::<PrefabEntityId>(restored).is_none(),
        "the restored group is authored again, not inherited"
    );
    assert!(
        dir.path().join("assets/prefabs/steading.bsn").is_file(),
        "undo does not reach the disk"
    );
}

#[test]
fn pack_refuses_to_overwrite_an_existing_prefab_unless_asked() {
    let (mut app, dir) = app_in_project();
    let written = dir.path().join("assets/prefabs/steading.bsn");
    std::fs::create_dir_all(written.parent().expect("parent")).expect("create prefabs dir");
    std::fs::write(&written, "#Kept\n").expect("write the existing prefab");

    let group = spawn_group(&mut app, "Steading", Vec3::ZERO, PIECES);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    call(
        &mut app,
        "prefab.pack",
        &[("path", PropertyValue::from("prefabs/steading.bsn"))],
    );

    assert_eq!(
        std::fs::read_to_string(&written).expect("read back"),
        "#Kept\n",
        "the file already there is left alone"
    );
    assert!(
        instance_roots(&mut app).is_empty(),
        "nothing is replaced when the write is refused"
    );

    call(
        &mut app,
        "prefab.pack",
        &[
            ("path", PropertyValue::from("prefabs/steading.bsn")),
            ("overwrite", PropertyValue::Bool(true)),
        ],
    );
    assert_ne!(
        std::fs::read_to_string(&written).expect("read back"),
        "#Kept\n",
        "overwrite=true replaces it"
    );
}

#[test]
fn pack_matching_replaces_the_groups_that_match_and_leaves_the_rest() {
    let (mut app, _dir) = app_in_project();
    let first = spawn_group(&mut app, "SteadingA", Vec3::new(0.0, 0.0, 0.0), PIECES);
    spawn_group(&mut app, "SteadingB", Vec3::new(10.0, 0.0, 0.0), PIECES);
    spawn_group(&mut app, "SteadingC", Vec3::new(20.0, 0.0, 0.0), PIECES);
    spawn_group(
        &mut app,
        "Barn",
        Vec3::new(30.0, 0.0, 0.0),
        &[("barn.gltf", Vec3::ZERO)],
    );
    app.world_mut().resource_mut::<Selection>().entities = vec![first];

    call(
        &mut app,
        "prefab.pack_matching",
        &[("path", PropertyValue::from("prefabs/steading.bsn"))],
    );

    let instances = instance_roots(&mut app);
    assert_eq!(
        instances.len(),
        3,
        "the three groups with the same child structure become instances"
    );
    let mut standing: Vec<f32> = instances
        .iter()
        .filter_map(|&instance| app.world().get::<Transform>(instance))
        .map(|at| at.translation.x)
        .collect();
    standing.sort_by(f32::total_cmp);
    assert_eq!(
        standing,
        vec![0.0, 10.0, 20.0],
        "each instance keeps the transform of the group it replaced"
    );

    let barn = entity_named(&mut app, "Barn").expect("the odd group is still there");
    assert!(
        app.world().get::<PrefabEntityId>(barn).is_none(),
        "a group with a different child structure is left authored"
    );
}

#[test]
fn pack_matching_by_prefix_reads_names_rather_than_structure() {
    let (mut app, _dir) = app_in_project();
    let first = spawn_group(&mut app, "SteadingA", Vec3::ZERO, PIECES);
    spawn_group(
        &mut app,
        "SteadingB",
        Vec3::new(10.0, 0.0, 0.0),
        &[("barn.gltf", Vec3::ZERO)],
    );
    spawn_group(&mut app, "Waystation", Vec3::new(20.0, 0.0, 0.0), PIECES);
    app.world_mut().resource_mut::<Selection>().entities = vec![first];

    call(
        &mut app,
        "prefab.pack_matching",
        &[
            ("path", PropertyValue::from("prefabs/steading.bsn")),
            ("match", PropertyValue::from("prefix")),
            ("prefix", PropertyValue::from("Steading")),
        ],
    );

    assert_eq!(
        instance_roots(&mut app).len(),
        2,
        "both names carrying the prefix become instances, whatever they hold"
    );
    let waystation = entity_named(&mut app, "Waystation").expect("the other group is still there");
    assert!(
        app.world().get::<PrefabEntityId>(waystation).is_none(),
        "a name outside the prefix is left authored"
    );
}

#[test]
fn spawn_instance_reads_a_path_relative_to_the_assets_directory() {
    let (mut app, dir) = app_in_project();
    let written = dir.path().join("assets/prefabs/rock.bsn");
    std::fs::create_dir_all(written.parent().expect("parent")).expect("create prefabs dir");
    std::fs::write(
        &written,
        "#Rock\nbevy_transform::components::transform::Transform\n",
    )
    .expect("write the prefab");

    call(
        &mut app,
        "prefab.spawn_instance",
        &[
            ("path", PropertyValue::from("prefabs/rock.bsn")),
            ("pos_x", PropertyValue::Float(1.0)),
            ("pos_y", PropertyValue::Float(0.0)),
            ("pos_z", PropertyValue::Float(2.0)),
        ],
    );

    let instances = instance_roots(&mut app);
    assert_eq!(instances.len(), 1, "the relative path resolved to the file");
    let source = app
        .world()
        .get::<IsA>(instances[0])
        .map(|isa| isa.source.clone())
        .expect("the instance carries IsA");
    assert_eq!(
        jackdaw::prefab::canonical_prefab_path(&source),
        jackdaw::prefab::canonical_prefab_path(&written),
        "an assets-relative path names the same file an absolute one does"
    );
}

/// Matching one level deep calls two groups copies when they agree on their
/// direct children and differ below, and `pack_matching` deletes what it matches.
#[test]
fn pack_matching_leaves_a_group_that_differs_below_its_direct_children() {
    let (mut app, _dir) = app_in_project();
    let first = spawn_group(&mut app, "SteadingA", Vec3::ZERO, PIECES);
    let second = spawn_group(&mut app, "SteadingB", Vec3::new(10.0, 0.0, 0.0), PIECES);
    let roof = entity_named(&mut app, "SteadingB.roof.gltf").expect("the second roof");
    spawn_child(&mut app, roof, "Chimney", "chimney.gltf", Vec3::Y);
    assert!(app.world().get_entity(second).is_ok());
    app.world_mut().resource_mut::<Selection>().entities = vec![first];

    call(
        &mut app,
        "prefab.pack_matching",
        &[("path", PropertyValue::from("prefabs/steading.bsn"))],
    );

    assert_eq!(
        instance_roots(&mut app).len(),
        1,
        "only the packed group became an instance"
    );
    assert!(
        entity_named(&mut app, "Chimney").is_some(),
        "the group with a piece the packed one has not is left standing"
    );
}

/// Two groups that agree all the way down are copies, however deep the
/// piece that would tell them apart sits.
#[test]
fn pack_matching_replaces_a_group_that_matches_all_the_way_down() {
    let (mut app, _dir) = app_in_project();
    let first = spawn_group(&mut app, "SteadingA", Vec3::ZERO, PIECES);
    let first_roof = entity_named(&mut app, "SteadingA.roof.gltf").expect("the first roof");
    spawn_child(&mut app, first_roof, "ChimneyA", "chimney.gltf", Vec3::Y);
    spawn_group(&mut app, "SteadingB", Vec3::new(10.0, 0.0, 0.0), PIECES);
    let second_roof = entity_named(&mut app, "SteadingB.roof.gltf").expect("the second roof");
    spawn_child(&mut app, second_roof, "ChimneyB", "chimney.gltf", Vec3::Y);
    app.world_mut().resource_mut::<Selection>().entities = vec![first];

    call(
        &mut app,
        "prefab.pack_matching",
        &[("path", PropertyValue::from("prefabs/steading.bsn"))],
    );

    assert_eq!(
        instance_roots(&mut app).len(),
        2,
        "a difference nobody has is no difference: both groups are copies"
    );
}

/// `path` reaches these operators from a remote caller and `prefab.pack` writes
/// where it points, so a path leaving the assets directory is refused.
#[test]
fn pack_refuses_a_path_that_leaves_the_assets_directory() {
    let (mut app, dir) = app_in_project();
    let group = spawn_group(&mut app, "Steading", Vec3::ZERO, PIECES);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];

    let outside = dir.path().join("escaped.bsn");
    let absolute = outside.to_string_lossy().to_string();
    for path in ["../escaped.bsn".to_string(), absolute] {
        call(
            &mut app,
            "prefab.pack",
            &[("path", PropertyValue::from(path))],
        );
        assert!(
            !outside.exists(),
            "a path outside the assets directory was written to"
        );
        assert!(
            instance_roots(&mut app).is_empty(),
            "a refused path still replaced the group"
        );
    }
}

/// The group comes out of the scene only once the file it inherits from is on
/// disk and reads back, or the group is gone with nothing standing where it was.
#[cfg(unix)]
#[test]
fn pack_matching_keeps_the_group_when_the_file_cannot_be_written() {
    use std::os::unix::fs::PermissionsExt as _;

    let (mut app, dir) = app_in_project();
    let prefabs = dir.path().join("assets/prefabs");
    std::fs::create_dir_all(&prefabs).expect("create prefabs dir");
    std::fs::set_permissions(&prefabs, std::fs::Permissions::from_mode(0o555))
        .expect("make the directory unwritable");

    let first = spawn_group(&mut app, "SteadingA", Vec3::ZERO, PIECES);
    spawn_group(&mut app, "SteadingB", Vec3::new(10.0, 0.0, 0.0), PIECES);
    app.world_mut().resource_mut::<Selection>().entities = vec![first];

    call(
        &mut app,
        "prefab.pack_matching",
        &[("path", PropertyValue::from("prefabs/steading.bsn"))],
    );

    std::fs::set_permissions(&prefabs, std::fs::Permissions::from_mode(0o755))
        .expect("restore the directory");

    assert!(
        instance_roots(&mut app).is_empty(),
        "a write that did not land still replaced the groups with instances"
    );
    for name in ["SteadingA", "SteadingB"] {
        assert!(
            entity_named(&mut app, name).is_some(),
            "{name} is gone from a scene that has no prefab to show instead"
        );
    }
}
