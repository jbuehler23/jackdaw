//! How a subtract hands back what it cut (`fragments`), cutting a group of
//! targets in one call (`targets`), and `brush.clip` by a plane given in
//! numbers.

use super::csg_ops::{
    assert_box, call, clear_selection, csg_app, cube, entity_of, history_len, ids, measured, named,
    scene_text,
};
use crate::util;

use bevy::prelude::*;
use jackdaw::commands::{CommandHistory, HierarchyLocation};
use jackdaw::selection::Selection;
use jackdaw_geometry::{MeshMirror, Modifier, ModifierEntry, ModifierStack};
use jackdaw_scene_types::{Brush, SceneNodeId};
use serde_json::json;

/// The world-space volume a brush encloses.
fn volume(app: &App, entity: Entity) -> f32 {
    let global = app
        .world()
        .get::<GlobalTransform>(entity)
        .expect("a transform");
    let topo = &app.world().get::<Brush>(entity).expect("a brush").topology;
    let corner = |index: u32| global.transform_point(topo.vertices[index as usize].position);
    let mut six_times = 0.0;
    for polygon in &topo.polygons {
        let start = polygon.loop_start as usize;
        let ring = &topo.loops[start..start + polygon.loop_total as usize];
        let apex = corner(ring[0].vert);
        for pair in ring[1..].windows(2) {
            six_times += apex.dot(corner(pair[0].vert).cross(corner(pair[1].vert)));
        }
    }
    (six_times / 6.0).abs()
}

fn node_id(app: &App, entity: Entity) -> u64 {
    app.world()
        .get::<SceneNodeId>(entity)
        .expect("a scene node id")
        .0
}

fn parent_of(app: &App, entity: Entity) -> Option<Entity> {
    app.world().get::<ChildOf>(entity).map(ChildOf::parent)
}

fn name_of(app: &App, entity: Entity) -> String {
    app.world()
        .get::<Name>(entity)
        .map(|name| name.as_str().to_string())
        .unwrap_or_default()
}

fn group(app: &mut App, name: &str) -> Entity {
    let outcome = call(app, "entity.add.group", json!({ "name": name }));
    entity_of(&outcome["entities"][0])
}

fn cube_under(app: &mut App, name: &str, size: [f32; 3], pos: [f32; 3], parent: Entity) -> Entity {
    let outcome = call(
        app,
        "entity.add.cube",
        json!({ "name": name, "size": size, "pos": pos, "parent": parent.to_bits() }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    entity_of(&outcome["entities"][0])
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
}

fn redo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    app.update();
}

/// Give `entity` a live X mirror across its local plane `x = at`, in the scene
/// and in the document; `bisect` cuts away what lies beyond the plane first.
fn mirror_across(app: &mut App, entity: Entity, at: f32, bisect: bool) {
    let mut stack = ModifierStack::default();
    stack
        .modifiers
        .push(ModifierEntry::new(Modifier::Mirror(MeshMirror {
            offset: Vec3::new(at, 0.0, 0.0),
            bisect: [bisect, false, false],
            ..MeshMirror::default()
        })));
    app.world_mut().entity_mut(entity).insert(stack.clone());
    jackdaw::commands::sync_component_to_ast(
        app.world_mut(),
        entity,
        ModifierStack::type_path(),
        &stack,
    );
    app.update();
}

#[track_caller]
fn assert_unmirrored(app: &mut App) {
    let live = app
        .world_mut()
        .query_filtered::<(), (With<Brush>, With<ModifierStack>)>()
        .iter(app.world())
        .count();
    assert_eq!(live, 0, "a brush still carries a live stack");
    assert!(
        !scene_text(app).contains("ModifierStack"),
        "the document still carries a stack"
    );
}

#[test]
fn merge_keeps_a_connected_result_in_the_targets_entity_and_node_id() {
    let mut app = csg_app();
    let wall = cube(&mut app, "Wall", [4.0, 2.0, 0.5], [0.0; 3], [0.0; 3]);
    cube(&mut app, "Window", [1.0, 1.0, 2.0], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);
    let id = node_id(&app, wall);
    let entries = history_len(&app);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Window", "fragments": "merge" }),
    );

    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert!(ids(&outcome["removed"]).is_empty(), "{outcome}");
    assert_eq!(ids(&outcome["created"]), vec![wall], "{outcome}");
    assert_eq!(history_len(&app), entries + 1, "one undo entry");
    assert_eq!(node_id(&app, wall), id, "the node id is kept");
    assert_eq!(named(&mut app, "Wall"), vec![wall]);
    assert!((volume(&app, wall) - (4.0 * 2.0 - 1.0) * 0.5).abs() < 1e-3);
    assert_box(
        measured(&app, wall),
        Vec3::new(-2.0, -1.0, -0.25),
        Vec3::new(2.0, 1.0, 0.25),
    );
    assert_eq!(outcome["report"]["fragments"], 1, "{outcome}");

    undo(&mut app);
    let walls = named(&mut app, "Wall");
    assert_eq!(walls.len(), 1);
    assert!(
        (volume(&app, walls[0]) - 4.0).abs() < 1e-3,
        "undo fills it back"
    );
}

