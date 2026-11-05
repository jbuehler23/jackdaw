//! Full-document save/load/save fixpoint: a scene holding an entity
//! hierarchy, a runtime material referenced by a brush face, and a prefab
//! instance emits to text, respawns from that text, and emits the identical
//! text again, with the reloaded world semantically matching the original.

use bevy::prelude::*;

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
    app
}

/// A two-entity prefab: a root and one inherited child named `part`.
const PREFAB_BSN: &str = "\
#tree
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 }, rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }, scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 } }
bevy_camera::visibility::Visibility::Inherited
bevy_ecs::hierarchy::Children [
    #part
    jackdaw::prefab::components::PrefabEntityId(1)
    bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 0.0, y: 1.0, z: 0.0 }, rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }, scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 } }
]
";

#[test]
fn scene_with_assets_and_prefab_instance_round_trips_to_a_fixpoint() {
    use bevy::pbr::StandardMaterial;
    use jackdaw_scene_types::Brush;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("tree.bsn");
    std::fs::write(&prefab_path, PREFAB_BSN).unwrap();

    let mut app = make_app();

    // A runtime material (no filesystem path) with a non-default color,
    // referenced by the first face of a brush.
    let color = Color::srgb(0.2, 0.7, 0.3);
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: color,
            ..Default::default()
        });
    let mut brush = Brush::cuboid(1.0, 1.0, 1.0);
    brush.faces[0].material = handle.clone();

    // A two-level authored hierarchy registered into the live document.
    let parent = app
        .world_mut()
        .spawn((
            Name::new("Parent"),
            Transform::from_xyz(1.0, 2.0, 3.0),
            Visibility::Inherited,
            brush,
        ))
        .id();
    let kid = app
        .world_mut()
        .spawn((
            Name::new("Kid"),
            Transform::from_xyz(0.0, 1.0, 0.0),
            Visibility::Inherited,
            ChildOf(parent),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), parent);
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), kid);

    // A prefab instance alongside the authored entities.
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(5.0, 0.0, 0.0),
    );

    let count_before = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<jackdaw_bsn::AstNodeRef>>();
        q.iter(world).count()
    };

    // Save -> load -> save: the emitted text must be a fixpoint.
    let text1 = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    assert!(
        text1.contains("StandardMaterial"),
        "the runtime material must embed into the emitted scene:\n{text1}"
    );

    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &text1);

    let text2 = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    assert_eq!(
        text1, text2,
        "emit -> respawn -> emit must reproduce the identical scene text"
    );

    // The reloaded world matches semantically: same document entity count.
    let count_after = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<jackdaw_bsn::AstNodeRef>>();
        q.iter(world).count()
    };
    assert_eq!(count_after, count_before, "document entity count survives");

    // The authored hierarchy survives with its names and parentage.
    let world = app.world_mut();
    let find = |world: &mut World, wanted: &str| -> Entity {
        let mut q = world.query::<(Entity, &Name)>();
        q.iter(world)
            .find(|(_, n)| n.as_str() == wanted)
            .map(|(e, _)| e)
            .unwrap_or_else(|| panic!("entity named {wanted} after reload"))
    };
    let new_parent = find(world, "Parent");
    let new_kid = find(world, "Kid");
    assert_eq!(
        world.get::<ChildOf>(new_kid).map(ChildOf::parent),
        Some(new_parent),
        "Kid stays a child of Parent"
    );

    // The brush face still resolves its runtime material with the color.
    let face_handle = world
        .get::<Brush>(new_parent)
        .expect("Parent keeps its Brush")
        .faces[0]
        .material
        .clone();
    let material = world
        .resource::<Assets<StandardMaterial>>()
        .get(&face_handle)
        .expect("face material asset survived the round trip");
    let want = color.to_linear();
    let got = material.base_color.to_linear();
    assert!(
        (want.red - got.red).abs() < 1e-4
            && (want.green - got.green).abs() < 1e-4
            && (want.blue - got.blue).abs() < 1e-4,
        "face material color must survive: want {want:?}, got {got:?}"
    );

    // The prefab instance is resolved: one IsA root with the inherited child.
    let mut isa_q = world.query::<(Entity, &jackdaw::prefab::IsA)>();
    let instances: Vec<Entity> = isa_q.iter(world).map(|(e, _)| e).collect();
    assert_eq!(instances.len(), 1, "exactly one prefab instance");
    let mut part_q = world.query::<(&jackdaw::prefab::PrefabEntityId, &Name, &ChildOf)>();
    let parts: Vec<Entity> = part_q
        .iter(world)
        .filter(|(id, name, child_of)| {
            id.0 == 1 && name.as_str() == "part" && child_of.parent() == instances[0]
        })
        .map(|(_, _, child_of)| child_of.parent())
        .collect();
    assert_eq!(
        parts.len(),
        1,
        "the instance resolved its inherited descendant"
    );
}

