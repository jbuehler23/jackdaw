//! Undo and redo across the two kinds of history entry: a whole-scene snapshot,
//! which respawns every entity, and a command that names the entities it
//! changed. Ignored tests name behaviour the history does not have yet.

use bevy::prelude::*;
use jackdaw::scenes::Scenes;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::SceneNodeId;

use super::batch_undo::{depth, editor, history, menu, scene_text};
use crate::util::OperatorResultExt as _;

/// Add a cube the way the menu does and return the document node it became.
fn cube(app: &mut App) -> SceneNodeId {
    let before: Vec<SceneNodeId> = nodes(app);
    menu(app, "entity.add.cube", true);
    nodes(app)
        .into_iter()
        .find(|node| !before.contains(node))
        .expect("the menu added a node")
}

fn nodes(app: &mut App) -> Vec<SceneNodeId> {
    app.world_mut()
        .query::<&SceneNodeId>()
        .iter(app.world())
        .copied()
        .collect()
}

/// The entity holding `node` now; a respawn gives the same node a new entity.
fn entity_of(app: &mut App, node: SceneNodeId) -> Option<Entity> {
    app.world_mut()
        .query::<(Entity, &SceneNodeId)>()
        .iter(app.world())
        .find(|(_, held)| **held == node)
        .map(|(entity, _)| entity)
}

fn x_of(app: &mut App, node: SceneNodeId) -> Option<f32> {
    let entity = entity_of(app, node)?;
    app.world()
        .get::<Transform>(entity)
        .map(|transform| transform.translation.x)
}

fn move_to(app: &mut App, node: SceneNodeId, x: f64) {
    let entity = entity_of(app, node).expect("the node is spawned");
    app.world_mut()
        .operator("entity.set_transform")
        .param("entity", entity)
        .param("x", x)
        .call()
        .expect("entity.set_transform dispatches")
        .assert_finished();
    app.update();
}

#[test]
fn a_move_undoes_and_redoes_while_nothing_respawns_the_scene() {
    let (mut app, _dir) = editor();
    let rock = cube(&mut app);
    let start = x_of(&mut app, rock);
    move_to(&mut app, rock, 5.0);
    move_to(&mut app, rock, 9.0);

    history(&mut app, "history.undo");
    assert_eq!(x_of(&mut app, rock), Some(5.0));
    history(&mut app, "history.undo");
    assert_eq!(x_of(&mut app, rock), start);
    history(&mut app, "history.redo");
    history(&mut app, "history.redo");
    assert_eq!(x_of(&mut app, rock), Some(9.0));
}

#[test]
fn a_snapshot_entry_above_a_move_puts_back_the_scene_as_the_move_left_it() {
    let (mut app, _dir) = editor();
    let rock = cube(&mut app);
    move_to(&mut app, rock, 5.0);
    let moved = scene_text(&mut app);
    let before = depth(&app);

    let crate_node = cube(&mut app);
    assert!(depth(&app) > before);
    while depth(&app) > before {
        history(&mut app, "history.undo");
    }

    assert_eq!(scene_text(&mut app), moved);
    assert_eq!(x_of(&mut app, rock), Some(5.0));
    assert!(entity_of(&mut app, crate_node).is_none());
}

#[test]
#[ignore = "a snapshot undo respawns every entity, and the move entry below it still names the entity it moved"]
fn a_move_below_a_snapshot_entry_undoes_after_the_snapshot_is_undone() {
    let (mut app, _dir) = editor();
    let rock = cube(&mut app);
    let start = x_of(&mut app, rock);
    let placed = depth(&app);
    move_to(&mut app, rock, 5.0);
    let moved = depth(&app);
    cube(&mut app);

    while depth(&app) > moved {
        history(&mut app, "history.undo");
    }
    history(&mut app, "history.undo");
    assert_eq!(depth(&app), placed);
    assert_eq!(x_of(&mut app, rock), start);

    history(&mut app, "history.redo");
    assert_eq!(x_of(&mut app, rock), Some(5.0));
}

#[test]
#[ignore = "switching tabs respawns the scene, and the move entry still names the entity it moved"]
fn a_move_undoes_after_switching_away_from_the_tab_and_back() {
    let (mut app, _dir) = editor();
    let rock = cube(&mut app);
    let start = x_of(&mut app, rock);
    move_to(&mut app, rock, 5.0);

    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    app.world_mut()
        .operator("scene.switch")
        .param("tab", 0_i64)
        .call()
        .expect("scene.switch dispatches")
        .assert_finished();
    app.update();

    assert_eq!(x_of(&mut app, rock), Some(5.0));
    history(&mut app, "history.undo");
    assert_eq!(x_of(&mut app, rock), start);
}