#[test]
fn merge_on_a_split_result_groups_named_fragments_under_the_targets_name() {
    let mut app = csg_app();
    let wall = cube(&mut app, "Wall", [4.0, 2.0, 2.0], [1.0, 0.0, 0.0], [0.0; 3]);
    cube(&mut app, "Slab", [1.0, 4.0, 4.0], [1.0, 0.0, 0.0], [0.0; 3]);
    clear_selection(&mut app);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Slab", "fragments": "merge" }),
    );

    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(ids(&outcome["removed"]), vec![wall], "{outcome}");
    assert!(app.world().get_entity(wall).is_err());
    let created = ids(&outcome["created"]);
    assert_eq!(created.len(), 2, "{outcome}");
    let holder = parent_of(&app, created[0]).expect("the fragments sit under a group");
    assert_eq!(parent_of(&app, created[1]), Some(holder));
    assert_eq!(name_of(&app, holder), "Wall");
    assert!(
        app.world().get::<Brush>(holder).is_none(),
        "the group is no brush"
    );
    let mut names: Vec<String> = created.iter().map(|&e| name_of(&app, e)).collect();
    names.sort();
    assert_eq!(names, ["Wall1", "Wall2"]);
    let mut boxes: Vec<(Vec3, Vec3)> = created.iter().map(|&e| measured(&app, e)).collect();
    boxes.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
    assert_box(
        boxes[0],
        Vec3::new(-1.0, -1.0, -1.0),
        Vec3::new(0.5, 1.0, 1.0),
    );
    assert_box(
        boxes[1],
        Vec3::new(1.5, -1.0, -1.0),
        Vec3::new(3.0, 1.0, 1.0),
    );
}

#[test]
fn group_mode_groups_a_connected_result_too() {
    let mut app = csg_app();
    let wall = cube(&mut app, "Wall", [4.0, 2.0, 0.5], [0.0; 3], [0.0; 3]);
    cube(&mut app, "Window", [1.0, 1.0, 2.0], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Window", "fragments": "group" }),
    );

    assert_eq!(ids(&outcome["removed"]), vec![wall], "{outcome}");
    let created = ids(&outcome["created"]);
    assert_eq!(created.len(), 1, "{outcome}");
    assert_eq!(name_of(&app, created[0]), "Wall1");
    let holder = parent_of(&app, created[0]).expect("a group");
    assert_eq!(name_of(&app, holder), "Wall");
}

/// Two walls in a group, a group of three window cutters running through both,
/// and a crate the cutters reach that is not one of the targets.
struct Street {
    north: Entity,
    south: Entity,
    crate_box: Entity,
}

fn street(app: &mut App) -> Street {
    let walls = group(app, "Walls");
    let north = cube_under(app, "North", [8.0, 3.0, 0.4], [0.0, 0.0, -3.0], walls);
    let south = cube_under(app, "South", [8.0, 3.0, 0.4], [0.0, 0.0, 3.0], walls);
    let windows = group(app, "Windows");
    for (index, x) in [-2.5, 0.0, 2.5].into_iter().enumerate() {
        let name = format!("Window {index}");
        cube_under(app, &name, [1.0, 1.0, 10.0], [x, 0.0, 0.0], windows);
    }
    let crate_box = cube(app, "Crate", [1.0, 1.0, 1.0], [0.0, 0.0, 0.0], [0.0; 3]);
    clear_selection(app);
    Street {
        north,
        south,
        crate_box,
    }
}