/// `NavmeshExclude` is editor-facing in effect but lives in
/// `jackdaw_scene_types` so the save filter, which drops every `jackdaw::`
/// path, keeps it. Without the round trip, reopening a scene and baking
/// again would bake over different geometry than the first bake saw.
#[test]
fn navmesh_exclude_survives_a_save_and_load() {
    let mut app = make_app();

    let tagged = app
        .world_mut()
        .spawn((
            Name::new("Grass"),
            Transform::default(),
            Visibility::Inherited,
            jackdaw_scene_types::NavmeshExclude,
        ))
        .id();
    let plain = app
        .world_mut()
        .spawn((
            Name::new("Wall"),
            Transform::default(),
            Visibility::Inherited,
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), tagged);
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), plain);

    let text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    assert!(
        text.contains(jackdaw_scene_types::NAVMESH_EXCLUDE_TYPE_PATH),
        "the tag has to reach the document:\n{text}"
    );

    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &text);

    let world = app.world_mut();
    let mut query = world.query_filtered::<&Name, With<jackdaw_scene_types::NavmeshExclude>>();
    let names: Vec<String> = query.iter(world).map(ToString::to_string).collect();
    assert_eq!(
        names,
        vec!["Grass".to_string()],
        "the tag comes back on the node it was authored on, and only that one"
    );
}

/// `HiddenInGame` lives in `jackdaw_scene_types` so the save filter, which
/// drops every editor-crate path, keeps it. Without the round trip, reopening
/// a scene would lose the tag and the volume would draw in the game.
#[test]
fn hidden_in_game_survives_a_save_and_load() {
    let mut app = make_app();

    let tagged = app
        .world_mut()
        .spawn((
            Name::new("EndBox"),
            Transform::default(),
            Visibility::Inherited,
            jackdaw_scene_types::HiddenInGame,
        ))
        .id();
    let plain = app
        .world_mut()
        .spawn((
            Name::new("Wall"),
            Transform::default(),
            Visibility::Inherited,
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), tagged);
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), plain);

    let text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    assert!(
        text.contains(jackdaw_scene_types::HIDDEN_IN_GAME_TYPE_PATH),
        "the tag has to reach the document:\n{text}"
    );

    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &text);

    let world = app.world_mut();
    let mut query = world.query_filtered::<&Name, With<jackdaw_scene_types::HiddenInGame>>();
    let names: Vec<String> = query.iter(world).map(ToString::to_string).collect();
    assert_eq!(
        names,
        vec!["EndBox".to_string()],
        "the tag comes back on the node it was authored on, and only that one"
    );
}