#[test]
#[ignore = "a tab is marked dirty when its history grows and undo never clears it"]
fn undoing_back_to_the_saved_scene_leaves_the_tab_clean() {
    let (mut app, dir) = editor();
    cube(&mut app);
    let path = dir.path().join("assets/scenes/level.bsn");
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("a scenes dir");
    app.world_mut()
        .operator("scene.save")
        .param("path", path.to_string_lossy().into_owned())
        .call()
        .expect("scene.save dispatches")
        .assert_finished();
    app.update();
    app.update();
    let saved = depth(&app);
    assert!(!active_tab_dirty(&app), "a save leaves the tab clean");

    cube(&mut app);
    app.update();
    assert!(active_tab_dirty(&app));
    while depth(&app) > saved {
        history(&mut app, "history.undo");
    }
    app.update();
    assert!(!active_tab_dirty(&app));
}

fn active_tab_dirty(app: &App) -> bool {
    let scenes = app.world().resource::<Scenes>();
    scenes.tabs.get(scenes.active).is_some_and(|tab| tab.dirty)
}

#[test]
fn an_undo_entry_costs_a_small_multiple_of_the_scene_text() {
    let (mut app, _dir) = editor();
    for _ in 0..8 {
        cube(&mut app);
    }
    let history_bytes = |app: &App| app.world().resource::<CommandHistory>().heap_bytes();
    let before = history_bytes(&app);
    cube(&mut app);
    let entry = history_bytes(&app) - before;
    let text = scene_text(&mut app).len();
    assert!(
        entry <= 4 * text + 1024,
        "one entry holds {entry} bytes for a {text}-byte scene"
    );
}

fn select(app: &mut App, nodes: &[SceneNodeId]) {
    let entities: Vec<Entity> = nodes
        .iter()
        .map(|&node| entity_of(app, node).expect("the node is spawned"))
        .collect();
    jackdaw::selection::select_many(app.world_mut(), &entities);
    app.update();
}

fn redo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().redo_stack.len()
}

/// How a recorded step is compared with the scene undo or redo leaves.
#[derive(Clone, Copy)]
enum Check {
    /// The scene text, byte for byte.
    Text,
    /// Each node's place in the hierarchy, its name and its position.
    Outline,
}

/// The scene after each edit, and the history depth that edit left.
#[derive(Default)]
struct Steps(Vec<(usize, String, Vec<String>)>);

impl Steps {
    fn record(&mut self, app: &mut App) {
        let text = scene_text(app);
        let outline = outline(app);
        self.0.push((depth(app), text, outline));
    }

    /// Undo back through every step and redo forward again, comparing the
    /// scene with each one.
    #[track_caller]
    fn assert_round_trip(&self, app: &mut App, check: Check) {
        let depths: Vec<usize> = self.0.iter().map(|(depth, ..)| *depth).collect();
        assert!(
            depths.windows(2).all(|pair| pair[0] < pair[1]),
            "every step adds an entry: {depths:?}"
        );
        for step in self.0.iter().rev() {
            while depth(app) > step.0 {
                history(app, "history.undo");
            }
            assert_step(app, step, check, "undo");
        }
        for step in &self.0 {
            while depth(app) < step.0 {
                history(app, "history.redo");
            }
            assert_step(app, step, check, "redo");
        }
    }
}

#[track_caller]
fn assert_step(
    app: &mut App,
    (depth, text, outline_was): &(usize, String, Vec<String>),
    check: Check,
    how: &str,
) {
    match check {
        Check::Text => {
            assert_same_scene(&scene_text(app), text, &format!("{how} to depth {depth}"));
        }
        Check::Outline => assert_eq!(&outline(app), outline_was, "{how} to depth {depth}"),
    }
}