#[test]
fn one_call_cuts_a_row_of_windows_through_two_walls_into_two_named_merged_brushes() {
    let mut app = csg_app();
    let scene = street(&mut app);
    let ids_before = (node_id(&app, scene.north), node_id(&app, scene.south));
    let crate_box = measured(&app, scene.crate_box);
    let crate_volume = volume(&app, scene.crate_box);
    let entries = history_len(&app);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "targets": "Walls", "cutters": "Windows", "fragments": "merge" }),
    );

    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert!(ids(&outcome["removed"]).is_empty(), "{outcome}");
    let mut created = ids(&outcome["created"]);
    created.sort();
    let mut walls = vec![scene.north, scene.south];
    walls.sort();
    assert_eq!(created, walls, "{outcome}");
    assert_eq!(history_len(&app), entries + 1, "one undo entry");
    assert_eq!(
        (node_id(&app, scene.north), node_id(&app, scene.south)),
        ids_before
    );
    assert_eq!(named(&mut app, "North"), vec![scene.north]);
    assert_eq!(named(&mut app, "South"), vec![scene.south]);
    let cut = (8.0 * 3.0 - 3.0) * 0.4;
    for wall in [scene.north, scene.south] {
        assert!(
            (volume(&app, wall) - cut).abs() < 1e-3,
            "{}",
            volume(&app, wall)
        );
    }
    assert_eq!(
        measured(&app, scene.crate_box),
        crate_box,
        "unlisted brushes are untouched"
    );
    assert_eq!(volume(&app, scene.crate_box), crate_volume);

    let report = &outcome["report"];
    assert_eq!(report["outcome"], "changed", "{outcome}");
    assert_eq!(report["fragments"], 2, "{outcome}");
    assert_eq!(report["watertight"], true, "{outcome}");
    let reported: Vec<f64> = report["volume"]
        .as_array()
        .expect("volume")
        .iter()
        .map(|v| v.as_f64().expect("a number"))
        .collect();
    assert!(
        (reported[0] - 2.0 * 8.0 * 3.0 * 0.4).abs() < 1e-2,
        "{outcome}"
    );
    assert!(
        (reported[1] - 2.0 * f64::from(cut)).abs() < 1e-2,
        "{outcome}"
    );

    undo(&mut app);
    for name in ["North", "South"] {
        let wall = named(&mut app, name);
        assert_eq!(wall.len(), 1);
        assert!((volume(&app, wall[0]) - 8.0 * 3.0 * 0.4).abs() < 1e-3);
    }
}

#[test]
fn a_multi_target_cut_saves_and_reopens() {
    let mut app = csg_app();
    street(&mut app);
    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "targets": "Walls", "cutters": "Windows", "fragments": "merge" }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("street.bsn");
    let saved = call(
        &mut app,
        "scene.save",
        json!({ "path": path.to_string_lossy() }),
    );
    assert_eq!(saved["result"], "finished", "{saved}");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    for _ in 0..8 {
        app.update();
    }
    let cut = (8.0 * 3.0 - 3.0) * 0.4;
    for name in ["North", "South"] {
        let wall = named(&mut app, name);
        assert_eq!(wall.len(), 1, "{name}");
        assert!((volume(&app, wall[0]) - cut).abs() < 1e-3, "{name}");
    }
    assert_eq!(named(&mut app, "Crate").len(), 1);
}

#[test]
fn a_loose_multi_target_cut_replaces_each_target_by_its_fragments() {
    let mut app = csg_app();
    let walls = group(&mut app, "Walls");
    let left = cube_under(&mut app, "Left", [4.0, 2.0, 2.0], [0.0, 0.0, -3.0], walls);
    let right = cube_under(&mut app, "Right", [4.0, 2.0, 2.0], [0.0, 0.0, 3.0], walls);
    cube(&mut app, "Slab", [1.0, 4.0, 10.0], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);

    let outcome = call(
        &mut app,
        "brush.csg_intersect",
        json!({ "targets": "Walls", "cutters": "Slab" }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let mut removed = ids(&outcome["removed"]);
    removed.sort();
    let mut both = vec![left, right];
    both.sort();
    assert_eq!(removed, both, "{outcome}");
    assert_eq!(ids(&outcome["created"]).len(), 2, "{outcome}");
    assert_eq!(outcome["report"]["fragments"], 2, "{outcome}");

    let mut app = csg_app();
    let walls = group(&mut app, "Walls");
    cube_under(&mut app, "Left", [4.0, 2.0, 2.0], [0.0, 0.0, -3.0], walls);
    cube_under(&mut app, "Right", [4.0, 2.0, 2.0], [0.0, 0.0, 3.0], walls);
    cube(&mut app, "Slab", [1.0, 4.0, 10.0], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);
    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "targets": "Walls", "cutters": "Slab" }),
    );
    assert_eq!(ids(&outcome["removed"]).len(), 2, "{outcome}");
    assert_eq!(ids(&outcome["created"]).len(), 4, "{outcome}");
    assert_eq!(outcome["report"]["fragments"], 4, "{outcome}");
    assert_eq!(
        named(&mut app, "Left").len(),
        2,
        "loose fragments keep the name"
    );
}