/// A value equal to its default is elided from the document, so the
/// default is what every scene that never touched the bake settings reads
/// back as. `min_obstacle_size` has to come back filtering nothing and
/// `climb` at the height the baker used before either was authorable, or
/// reopening a scene bakes a different surface than the one that was
/// saved.
#[test]
fn a_terrain_that_authored_no_bake_settings_reads_them_back_at_their_defaults() {
    let mut app = make_app();

    let terrain = app
        .world_mut()
        .spawn((
            Name::new("Ground"),
            Transform::default(),
            Visibility::Inherited,
            jackdaw_scene_types::Terrain {
                data_path: "level.terrain-0.jdterrain".to_string(),
                ..default()
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), terrain);

    let text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    assert!(
        !text.contains("navmesh"),
        "the default bake settings reached the document, so this pins nothing:\n{text}"
    );

    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &text);

    let world = app.world_mut();
    let mut query = world.query::<&jackdaw_scene_types::Terrain>();
    let navmesh = &query
        .iter(world)
        .next()
        .expect("the terrain came back")
        .navmesh;
    assert_eq!(
        navmesh.min_obstacle_size, 0.0,
        "an elided obstacle floor has to filter nothing"
    );
    assert_eq!(
        navmesh.climb, 0.9,
        "an elided climb has to be the height the baker used before the field"
    );
}

fn brush_face_color(world: &mut World, name: &str) -> LinearRgba {
    use bevy::pbr::StandardMaterial;
    use jackdaw_scene_types::Brush;

    let mut q = world.query::<(&Name, &Brush)>();
    let handle = q
        .iter(world)
        .find(|(n, _)| n.as_str() == name)
        .map(|(_, b)| b.faces[0].material.clone())
        .unwrap_or_else(|| panic!("brush named {name} after reload"));
    world
        .resource::<Assets<StandardMaterial>>()
        .get(&handle)
        .unwrap_or_else(|| panic!("{name} material asset resolved"))
        .base_color
        .to_linear()
}

fn spawn_registered_brush(app: &mut App, name: &'static str, color: Color) {
    use bevy::pbr::StandardMaterial;
    use jackdaw_scene_types::Brush;

    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: color,
            ..Default::default()
        });
    let mut brush = Brush::cuboid(1.0, 1.0, 1.0);
    brush.faces[0].material = handle;
    let entity = app
        .world_mut()
        .spawn((
            Name::new(name),
            Transform::default(),
            Visibility::Inherited,
            brush,
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
}

fn assert_color(got: LinearRgba, want: Color, what: &str) {
    let want = want.to_linear();
    assert!(
        (want.red - got.red).abs() < 1e-4
            && (want.green - got.green).abs() < 1e-4
            && (want.blue - got.blue).abs() < 1e-4,
        "{what}: want {want:?}, got {got:?}"
    );
}

/// A material added after a reload must not take the name of one the loaded
/// document already embeds, or the old brush picks up the new colour.
#[test]
fn a_new_material_saved_after_a_reload_does_not_take_an_embedded_name() {
    let mut app = make_app();
    let old = Color::srgb(0.9, 0.1, 0.1);
    let new = Color::srgb(0.1, 0.1, 0.9);

    spawn_registered_brush(&mut app, "Old", old);
    let text1 = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &text1);

    spawn_registered_brush(&mut app, "New", new);
    let text2 = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    let roots: Vec<&str> = text2
        .lines()
        .filter(|l| l.trim_start().starts_with("#StandardMaterial"))
        .collect();
    let distinct: std::collections::HashSet<&&str> = roots.iter().collect();
    assert_eq!(roots.len(), 2, "two material roots:\n{text2}");
    assert_eq!(distinct.len(), 2, "material roots are unique:\n{text2}");

    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &text2);
    let world = app.world_mut();
    let got_old = brush_face_color(world, "Old");
    let got_new = brush_face_color(world, "New");
    assert_color(got_old, old, "Old keeps its material");
    assert_color(got_new, new, "New keeps its material");

    let text3 = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    assert_eq!(text2, text3, "a second save is byte-identical");
}

/// An entity already called `StandardMaterial0` keeps its name, and the
/// generated material name steps past it.
#[test]
fn a_generated_material_name_steps_past_an_entity_name() {
    let mut app = make_app();
    spawn_registered_brush(&mut app, "StandardMaterial0", Color::srgb(0.9, 0.1, 0.1));
    let text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    let defining = text
        .lines()
        .filter(|l| l.trim_start().starts_with("#StandardMaterial0"))
        .count();
    assert_eq!(defining, 1, "one root spells #StandardMaterial0:\n{text}");
    assert!(text.contains("#StandardMaterial1"), "{text}");
}