/// Compare two scene texts, naming the first line they disagree on.
#[track_caller]
fn assert_same_scene(got: &str, want: &str, context: &str) {
    if got == want {
        return;
    }
    let got_lines: Vec<&str> = got.lines().collect();
    let want_lines: Vec<&str> = want.lines().collect();
    let at = got_lines
        .iter()
        .zip(&want_lines)
        .position(|(a, b)| a != b)
        .unwrap_or(got_lines.len().min(want_lines.len()));
    let window = |lines: &[&str]| lines[at.saturating_sub(3)..(at + 4).min(lines.len())].join("\n");
    panic!(
        "{context}: the scene differs at line {at}\n--- want\n{}\n--- got\n{}",
        window(&want_lines),
        window(&got_lines)
    );
}

/// One line per document node in document order: its depth, node id, name and
/// position.
fn outline(app: &mut App) -> Vec<String> {
    let world = app.world();
    let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
    let mut lines = Vec::new();
    let mut stack: Vec<(usize, Entity)> = ast.roots.iter().rev().map(|&node| (0, node)).collect();
    while let Some((level, node)) = stack.pop() {
        let position = ast
            .ecs_for_ast(node)
            .and_then(|entity| world.get::<Transform>(entity))
            .map(|transform| transform.translation);
        lines.push(format!(
            "{level} {:?} {:?} {position:?}",
            ast.stable_id_of(node),
            ast.get_name(node)
        ));
        let children = ast.get_children_ast(node);
        stack.extend(children.into_iter().rev().map(|child| (level + 1, child)));
    }
    lines
}

fn set_field(app: &mut App, entity: Entity, field: &str, value: &str) {
    app.world_mut()
        .operator("field.set")
        .param("entity", entity)
        .param(
            "type_path",
            "bevy_transform::components::transform::Transform",
        )
        .param("field", field.to_string())
        .param("value", value.to_string())
        .call()
        .expect("field.set dispatches")
        .assert_finished();
    app.update();
}

fn set_node_field(app: &mut App, node: SceneNodeId, field: &str, value: &str) {
    let entity = entity_of(app, node).expect("the node is spawned");
    set_field(app, entity, field, value);
}

fn reparent(app: &mut App, child: SceneNodeId, parent: SceneNodeId) {
    let child = entity_of(app, child).expect("the child is spawned");
    let parent = entity_of(app, parent).expect("the parent is spawned");
    app.world_mut()
        .operator("entity.reparent")
        .param("child", child)
        .param("parent", parent)
        .call()
        .expect("entity.reparent dispatches")
        .assert_finished();
    app.update();
}

fn parent_of(app: &mut App, node: SceneNodeId) -> Option<SceneNodeId> {
    let entity = entity_of(app, node)?;
    let parent = app.world().get::<ChildOf>(entity)?.parent();
    app.world().get::<SceneNodeId>(parent).copied()
}

/// Adds, a duplicate and a visibility toggle, with a move after the first add
/// when `with_a_move`.
fn mixed_edits(app: &mut App, with_a_move: bool) -> Steps {
    let mut steps = Steps::default();
    steps.record(app);
    let first = cube(app);
    steps.record(app);
    if with_a_move {
        move_to(app, first, 3.0);
        steps.record(app);
    }
    menu(app, "entity.add.sphere", true);
    steps.record(app);
    let third = cube(app);
    steps.record(app);
    select(app, &[third]);
    menu(app, "entity.duplicate", true);
    steps.record(app);
    menu(app, "entity.toggle_visibility", true);
    steps.record(app);
    steps
}

#[test]
fn every_undo_and_redo_of_snapshot_entries_puts_back_the_exact_scene_text() {
    let (mut app, _dir) = editor();
    let steps = mixed_edits(&mut app, false);
    steps.assert_round_trip(&mut app, Check::Text);
}

#[test]
#[ignore = "a snapshot undo respawns every entity, and the move entry below it still names the entity it moved"]
fn every_undo_and_redo_of_mixed_entries_puts_back_the_exact_scene_text() {
    let (mut app, _dir) = editor();
    let steps = mixed_edits(&mut app, true);
    steps.assert_round_trip(&mut app, Check::Text);
}

#[test]
fn moving_a_node_up_its_siblings_undoes_and_redoes() {
    let (mut app, _dir) = editor();
    cube(&mut app);
    menu(&mut app, "entity.add.sphere", true);
    let third = cube(&mut app);
    select(&mut app, &[third]);
    let mut steps = Steps::default();
    steps.record(&mut app);
    menu(&mut app, "entity.move_up", true);
    steps.record(&mut app);

    steps.assert_round_trip(&mut app, Check::Text);
}