#[test]
fn refused_fragment_target_and_clip_params_leave_the_scene_byte_identical() {
    let mut app = csg_app();
    let scene = street(&mut app);
    let empty = group(&mut app, "Empty");
    clear_selection(&mut app);
    let before = scene_text(&mut app);
    let entries = history_len(&app);

    for (id, params) in [
        (
            "brush.csg_subtract",
            json!({ "targets": "Walls", "cutters": "Windows", "fragments": "shatter" }),
        ),
        (
            "brush.csg_subtract",
            json!({ "target": "North", "targets": "Walls", "cutters": "Windows" }),
        ),
        (
            "brush.csg_subtract",
            json!({ "targets": empty.to_bits(), "cutters": "Windows" }),
        ),
        (
            "brush.csg_subtract",
            json!({ "targets": "Walls", "cutters": "North" }),
        ),
        (
            "brush.csg_intersect",
            json!({ "targets": "Walls", "cutters": "Walls" }),
        ),
        (
            "brush.csg_intersect",
            json!({ "targets": "Walls", "cutters": "Windows", "fragments": "merge" }),
        ),
        (
            "brush.clip",
            json!({ "entity": "North", "normal": [0.0, 0.0, 0.0] }),
        ),
        (
            "brush.clip",
            json!({ "entity": "North", "normal": [1.0, 0.0, 0.0], "keep": "sideways" }),
        ),
        (
            "brush.clip",
            json!({ "entity": "Walls", "normal": [1.0, 0.0, 0.0] }),
        ),
        ("brush.clip", json!({ "entity": "North" })),
    ] {
        let outcome = call(&mut app, id, params.clone());
        assert_eq!(outcome["outcome"], "invalid", "{id} {params}: {outcome}");
        assert_eq!(outcome["result"], "cancelled", "{id} {params}: {outcome}");
        assert!(
            !outcome["warnings"].as_array().expect("warnings").is_empty(),
            "{id} {params}: the refusal says why: {outcome}"
        );
        assert_eq!(history_len(&app), entries, "{id} {params}");
        assert_eq!(scene_text(&mut app), before, "{id} {params}");
    }

    let outcome = call(
        &mut app,
        "brush.clip",
        json!({ "entity": "North", "normal": [1.0, 0.0, 0.0], "point": [20.0, 0.0, 0.0] }),
    );
    assert_eq!(
        outcome["outcome"], "unchanged",
        "a plane that misses: {outcome}"
    );
    assert_eq!(history_len(&app), entries);
    assert_eq!(scene_text(&mut app), before);
    assert!(app.world().get::<Brush>(scene.north).is_some());
}

#[test]
fn clip_keep_both_gives_two_brushes_whose_volumes_sum_to_the_original() {
    let mut app = csg_app();
    let block = cube(
        &mut app,
        "Block",
        [4.0, 2.0, 2.0],
        [1.0, 0.0, 0.0],
        [0.0, 30.0, 0.0],
    );
    clear_selection(&mut app);
    let whole = volume(&app, block);
    let id = node_id(&app, block);
    let entries = history_len(&app);

    let outcome = call(
        &mut app,
        "brush.clip",
        json!({
            "entity": "Block",
            "normal": [1.0, 0.0, 0.0],
            "point": [1.5, 0.0, 0.0],
            "keep": "both",
        }),
    );

    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(history_len(&app), entries + 1, "one undo entry");
    let created = ids(&outcome["created"]);
    assert_eq!(created.len(), 1, "{outcome}");
    let back = created[0];
    assert_eq!(node_id(&app, block), id, "the target keeps its node id");
    assert_eq!(name_of(&app, back), "Block2");
    let (front_volume, back_volume) = (volume(&app, block), volume(&app, back));
    assert!(front_volume > 0.1 && back_volume > 0.1);
    assert!(
        (front_volume + back_volume - whole).abs() < 1e-3,
        "{front_volume} + {back_volume} != {whole}"
    );
    assert!(
        measured(&app, block).0.x >= 1.5 - 1e-3,
        "the target keeps the front"
    );
    assert!(measured(&app, back).1.x <= 1.5 + 1e-3);

    undo(&mut app);
    assert_eq!(named(&mut app, "Block").len(), 1);
    assert!(named(&mut app, "Block2").is_empty());
    let blocks = named(&mut app, "Block");
    assert!((volume(&app, blocks[0]) - whole).abs() < 1e-3);
}