/// A file holding two roots of one name loads, and the later root wins the
/// name; the earlier root's users read the later material.
#[test]
fn a_file_with_two_roots_of_one_name_loads_last_wins() {
    let mut app = make_app();
    spawn_registered_brush(&mut app, "A", Color::srgb(0.9, 0.1, 0.1));
    spawn_registered_brush(&mut app, "B", Color::srgb(0.1, 0.1, 0.9));
    let text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    let dup = text.replace("#StandardMaterial1", "#StandardMaterial0");
    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &dup);
    let world = app.world_mut();
    let a = brush_face_color(world, "A");
    let b = brush_face_color(world, "B");
    assert_color(a, Color::srgb(0.1, 0.1, 0.9), "A reads the last definition");
    assert_color(b, Color::srgb(0.1, 0.1, 0.9), "B reads the last definition");
}

/// A prefab in the form older builds wrote it: every value a widened float
/// written out in full, and every default of a brush face spelled.
const OLDER_PREFAB_BSN: &str = "\
#tree
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
bevy_transform::components::transform::Transform {
    translation: glam::Vec3 {
        x: 0.10000000149011612,
        y: 0.0,
        z: 0.0,
    },
}
bevy_camera::visibility::Visibility::Inherited
bevy_ecs::hierarchy::Children [
    #part
    jackdaw::prefab::components::PrefabEntityId(1)
    bevy_transform::components::transform::Transform {
        translation: glam::Vec3 {
            x: 0.0,
            y: 1.100000023841858,
            z: 0.0,
        },
    }
    jackdaw_scene_types::types::Brush {
        faces: [
            jackdaw_geometry::BrushFaceData {
                plane: jackdaw_geometry::BrushPlane {
                    normal: glam::Vec3 {
                        x: -0.0,
                        y: 0.6000000238418579,
                        z: 0.0,
                    },
                    distance: 1.5,
                },
                material: \"\",
                uv_offset: glam::Vec2 {
                    x: 0.0,
                    y: 0.0,
                },
                uv_scale: glam::Vec2 {
                    x: 1.0,
                    y: 1.0,
                },
                uv_rotation: 0.0,
                uv_u_axis: glam::Vec3 {
                    x: 1.0,
                    y: 0.0,
                    z: 0.0,
                },
                uv_v_axis: glam::Vec3 {
                    x: 0.0,
                    y: 0.0,
                    z: -1.0,
                },
            },
        ],
    }
]
";

/// The scene text after a prefab instance, opened from `dir`, has every value
/// the live world holds written back into its document, as an edit and its
/// undo would.
fn resaved_instance_text(dir: &std::path::Path) -> String {
    use jackdaw_scene_types::Brush;

    let scene_path = dir.join("scene.bsn");
    let mut app = make_app();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);
    let world = app.world_mut();

    let mut parts = world.query::<(Entity, &Name, &Brush)>();
    let part = parts
        .iter(world)
        .find(|(_, name, _)| name.as_str() == "part")
        .map(|(entity, _, _)| entity)
        .expect("the instance resolved its brush");
    assert_eq!(
        world.get::<Brush>(part).expect("the brush").faces[0]
            .plane
            .normal
            .x
            .to_bits(),
        (-0.0_f32).to_bits(),
        "the prefab's values reach the instance",
    );
    jackdaw_bsn::sync_to_ast(world, part, std::any::TypeId::of::<Brush>());
    jackdaw_bsn::sync_to_ast(world, part, std::any::TypeId::of::<Transform>());
    jackdaw::scene_io::emit_bsn_scene_for_file(world, dir).expect("the scene emits")
}

fn write_instance_scene(dir: &std::path::Path) {
    std::fs::write(
        dir.join("scene.bsn"),
        "#instance\n\
         jackdaw::prefab::components::IsA { source: \"tree.bsn\", deleted: [] }\n",
    )
    .expect("the scene");
}