#[test]
fn a_new_edit_after_an_undo_drops_what_could_be_redone() {
    let (mut app, _dir) = editor();
    let rock = cube(&mut app);
    move_to(&mut app, rock, 5.0);
    cube(&mut app);
    history(&mut app, "history.undo");
    assert!(redo_depth(&app) > 0);

    move_to(&mut app, rock, 8.0);
    assert_eq!(redo_depth(&app), 0);
    history(&mut app, "history.redo");
    assert_eq!(x_of(&mut app, rock), Some(8.0));
}

fn delete_two_of_four(app: &mut App) -> Steps {
    let cubes: Vec<SceneNodeId> = (0..4).map(|_| cube(app)).collect();
    for (index, &node) in cubes.iter().enumerate() {
        move_to(app, node, index as f64);
    }
    let mut steps = Steps::default();
    steps.record(app);
    select(app, &[cubes[1], cubes[3]]);
    menu(app, "entity.delete", true);
    steps.record(app);
    assert!(entity_of(app, cubes[1]).is_none());
    assert!(entity_of(app, cubes[3]).is_none());
    steps
}

#[test]
fn undoing_a_delete_of_several_selected_nodes_puts_them_back_in_order() {
    let (mut app, _dir) = editor();
    let steps = delete_two_of_four(&mut app);
    steps.assert_round_trip(&mut app, Check::Outline);
}

#[test]
#[ignore = "undoing a delete of a brush writes its computed physics components into the document and drops its sibling's collider"]
fn undoing_a_delete_of_several_selected_nodes_puts_back_the_exact_scene_text() {
    let (mut app, _dir) = editor();
    let steps = delete_two_of_four(&mut app);
    steps.assert_round_trip(&mut app, Check::Text);
}

fn edit_three_selected(app: &mut App) -> Steps {
    let cubes: Vec<SceneNodeId> = (0..3).map(|_| cube(app)).collect();
    let mut steps = Steps::default();
    steps.record(app);
    select(app, &cubes);
    set_node_field(app, cubes[2], "translation.y", "4.0");
    steps.record(app);
    for &node in &cubes {
        let entity = entity_of(app, node).expect("spawned");
        let y = app
            .world()
            .get::<Transform>(entity)
            .map(|t| t.translation.y);
        assert_eq!(y, Some(4.0));
    }
    steps
}

#[test]
fn a_field_edit_on_a_multi_selection_is_one_entry_that_undoes_on_every_member() {
    let (mut app, _dir) = editor();
    let steps = edit_three_selected(&mut app);
    steps.assert_round_trip(&mut app, Check::Outline);
}

#[test]
#[ignore = "undoing the first edit of a field leaves an empty struct patch where the component had none"]
fn a_field_edit_on_a_multi_selection_undoes_to_the_exact_scene_text() {
    let (mut app, _dir) = editor();
    let steps = edit_three_selected(&mut app);
    steps.assert_round_trip(&mut app, Check::Text);
}

fn reparent_the_third_of_four(app: &mut App) -> (Steps, SceneNodeId, SceneNodeId) {
    let parent = cube(app);
    cube(app);
    let child = cube(app);
    cube(app);
    move_to(app, child, 2.0);
    let mut steps = Steps::default();
    steps.record(app);
    reparent(app, child, parent);
    steps.record(app);
    (steps, parent, child)
}

#[test]
fn a_reparent_undoes_and_redoes_under_the_same_parent() {
    let (mut app, _dir) = editor();
    let (_, parent, child) = reparent_the_third_of_four(&mut app);
    let root = parent_of(&mut app, parent);
    assert_eq!(parent_of(&mut app, child), Some(parent));
    history(&mut app, "history.undo");
    assert_eq!(parent_of(&mut app, child), root);
    history(&mut app, "history.redo");
    assert_eq!(parent_of(&mut app, child), Some(parent));
}

#[test]
#[ignore = "undoing a reparent puts the child at the end of its old sibling list, not in the slot it left"]
fn a_reparent_undoes_into_the_sibling_slot_it_left() {
    let (mut app, _dir) = editor();
    let (steps, ..) = reparent_the_third_of_four(&mut app);
    steps.assert_round_trip(&mut app, Check::Text);
}