#[test]
fn clip_keep_front_and_back_trim_the_brush_in_world_space() {
    for (keep, min_x, max_x) in [("front", 0.5, 2.0), ("back", -2.0, 0.5)] {
        let mut app = csg_app();
        let block = cube(&mut app, "Block", [4.0, 2.0, 2.0], [0.0; 3], [0.0; 3]);

        let outcome = call(
            &mut app,
            "brush.clip",
            json!({
                "entity": block.to_bits(),
                "normal": [2.0, 0.0, 0.0],
                "point": [0.5, 0.0, 0.0],
                "keep": keep,
            }),
        );
        assert_eq!(outcome["outcome"], "changed", "{keep}: {outcome}");
        assert!(ids(&outcome["created"]).is_empty(), "{keep}: {outcome}");
        assert_box(
            measured(&app, block),
            Vec3::new(min_x, -1.0, -1.0),
            Vec3::new(max_x, 1.0, 1.0),
        );
        assert!(
            (volume(&app, block) - (max_x - min_x) * 4.0).abs() < 1e-3,
            "{keep}"
        );
    }
}

#[test]
fn clip_keep_both_on_a_mirrored_brush_mirrors_neither_half_again() {
    let mut app = csg_app();
    let block = cube(&mut app, "Block", [2.0; 3], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);
    mirror_across(&mut app, block, 0.0, true);

    let outcome = call(
        &mut app,
        "brush.clip",
        json!({
            "entity": "Block",
            "normal": [1.0, 0.0, 0.0],
            "point": [0.5, 0.0, 0.0],
            "keep": "both",
        }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let check = |app: &mut App| {
        assert_unmirrored(app);
        let front = named(app, "Block");
        let back = named(app, "Block2");
        assert_eq!((front.len(), back.len()), (1, 1));
        assert!(
            (volume(app, front[0]) - 2.0).abs() < 1e-3,
            "{}",
            volume(app, front[0])
        );
        assert!(
            (volume(app, back[0]) - 6.0).abs() < 1e-3,
            "{}",
            volume(app, back[0])
        );
    };
    check(&mut app);
    undo(&mut app);
    assert!(named(&mut app, "Block2").is_empty());
    redo(&mut app);
    check(&mut app);
}

#[test]
fn merging_a_target_with_a_live_stack_leaves_no_stack_to_fold_again() {
    let mut app = csg_app();
    let wall = cube(&mut app, "Wall", [4.0, 2.0, 0.5], [0.0; 3], [0.0; 3]);
    cube(&mut app, "Window", [1.0, 1.0, 2.0], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);
    mirror_across(&mut app, wall, 0.0, false);
    if let Some(mut stack) = app.world_mut().get_mut::<ModifierStack>(wall)
        && let Modifier::Mirror(mirror) = &mut stack.modifiers[0].modifier
    {
        mirror.mirror_x = false;
    }
    let stack = app
        .world()
        .get::<ModifierStack>(wall)
        .cloned()
        .expect("a stack");
    jackdaw::commands::sync_component_to_ast(
        app.world_mut(),
        wall,
        ModifierStack::type_path(),
        &stack,
    );
    app.update();

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Window", "fragments": "merge" }),
    );
    assert_eq!(ids(&outcome["created"]), vec![wall], "{outcome}");
    let merged = (4.0 * 2.0 - 1.0) * 0.5;
    let check = |app: &mut App| {
        assert_unmirrored(app);
        let walls = named(app, "Wall");
        assert_eq!(walls.len(), 1);
        assert!((volume(app, walls[0]) - merged).abs() < 1e-3);
    };
    check(&mut app);
    undo(&mut app);
    redo(&mut app);
    check(&mut app);
}

#[test]
fn fragments_cut_from_a_mirrored_target_are_mirrored_once() {
    let mut app = csg_app();
    let wall = cube(&mut app, "Wall", [4.0, 2.0, 0.5], [0.0; 3], [0.0; 3]);
    cube(&mut app, "Window", [1.0, 1.0, 2.0], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);
    mirror_across(&mut app, wall, 0.0, false);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Window", "fragments": "merge" }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let count = ids(&outcome["created"]).len();
    let total = outcome["report"]["volume"][1].as_f64().expect("a volume") as f32;
    let check = |app: &mut App| {
        assert_unmirrored(app);
        let mut fragments = app
            .world_mut()
            .query_filtered::<Entity, With<Brush>>()
            .iter(app.world())
            .collect::<Vec<_>>();
        fragments.retain(|&e| name_of(app, e).starts_with("Wall"));
        assert_eq!(fragments.len(), count);
        let sum: f32 = fragments.iter().map(|&e| volume(app, e)).sum();
        assert!((sum - total).abs() < 1e-2, "{sum} != {total}");
    };
    check(&mut app);
    undo(&mut app);
    redo(&mut app);
    check(&mut app);
}

#[test]
fn cutting_a_wall_and_its_child_wall_on_the_selection_keeps_every_fragment() {
    for fragments in ["loose", "group"] {
        let mut app = csg_app();
        let outer = cube(&mut app, "Outer", [4.0, 2.0, 2.0], [0.0; 3], [0.0; 3]);
        cube_under(&mut app, "Inner", [4.0, 2.0, 2.0], [0.0, 0.0, 3.0], outer);
        let lamp = cube_under(&mut app, "Lamp", [1.0; 3], [10.0, 0.0, 0.0], outer);
        let slab = cube(&mut app, "Slab", [1.0, 4.0, 10.0], [0.0; 3], [0.0; 3]);
        clear_selection(&mut app);
        app.world_mut().resource_mut::<Selection>().entities = vec![slab];
        app.update();

        let outcome = call(
            &mut app,
            "brush.csg_subtract",
            json!({ "fragments": fragments }),
        );
        assert_eq!(outcome["outcome"], "changed", "{fragments}: {outcome}");
        let created = ids(&outcome["created"]);
        assert_eq!(created.len(), 4, "{fragments}: {outcome}");
        for &fragment in &created {
            assert!(
                app.world().get::<Brush>(fragment).is_some(),
                "{fragments}: a fragment went with its doomed parent"
            );
        }
        assert!(app.world().get::<Brush>(lamp).is_some(), "{fragments}");
        assert_eq!(
            parent_of(&app, lamp),
            None,
            "{fragments}: the lamp moves up"
        );
        let entities = ids(&outcome["entities"]);
        let groups = if fragments == "group" { 2 } else { 0 };
        assert_eq!(entities.len(), 4 + groups, "{fragments}: {outcome}");
        for &entity in &entities {
            assert!(app.world().get_entity(entity).is_ok(), "{fragments}");
        }
    }
}

#[test]
fn a_fragment_group_takes_the_targets_place_among_its_siblings() {
    let mut app = csg_app();
    let row = group(&mut app, "Row");
    cube_under(&mut app, "First", [1.0; 3], [-6.0, 0.0, 0.0], row);
    let wall = cube_under(&mut app, "Wall", [4.0, 2.0, 2.0], [0.0; 3], row);
    cube_under(&mut app, "Last", [1.0; 3], [6.0, 0.0, 0.0], row);
    cube(&mut app, "Slab", [1.0, 4.0, 4.0], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);
    let slot = HierarchyLocation::from_world(app.world(), wall);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Slab", "fragments": "group" }),
    );
    let created = ids(&outcome["created"]);
    let holder = parent_of(&app, created[0]).expect("a group");
    let placed = HierarchyLocation::from_world(app.world(), holder);
    assert_eq!(placed.parent, Some(row));
    assert_eq!(placed.index, slot.index);
    let children: Vec<Entity> = app
        .world()
        .get::<Children>(row)
        .expect("the row keeps children")
        .iter()
        .collect();
    let names: Vec<String> = children.iter().map(|&e| name_of(&app, e)).collect();
    assert_eq!(names, ["First", "Wall", "Last"]);
}