fn assert_no_overrides(text: &str) {
    assert!(
        !text.contains("Brush"),
        "a brush override appeared:\n{text}"
    );
    assert!(
        !text.contains("translation"),
        "a transform override appeared:\n{text}"
    );
}

#[test]
fn an_instance_of_an_older_prefab_gains_no_overrides_from_live_values() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("tree.bsn"), OLDER_PREFAB_BSN).unwrap();
    write_instance_scene(tmp.path());
    assert_no_overrides(&resaved_instance_text(tmp.path()));
}

#[test]
fn an_instance_gains_no_overrides_when_its_prefab_is_resaved_in_the_compact_form() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("tree.bsn");
    let app = make_app();
    let mut prefab = jackdaw_bsn::parse_bsn_text(OLDER_PREFAB_BSN).unwrap();
    jackdaw_bsn::compact_document(
        &mut prefab,
        &app.world().resource::<AppTypeRegistry>().read(),
    );
    let compact = jackdaw_bsn::emit_scene(&prefab);
    assert!(compact.len() * 2 < OLDER_PREFAB_BSN.len(), "{compact}");
    assert!(compact.contains("y: 0.6"), "{compact}");
    assert!(!compact.contains("uv_offset"), "{compact}");
    std::fs::write(&prefab_path, compact).unwrap();
    write_instance_scene(tmp.path());
    assert_no_overrides(&resaved_instance_text(tmp.path()));
}

#[test]
fn a_scene_in_the_older_form_loads_and_resaves_to_a_fixpoint() {
    let tmp = tempfile::tempdir().unwrap();
    let scene_path = tmp.path().join("older.bsn");
    let body = OLDER_PREFAB_BSN
        .replace("jackdaw::prefab::components::Prefab\n", "")
        .replace("jackdaw::prefab::components::PrefabEntityId(0)\n", "");
    std::fs::write(&scene_path, &body).unwrap();

    let mut app = make_app();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);
    assert_eq!(
        std::fs::read_to_string(&scene_path).unwrap(),
        body,
        "opening a file leaves it as it was"
    );
    let first = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path()).unwrap();
    assert!(first.len() < body.len(), "{first}");
    assert!(first.contains("y: 1.1"), "{first}");
    assert!(!first.contains("uv_offset"), "{first}");

    std::fs::write(&scene_path, &first).unwrap();
    let mut reloaded = make_app();
    jackdaw::scene_io::load_scene_from_file(reloaded.world_mut(), &scene_path);
    let second =
        jackdaw::scene_io::emit_bsn_scene_for_file(reloaded.world_mut(), tmp.path()).unwrap();
    assert_eq!(first, second);
}

#[test]
fn a_list_element_holding_only_an_unresolved_reference_keeps_it() {
    let tmp = tempfile::tempdir().unwrap();
    let scene_path = tmp.path().join("scene.bsn");
    std::fs::write(
        &scene_path,
        "#Block\n\
         jackdaw_scene_types::types::Brush { faces: [\
         jackdaw_geometry::BrushFaceData { material: \"#Gone\" }] }\n",
    )
    .unwrap();
    let mut app = make_app();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path()).unwrap();
    assert!(text.contains("material: \"#Gone\""), "{text}");
}

#[derive(Reflect, Clone, Debug, Default)]
#[reflect(Default)]
struct HeldMeta {
    tex: Handle<Image>,
}

#[derive(Reflect, Clone, Debug, Default)]
#[reflect(Default)]
struct HeldItem {
    meta: HeldMeta,
}

#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component, Default)]
struct HeldItems {
    items: Vec<HeldItem>,
}

#[test]
fn a_struct_nested_in_a_list_element_keeps_an_unresolved_reference() {
    use bevy::reflect::TypePath;

    let tmp = tempfile::tempdir().unwrap();
    let scene_path = tmp.path().join("scene.bsn");
    std::fs::write(
        &scene_path,
        format!(
            "#Thing\n{} {{ items: [{} {{ meta: {} {{ tex: \"#Gone\" }} }}] }}\n",
            HeldItems::type_path(),
            HeldItem::type_path(),
            HeldMeta::type_path(),
        ),
    )
    .unwrap();
    let mut app = make_app();
    app.register_type::<HeldItems>();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path()).unwrap();
    assert!(text.contains("tex: \"#Gone\""), "{text}");
    assert_eq!(
        jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path()).unwrap(),
        text,
    );
}