/// Spawn an instance of the rock prefab and return it.
fn spawn_rock(app: &mut App, x: f64) -> Entity {
    app.world_mut()
        .operator("prefab.spawn_instance")
        .param("path", "prefabs/rock.bsn")
        .param("pos_x", x)
        .param("pos_y", 0.0)
        .param("pos_z", 0.0)
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Execute,
            creates_history_entry: true,
        })
        .call()
        .expect("prefab.spawn_instance dispatches")
        .assert_finished();
    app.update();
    spawn_rock_found(app)
}

/// The one rock instance in the scene.
fn spawn_rock_found(app: &mut App) -> Entity {
    let rocks: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .filter(|(_, name)| name.as_str() == "Rock")
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(rocks.len(), 1, "one rock instance");
    rocks[0]
}

/// A rock instance spawned, a cube added, then a field edit on the rock, with
/// only the cube and the edit recorded unless `whole_run`.
fn edit_a_rock(app: &mut App, whole_run: bool) -> Steps {
    let mut steps = Steps::default();
    if whole_run {
        steps.record(app);
    }
    spawn_rock(app, 2.0);
    if whole_run {
        steps.record(app);
    }
    cube(app);
    steps.record(app);
    let rock = spawn_rock_found(app);
    jackdaw::selection::select_many(app.world_mut(), &[rock]);
    set_field(app, rock, "translation.z", "6.0");
    steps.record(app);
    steps
}

#[test]
fn an_edit_on_a_prefab_instance_undoes_and_redoes() {
    let (mut app, _dir) = editor();
    let steps = edit_a_rock(&mut app, false);
    steps.assert_round_trip(&mut app, Check::Text);
}

#[test]
#[ignore = "a snapshot redo respawns every entity, and the field entry above it still names the entity it edited"]
fn an_edit_on_a_prefab_instance_redoes_after_the_spawn_is_undone_and_redone() {
    let (mut app, _dir) = editor();
    let steps = edit_a_rock(&mut app, true);
    steps.assert_round_trip(&mut app, Check::Text);
}

#[test]
fn a_trimmed_history_still_undoes_every_entry_it_kept() {
    let (mut app, _dir) = editor();
    let start = depth(&app);
    cube(&mut app);
    let per_cube = depth(&app) - start;
    let one_cube = app.world().resource::<CommandHistory>().heap_bytes();
    app.world_mut()
        .resource_mut::<CommandHistory>()
        .budget_bytes = one_cube * 6;
    let mut texts = vec![scene_text(&mut app)];
    for _ in 0..12 {
        cube(&mut app);
        texts.push(scene_text(&mut app));
    }
    let kept = depth(&app);
    assert!(
        kept < 13 * per_cube,
        "the budget trimmed the oldest entries"
    );

    for undone in 1..kept / per_cube {
        for _ in 0..per_cube {
            history(&mut app, "history.undo");
        }
        assert_same_scene(
            &scene_text(&mut app),
            &texts[texts.len() - 1 - undone],
            &format!("{undone} cubes undone"),
        );
    }
}

#[test]
#[ignore = "a snapshot undo respawns every entity, and the reparent entry below it still names the entities it moved"]
fn a_reparent_below_a_snapshot_entry_undoes_after_the_snapshot_is_undone() {
    let (mut app, _dir) = editor();
    let parent = cube(&mut app);
    let child = cube(&mut app);
    let before = depth(&app);
    reparent(&mut app, child, parent);
    let reparented = depth(&app);
    cube(&mut app);

    while depth(&app) > reparented {
        history(&mut app, "history.undo");
    }
    history(&mut app, "history.undo");
    assert_eq!(depth(&app), before);
    assert_eq!(parent_of(&mut app, child), parent_of(&mut app, parent));
    history(&mut app, "history.redo");
    assert_eq!(parent_of(&mut app, child), Some(parent));
}

#[test]
#[ignore = "a snapshot undo respawns every entity, and the field entry below it still names the entity it edited"]
fn a_field_edit_below_a_snapshot_entry_undoes_after_the_snapshot_is_undone() {
    let (mut app, _dir) = editor();
    let rock = cube(&mut app);
    let mut steps = Steps::default();
    steps.record(&mut app);
    select(&mut app, &[rock]);
    set_node_field(&mut app, rock, "translation.y", "3.0");
    steps.record(&mut app);
    cube(&mut app);
    steps.record(&mut app);

    steps.assert_round_trip(&mut app, Check::Outline);
}