#[derive(Reflect, Clone, Debug, Default)]
#[reflect(Default)]
struct HeldTex(Handle<Image>);

#[derive(Reflect, Clone, Debug, Default)]
#[reflect(Default)]
struct HeldCell {
    tex: HeldTex,
    pair: [Handle<Image>; 2],
    both: (Handle<Image>, f32),
}

#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component, Default)]
struct HeldCells {
    cells: Vec<HeldCell>,
}

#[test]
fn a_tuple_struct_array_and_tuple_in_a_list_element_keep_unresolved_references() {
    use bevy::reflect::TypePath;

    let tmp = tempfile::tempdir().unwrap();
    let scene_path = tmp.path().join("scene.bsn");
    std::fs::write(
        &scene_path,
        format!(
            "#Thing\n{} {{ cells: [{cell} {{ tex: {tex}(\"#GoneA\") }}, \
             {cell} {{ pair: [\"#GoneB\", \"#GoneC\"] }}, \
             {cell} {{ both: [\"#GoneD\", 0.0] }}] }}\n",
            HeldCells::type_path(),
            cell = HeldCell::type_path(),
            tex = HeldTex::type_path(),
        ),
    )
    .unwrap();
    let mut app = make_app();
    app.register_type::<HeldCells>();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path()).unwrap();
    for name in ["#GoneA", "#GoneB", "#GoneC", "#GoneD"] {
        assert!(text.contains(name), "{name} in {text}");
    }
    assert_eq!(
        jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path()).unwrap(),
        text,
    );
}

/// A saved brush leaves out of each list element the fields equal to these
/// defaults, so changing one changes what every file on disk means: a saved
/// zero `uv_scale` would read back as whatever the default became. A default
/// here moves only with a migration of the files written under the old one.
#[test]
fn the_defaults_saved_brushes_rely_on_do_not_move() {
    use jackdaw_geometry::{
        BrushFaceData, BrushPlane, BrushTopology, MeshEdge, MeshLoop, MeshPoly, MeshVert,
    };

    let BrushPlane { normal, distance } = BrushPlane::default();
    assert_eq!((normal, distance), (Vec3::ZERO, 0.0));

    let BrushFaceData {
        plane,
        material,
        uv_offset,
        uv_scale,
        uv_rotation,
        uv_u_axis,
        uv_v_axis,
        is_cap: _,
    } = BrushFaceData::default();
    assert_eq!((plane.normal, plane.distance), (Vec3::ZERO, 0.0));
    assert_eq!(material, Handle::default());
    assert_eq!(
        (uv_offset, uv_scale, uv_rotation),
        (Vec2::ZERO, Vec2::ZERO, 0.0)
    );
    assert_eq!((uv_u_axis, uv_v_axis), (Vec3::ZERO, Vec3::ZERO));

    let MeshVert { position } = MeshVert::default();
    assert_eq!(position, Vec3::ZERO);

    let MeshEdge { v, flags: _ } = MeshEdge::default();
    assert_eq!(v, [0, 0]);

    let MeshPoly {
        loop_start,
        loop_total,
    } = MeshPoly::default();
    assert_eq!((loop_start, loop_total), (0, 0));

    let MeshLoop { vert, edge } = MeshLoop::default();
    assert_eq!((vert, edge), (0, 0));

    let BrushTopology {
        vertices,
        edges,
        polygons,
        loops,
        attributes,
    } = BrushTopology::default();
    assert!(vertices.is_empty() && edges.is_empty() && polygons.is_empty() && loops.is_empty());
    assert!(
        attributes.vert.is_empty()
            && attributes.edge.is_empty()
            && attributes.loop_.is_empty()
            && attributes.face.is_empty()
    );
}
